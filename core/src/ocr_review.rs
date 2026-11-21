//! Reversible image-evidence checks for OCR hallucinations on screentone.
//! The recognizer always produces text; that alone does not prove ink exists.
//! Keep every source/translation and ID, but exclude suspicious candidates from
//! automatic translation, context and compositing until a person confirms them.
use crate::{compat::{array, digest, truth}, ink, raster::*};
use anyhow::{Result, ensure};
use opencv::{core::{self, Mat, Size}, imgproc, prelude::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERSION: u32 = 1;
// Bump when the original/legacy canvas reconstruction or BGR-to-gray conversion changes.
const CANVAS_VERSION: u32 = 1;

fn block_inputs(page: &Value) -> Vec<Value> {
    array(&page["blocks"]).iter().map(|b| json!([b["id"], b["box"], b["source"]])).collect()
}

fn source_receipt(page: &Value, source_sha256: &str) -> Value {
    let signature = digest(&json!([source_sha256, page["width"], page["height"],
        page["layout_provenance"], block_inputs(page), page["ocr_review_signature"]]), false);
    json!({"review_version": VERSION, "canvas_version": CANVAS_VERSION, "signature": signature})
}

/// The caller has verified the encoded file's digest. Its pixels need not be
/// decoded again while all inputs to the original review still match.
pub fn source_current(page: &Value, source_sha256: &str) -> bool {
    page.get("ocr_review_cache").is_some_and(|receipt| *receipt == source_receipt(page, source_sha256))
}

pub fn ensure_for_source(page: &mut Value, source_sha256: &str,
    load_gray: impl FnOnce(&Value) -> Result<Mat>) -> Result<bool> {
    if source_current(page, source_sha256) { return Ok(false); }
    let gray = load_gray(page)?;
    // A legacy pixel signature can still prove the review valid after decoding.
    // Add the source receipt even when no block needed to be reviewed again.
    ensure(page, &gray)?;
    page["ocr_review_cache"] = source_receipt(page, source_sha256);
    Ok(true)
}

pub fn suppressed(block: &Value) -> bool {
    block["ocr_review"]["status"] == "suspect"
        && !truth(&block["ocr_confirmed"]) && !truth(&block["edited"])
}

pub fn reason(block: &Value) -> &str {
    block["ocr_review"]["reason"].as_str().unwrap_or("疑似非文字，请核对原图")
}

fn interior(stat: &[i32; 5], width: i32, height: i32) -> bool {
    let [x, y, w, h, _] = *stat;
    x > 0 && y > 0 && x + w < width && y + h < height
}

fn support(mask: &Mat, min_area: f64, min_extent: f64) -> Result<(usize, usize)> {
    let (_, stats) = components(mask)?;
    let mut total = 0;
    let mut coherent = 0;
    for s in stats.iter().skip(1).filter(|s| interior(s, mask.cols(), mask.rows())) {
        total += s[4] as usize;
        if s[4] as f64 >= min_area && s[2].max(s[3]) as f64 >= min_extent {
            coherent += s[4] as usize;
        }
    }
    Ok((total, coherent))
}

pub fn ensure(page: &mut Value, gray: &Mat) -> Result<bool> {
    ensure!(gray.typ() == core::CV_8UC1 && !gray.empty(), "invalid OCR review image");
    ensure!(page["width"].as_i64() == Some(gray.cols() as i64)
        && page["height"].as_i64() == Some(gray.rows() as i64), "OCR review image size mismatch");
    ensure!(page["blocks"].is_array() && array(&page["blocks"]).iter().all(Value::is_object), "invalid OCR blocks");
    let signature = digest(&json!([VERSION, page["width"], page["height"],
        format!("{:x}", Sha256::digest(gray.data_bytes()?)),
        block_inputs(page)]), false);
    if page["ocr_review_signature"] == signature { return Ok(false); }

    let scale = f64::from(gray.cols().min(gray.rows())) / 1280.;
    let kernel = ((scale * 3.).round() as i32 | 1).clamp(3, 15);
    let mut smooth = Mat::default();
    imgproc::gaussian_blur_def(gray, &mut smooth, Size::new(kernel, kernel), 0.)?;
    let min_area = (6. * scale * scale).max(4.);
    let min_extent = (4. * scale).max(3.);
    if let Some(blocks) = page["blocks"].as_array_mut() {
        for block in blocks {
            // Re-review never edits recognition, translations, or manual choices.
            block.as_object_mut().unwrap().remove("ocr_review");
            let Some([x,y,r,b]) = ink::clipped_box(block, gray.cols(), gray.rows()) else { continue; };
            let patch = crop(gray, x, y, r-x, b-y)?;
            let blurred = crop(&smooth, x, y, r-x, b-y)?;
            let pixels = patch.data_bytes()?;
            let median = percentile(pixels, 50.);
            let inverse = median < 128.;
            let raw = threshold(&patch, |v| if inverse { v > 115 } else { v < 140 })?;
            let ink = threshold(&blurred, |v| if inverse { v > 115 } else { v < 140 })?;
            let raw_total = raw.data_bytes()?.iter().filter(|&&v| v > 0).count();
            let (raw_interior, _) = support(&raw, min_area, min_extent)?;
            let (smooth_interior, mut coherent) = support(&ink, min_area, min_extent)?;
            // White letters on colored/dark grounds must remain valid evidence.
            // Small antialiased white letters may never reach pure white.
            let white_cutoff = (median + 35.).min(220.);
            let white = threshold(&blurred, |v| f64::from(v) > white_cutoff)?;
            coherent = coherent.max(support(&white, min_area, min_extent)?.1);
            let loss = 1. - smooth_interior as f64 / raw_interior.max(1) as f64;
            let mean = pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / pixels.len() as f64;
            let std = (pixels.iter().map(|&v| (f64::from(v)-mean).powi(2)).sum::<f64>() / pixels.len() as f64).sqrt();
            let empty = (raw_total as f64) < min_area && std < 32.;
            let fragmented = ((coherent as f64) < min_area && loss > 0.5)
                || ((coherent as f64) < (15. * scale * scale).max(8.) && loss > 0.75);
            if ((coherent as f64) < min_area && empty) || fragmented {
                let (code, reason) = if empty {
                    ("no_ink", "区域内缺少文字笔画，疑似将背景识别成文字。已暂停翻译和回填，可核对后确认为文字。")
                } else {
                    ("screentone", "区域主要是细碎网点，缺少连续文字笔画。已暂停翻译和回填，可核对后确认为文字。")
                };
                block["ocr_review"] = json!({"version":VERSION,"status":"suspect","code":code,"reason":reason,
                    "evidence":{"raw_ink":raw_interior,"coherent_ink":coherent,"fine_ink_loss":loss}});
            }
        }
    }
    page["ocr_review_signature"] = json!(signature);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencv::core::{Point, Rect, Scalar};

    #[test]
    fn legacy_review_migrates_once_and_only_matching_inputs_skip_decode() -> Result<()> {
        let gray = blank(64, 32, 255)?;
        let mut page = json!({"width":64,"height":32,"blocks":[
            {"id":"1","box":[4,4,18,20],"source":"こんにちは","translation":"旧译文",
             "edited":true,"ocr_confirmed":true,"keep_original":true}]});
        ensure(&mut page, &gray)?;
        let legacy_page = page.clone();
        let source = "a".repeat(64);
        let changed_source = "b".repeat(64);
        assert!(!source_current(&page, &source));
        assert!(ensure_for_source(&mut page, &source, |_| Ok(gray.clone()))?);
        let mut migrated = page.clone();
        migrated.as_object_mut().unwrap().remove("ocr_review_cache");
        assert_eq!(migrated, legacy_page);

        page["blocks"][0]["translation"] = json!("新译文");
        page["blocks"][0]["ocr_confirmed"] = json!(false);
        page["blocks"][0]["edited"] = json!(false);
        page["blocks"][0]["keep_original"] = json!(false);
        assert!(!ensure_for_source(&mut page, &source, |_| panic!("decoded a reviewed page"))?);
        assert!(!source_current(&page, &changed_source));

        for (pointer, value) in [
            ("/blocks/0/id", json!("2")), ("/blocks/0/box/0", json!(5)),
            ("/blocks/0/source", json!("さようなら")), ("/width", json!(65)),
            ("/height", json!(33)), ("/ocr_review_cache/review_version", json!(0)),
            ("/ocr_review_cache/canvas_version", json!(0)), ("/ocr_review_signature", json!("stale")),
        ] {
            let mut changed = page.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(!source_current(&changed, &source), "failed to invalidate {pointer}");
        }
        // The layout can change at the same dimensions. Its provenance also
        // identifies the reconstruction settings used for an imported page.
        page["layout_provenance"] = json!({"version":1,"source_sha256":source,"settings":{"target":64}});
        assert!(ensure_for_source(&mut page, &source, |_| Ok(gray.clone()))?);
        assert!(source_current(&page, &source));
        page["layout_provenance"]["source_sha256"] = json!(changed_source);
        assert!(!source_current(&page, &source));
        page["layout_provenance"]["source_sha256"] = json!(source);
        page["layout_provenance"]["settings"]["target"] = json!(128);
        assert!(!source_current(&page, &source));
        Ok(())
    }

    #[test]
    fn dotted_art_is_reversible_and_real_ink_survives() -> Result<()> {
        let mut image = blank(1280, 900, 255)?;
        for y in (20..150).step_by(6) { for x in (20..230).step_by(6) {
            imgproc::circle(&mut image, Point::new(x,y), 0, Scalar::all(20.), -1, 8, 0)?;
        }}
        // A real thin vertical annotation, and white lettering on a dark panel.
        for y in (30..160).step_by(20) {
            imgproc::rectangle(&mut image, Rect::new(310,y,8,10), Scalar::all(20.), -1, 8, 0)?;
        }
        imgproc::rectangle(&mut image, Rect::new(400,10,150,180), Scalar::all(50.), -1, 8, 0)?;
        for y in (30..160).step_by(20) {
            imgproc::rectangle(&mut image, Rect::new(430,y,8,10), Scalar::all(250.), -1, 8, 0)?;
        }
        let mut page = json!({"width":1280,"height":900,"blocks":[
            {"id":"pattern","box":[10,10,240,180],"source":"そんなはずない","translation":"不会吧"},
            {"id":"blank","box":[600,10,700,100],"source":"こんにちは","translation":"你好"},
            {"id":"vertical","box":[300,10,330,180],"source":"言われてみれば","translation":"这么一说"},
            {"id":"white","box":[410,20,540,180],"source":"あいうえお","translation":"测试"}]});
        let texts: Vec<_> = array(&page["blocks"]).iter().map(|b| (b["source"].clone(),b["translation"].clone())).collect();
        assert!(ensure(&mut page,&image)?);
        assert!(suppressed(&page["blocks"][0])); assert!(suppressed(&page["blocks"][1]));
        assert!(!suppressed(&page["blocks"][2])); assert!(!suppressed(&page["blocks"][3]));
        assert!(!ensure(&mut page,&image)?);
        assert_eq!(texts, array(&page["blocks"]).iter().map(|b| (b["source"].clone(),b["translation"].clone())).collect::<Vec<_>>());
        page["blocks"][0]["ocr_confirmed"] = json!(true);
        page["blocks"][1]["edited"] = json!(true);
        assert!(!suppressed(&page["blocks"][0])); assert!(!suppressed(&page["blocks"][1]));
        // A different image with the same dimensions must be checked again.
        let different = blank(1280, 900, 255)?;
        assert!(ensure(&mut page, &different)?);
        assert!(suppressed(&page["blocks"][2]));
        assert!(!suppressed(&page["blocks"][0]));
        Ok(())
    }

    #[test]
    fn antialiased_light_letters_are_not_screentone() -> Result<()> {
        let mut image = blank(1280, 900, 150)?;
        for x in (30..130).step_by(12) {
            imgproc::rectangle(&mut image, Rect::new(x,30,3,10), Scalar::all(215.), -1, 8, 0)?;
        }
        let mut page = json!({"width":1280,"height":900,"blocks":[
            {"id":"small","box":[20,20,150,50],"source":"白い文字","translation":"白字"}]});
        ensure(&mut page, &image)?;
        assert!(!suppressed(&page["blocks"][0]));
        page["blocks"] = json!([null]);
        assert!(ensure(&mut page, &image).is_err());
        Ok(())
    }
}
