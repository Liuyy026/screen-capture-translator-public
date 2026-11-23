//! Read-only resolution is shared by page seeding, export previews and revision-bound export.
use super::{PageRevisionChanged, PageVersion, Runtime, cache_source_digest, ocr_review_current,
    read_page_image, review_ocr, upgrade_original_selections, page_source::{self, Original}};
use crate::{persistence, preserved_bubbles, structure, super_resolution::Settings, vision_pipeline};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io::ErrorKind, path::{Path, PathBuf}, sync::Mutex};

// Pages with equal source bytes can share one canonical record even when the
// filenames differ. Only this export's own verified materialization may advance
// its confirmed revision. Other jobs and later edits cannot use that transition.
#[derive(Default)]
pub(super) struct ExportTransitions(Mutex<HashMap<String, HashMap<String, String>>>);

impl ExportTransitions {
    fn accepts(&self, session: &str, expected: &str, observed: &str) -> bool {
        expected == observed || (!session.is_empty() && self.0.lock().unwrap().get(session)
            .and_then(|pages| pages.get(expected)).is_some_and(|updated| updated == observed))
    }

    fn remember(&self, session: &str, before: String, after: String) -> Result<()> {
        if session.is_empty() || before == after { return Ok(()); }
        let mut sessions = self.0.lock().unwrap();
        ensure!(sessions.len() < 64 || sessions.contains_key(session), "同时导出的任务过多");
        sessions.entry(session.to_owned()).or_default().insert(before, after);
        Ok(())
    }

    pub(super) fn release(&self, session: &str) { self.0.lock().unwrap().remove(session); }
}

#[derive(Clone)]
struct Record {
    origin: &'static str,
    identity: Value,
    // Keep a parse error attached to its exact revision. A corrupt comparison
    // page can be reported without preventing export of a valid selected page.
    page: Result<Value, String>,
    bytes: Option<Vec<u8>>,
    touch: Option<PathBuf>,
    undecided: bool,
}

fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

impl Record {
    fn stored(path: &Path, origin: &'static str) -> Result<Option<Self>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(Self {
            origin, identity: json!([origin, path, sha(&bytes)]),
            page: persistence::parse_page(&bytes).map_err(|error| error.to_string()),
            bytes: Some(bytes), touch: (origin == "existing").then(|| path.to_owned()), undecided: false,
        }))
    }

    fn from_text(mut self) -> Self {
        self.origin = "derived";
        self.identity = json!(["derived", self.identity]);
        self.page = self.page.map(vision_pipeline::from_text);
        self.bytes = None;
        self
    }

    fn migration_needed(&self, cache: &Path) -> bool {
        self.page.as_ref().is_ok_and(|page| !ocr_review_current(page, cache)
            || !preserved_bubbles::legacy_ids(page).is_empty())
    }
}

struct Sources {
    cache: PathBuf,
    selected: Option<Record>,
    other: Option<Record>,
    fingerprint: String,
    unresolved: [Option<String>; 2],
    vision: bool,
}

impl Sources {
    fn verify(&self, transitions: &ExportTransitions, session: &str, expected: &str) -> Result<()> {
        ensure!(transitions.accepts(session, expected, &self.fingerprint), PageRevisionChanged);
        Ok(())
    }

    fn fingerprint(cache: &Path, mode: &str, settings: &Settings, selected: Option<&Value>,
        other: Option<&Record>, unresolved: &[Option<String>; 2]) -> Result<String> {
        Ok(sha(&serde_json::to_vec(&json!([1, cache_source_digest(cache)?, mode, settings,
            selected, other.map(|r| &r.identity), unresolved]))?))
    }

    fn ready_existing(&self) -> Result<Option<Value>> {
        let Some(record) = self.selected.as_ref().filter(|record| record.origin == "existing"
            && record.page.is_ok() && !record.migration_needed(&self.cache)) else { return Ok(None) };
        let mut page = record.page.as_ref().unwrap().clone();
        audit_for_read(&mut page)?;
        Ok(Some(json!({"cache": self.cache, "page": page, "revision": record.identity[2]})))
    }

    fn report(self) -> Result<Value> {
        let selected = self.selected.as_ref();
        let other = self.other.as_ref();
        let unknown_source = self.unresolved[0].is_some() || (self.unresolved[1].is_some()
            && (selected.is_some_and(|r| r.origin == "derived") || (selected.is_none() && self.vision)));
        let mut status = "ready";
        let mut note = None;
        let mut analysis = crate::export_preflight::analyze(&Value::Null, None);
        if let Some(selected) = selected {
            let uncertain = |record: &Record| record.undecided || record.page.is_err()
                || record.migration_needed(&self.cache);
            if unknown_source {
                note = Some("存在缺少原图凭据的旧增强缓存，正式增强后才能核实来源。");
            } else if selected.page.is_err() {
                note = Some("所选页面缓存无法解析，请重新载入后核对。");
            } else if selected.undecided {
                note = Some("有多个旧版增强缓存，正式导出时才能确定使用哪一份。");
            } else if selected.migration_needed(&self.cache) {
                note = Some("旧页尚待核对 OCR 或迁移保留原图标记，正式载入后才能确定最终统计。");
            } else if self.unresolved[1].is_some() || other.is_some_and(uncertain) {
                note = Some("另一流程有待核实的旧缓存，暂无法完整比较手改内容。");
            }
            if note.is_some() { status = "uncertain"; }
            if !unknown_source && !selected.undecided && let Ok(mut page) = selected.page.clone() {
                audit_for_read(&mut page)?;
                let alternate = if selected.origin != "derived" && !uncertain(selected) && self.unresolved[1].is_none() {
                    other.filter(|record| !uncertain(record)).and_then(|record| record.page.clone().ok())
                } else { None };
                let mut alternate = alternate;
                if let Some(page) = alternate.as_mut() { audit_for_read(page)?; }
                analysis = crate::export_preflight::analyze(&page, alternate.as_ref());
            }
        } else if unknown_source {
            status = "uncertain";
            note = Some("有与此页尺寸相符的旧缓存缺少原图凭据，尚无法确定能否导入。");
        } else {
            status = "missing";
        }
        let mut note = note.map(str::to_owned);
        if !unknown_source && let Some(record) = selected && !record.undecided
            && record.page.as_ref().is_ok_and(|page| page["manual_ocr_partial_page"] == true) {
            note.get_or_insert_with(String::new).push_str("本页仅识别了手工框选的区域，整页其他文字尚未检查。");
        }
        Ok(json!({
            "status": status, "origin": selected.map_or("missing", |record| record.origin),
            "other_available": other.is_some_and(|record| record.page.is_ok()), "fingerprint":self.fingerprint,
            "analysis": analysis, "note": note
        }))
    }
}

pub(super) fn audit_for_read(page: &mut Value) -> Result<()> {
    if crate::translation::audit_cache(page, None) { structure::sync(page)?; }
    Ok(())
}

impl Runtime {
    fn native_record(&self, image: Original<'_>, effective: Option<&Path>, settings: Option<&Settings>,
        mode: &str, cache: &Path) -> Result<(Option<Record>, Option<String>)> {
        if let Some(record) = Record::stored(cache, "existing")? { return Ok((Some(record), None)); }
        if let Some(seed) = &self.seed_root {
            let root = if mode == "vision" { seed.join("vision-v1") } else { seed.clone() };
            let path = root.join(cache.file_name().context("missing cache name")?);
            if path.is_file() && let Some(record) = Record::stored(&path, "seed")? { return Ok((Some(record), None)); }
        }
        let (legacy, unresolved) = page_source::legacy_enhanced_page(image, effective, settings, cache, &self.sr.root, &self.cache_root, mode)?;
        Ok((legacy.map(|legacy| Record {
                origin:"legacy", identity:json!(["legacy", legacy.revisions]), page:Ok(legacy.page),
                bytes:None, touch:None, undecided:legacy.undecided,
            }), unresolved))
    }

    fn other_cache(&self, cache: &Path, mode: &str) -> Result<PathBuf> {
        let root = if mode == "vision" { self.cache_root.clone() } else { self.cache_root.join("vision-v1") };
        Ok(root.join(cache.file_name().context("missing cache name")?))
    }

    pub(super) fn seed_page(&self, image: &Path, effective: &Path, mode: &str, cache: &Path) -> Result<Option<Value>> {
        let (mut selected, _) = self.native_record(Original::Path(image), Some(effective), None, mode, cache)?;
        if selected.is_none() && mode == "vision" {
            selected = self.native_record(Original::Path(image), Some(effective), None, "text", &self.other_cache(cache, mode)?)?.0
                .map(Record::from_text);
        }
        let Some(record) = selected else { return Ok(None) };
        let page = record.page.map_err(anyhow::Error::msg)?;
        if let Some(path) = record.touch { let _ = crate::cache_policy::touch(&path); }
        if record.origin == "existing" { return Ok(Some(page)); }
        let bytes = match record.bytes { Some(bytes) => bytes, None => serde_json::to_vec_pretty(&page)? };
        Ok(Some(persistence::seed_page(cache, &bytes, page)?))
    }

    fn export_sources(&self, image: &Path, mode: &str, settings: &Settings, effective: Option<&Path>) -> Result<Sources> {
        let original = fs::read(image)?;
        let cache = self.cache_directory(mode)?.join(format!("{}.json", sha(&original)));
        let image = Original::Bytes(&original);
        let other_mode = if mode == "vision" { "text" } else { "vision" };
        let (native, unverified) = self.native_record(image, effective, Some(settings), mode, &cache)?;
        let (other, other_unverified) = self.native_record(image, effective, Some(settings), other_mode, &self.other_cache(&cache, mode)?)?;
        let unresolved = [unverified, other_unverified];
        let selected = native.or_else(|| if mode == "vision" { other.clone().map(Record::from_text) } else { None });
        let fingerprint = Sources::fingerprint(&cache, mode, settings, selected.as_ref().map(|r| &r.identity), other.as_ref(), &unresolved)?;
        Ok(Sources { cache, selected, other, fingerprint, unresolved, vision: mode == "vision" })
    }

    pub fn export_preflight(&self, image: &Path, mode: &str, settings: &Settings) -> Result<Value> {
        self.export_sources(image, mode, settings, None)?.report()
    }

    fn confirmed_sources(&self, image: &Path, mode: &str, settings: &Settings, effective: Option<&Path>) -> Result<Sources> {
        self.export_sources(image, mode, settings, effective).map_err(|error| {
            if error.downcast_ref::<std::io::Error>().is_some_and(|cause| cause.kind() == ErrorKind::NotFound)
                && !image.exists() { PageRevisionChanged.into() } else { error }
        })
    }

    pub(super) fn verify_export(&self, image: &Path, mode: &str, settings: &Settings, session: &str, expected: &str) -> Result<()> {
        self.confirmed_sources(image, mode, settings, None)?.verify(&self.export_transitions, session, expected)
    }

    pub(super) async fn load_confirmed_export(&self, image: &Path, effective: &Path, mode: &str,
        settings: &Settings, session: &str, expected: &str) -> Result<(Value, String)> {
        let current = self.confirmed_sources(image, mode, settings, Some(effective))?;
        current.verify(&self.export_transitions, session, expected)?;
        if let Some(mut reply) = current.ready_existing()? {
            // Display reopens and validates this exact revision. A writer waiting
            // for its translation service must not block a committed-page export.
            reply["layout_image"] = json!(image);
            return Ok((reply, current.fingerprint));
        }
        let cache = current.cache;
        let _pin = self.cache_policy.pin_page(&cache)?;
        let _writer = self.cache_policy.lock_page(&cache).await?;
        // Enhancement can take minutes. Re-resolve both sources inside the writer
        // lock, then materialize only the page read by this second comparison.
        let snapshot = self.confirmed_sources(image, mode, settings, Some(effective))?;
        ensure!(snapshot.cache == cache, PageRevisionChanged);
        snapshot.verify(&self.export_transitions, session, expected)?;
        let record = snapshot.selected.context("这页尚未处理，请先翻译整个文件夹。")?;
        let mut page = record.page.map_err(anyhow::Error::msg)?;
        let upgraded = upgrade_original_selections(&mut page, image, &cache)?;
        let reviewed = review_ocr(&mut page, image, &cache)?;
        let migrated = upgraded || reviewed;
        let bytes = match record.bytes {
            Some(bytes) if !migrated => bytes,
            _ => serde_json::to_vec_pretty(&page)?,
        };
        if record.origin == "existing" {
            if migrated {
                let prior = record.identity[2].as_str().context("missing stored revision")?;
                PageVersion { cache: &cache, revision: prior }.verified_bytes(&cache)?;
                persistence::atomic_bytes(&cache, &bytes)?;
            }
        } else {
            persistence::seed_page(&cache, &bytes, page.clone())?;
        }
        let revision = sha(&bytes);
        // A competing external import or replacement must never be silently
        // adopted, including when it happened during first materialization.
        PageVersion { cache: &cache, revision: &revision }.verified_bytes(&cache)?;
        read_page_image(image, &cache)?;
        if let Some(path) = record.touch { let _ = crate::cache_policy::touch(&path); }
        audit_for_read(&mut page)?;
        let canonical = json!(["existing", cache, revision]);
        let updated = Sources::fingerprint(&cache, mode, settings, Some(&canonical), snapshot.other.as_ref(),
            &[None, snapshot.unresolved[1].clone()])?;
        self.export_transitions.remember(session, snapshot.fingerprint, updated.clone())?;
        Ok((json!({"cache":cache,"page":page,"revision":revision,"layout_image":image}), updated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::isolated_runtime;
    use std::{collections::BTreeMap, time::SystemTime};
    use tokio::time::{Duration, timeout};

    fn image(root: &Path) -> Result<PathBuf> {
        let image = root.join("01.pgm");
        fs::write(&image, [b"P5\n32 32\n255\n".as_slice(), &[255; 1024]].concat())?;
        Ok(image)
    }

    fn sample_page() -> Value {
        json!({"width":32,"height":32,"blocks":[
            {"id":"a","box":[0,0,12,12],"source":"こんにちは","translation":"人工","edited":true},
            {"id":"b","box":[12,12,31,31],"source":"次","translation":"下一页","edited":false}
        ]})
    }

    fn tree(root: &Path) -> Result<BTreeMap<PathBuf, (u64, SystemTime)>> {
        let mut entries = BTreeMap::new();
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            entries.insert(entry.path(), (metadata.len(), metadata.modified()?));
            if metadata.is_dir() { entries.extend(tree(&entry.path())?); }
        }
        Ok(entries)
    }

    #[tokio::test]
    async fn seed_and_vision_projection_are_inspected_without_writing_or_touching_any_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let mut runtime = isolated_runtime(root)?;
        let original = image(root)?;
        let cache = runtime.cache_for(&original, "text")?;
        let name = cache.file_name().unwrap();
        runtime.seed_root = Some(root.join("stable"));
        let stable = runtime.seed_root.as_ref().unwrap().join(name);
        let mut page = sample_page();
        page["manual_ocr_partial_page"] = json!(true);
        persistence::atomic_json(&stable, &page)?;
        let settings = Settings { engine: "metalfx".into(), ..Default::default() };
        let before = tree(root)?;
        let preview = runtime.export_preflight(&original, "text", &settings)?;
        assert_eq!(preview["origin"], "seed");
        assert!(preview["note"].as_str().unwrap().contains("仅识别了手工框选"));
        assert_eq!(preview["status"], "uncertain"); // Legacy OCR review is a real write at load.
        assert_eq!(before, tree(root)?);
        review_ocr(&mut page, &original, &cache)?;
        let mut bytes = serde_json::to_vec(&page)?;
        bytes.extend_from_slice(b"  \n");
        persistence::atomic_bytes(&stable, &bytes)?;
        let before = tree(root)?;
        let preview = runtime.export_preflight(&original, "vision", &settings)?;
        assert_eq!(preview["origin"], "derived");
        assert_eq!(preview["status"], "ready");
        assert_eq!(preview["note"], "本页仅识别了手工框选的区域，整页其他文字尚未检查。");
        assert_eq!(preview["analysis"]["edits"], json!([]));
        assert_eq!(preview["other_available"], true);
        assert_eq!(before, tree(root)?);
        assert!(!cache.exists());
        let fingerprint = preview["fingerprint"].as_str().unwrap();
        let (imported, _) = runtime.load_confirmed_export(&original, &original, "vision", &settings, "first", fingerprint).await?;
        assert_eq!(imported["page"], vision_pipeline::from_text(page.clone()));
        assert_eq!(fs::read(&stable)?, bytes);
        assert!(!cache.exists());

        let duplicate = root.join("duplicate.pgm");
        fs::copy(&original, &duplicate)?;
        runtime.verify_export(&duplicate, "vision", &settings, "first", fingerprint)?;
        assert!(runtime.verify_export(&duplicate, "vision", &settings, "another", fingerprint).is_err());

        let text = runtime.export_preflight(&original, "text", &settings)?;
        let (imported, _) = runtime.load_confirmed_export(&original, &original, "text", &settings, "first",
            text["fingerprint"].as_str().unwrap()).await?;
        assert_eq!(imported["revision"], sha(&bytes));
        assert_eq!(fs::read(&cache)?, bytes);
        Ok(())
    }

    #[tokio::test]
    async fn shared_content_migration_advances_only_the_export_that_published_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        let original = image(root)?;
        let duplicate = root.join("02.pgm");
        fs::copy(&original, &duplicate)?;
        let cache = runtime.cache_for(&original, "text")?;
        persistence::atomic_json(&cache, &sample_page())?;
        let settings = Settings::default();
        let before = runtime.export_preflight(&original, "text", &settings)?;
        let duplicate_before = runtime.export_preflight(&duplicate, "text", &settings)?;
        assert_eq!(before["fingerprint"], duplicate_before["fingerprint"]);
        let confirmed = before["fingerprint"].as_str().unwrap();
        let (first, _) = runtime.load_confirmed_export(&original, &original, "text", &settings, "chapter", confirmed).await?;
        assert_ne!(runtime.export_preflight(&duplicate, "text", &settings)?["fingerprint"], confirmed);
        assert!(runtime.verify_export(&duplicate, "text", &settings, "other", confirmed).unwrap_err().is::<PageRevisionChanged>());
        let writer = runtime.cache_policy.lock_page(&cache).await?;
        let (second, _) = timeout(Duration::from_secs(1), runtime.load_confirmed_export(&duplicate, &duplicate, "text",
            &settings, "chapter", confirmed)).await??;
        assert_eq!(first["revision"], second["revision"]);
        drop(writer);

        let mut page = first["page"].clone();
        page["translation_config"] = json!({"model":"changed metadata only"});
        persistence::atomic_json(&cache, &page)?;
        assert!(runtime.verify_export(&duplicate, "text", &settings, "chapter", confirmed).unwrap_err().is::<PageRevisionChanged>());
        persistence::atomic_json(&cache, &first["page"])?;
        runtime.verify_export(&duplicate, "text", &settings, "chapter", confirmed)?;
        runtime.release_export("chapter");
        assert!(runtime.verify_export(&duplicate, "text", &settings, "chapter", confirmed).unwrap_err().is::<PageRevisionChanged>());
        Ok(())
    }

    #[tokio::test]
    async fn blocked_materialization_rechecks_the_confirmed_input_after_the_lock() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        let original = image(root)?;
        let text = runtime.cache_for(&original, "text")?;
        let vision = runtime.cache_for(&original, "vision")?;
        let mut page = sample_page();
        persistence::atomic_json(&text, &page)?;
        let settings = Settings::default();
        let preview = runtime.export_preflight(&original, "vision", &settings)?;
        let writer = runtime.cache_policy.lock_page(&vision).await?;
        let loading = runtime.load_confirmed_export(&original, &original, "vision", &settings, "job",
            preview["fingerprint"].as_str().unwrap());
        tokio::pin!(loading);
        assert!(timeout(Duration::from_millis(25), &mut loading).await.is_err());
        page["blocks"][0]["translation"] = json!("另一次手改");
        persistence::atomic_json(&text, &page)?;
        drop(writer);
        let error = timeout(Duration::from_secs(1), loading).await?.unwrap_err();
        assert!(error.is::<PageRevisionChanged>() && !vision.exists());
        Ok(())
    }

    #[tokio::test]
    async fn new_native_or_changed_comparison_and_original_invalidate_confirmed_projection() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        let original = image(root)?;
        let text = runtime.cache_for(&original, "text")?;
        let vision = runtime.cache_for(&original, "vision")?;
        let settings = Settings::default();
        let missing = runtime.export_preflight(&original, "text", &settings)?;
        assert_eq!(missing["status"], "missing");
        let page = sample_page();
        persistence::atomic_json(&text, &page)?;
        let preview = runtime.export_preflight(&original, "vision", &settings)?;
        let confirmed = preview["fingerprint"].as_str().unwrap();
        let is_stale = |image: &Path| runtime.verify_export(image, "vision", &settings, "job", confirmed)
            .is_err_and(|error| error.is::<PageRevisionChanged>());
        persistence::atomic_json(&vision, &vision_pipeline::from_text(page.clone()))?;
        assert!(is_stale(&original));
        fs::remove_file(&vision)?;
        let mut edited = page.clone();
        edited["blocks"][1]["translation_config_override"] = json!({"model":"a different model"});
        persistence::atomic_json(&text, &edited)?;
        assert!(is_stale(&original));
        persistence::atomic_json(&text, &page)?;
        fs::write(&original, b"changed original")?;
        assert!(is_stale(&original));
        fs::remove_file(&original)?;
        assert!(is_stale(&original));
        assert!(!vision.exists());
        Ok(())
    }
}
