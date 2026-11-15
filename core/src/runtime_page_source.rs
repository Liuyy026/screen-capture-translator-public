//! Fixed OCR canvas and compatibility with the older per-enhancement cache.
use crate::{
    cache_policy::{hex_key, plain_dir},
    compat::{array, keeps_original, truth},
    imaging,
    super_resolution::{self, Settings},
};
use anyhow::{Context, Result, ensure};
use opencv::{
    core::{Mat, Size, Vector},
    imgcodecs, imgproc,
    prelude::*,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{borrow::Cow, fs, path::Path};

fn read_json(path: &Path, limit: u64) -> Option<Value> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn canvas(page: &Value) -> Option<(i32, i32)> {
    let width = i32::try_from(page["width"].as_u64()?).ok()?;
    let height = i32::try_from(page["height"].as_u64()?).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

pub(super) fn legacy_enhanced_page(
    original: &Path,
    effective: &Path,
    cache: &Path,
    sr_root: &Path,
    page_root: &Path,
    mode: &str,
) -> Result<Option<Value>> {
    let source_sha256 = cache
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("missing source hash")?;
    let legacy_root = page_root.join("enhanced-v1");
    if !plain_dir(&legacy_root) {
        return Ok(None);
    }
    let mut original_size = None;
    let mut best = None;
    let mut candidates = 0;
    for entry in fs::read_dir(legacy_root)? {
        let entry = entry?;
        let key = entry.file_name();
        let Some(key) = key.to_str().filter(|key| hex_key(key)) else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let sr_dir = sr_root.join(key);
        if !plain_dir(&sr_dir) {
            continue;
        }
        let Some(sidecar) = read_json(&sr_dir.join("identity.json"), 65_536) else {
            continue;
        };
        let identity = &sidecar["identity"];
        if identity["schema"] != 1
            || identity["source_sha256"] != source_sha256
            || format!("{:x}", Sha256::digest(serde_json::to_vec(identity)?)) != key
        {
            continue;
        }
        let Some(output_sha256) = sidecar["output_sha256"]
            .as_str()
            .filter(|hash| hex_key(hash))
        else {
            continue;
        };
        let directory = if mode == "vision" {
            entry.path().join("vision-v1")
        } else {
            entry.path()
        };
        if !plain_dir(&directory) {
            continue;
        }
        let legacy = directory.join(format!("{output_sha256}.json"));
        let Some(page) = read_json(&legacy, 16 * 1024 * 1024) else {
            continue;
        };
        let Some((width, height)) = canvas(&page).filter(|_| page["blocks"].is_array()) else {
            continue;
        };
        if identity["width"] != width || identity["height"] != height {
            continue;
        }
        let Ok(settings) = serde_json::from_value::<Settings>(identity["settings"].clone()) else {
            continue;
        };
        let (source_width, source_height) = match original_size {
            Some(size) => size,
            None => {
                let image = imaging::decode_color(original)?;
                let size = (image.cols() as u32, image.rows() as u32);
                original_size = Some(size);
                size
            }
        };
        if super_resolution::dimensions(source_width, source_height, &settings)
            .ok()
            .flatten()
            != Some((width as u32, height as u32))
        {
            continue;
        }

        let mut manual = page["cache_retention"]["kind"] == "manual";
        let mut translated = 0;
        let mut eligible = 0;
        for block in array(&page["blocks"]) {
            manual |= truth(&block["edited"]) || keeps_original(block);
            if keeps_original(block) || block["overlay"] == "skip" || block["text_kind"] == "noise"
            {
                continue;
            }
            eligible += 1;
            if truth(&block["translation"]) && !truth(&block["error"]) {
                translated += 1;
            }
        }
        candidates += 1;
        let exact = effective.parent() == Some(sr_dir.as_path());
        let rank = (
            manual,
            translated > 0 && translated == eligible,
            translated,
            exact,
            key.to_owned(),
        );
        if best
            .as_ref()
            .is_none_or(|(previous, _, _)| rank > *previous)
        {
            best = Some((rank, page, identity["settings"].clone()));
        }
    }
    Ok(best.map(|((_, _, _, _, identity), mut page, settings)| {
        page["layout_provenance"] = json!({
            "version": 1, "source_sha256": source_sha256, "legacy_enhancement": identity,
            "settings": settings, "legacy_candidates": candidates
        });
        page
    }))
}

pub(super) fn source_canvas<'a>(page: &Value, image: &'a [u8]) -> Result<Cow<'a, [u8]>> {
    let provenance = &page["layout_provenance"];
    if provenance["version"] != 1
        || provenance["source_sha256"] != format!("{:x}", Sha256::digest(image))
    {
        return Ok(Cow::Borrowed(image));
    }
    let (width, height) = canvas(page).context("missing OCR size")?;
    let original = imaging::decode_bytes(image, imgcodecs::IMREAD_COLOR)?;
    let settings: Settings = serde_json::from_value(provenance["settings"].clone())?;
    ensure!(
        super_resolution::dimensions(original.cols() as u32, original.rows() as u32, &settings)?
            == Some((width as u32, height as u32)),
        "legacy OCR differs from the original page geometry"
    );
    let mut scaled = Mat::default();
    imgproc::resize(
        &original,
        &mut scaled,
        Size::new(width, height),
        0.,
        0.,
        imgproc::INTER_CUBIC,
    )?;
    let mut encoded = Vector::new();
    ensure!(
        imgcodecs::imencode(".png", &scaled, &mut encoded, &Vector::new())?,
        "cannot encode OCR canvas"
    );
    Ok(Cow::Owned(encoded.to_vec()))
}

pub(super) fn rebase_ocr(page: &mut Value, original: &Path) -> Result<()> {
    let original = imaging::decode_color(original)?;
    let (width, height) = canvas(page).context("missing OCR size")?;
    let sx = f64::from(original.cols()) / f64::from(width);
    let sy = f64::from(original.rows()) / f64::from(height);
    for block in page["blocks"]
        .as_array_mut()
        .context("missing OCR blocks")?
    {
        for (coord, scale) in block["box"]
            .as_array_mut()
            .context("missing OCR box")?
            .iter_mut()
            .zip([sx, sy, sx, sy])
        {
            *coord = json!(coord.as_f64().context("invalid OCR coordinate")? * scale);
        }
    }
    page["width"] = json!(original.cols());
    page["height"] = json!(original.rows());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence;
    use opencv::core::{CV_8UC3, Scalar};
    use std::path::PathBuf;

    fn original_file(root: &Path) -> Result<PathBuf> {
        let image = Mat::new_rows_cols_with_default(540, 389, CV_8UC3, Scalar::all(255.))?;
        let path = root.join("page.png");
        ensure!(imgcodecs::imwrite(
            path.to_str().unwrap(),
            &image,
            &Vector::new()
        )?);
        Ok(path)
    }

    fn legacy_fixture(
        root: &Path,
        source_sha256: &str,
        mode: &str,
        preset: &str,
        edited: bool,
    ) -> Result<(PathBuf, PathBuf)> {
        let output_sha256 = format!("{:x}", Sha256::digest(preset.as_bytes()));
        let identity = json!({
            "schema": 1, "source_sha256": source_sha256, "width": 1037, "height": 1440,
            "settings": Settings { engine: "realcugan".into(), preset: preset.into(), ..Settings::default() }
        });
        let key = format!("{:x}", Sha256::digest(serde_json::to_vec(&identity)?));
        let directory = root.join("sr").join(&key);
        persistence::atomic_json(
            &directory.join("identity.json"),
            &json!({
                "identity": identity, "output_sha256": output_sha256
            }),
        )?;
        let legacy = root.join("pages/enhanced-v1").join(key);
        let legacy = if mode == "vision" {
            legacy.join("vision-v1")
        } else {
            legacy
        };
        let path = legacy.join(format!("{output_sha256}.json"));
        persistence::atomic_json(
            &path,
            &json!({
                "width":1037, "height":1440,
                "blocks":[{"id":"1","source":"原文","translation":preset,"edited":edited,"box":[10,20,30,40]}]
            }),
        )?;
        Ok((directory.join("effective.png"), path))
    }

    #[test]
    fn verified_legacy_layout_is_reusable_after_its_image_is_gone() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let image = original_file(root)?;
        let source_sha = format!("{:x}", Sha256::digest(fs::read(&image)?));
        let canonical = root.join("pages").join(format!("{source_sha}.json"));
        let (effective, automatic) = legacy_fixture(root, &source_sha, "text", "standard", false)?;
        let (_, manual) = legacy_fixture(root, &source_sha, "text", "conservative", true)?;
        let (_, vision) = legacy_fixture(root, &source_sha, "vision", "denoise", true)?;
        legacy_fixture(root, &"f".repeat(64), "text", "denoise", true)?;
        let old = [
            fs::read(&automatic)?,
            fs::read(&manual)?,
            fs::read(&vision)?,
        ];
        let find = |mode| {
            legacy_enhanced_page(
                &image,
                &effective,
                &canonical,
                &root.join("sr"),
                &root.join("pages"),
                mode,
            )
        };
        let text = find("text")?.context("missing same-source text record")?;
        let visual = find("vision")?.context("missing same-source visual record")?;
        assert_eq!(text["blocks"][0]["translation"], "conservative");
        assert_eq!(text["blocks"][0]["box"], json!([10, 20, 30, 40]));
        assert_eq!(text["layout_provenance"]["legacy_candidates"], 2);
        assert_eq!(visual["blocks"][0]["translation"], "denoise");
        assert_eq!(
            old,
            [fs::read(automatic)?, fs::read(manual)?, fs::read(vision)?]
        );
        fs::remove_dir_all(root.join("sr"))?;
        let image_bytes = fs::read(image)?;
        let canvas = source_canvas(&text, &image_bytes)?;
        let pixels = imaging::decode_bytes(&canvas, imgcodecs::IMREAD_COLOR)?;
        assert_eq!((pixels.cols(), pixels.rows()), (1037, 1440));
        assert_eq!(pixels.at_2d::<opencv::core::Vec3b>(0, 0)?.0, [255; 3]);
        Ok(())
    }

    #[test]
    fn newly_enhanced_ocr_uses_original_width_and_height_independently() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let image = original_file(dir.path())?;
        let mut page = json!({"width":1037,"height":1440,
            "blocks":[{"id":"1","box":[0,720,1037,1440],"source":"原文"}]});
        rebase_ocr(&mut page, &image)?;
        assert_eq!(page["width"], 389);
        assert_eq!(page["height"], 540);
        assert_eq!(page["blocks"][0]["box"], json!([0.0, 270.0, 389.0, 540.0]));
        assert_eq!(page["blocks"][0]["source"], "原文");
        Ok(())
    }
}
