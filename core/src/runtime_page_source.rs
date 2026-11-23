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
use std::{borrow::Cow, collections::BTreeMap, fs, path::Path};

fn bounded_bytes(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    fs::read(path).ok()
}

fn canvas(page: &Value) -> Option<(i32, i32)> {
    let width = i32::try_from(page["width"].as_u64()?).ok()?;
    let height = i32::try_from(page["height"].as_u64()?).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

#[derive(Clone, Copy)]
pub(super) enum Original<'a> {
    Path(&'a Path),
    Bytes(&'a [u8]),
}

fn image_size(original: Original<'_>, size: &mut Option<(u32, u32)>) -> Result<(u32, u32)> {
    if let Some(size) = *size { return Ok(size); }
    let image = match original {
        Original::Path(path) => imaging::decode_color(path)?,
        Original::Bytes(bytes) => imaging::decode_bytes(bytes, imgcodecs::IMREAD_COLOR)?,
    };
    Ok(*size.insert((image.cols() as u32, image.rows() as u32)))
}

// Rebuilding an evicted SR sidecar can make a formerly unidentifiable legacy
// page eligible. Geometry can rule this out, but cannot prove source identity.
fn unresolved_revisions(directory: &Path, settings: Option<&Settings>, original: Original<'_>,
    original_size: &mut Option<(u32,u32)>, revisions: &mut BTreeMap<std::path::PathBuf, String>) -> Result<()> {
    let Some(settings) = settings.filter(|s| s.engine != "off") else { return Ok(()) };
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") ||
            !path.file_stem().and_then(|s| s.to_str()).is_some_and(hex_key) { continue; }
        let Some(bytes) = bounded_bytes(&path, 16 * 1024 * 1024) else { continue };
        let Ok(page) = crate::persistence::parse_page(&bytes) else { continue };
        let (w, h) = image_size(original, original_size)?;
        if let Some((x, y)) = super_resolution::dimensions(w, h, settings)?
            && canvas(&page) == Some((x as i32, y as i32)) {
            revisions.insert(path, format!("{:x}", Sha256::digest(bytes)));
        }
    }
    Ok(())
}

pub(super) struct LegacyPage {
    pub page: Value,
    // Bind all eligible revisions: before enhancement the exact-key tie-break
    // can be unknown, including when a newer helper would produce a new key.
    pub revisions: BTreeMap<String, (String, String)>,
    pub undecided: bool,
}

pub(super) fn legacy_enhanced_page(
    original: Original<'_>,
    effective: Option<&Path>,
    requested: Option<&Settings>,
    cache: &Path,
    sr_root: &Path,
    page_root: &Path,
    mode: &str,
) -> Result<(Option<LegacyPage>, Option<String>)> {
    let source_sha256 = cache
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("missing source hash")?;
    let legacy_root = page_root.join("enhanced-v1");
    if !plain_dir(&legacy_root) {
        return Ok((None, None));
    }
    let mut original_size = None;
    let mut best = None;
    let mut revisions = BTreeMap::new();
    let mut possible_exact = Vec::new();
    let mut unresolved = BTreeMap::new();
    for entry in fs::read_dir(legacy_root)? {
        let entry = entry?;
        let key = entry.file_name();
        let Some(key) = key.to_str().filter(|key| hex_key(key)) else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let directory = if mode == "vision" { entry.path().join("vision-v1") } else { entry.path() };
        if !plain_dir(&directory) { continue; }
        let sr_dir = sr_root.join(key);
        let sidecar = plain_dir(&sr_dir).then(|| bounded_bytes(&sr_dir.join("identity.json"), 65_536))
            .flatten().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let identity = sidecar.as_ref().map(|sidecar| &sidecar["identity"]);
        let valid_identity = match identity {
            Some(identity) if identity["schema"] == 1 =>
                format!("{:x}", Sha256::digest(serde_json::to_vec(identity)?)) == key,
            _ => false,
        };
        if !valid_identity {
            unresolved_revisions(&directory, requested, original, &mut original_size, &mut unresolved)?;
            continue;
        }
        let sidecar = sidecar.unwrap();
        let identity = &sidecar["identity"];
        if identity["source_sha256"] != source_sha256 { continue; }
        let Some(output_sha256) = sidecar["output_sha256"].as_str().filter(|hash| hex_key(hash)) else {
            unresolved_revisions(&directory, requested, original, &mut original_size, &mut unresolved)?;
            continue;
        };
        let legacy = directory.join(format!("{output_sha256}.json"));
        let Some(bytes) = bounded_bytes(&legacy, 16 * 1024 * 1024) else { continue };
        let Ok(page) = crate::persistence::parse_page(&bytes) else {
            continue;
        };
        let Some((width, height)) = canvas(&page) else {
            continue;
        };
        if identity["width"] != width || identity["height"] != height {
            continue;
        }
        let Ok(settings) = serde_json::from_value::<Settings>(identity["settings"].clone()) else {
            continue;
        };
        let (source_width, source_height) = image_size(original, &mut original_size)?;
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
        revisions.insert(key.to_owned(), (output_sha256.to_owned(), format!("{:x}", Sha256::digest(bytes))));
        let quality = (manual, translated > 0 && translated == eligible, translated);
        if effective.is_none() && requested.is_some_and(|s| s.engine == settings.engine
            && s.target == settings.target && s.axis == settings.axis && s.preset == settings.preset) {
            possible_exact.push((quality, key.to_owned()));
        }
        let exact = effective.and_then(Path::parent) == Some(sr_dir.as_path());
        let rank = (quality, exact, key.to_owned());
        if best
            .as_ref()
            .is_none_or(|(previous, _, _)| rank > *previous)
        {
            best = Some((rank, page, identity["settings"].clone()));
        }
    }
    let best = best.map(|((quality, _, identity), mut page, settings)| {
        let undecided = possible_exact.iter().any(|(rank, key)| *rank == quality && *key != identity);
        page["layout_provenance"] = json!({
            "version": 1, "source_sha256": source_sha256, "legacy_enhancement": identity,
            "settings": settings, "legacy_candidates": revisions.len()
        });
        LegacyPage { page, revisions, undecided }
    });
    let unresolved = (!unresolved.is_empty()).then(|| serde_json::to_vec(&unresolved))
        .transpose()?.map(|bytes| format!("{:x}", Sha256::digest(bytes)));
    Ok((best, unresolved))
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
        let scale_box = |bounds: &mut Value| -> Result<()> {
            let coords = bounds.as_array_mut().context("missing OCR box")?;
            ensure!(coords.len() == 4, "invalid OCR box");
            for (coord, scale) in coords.iter_mut().zip([sx,sy,sx,sy]) {
                *coord = json!(coord.as_f64().context("invalid OCR coordinate")? * scale);
            }
            Ok(())
        };
        scale_box(&mut block["box"])?;
        if let Some(anchor) = block.get_mut("typography_anchor") { scale_box(anchor)?; }
        for key in ["source_boxes", "manual_ocr_boxes"] {
            if let Some(boxes) = block.get_mut(key) {
                for bounds in boxes.as_array_mut().context("invalid source rectangles")? { scale_box(bounds)?; }
            }
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
    fn tied_legacy_revisions_and_evicted_identities_are_unresolved_in_preflight() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let image = original_file(root)?;
        let digest = format!("{:x}", Sha256::digest(fs::read(&image)?));
        let cache = root.join("pages").join(format!("{digest}.json"));
        let (a, _) = legacy_fixture(root, &digest, "text", "standard", true)?;
        let (b, _) = legacy_fixture(root, &digest, "text", "denoise", true)?;
        // The actual SR key takes precedence over lexicographic order only
        // among equally good candidates. Select the lesser key's own preset.
        let (matched, preset) = if a < b { (&a, "standard") } else { (&b, "denoise") };
        let settings = Settings { engine: "realcugan".into(), preset:preset.into(), ..Default::default() };
        let lookup = |effective| legacy_enhanced_page(Original::Path(&image), effective, Some(&settings),
            &cache, &root.join("sr"), &root.join("pages"), "text");
        let (preview, unresolved) = lookup(None)?;
        let preview = preview.unwrap();
        assert!(preview.undecided && unresolved.is_none());
        let exact = lookup(Some(matched))?.0.unwrap();
        assert!(!exact.undecided);
        assert_eq!(exact.page["blocks"][0]["translation"], preset);
        assert_eq!(exact.revisions, preview.revisions);
        assert_ne!(exact.page, preview.page);

        fs::remove_dir_all(root.join("sr"))?;
        let (known, unresolved) = lookup(None)?;
        assert!(known.is_none() && unresolved.is_some());
        let (_, no_enhancement) = legacy_enhanced_page(Original::Path(&image), None, Some(&Settings::default()),
            &cache, &root.join("sr"), &root.join("pages"), "text")?;
        assert!(no_enhancement.is_none());
        Ok(())
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
                Original::Path(&image),
                Some(&effective),
                None,
                &canonical,
                &root.join("sr"),
                &root.join("pages"),
                mode,
            )
        };
        let text = find("text")?.0.context("missing same-source text record")?.page;
        let visual = find("vision")?.0.context("missing same-source visual record")?.page;
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
