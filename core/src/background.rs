//! Plan background restoration in native code. The model receives only the
//! selected local crop and final ink mask; it makes no geometric decisions.
use crate::{ink::Box4, raster::*};
use anyhow::Result;
use opencv::{
    core::{self, Mat, Point, Size},
    imgproc,
    prelude::*,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub struct BackgroundPlan {
    pub bounds: Box4,
    pub patch: Mat,
    pub allowed: Mat,
    pub byte_mask: Mat,
    pub textured: bool,
}

impl BackgroundPlan {
    pub fn snapshot(&self) -> Result<Value> {
        let hash =
            |m: &Mat| -> Result<String> { Ok(format!("{:x}", Sha256::digest(m.data_bytes()?))) };
        Ok(
            json!({"background_box": self.bounds, "textured": self.textured,
            "patch_sha256": hash(&self.patch)?, "allowed_sha256": hash(&self.allowed)?, "mask_sha256": hash(&self.byte_mask)?}),
        )
    }
}

fn paint_rect(mask: &mut Mat, bounds: Box4, value: u8) -> Result<()> {
    let (w, h) = (mask.cols(), mask.rows());
    let [a, b, c, d] = bounds;
    let (a, c) = (a.clamp(0, w), c.clamp(0, w));
    if c <= a {
        return Ok(());
    }
    let data = mask.data_bytes_mut()?;
    for y in b.clamp(0, h)..d.clamp(0, h) {
        data[(y * w + a) as usize..(y * w + c) as usize].fill(value);
    }
    Ok(())
}

fn surrounding(patch: &Mat, stat: [i32; 5], margin: i32, border_only: bool) -> Result<Vec<u8>> {
    let [x, y, w, h, _] = stat;
    let (pw, ph) = (patch.cols(), patch.rows());
    let data = patch.data_bytes()?;
    let mut result = vec![];
    for yy in (y - margin).max(0)..(y + h + margin).min(ph) {
        for xx in (x - margin).max(0)..(x + w + margin).min(pw) {
            if !border_only || yy < y || yy >= y + h || xx < x || xx >= x + w {
                result.push(data[(yy * pw + xx) as usize]);
            }
        }
    }
    Ok(result)
}

fn mean(values: &[u8]) -> f64 {
    values.iter().map(|&v| v as f64).sum::<f64>() / values.len() as f64
}

fn label_counts(labels: &[i32], mask: &[u8], count: usize) -> Vec<usize> {
    let mut result = vec![0; count];
    for (&id, &v) in labels.iter().zip(mask) {
        if v > 0 {
            result[id as usize] += 1;
        }
    }
    result
}

pub fn prepare(
    gray: &Mat,
    boxes: &[Box4],
    clip: Option<(&Mat, [i32; 2])>,
    mut textured: bool,
    source: Option<&Mat>,
) -> Result<BackgroundPlan> {
    anyhow::ensure!(
        !boxes.is_empty() && gray.typ() == core::CV_8UC1,
        "background requires source boxes and a gray image"
    );
    anyhow::ensure!(
        boxes.iter().all(|&[a, b, c, d]| a >= 0
            && b >= 0
            && c <= gray.cols()
            && d <= gray.rows()
            && c > a
            && d > b),
        "background bounds outside image"
    );
    if let Some(source) = source {
        anyhow::ensure!(
            source.typ() == core::CV_8UC1 && source.size()? == gray.size()?,
            "text mask size mismatch"
        );
    }
    let reach = ((boxes
        .iter()
        .map(|b| (b[2] - b[0]).min(b[3] - b[1]))
        .min()
        .unwrap() as f64
        * 0.28) as i32)
        .clamp(10, 24);
    let margin = (reach + 8).max(96);
    let left = (boxes.iter().map(|b| b[0]).min().unwrap() - margin).max(0);
    let top = (boxes.iter().map(|b| b[1]).min().unwrap() - margin).max(0);
    let right = (boxes.iter().map(|b| b[2]).max().unwrap() + margin).min(gray.cols());
    let bottom = (boxes.iter().map(|b| b[3]).max().unwrap() + margin).min(gray.rows());
    let (width, height) = (right - left, bottom - top);
    let patch = crop(gray, left, top, width, height)?;
    let mut allowed = blank(width, height, 1)?;
    if let Some((mask, [x, y])) = clip {
        let eroded = morph(&pad(mask, 3, false)?, 7, false)?;
        let safe = crop(&eroded, 3, 3, mask.cols(), mask.rows())?;
        let data = allowed.data_bytes_mut()?;
        data.fill(0);
        let (a, b, c, d) = (
            left.max(x),
            top.max(y),
            right.min(x + mask.cols()),
            bottom.min(y + mask.rows()),
        );
        if c > a && d > b {
            let pixels = safe.data_bytes()?;
            for yy in b..d {
                for xx in a..c {
                    data[((yy - top) * width + xx - left) as usize] =
                        pixels[((yy - y) * mask.cols() + xx - x) as usize];
                }
            }
        }
    }
    let mut seeds = blank(width, height, 0)?;
    for &[a, b, c, d] in boxes {
        paint_rect(
            &mut seeds,
            [
                a - left - reach,
                b - top - reach,
                c - left + reach,
                d - top + reach,
            ],
            1,
        )?;
    }
    let dark = threshold(&patch, |v| v < if textured { 75 } else { 205 })?;
    let (labels_mat, stats) = components(&dark)?;
    let labels = labels_mat.data_typed::<i32>()?;
    let allowed_pixels = allowed.data_bytes()?;
    let seed_pixels = seeds.data_bytes()?;
    let combined: Vec<_> = seed_pixels
        .iter()
        .zip(allowed_pixels)
        .map(|(&s, &a)| u8::from(s > 0 && a > 0))
        .collect();
    let overlaps = label_counts(labels, &combined, stats.len());
    let contained = label_counts(labels, allowed_pixels, stats.len());
    let mut selected = vec![0u8; stats.len()];
    let mut dashes = selected.clone();
    for (index, &stat) in stats.iter().enumerate().skip(1) {
        let [x, y, w, h, area] = stat;
        if (contained[index] as f64) < area as f64 * 0.98 {
            continue;
        }
        let mut glyph = area >= 10 && w.max(h) >= 6;
        if textured && glyph {
            let border = surrounding(&patch, stat, 3, true)?;
            glyph = !border.is_empty() && mean(&border) > 145.0;
        }
        if textured && !glyph && area >= 3 {
            glyph =
                mean(&surrounding(&patch, stat, 3, false)?) > if area >= 7 { 190.0 } else { 220.0 };
        }
        let middle = (left + x) as f64 + w as f64 / 2.0;
        let aligned = boxes.iter().any(|&[a, b, c, d]| {
            (a - 4) as f64 <= middle
                && middle <= (c + 4) as f64
                && b - 80 <= top + y
                && top + y + h <= d + 80
        });
        let dash = !textured
            && w.min(h) <= 8
            && w.max(h) >= 30
            && w.max(h) >= 8 * w.min(h)
            && (overlaps[index] as f64 >= area as f64 * 0.98
                || (clip.is_some() && h > w && aligned));
        dashes[index] = u8::from(dash);
        if overlaps[index] >= 2
            && (if textured { glyph } else { area >= 2 })
            && (w.max(h) <= (reach * 5).max(90) || dash)
        {
            selected[index] = 1;
        }
    }
    let mut ink = blank(width, height, 0)?;
    for (v, &id) in ink.data_bytes_mut()?.iter_mut().zip(labels) {
        *v = selected[id as usize];
    }
    if textured {
        for (index, &[x, y, w, h, _]) in stats.iter().enumerate() {
            if selected[index] > 0 {
                paint_rect(&mut ink, [x, y, x + w, y + h], 1)?;
            }
        }
        let mut core = blank(width, height, 0)?;
        for &[a, b, c, d] in boxes {
            paint_rect(&mut core, [a - left, b - top, c - left, d - top], 1)?;
        }
        let (light_labels_mat, light_stats) = components(&threshold(&patch, |v| v > 210)?)?;
        let light_labels = light_labels_mat.data_typed::<i32>()?;
        let inside = label_counts(light_labels, core.data_bytes()?, light_stats.len());
        let light_contained = label_counts(light_labels, allowed_pixels, light_stats.len());
        let core_values = select(&patch, &core)?;
        let dark_ground = !core_values.is_empty() && percentile(&core_values, 50.0) < 180.0;
        for (index, &stat) in light_stats.iter().enumerate().skip(1) {
            let [x, y, w, h, area] = stat;
            if area < 8
                || w.max(h) < 6
                || w.max(h) > (reach * 5).max(90)
                || (inside[index] as f64) < area as f64 * 0.75
                || (light_contained[index] as f64) < area as f64 * 0.98
                || !dark_ground
            {
                continue;
            }
            if percentile(&surrounding(&patch, stat, 5, false)?, 50.0) < 185.0 {
                paint_rect(&mut ink, [x, y, x + w, y + h], 1)?;
            }
        }
        let mut closed = Mat::default();
        imgproc::morphology_ex_def(&ink, &mut closed, imgproc::MORPH_CLOSE, &blank(5, 5, 1)?)?;
        ink = closed;
    }
    if let Some(source) = source {
        let local = crop(source, left, top, width, height)?;
        let evidence = local.data_bytes()?;
        for ((v, &s), &seed) in ink
            .data_bytes_mut()?
            .iter_mut()
            .zip(evidence)
            .zip(seed_pixels)
        {
            *v = u8::from(s > 0 && seed > 0);
        }
        for (index, &[x, y, w, h, area]) in stats.iter().enumerate() {
            if dashes[index] == 0 || overlaps[index] as f64 >= area as f64 * 0.98 {
                continue;
            }
            let (mut size, mut present) = (0, 0);
            for yy in y..y + h {
                for xx in x..x + w {
                    let p = (yy * width + xx) as usize;
                    if labels[p] as usize == index {
                        size += 1;
                        present += usize::from(evidence[p] > 0);
                    }
                }
            }
            if (present as f64 / size as f64) < 0.12 {
                dashes[index] = 0;
            }
        }
        for (v, &label) in ink.data_bytes_mut()?.iter_mut().zip(labels) {
            *v |= dashes[label as usize];
        }
        if ink.data_bytes()?.iter().any(|&v| v > 0) {
            let distance = distance(&threshold(&ink, |v| v == 0)?, false)?;
            let distances = distance.data_typed::<f32>()?;
            for (index, &stat) in stats.iter().enumerate() {
                let [x, y, w, h, area] = stat;
                if selected[index] == 0
                    || !(8..=900).contains(&area)
                    || w.min(h) < 3
                    || w.max(h) > 60
                    || (overlaps[index] as f64) < area as f64 * 0.98
                    || (contained[index] as f64) < area as f64 * 0.98
                {
                    continue;
                }
                let mut pixels = vec![];
                let mut nearby = f32::INFINITY;
                for yy in y..y + h {
                    for xx in x..x + w {
                        let p = (yy * width + xx) as usize;
                        if labels[p] as usize == index {
                            pixels.push(p);
                            nearby = nearby.min(distances[p]);
                        }
                    }
                }
                if nearby > 40.0 {
                    continue;
                }
                let border = surrounding(&patch, stat, 4, true)?;
                if !border.is_empty()
                    && mean(&border) > 220.0
                    && fraction(&border, |v| v >= 245) >= 0.65
                {
                    let values = ink.data_bytes_mut()?;
                    for p in pixels {
                        values[p] = 1;
                    }
                }
            }
        }
        let surround = morph(&ink, 17, true)?;
        let values: Vec<_> = patch
            .data_bytes()?
            .iter()
            .zip(surround.data_bytes()?)
            .zip(ink.data_bytes()?)
            .zip(allowed_pixels)
            .filter_map(|(((&p, &s), &i), &a)| (s > 0 && i == 0 && a > 0).then_some(p))
            .collect();
        if !values.is_empty() {
            let m = mean(&values);
            let std = (values.iter().map(|&v| (v as f64 - m).powi(2)).sum::<f64>()
                / values.len() as f64)
                .sqrt();
            textured |= fraction(&values, |v| v < 235) > 0.12 || std > 8.0;
        }
    }
    let radius = if textured {
        if source.is_some() { 5 } else { 6 }
    } else {
        2
    };
    let kernel = imgproc::get_structuring_element(
        imgproc::MORPH_ELLIPSE,
        Size::new(2 * radius + 1, 2 * radius + 1),
        Point::new(-1, -1),
    )?;
    let mut expanded = Mat::default();
    imgproc::dilate_def(&ink, &mut expanded, &kernel)?;
    let dark_pixels = dark.data_bytes()?;
    for (i, v) in expanded.data_bytes_mut()?.iter_mut().enumerate() {
        *v &= allowed_pixels[i];
        if source.is_none() && dark_pixels[i] > 0 && selected[labels[i] as usize] == 0 {
            *v = 0;
        }
        *v *= 255;
    }
    Ok(BackgroundPlan {
        bounds: [left, top, right, bottom],
        patch,
        allowed,
        byte_mask: expanded,
        textured,
    })
}
