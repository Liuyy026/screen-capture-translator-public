//! Final display regions. OCR and translation caches are immutable inputs.
use crate::{
    background,
    compat::{array, keeps_original, text, truth},
    geometry::{self, Candidate},
    ink::{self, Box4},
    model_images::ImageModels,
    preserved_bubbles,
    raster::*,
};
use anyhow::{Result, ensure};
use opencv::{
    core::{self, Mat},
    prelude::*,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

fn merge(region: &mut Value, patch: Value) {
    region
        .as_object_mut()
        .unwrap()
        .extend(patch.as_object().unwrap().clone());
}

fn union(boxes: &[Box4]) -> Box4 {
    [
        boxes.iter().map(|b| b[0]).min().unwrap(),
        boxes.iter().map(|b| b[1]).min().unwrap(),
        boxes.iter().map(|b| b[2]).max().unwrap(),
        boxes.iter().map(|b| b[3]).max().unwrap(),
    ]
}

fn intersects([a, b, c, d]: Box4, [x, y, r, s]: Box4) -> bool {
    a < r && x < c && b < s && y < d
}

fn padded([x, y, r, b]: Box4, pad: i32, width: i32, height: i32) -> Box4 {
    [(x - pad).max(0), (y - pad).max(0), (r + pad).min(width), (b + pad).min(height)]
}

fn trim_around_originals(mut area: Box4, text: Box4, originals: &[Box4]) -> Option<Box4> {
    for &[left, top, right, bottom] in originals {
        if !intersects(area, [left, top, right, bottom]) {
            continue;
        }
        let [x, y, r, b] = area;
        // Keep the translated text's ink bounds outside the preserved frame.
        area = [[x, y, left, b], [right, y, r, b], [x, y, r, top], [x, bottom, r, b]]
            .into_iter()
            .filter(|[a, b, c, d]| *a <= text[0] && *b <= text[1] && *c >= text[2] && *d >= text[3])
            .max_by_key(|[a, b, c, d]| i64::from(c - a) * i64::from(d - b))?;
    }
    Some(area)
}

pub async fn regions(
    page: &Value,
    gray: &Mat,
    color: Option<&Mat>,
    source: Option<&Mat>,
    mut models: Option<&mut ImageModels>,
) -> Result<Vec<Value>> {
    ensure!(
        page.is_object() && gray.typ() == core::CV_8UC1 && !gray.empty(),
        "invalid page or image"
    );
    let (width, height) = (gray.cols(), gray.rows());
    ensure!(
        page["width"].as_f64() == Some(width as f64)
            && page["height"].as_f64() == Some(height as f64),
        "image size does not match cached OCR coordinates"
    );
    ensure!(
        page.get("blocks").is_none_or(Value::is_array),
        "invalid OCR blocks"
    );
    let blocks = array(&page["blocks"]);
    ensure!(blocks.iter().all(Value::is_object), "invalid OCR blocks");
    let ids: Vec<_> = blocks.iter().map(|b| text(&b["id"])).collect();
    ensure!(
        ids.iter().collect::<HashSet<_>>().len() == ids.len()
            && blocks.iter().all(|b| !b["id"].is_null()),
        "missing or duplicate OCR identifiers"
    );
    if let Some(color) = color {
        ensure!(
            color.typ() == core::CV_8UC3 && color.size()? == gray.size()?,
            "invalid color image"
        );
    }
    let preserved = ink::preserved_markers(page);
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| {
            if keeps_original(b) || preserved.get(text(&b["id"])).is_some() {
                None
            } else {
                ink::clipped_box(b, width, height)
            }
        })
        .collect();
    if boxes.iter().all(Option::is_none) {
        return Ok(vec![]);
    }
    let (boxes, supplemented) = ink::complete_vertical_strokes(gray, &boxes, blocks, source)?;
    let source = supplemented.as_ref();
    let originals = preserved_bubbles::protected_bounds(page, width, height);
    let bubbles = geometry::bubbles(gray, &boxes)?;
    let narrations = geometry::narration_groups(gray, blocks, &boxes, &bubbles)?;
    let mut regions = vec![];
    let mut accepted: Vec<Candidate> = vec![];
    let mut claimed: HashSet<String> = HashSet::new();
    let mut ordered: Vec<_> = bubbles.iter().collect();
    ordered.sort_by_key(|c| c.members.iter().min().copied());
    for candidate in ordered {
        let indexes = &candidate.members;
        if preserved_bubbles::touches(candidate, &originals)? {
            continue;
        }
        claimed.extend(indexes.iter().map(|&i| ids[i].clone()));
        if !indexes.iter().all(|&i| ink::ready(&blocks[i])) {
            continue;
        }
        let Some(inner) = geometry::inner_rectangle(&candidate.mask)? else {
            continue;
        };
        let [x, y] = candidate.origin;
        let mut region = json!({"id":format!("reading-{}",regions.len()+1),"block_ids":indexes.iter().map(|&i| &ids[i]).collect::<Vec<_>>(),
            "overlay":"balloon","overlay_score":candidate.score,"overlay_reason":"阅读显示：已确认封闭气泡，仅清除原文字范围",
            "points":candidate.points,"text_box":[x+inner[0],y+inner[1],x+inner[2],y+inner[3]],
            "erase_points":geometry::erase_polygons(candidate,&boxes)?,"fill_color":candidate.fill_color,
            "vertical":indexes.iter().all(|&i| truth(&blocks[i]["vertical"]))});
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let plan = background::prepare(
            gray,
            &member_boxes,
            Some((&candidate.mask, candidate.origin)),
            candidate.textured.unwrap_or(false),
            source,
        )?;
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        accepted.push(candidate.clone());
    }
    for candidate in &narrations {
        if preserved_bubbles::touches(candidate, &originals)? {
            continue;
        }
        let mut indexes = candidate.members.clone();
        indexes.sort_by(|&a, &b| {
            blocks[a]["reading_order"]
                .as_f64()
                .unwrap_or(1e9)
                .total_cmp(&blocks[b]["reading_order"].as_f64().unwrap_or(1e9))
                .then(boxes[b].unwrap()[0].cmp(&boxes[a].unwrap()[0]))
        });
        claimed.extend(indexes.iter().map(|&i| ids[i].clone()));
        if !indexes.iter().all(|&i| ink::ready(&blocks[i])) {
            continue;
        }
        let mut region = json!({"id":format!("reading-{}",regions.len()+1),"block_ids":indexes.iter().map(|&i| &ids[i]).collect::<Vec<_>>(),
            "overlay":"narration","overlay_score":0.85,"overlay_reason":"阅读显示：连续旁白横排，保留背景纹理",
            "points":candidate.points,"text_box":candidate.text_box,"erase_points":[],"fill_color":[255,255,255],"vertical":false});
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let plan = background::prepare(
            gray,
            &member_boxes,
            Some((&candidate.mask, candidate.origin)),
            true,
            source,
        )?;
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        accepted.push(candidate.clone());
    }
    let by_id: HashMap<_, _> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i))
        .collect();
    for old in array(&page["overlay_regions"]) {
        if !old.is_object()
            || !matches!(old["overlay"].as_str(), Some("balloon" | "rectangle"))
            || array(&old["block_ids"]).is_empty()
        {
            continue;
        }
        let members: Vec<_> = array(&old["block_ids"]).iter().map(text).collect();
        if members.iter().any(|id| {
            claimed.contains(id) || by_id.get(id).is_none_or(|&i| !ink::ready(&blocks[i]))
        }) {
            continue;
        }
        let Some(shape) = geometry::cached_shape(old, width, height)? else {
            continue;
        };
        if preserved_bubbles::touches(&shape, &originals)? {
            continue;
        }
        let mut overlaps = false;
        for other in bubbles.iter().chain(&narrations).chain(&accepted) {
            if geometry::overlap(&shape, other)? > 0.0 {
                overlaps = true;
                break;
            }
        }
        if overlaps || members.iter().any(|id| boxes[by_id[id]].is_none()) {
            continue;
        }
        let member_boxes: Vec<_> = members.iter().map(|id| boxes[by_id[id]].unwrap()).collect();
        let mut region = old.clone();
        let [x, y] = shape.origin;
        let w = shape.mask.cols().min(width - x);
        let h = shape.mask.rows().min(height - y);
        let textured =
            w > 0 && h > 0 && fraction(crop(gray, x, y, w, h)?.data_bytes()?, |v| v < 210) > 0.18;
        let clip = (old["overlay"] == "balloon" || !originals.is_empty()).then_some((&shape.mask, shape.origin));
        let plan = background::prepare(gray, &member_boxes, clip, textured, source)?;
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        claimed.extend(members);
        accepted.push(shape);
    }
    let mut units = vec![];
    let mut grouped = HashSet::new();
    for dialogue in array(&page["dialogues"]) {
        let keys = array(&dialogue["block_ids"]);
        if !(2..=4).contains(&keys.len())
            || keys.iter().any(|k| {
                k.as_str().is_none_or(|id| {
                    !by_id.contains_key(id) || claimed.contains(id) || grouped.contains(id)
                })
            })
        {
            continue;
        }
        let indexes: Vec<_> = keys.iter().map(|id| by_id[id.as_str().unwrap()]).collect();
        if !indexes
            .iter()
            .all(|&i| boxes[i].is_some() && ink::ready(&blocks[i]))
        {
            continue;
        }
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let [a, b, c, d] = union(&member_boxes);
        if originals.iter().any(|&bounds| intersects([a, b, c, d], bounds)) {
            continue;
        }
        let source_area: i32 = member_boxes
            .iter()
            .map(|b| (b[2] - b[0]) * (b[3] - b[1]))
            .sum();
        if ((c - a) * (d - b)) as f64 > source_area as f64 * 1.7 {
            continue;
        }
        grouped.extend(keys.iter().map(|k| k.as_str().unwrap().to_owned()));
        units.push(indexes);
    }
    units.extend(
        (0..blocks.len())
            .filter(|&i| !grouped.contains(&ids[i]))
            .map(|i| vec![i]),
    );
    for indexes in units {
        let index = indexes[0];
        let block = &blocks[index];
        if indexes
            .iter()
            .any(|&i| boxes[i].is_none() || claimed.contains(&ids[i]) || !ink::ready(&blocks[i]))
        {
            continue;
        }
        let source_text: String = indexes
            .iter()
            .map(|&i| blocks[i]["source"].as_str().unwrap_or(""))
            .collect::<String>()
            .nfkc()
            .collect();
        let translated: String = indexes
            .iter()
            .map(|&i| blocks[i]["translation"].as_str().unwrap())
            .collect::<String>()
            .nfkc()
            .collect();
        if source_text.trim() == translated.trim() {
            continue;
        }
        let member_boxes: Vec<_> = indexes.iter().map(|&i| boxes[i].unwrap()).collect();
        let bounds = union(&member_boxes);
        let expanded = padded(bounds, 8, width, height);
        let Some([x, y, r, end]) = trim_around_originals(expanded, bounds, &originals) else {
            continue;
        };
        let mut mask = blank(r - x, end - y, 1)?;
        for other in &accepted {
            let [ox, oy] = other.origin;
            let (ow, oh) = (other.mask.cols(), other.mask.rows());
            let (aa, bb, cc, dd) = (x.max(ox), y.max(oy), r.min(ox + ow), end.min(oy + oh));
            if cc <= aa || dd <= bb {
                continue;
            }
            let target = mask.data_bytes_mut()?;
            let occupied = other.mask.data_bytes()?;
            for yy in bb..dd {
                for xx in aa..cc {
                    target[((yy - y) * (r - x) + xx - x) as usize] &=
                        1 - occupied[((yy - oy) * ow + xx - ox) as usize];
                }
            }
        }
        let Some(shape) = geometry::shape(&mask, [x, y])? else {
            continue;
        };
        let mut covered = true;
        for &bbox in &member_boxes {
            if geometry::coverage(&shape, Some(bbox))? < 0.97 {
                covered = false;
                break;
            }
        }
        if !covered {
            continue;
        }
        let plan = background::prepare(
            gray,
            &member_boxes,
            Some((&shape.mask, shape.origin)),
            true,
            source,
        )?;
        // This is exactly the alpha channel that the old path checked after
        // PNG encoding; count it directly without a redundant decode.
        if plan
            .byte_mask
            .data_bytes()?
            .iter()
            .filter(|&&v| v > 0)
            .count()
            < 8
        {
            continue;
        }
        let mut region = json!({"id":format!("text-{}",ids[index]),"block_ids":indexes.iter().map(|&i| &ids[i]).collect::<Vec<_>>(),
            "overlay":"text","overlay_score":0.6,"overlay_reason":"原文字范围内排版，仅修复字迹，保留画面",
            "points":shape.points,"text_box":[x,y,r,end],"erase_points":[],"fill_color":[255,255,255],"vertical":truth(&block["vertical"])});
        merge(
            &mut region,
            plan.render(color, models.as_deref_mut()).await?,
        );
        regions.push(region);
        claimed.extend(indexes.iter().map(|&i| ids[i].clone()));
        accepted.push(shape);
    }
    Ok(regions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::{core::{Point, Rect, Scalar, Size, Vector}, imgcodecs, imgproc};

    #[tokio::test]
    async fn preserves_all_members_of_one_bubble_and_renders_a_separate_bubble() -> Result<()> {
        let mut gray = blank(480, 640, 25)?;
        for centre in [180, 355] {
            imgproc::ellipse(&mut gray, Point::new(centre, 220), Size::new(64, 80), 0.0, 0.0, 360.0,
                             Scalar::all(255.0), imgproc::FILLED, imgproc::LINE_8, 0)?;
        }
        for x in [154, 197, 347] {
            for y in (180..250).step_by(15) {
                imgproc::rectangle(&mut gray, Rect::new(x, y, 12, 8), Scalar::all(30.0),
                                   imgproc::FILLED, imgproc::LINE_8, 0)?;
            }
        }
        let mut page = json!({"width": 480, "height": 640, "blocks": [
            {"id": "1", "box": [151, 172, 172, 260], "source": "こんにちは", "translation": "你好", "vertical": true},
            {"id": "2", "box": [195, 172, 213, 260], "source": "おはよう", "translation": "早上好", "vertical": true},
            {"id": "3", "box": [343, 172, 363, 260], "source": "またね", "translation": "回头见", "vertical": true}
        ]});
        let combined = regions(&page, &gray, None, None, None).await?;
        assert!(combined.iter().any(|region| region["block_ids"] == json!(["1", "2"])));

        let mut image = Vector::new();
        imgcodecs::imencode(".png", &gray, &mut image, &Vector::new())?;
        page["blocks"][1]["keep_original"] = json!(true);
        assert_eq!(preserved_bubbles::legacy_ids(&page), ["2"]);
        preserved_bubbles::set(&mut page, "2", true, image.as_slice(), &[])?;
        assert!(preserved_bubbles::legacy_ids(&page).is_empty());
        assert!(keeps_original(&page["blocks"][0]));
        assert!(keeps_original(&page["blocks"][1]));
        assert!(!keeps_original(&page["blocks"][2]));
        let displayed = regions(&page, &gray, None, None, None).await?;
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed[0]["block_ids"], json!(["3"]));
        assert!(array(&displayed[0]["points"]).iter().all(|point| point[0].as_i64().unwrap() > 245));

        crate::overlay::ensure_gray(&mut page, Some(&gray))?;
        assert!(array(&page["overlay_regions"]).iter().all(|region| array(&region["block_ids"]).iter().all(|id| id == "3")));
        preserved_bubbles::set(&mut page, "1", false, &[], &[])?;
        assert!(array(&page["blocks"]).iter().all(|block| !keeps_original(block)));
        assert_eq!(page["blocks"][0]["translation"], "你好");
        assert_eq!(page["blocks"][1]["translation"], "早上好");
        Ok(())
    }
}
