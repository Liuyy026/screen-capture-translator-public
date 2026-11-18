//! Display-only recovery of detached vertical punctuation. Neither OCR boxes
//! in the page cache nor the source image are changed.
use crate::compat::{array, keeps_original, number, text, truth};
use anyhow::Result;
use opencv::{
    core::{self, Mat, Scalar},
    imgproc,
    prelude::*,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub type Box4 = [i32; 4];

pub fn clipped_box(block: &Value, width: i32, height: i32) -> Option<Box4> {
    let values = block["box"].as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut b = [0; 4];
    for (i, v) in values.iter().enumerate() {
        let n = number(v)?;
        if !n.is_finite() {
            return None;
        }
        b[i] = n.round_ties_even().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    }
    b[0] = b[0].max(0);
    b[1] = b[1].max(0);
    b[2] = b[2].min(width);
    b[3] = b[3].min(height);
    ((b[2] as i64 - b[0] as i64) >= 4 && (b[3] as i64 - b[1] as i64) >= 4).then_some(b)
}

pub fn ready(block: &Value) -> bool {
    !keeps_original(block)
        && !crate::ocr_review::suppressed(block)
        && block["translation"].as_str().is_some_and(|s| !s.trim().is_empty())
        && !truth(&block["error"])
}

pub fn preserved_markers(page: &Value) -> Value {
    let (width, height) = (
        page["width"].as_i64().unwrap_or(0) as i32,
        page["height"].as_i64().unwrap_or(0) as i32,
    );
    let mut preserved = json!({});
    for block in array(&page["blocks"]) {
        if keeps_original(block) { continue; }
        if crate::ocr_review::suppressed(block) {
            preserved[text(&block["id"])] = json!(crate::ocr_review::reason(block));
            continue;
        }
        if truth(&block["edited"]) || !ready(block) {
            continue;
        }
        if let Some([x, y, r, d]) = clipped_box(block, width, height)
            && block["source"]
                .as_str()
                .unwrap_or("")
                .trim()
                .chars()
                .count()
                <= 3
            && y as f64 > height as f64 * 0.92
            && ((r - x) as f64) < width as f64 * 0.045
            && ((d - y) as f64) < height as f64 * 0.025
            && ((r as f64) < width as f64 * 0.18 || x as f64 > width as f64 * 0.82)
        {
            preserved[text(&block["id"])] = json!("疑似页码，已保留原图；核对并手动保存后可覆盖。");
        }
    }
    preserved
}

// Tiny kana marks and punctuation can fall just outside a narrow OCR column.
// Bound recovery to less than one character around that column; large connected
// artwork/frame strokes cannot qualify. This does not alter stored OCR boxes.
fn near_narrow_column(bounds: Box4, stat: [i32; 5], page_width: i32) -> bool {
    let [a, b, c, d] = bounds;
    let [x, y, w, h, _] = stat;
    let width = c - a;
    let reach = ((width as f64 * 0.8).ceil() as i32).clamp(6, 28);
    width <= (page_width as f64 * 0.04).max(16.) as i32 && d - b >= width * 3
        && x >= a - reach && x + w <= c + reach
        && y >= b - reach && y + h <= d + reach
        && (x < a || x + w > c || y < b || y + h > d)
        && (x + w).min(c) - x.max(a) >= -3
}

pub fn complete_vertical_strokes(
    gray: &Mat,
    boxes: &[Option<Box4>],
    blocks: &[Value],
    source: Option<&Mat>,
) -> Result<(Vec<Option<Box4>>, Option<Mat>)> {
    let Some(source) = source else {
        return Ok((boxes.to_vec(), None));
    };
    anyhow::ensure!(
        gray.typ() == core::CV_8UC1
            && source.typ() == core::CV_8UC1
            && gray.size()? == source.size()?,
        "expected matching gray image and source mask"
    );
    anyhow::ensure!(boxes.len() == blocks.len(), "box and block count mismatch");
    let width = gray.cols();
    let height = gray.rows();
    let image = gray.data_bytes()?;
    let original = source.data_bytes()?;
    let mut binary =
        Mat::new_rows_cols_with_default(height, width, core::CV_8UC1, Scalar::all(0.0))?;
    for (dst, &pixel) in binary.data_bytes_mut()?.iter_mut().zip(image) {
        *dst = u8::from(pixel < 150);
    }
    let (mut labels, mut stats, mut centroids) = (Mat::default(), Mat::default(), Mat::default());
    let count = imgproc::connected_components_with_stats(
        &binary,
        &mut labels,
        &mut stats,
        &mut centroids,
        8,
        core::CV_32S,
    )?;
    for (dst, &pixel) in binary.data_bytes_mut()?.iter_mut().zip(original) {
        *dst = u8::from(pixel == 0);
    }
    let mut distance = Mat::default();
    imgproc::distance_transform_def(&binary, &mut distance, imgproc::DIST_L2, 5)?;
    let distances = distance.data_typed::<f32>()?;
    let label_pixels = labels.data_typed::<i32>()?;
    let statistics = stats.data_typed::<i32>()?;
    let mut expanded = boxes.to_vec();
    let mut supplemented = source.try_clone()?;
    let added = supplemented.data_bytes_mut()?;
    for label in 1..count {
        let start = label as usize * 5;
        let [x, y, w, h, area]: [i32; 5] = statistics[start..start + 5].try_into()?;
        let long_stroke = (2..=10).contains(&w) && (35..=180).contains(&h)
            && h >= 8 * w && (area as f64) >= h as f64 * 0.5;
        let compact = area >= 3 && w >= 1 && h >= 2 && w.max(h) <= 32 && w.min(h) <= 12
            && boxes.iter().zip(blocks).any(|(bbox, block)| bbox.is_some_and(|bounds|
                truth(&block["vertical"]) && ready(block) && near_narrow_column(bounds, [x,y,w,h,area], width)));
        if !long_stroke && !compact {
            continue;
        }
        let mut members = Vec::new();
        let mut detected = 0;
        let mut nearby = f32::INFINITY;
        for yy in y..y + h {
            for xx in x..x + w {
                let index = (yy * width + xx) as usize;
                if label_pixels[index] == label {
                    members.push(index);
                    detected += usize::from(original[index] > 0);
                    nearby = nearby.min(distances[index]);
                }
            }
        }
        let evidence = detected as f64 / members.len() as f64;
        let (mut border_count, mut border_sum, mut light) = (0, 0u64, 0);
        for yy in (y - 4).max(0)..(y + h + 4).min(height) {
            for xx in (x - 4).max(0)..(x + w + 4).min(width) {
                if yy >= y && yy < y + h && xx >= x && xx < x + w {
                    continue;
                }
                let p = image[(yy * width + xx) as usize];
                border_count += 1;
                border_sum += p as u64;
                light += usize::from(p >= 235);
            }
        }
        if border_count == 0
            || (border_sum as f64 / border_count as f64) < 225.0
            || (light as f64 / border_count as f64) < 0.75
        {
            continue;
        }
        for (i, (bbox, block)) in boxes.iter().zip(blocks).enumerate() {
            let Some([a, b, c, d]) = *bbox else {
                continue;
            };
            if !truth(&block["vertical"]) || !ready(block) {
                continue;
            }
            let extension = (b - y).max(0) + (y + h - d).max(0);
            let middle = x as f64 + w as f64 / 2.0;
            let compact_mark = compact && near_narrow_column([a,b,c,d], [x,y,w,h,area], width)
                && (evidence >= 0.45 || (nearby <= 3.0 && y >= b && y + h <= d));
            let aligned_stroke = long_stroke && a as f64 + (c - a) as f64 * 0.15 < middle
                && middle < c as f64 - (c - a) as f64 * 0.15
                && extension > 8
                && extension <= 160;
            let overlap = (y + h).min(d) - y.max(b);
            let detected_dash = overlap >= 2 && evidence >= 0.12;
            let trailing = d - y >= -2
                && ((d - y) as f64) <= 6.0f64.min(h as f64 * 0.1)
                && nearby <= 3.0
                && evidence == 0.0;
            if !compact_mark && !(aligned_stroke && (detected_dash || trailing)) {
                continue;
            }
            if boxes.iter().enumerate().any(|(j, other)| {
                j != i
                    && other.is_some_and(|o| {
                        x.max(o[0]) < (x + w).min(o[2]) && y.max(o[1]) < (y + h).min(o[3])
                    })
            }) {
                continue;
            }
            let expanded = expanded[i].as_mut().unwrap();
            if compact_mark {
                expanded[0] = expanded[0].min((x - 2).max(0));
                expanded[2] = expanded[2].max((x + w + 2).min(width));
            }
            expanded[1] = expanded[1].min((y - 2).max(0));
            expanded[3] = expanded[3].max((y + h + 2).min(height));
            for &p in &members {
                added[p] = 255;
            }
            break;
        }
    }
    Ok((expanded, Some(supplemented)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::blank;
    use opencv::core::Rect;

    #[test]
    fn recover_clipped_note_marks_without_frame_or_other_text() -> Result<()> {
        let mut gray = blank(400, 500, 255)?;
        let mut source = blank(400, 500, 0)?;
        // Column ends at y=190; the detector still sees its detached final mark.
        for y in [70, 95, 120, 145, 170, 193] {
            imgproc::rectangle(&mut gray, Rect::new(82,y,4,9), Scalar::all(0.), -1, 8, 0)?;
            imgproc::rectangle(&mut source, Rect::new(82,y,4,9), Scalar::all(255.), -1, 8, 0)?;
        }
        // A neighboring frame and an unsupported drawing mark must survive.
        imgproc::rectangle(&mut gray, Rect::new(102,30,3,250), Scalar::all(0.), -1, 8, 0)?;
        imgproc::rectangle(&mut gray, Rect::new(75,196,3,6), Scalar::all(0.), -1, 8, 0)?;
        let blocks = vec![json!({"id":"note","vertical":true,"translation":"这么一说","error":""})];
        let boxes = vec![Some([78,65,92,190])];
        let (recovered, _) = complete_vertical_strokes(&gray, &boxes, &blocks, Some(&source))?;
        assert_eq!(recovered, vec![Some([78,65,92,204])]);
        assert_eq!(boxes, vec![Some([78,65,92,190])]);
        let mut other = blocks.clone(); other.push(json!({"id":"other","vertical":true,"translation":"其他"}));
        let (protected, _) = complete_vertical_strokes(&gray, &[boxes[0],Some([80,191,90,205])], &other, Some(&source))?;
        assert_eq!(protected[0], boxes[0]);
        let mut kept = blocks.clone(); kept[0]["keep_original"] = json!(true);
        assert_eq!(complete_vertical_strokes(&gray, &boxes, &kept, Some(&source))?.0, boxes);
        Ok(())
    }
}

pub fn probe(page: &Value, gray: &Mat, source: Option<&Mat>) -> Result<Value> {
    let preserved = preserved_markers(page);
    let blocks = array(&page["blocks"]);
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| {
            if preserved.get(text(&b["id"])).is_some() {
                None
            } else {
                clipped_box(b, gray.cols(), gray.rows())
            }
        })
        .collect();
    let (expanded, supplemented) = complete_vertical_strokes(gray, &boxes, blocks, source)?;
    let hash = supplemented
        .as_ref()
        .map(|m| {
            m.data_bytes()
                .map(|data| format!("{:x}", Sha256::digest(data)))
        })
        .transpose()?;
    Ok(json!({"preserved": preserved, "boxes": expanded, "mask_sha256": hash}))
}
