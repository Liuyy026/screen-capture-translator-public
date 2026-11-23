//! Extra evidence for an explicitly selected source rectangle. The bounding
//! rectangle alone cannot distinguish Japanese strokes from panel artwork.
use super::{Box4, clipped_rect, ready, source_boxes};
use crate::{compat::array, raster};
use anyhow::{Context, Result};
use opencv::{core::Mat, prelude::*};
use serde_json::{Value, json};

enum Evidence {
    Detected,
    Supplement(Vec<usize>),
    Uncertain,
}

fn evidence(gray: &Mat, detector: &Mat, rect: Box4, protected: &[Box4]) -> Result<Evidence> {
    let [l, t, r, b] = rect;
    let patch = raster::crop(gray, l, t, r - l, b - t)?;
    let pixels = patch.data_bytes()?;
    let ground = raster::percentile(pixels, 50.);
    let foreground = raster::threshold(&patch, |v| {
        if ground >= 128. {
            f64::from(v) < ground - 45.
        } else {
            f64::from(v) > ground + 45.
        }
    })?;
    let fg = foreground.data_bytes()?;
    let detector = detector.data_bytes()?;
    let local_width = (r - l) as usize;
    let global = |p: usize| {
        (p / local_width + t as usize) * gray.cols() as usize + p % local_width + l as usize
    };
    let ink = fg.iter().filter(|&&v| v != 0).count();
    let supported = fg
        .iter()
        .enumerate()
        .filter(|(p, v)| **v != 0 && detector[global(*p)] > 0)
        .count();
    if ink >= 8 && supported as f64 >= 0.65 * ink as f64 {
        return Ok(Evidence::Detected);
    }

    if ink < 8 {
        return Ok(Evidence::Uncertain);
    }
    let (labels, stats) = raster::components(&foreground)?;
    let labels = labels.data_typed::<i32>()?;
    let mut accepted = vec![false; stats.len()];
    let mut dots = 0;
    let mut glyphs = 0;
    let em = (r - l).min(b - t);
    for (i, &[x, y, w, h, area]) in stats.iter().enumerate().skip(1) {
        if area <= 4 {
            dots += 1;
            continue;
        }
        // A hair, frame or outline crossing the crop continues outside it.
        // A long thin confined mark also lacks enough evidence on its own.
        if x == 0
            || y == 0
            || x + w >= r - l
            || y + h >= b - t
            || area < 8
            || w.min(h) < 2
            || w.max(h) > 5 * w.min(h)
            || w.max(h) > (em * 3 / 2).max(8)
            || f64::from(area) < 0.14 * f64::from(w) * f64::from(h)
        {
            continue;
        }
        let bounds = [l + x, t + y, l + x + w, t + y + h];
        if protected
            .iter()
            .any(|o| bounds[0] < o[2] && bounds[2] > o[0] && bounds[1] < o[3] && bounds[3] > o[1])
        {
            continue;
        }
        accepted[i] = true;
        glyphs += 1;
    }
    if glyphs == 0 {
        return Ok(Evidence::Uncertain);
    }
    // A printed screen or a frame segment can dominate the crop's contrast
    // while CTD already covers every character in the line. Consult the
    // detector on the complete, aligned components without authorizing new ink.
    let mut hits = vec![0; stats.len()];
    for (p, &label) in labels.iter().enumerate() {
        if detector[global(p)] > 0 {
            hits[label as usize] += 1;
        }
    }
    let largest = stats
        .iter()
        .zip(&accepted)
        .filter(|(_, ok)| **ok)
        .map(|(s, _)| s[4])
        .max()
        .unwrap();
    let major: Vec<_> = stats
        .iter()
        .enumerate()
        .filter(|(i, s)| accepted[*i] && s[4] >= 12.max(largest / 8))
        .collect();
    let mut across = (i32::MAX, i32::MIN);
    let mut along = (i32::MAX, i32::MIN);
    for &(_, &[x, y, w, h, _]) in &major {
        let (cross, track) = if b - t >= r - l {
            (x + w / 2, y + h / 2)
        } else {
            (y + h / 2, x + w / 2)
        };
        across = (across.0.min(cross), across.1.max(cross));
        along = (along.0.min(track), along.1.max(track));
    }
    if major.len() >= 2
        && across.1 - across.0 <= em / 2
        && along.1 - along.0 >= em / 2
        && major
            .iter()
            .all(|(i, s)| f64::from(hits[*i]) >= 0.8 * f64::from(s[4]))
    {
        return Ok(Evidence::Detected);
    }
    // Adding pixels requires a simple light/dark ground. Texture on an
    // unrecognized line remains untouched even when the crop was confirmed.
    if (dots >= 8 && dots > 2 * glyphs)
        || (65. ..190.).contains(&ground)
        || raster::fraction(pixels, |v| (f64::from(v) - ground).abs() <= 12.) < 0.72
    {
        return Ok(Evidence::Uncertain);
    }
    // Use a confined, character-sized component only when its shape also
    // has recognizably varying strokes, or several such components support
    // one another. A lone line or flat printed blob stays on the original.
    if glyphs == 1 && supported == 0 {
        let i = accepted.iter().position(|is_glyph| *is_glyph).unwrap();
        let [_, _, w, h, area] = stats[i];
        if w < 5 || h < 5 || f64::from(area) > 0.72 * f64::from(w) * f64::from(h) {
            return Ok(Evidence::Uncertain);
        }
    }
    Ok(Evidence::Supplement(
        labels
            .iter()
            .enumerate()
            .filter(|(_, label)| accepted[**label as usize])
            .map(|(i, _)| global(i))
            .collect(),
    ))
}

pub fn prepare_manual_ink(page: &mut Value, gray: &Mat, source: Option<&mut Mat>) -> Result<()> {
    if !array(&page["blocks"])
        .iter()
        .any(|b| b["manual_ocr_boxes"].is_array() && ready(b))
    {
        return Ok(());
    }
    let width = gray.cols();
    let height = gray.rows();
    if let Some(detector) = source.as_deref() {
        anyhow::ensure!(
            detector.typ() == opencv::core::CV_8UC1 && detector.size()? == gray.size()?,
            "选区检测掩码和原图尺寸不同"
        );
    }
    let blocks = page["blocks"].as_array_mut().context("缺少文字块")?;
    let boxes: Vec<_> = blocks
        .iter()
        .map(|b| source_boxes(b, width, height))
        .collect();
    let mut all_additions = Vec::new();
    for (i, block) in blocks.iter_mut().enumerate() {
        let manual = array(&block["manual_ocr_boxes"]);
        if manual.is_empty() || !ready(block) {
            continue;
        }
        let protected: Vec<_> = boxes
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .flat_map(|(_, boxes)| boxes.iter().copied())
            .collect();
        let mut additions = Vec::new();
        let mut safe = true;
        for value in manual {
            let result = match (clipped_rect(value, width, height), source.as_deref()) {
                (Some(rect), Some(detector)) => evidence(gray, detector, rect, &protected)?,
                _ => Evidence::Uncertain,
            };
            match result {
                Evidence::Detected => {}
                Evidence::Supplement(pixels) => additions.extend(pixels),
                Evidence::Uncertain => {
                    safe = false;
                    break;
                }
            }
        }
        if safe {
            all_additions.extend(additions);
        } else {
            // This is a disposable display-page copy. The confirmed text and
            // translation remain intact in the user's page cache and inspector.
            block["manual_ink_uncertain"] = json!(true);
        }
    }
    if let Some(mask) = source
        && !all_additions.is_empty()
    {
        let output = mask.data_bytes_mut()?;
        for pixel in all_additions {
            output[pixel] = 255;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::{
        core::{Point, Rect, Scalar},
        imgproc,
    };

    #[test]
    fn repair_keeps_crossing_art_and_covered_glyphs_in_a_generous_crop() -> Result<()> {
        let mut gray = raster::blank(120, 110, 250)?;
        let mut mask = raster::blank(120, 110, 0)?;
        let boxes = [
            Rect::new(49, 33, 7, 10),
            Rect::new(48, 51, 9, 12),
            Rect::new(47, 71, 8, 12),
        ];
        for rect in boxes {
            imgproc::rectangle(&mut gray, rect, Scalar::all(15.), 2, 8, 0)?;
        }
        imgproc::line(
            &mut gray,
            Point::new(10, 12),
            Point::new(44, 99),
            Scalar::all(15.),
            2,
            8,
            0,
        )?;
        // Part of the real text is known to CTD. The third glyph is missing.
        for rect in &boxes[..2] {
            imgproc::rectangle(&mut mask, *rect, Scalar::all(255.), 2, 8, 0)?;
        }
        let rect = [20, 20, 79, 93];
        let mut page = json!({"blocks":[{"id":"1","box":rect,"source_boxes":[rect],"manual_ocr_boxes":[rect],
            "translation":"已确认","edited":true}]});
        prepare_manual_ink(&mut page, &gray, Some(&mut mask))?;
        assert!(ready(&page["blocks"][0]));
        assert_eq!(*mask.at_2d::<u8>(74, 47)?, 255); // missing glyph recovered
        assert_eq!(*mask.at_2d::<u8>(35, 49)?, 255); // supported glyph retained
        assert_eq!(*gray.at_2d::<u8>(63, 30)?, 15);
        assert_eq!(*mask.at_2d::<u8>(63, 30)?, 0); // diagonal artwork preserved
        Ok(())
    }

    #[test]
    fn strong_detector_is_authoritative_and_screentone_stays_original() -> Result<()> {
        let mut gray = raster::blank(64, 80, 250)?;
        let mut mask = raster::blank(64, 80, 0)?;
        for (i, y) in [17, 31, 45].into_iter().enumerate() {
            let rect = Rect::new(30, y, 7, 8);
            imgproc::rectangle(&mut gray, rect, Scalar::all(10.), -1, 8, 0)?;
            if i < 2 {
                imgproc::rectangle(&mut mask, rect, Scalar::all(255.), -1, 8, 0)?;
            }
        }
        let rect = [14, 9, 49, 62];
        assert!(matches!(
            evidence(&gray, &mask, rect, &[])?,
            Evidence::Detected
        ));
        let mut pattern = raster::blank(64, 80, 250)?;
        for y in (10..70).step_by(4) {
            for x in (8..53).step_by(4) {
                *pattern.at_2d_mut::<u8>(y, x)? = 30;
            }
        }
        let original_mask = raster::blank(64, 80, 0)?;
        let mut mask = original_mask.try_clone()?;
        let mut page =
            json!({"blocks":[{"id":"1","box":rect,"manual_ocr_boxes":[rect],"translation":"旧"}]});
        prepare_manual_ink(&mut page, &pattern, Some(&mut mask))?;
        assert!(!ready(&page["blocks"][0]));
        assert!(super::super::preserved_markers(&page).get("1").is_some());
        assert_eq!(mask.data_bytes()?, original_mask.data_bytes()?);
        // Dense dots dilute whole-rectangle support. Individually recognized
        // aligned glyphs use the detector exactly and leave all dots alone.
        for y in [16, 35, 51] {
            let glyph = Rect::new(29, y, 5, 7);
            imgproc::rectangle(&mut pattern, glyph, Scalar::all(8.), -1, 8, 0)?;
            imgproc::rectangle(&mut mask, glyph, Scalar::all(255.), -1, 8, 0)?;
        }
        assert!(matches!(
            evidence(&pattern, &mask, rect, &[])?,
            Evidence::Detected
        ));
        Ok(())
    }
}
