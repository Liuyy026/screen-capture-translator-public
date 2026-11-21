//! Application-owned processing state. Candidate caches are writable; the
//! stable seed caches and source images are always read-only.
use crate::{compat::{array,keeps_original,truth},display_cache,model_images::ImageModels,ocr,overlay,persistence,preserved_bubbles,
    service::TranslationService,structure,translate_pipeline::{self,Observer},vision_pipeline};
use anyhow::{Result,ensure,Context};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{io::Write,path::{Path,PathBuf},sync::Arc};
use tokio::sync::Mutex;

#[path = "runtime_page_source.rs"]
mod page_source;

pub struct Runtime{
    pub cache_root:PathBuf,
    pub sr:crate::super_resolution::Store,
    pub cache_policy:Arc<crate::cache_policy::Manager>,
    pub seed_root:Option<PathBuf>,
    pub display:display_cache::Store,
    models:Mutex<ImageModels>,
    pub engine:crate::local_engine::LocalEngine,
}

#[path = "ocr_residency.rs"]
mod ocr_residency;

#[derive(Clone, Copy)]
pub struct ProcessOptions<'a>{
    pub operation:&'a str,pub mode:&'a str,pub model:&'a str,pub block_id:Option<&'a str>,
    pub previous_image: Option<&'a Path>,
    pub expected_page: Option<PageVersion<'a>>,
    pub unit:&'a str,pub batch_size:usize,pub retry:bool,
    pub reuse_ocr: bool,
}

#[derive(Clone, Copy)]
pub struct PageVersion<'a> {
    pub cache: &'a Path,
    pub revision: &'a str,
}

impl PageVersion<'_> {
    // The caller holds the page writer lock. A missing page must not be
    // re-created from a seed for a request bound to a previously viewed page.
    fn verified_bytes(&self, cache: &Path) -> Result<Vec<u8>> {
        ensure!(self.cache == cache, PageRevisionChanged);
        let bytes = match std::fs::read(cache) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(PageRevisionChanged.into()),
            result => result?,
        };
        ensure!(format!("{:x}", Sha256::digest(&bytes)) == self.revision, PageRevisionChanged);
        Ok(bytes)
    }

    fn open(&self, source: &Path, cache: &Path) -> Result<(Value, tempfile::NamedTempFile)> {
        let page = persistence::parse_page(&self.verified_bytes(cache)?)?;
        let image = read_page_image(source, cache)?;
        // Enhancement and visual translation reopen paths after awaiting other
        // work. Keep their inputs bound to these verified bytes until commit.
        let suffix = source.extension().and_then(|s| s.to_str()).map(|s| format!(".{s}")).unwrap_or_default();
        let mut snapshot = tempfile::Builder::new().prefix("koma-retry-").suffix(&suffix).tempfile()?;
        snapshot.write_all(&image)?;
        Ok((page, snapshot))
    }
}

impl ProcessOptions<'_> {
    pub fn for_queue_page(mut self, index: usize, total: usize, sr_engine: &str) -> Self {
        // Real-CUGAN needs its own inference workspace before the next OCR.
        self.reuse_ocr = self.operation == "both" && self.block_id.is_none() && !self.retry
            && sr_engine != "realcugan" && ocr_residency::has_next_in_batch(index, total);
        self
    }
}

// A single-block retry replaces an existing translation. Until a valid
// replacement is ready, its cleared working copy must never reach disk.
struct Checkpoints<'a, O> {
    observer: &'a mut O,
    retry_source: Option<&'a Path>,
}

impl<O: Observer> Observer for Checkpoints<'_, O> {
    fn source_image(&self) -> Option<&Path> { self.retry_source }
    fn progress(&mut self, message: &str) -> Result<()> { self.observer.progress(message) }
    fn metrics(&mut self, event: &Value) -> Result<()> { self.observer.metrics(event) }
    fn save(&mut self, page: &Value) -> Result<()> {
        if self.retry_source.is_some() { Ok(()) } else { self.observer.save(page) }
    }
}

fn cache_source_digest(cache: &Path) -> Result<&str> {
    cache.file_stem().and_then(|stem| stem.to_str()).context("缺少漫画页面的文件摘要")
}

fn read_page_image(image: &Path, cache: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(image)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    ensure!(cache_source_digest(cache)? == digest, PageRevisionChanged);
    Ok(bytes)
}

// Page writers replace the file atomically. Opening it once provides a whole
// committed revision even if its pathname is replaced before parsing finishes.
fn committed_page(cache: &Path) -> Result<Option<(Value, String)>> {
    let bytes = match std::fs::read(cache) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some((persistence::parse_page(&bytes)?, format!("{:x}", Sha256::digest(&bytes)))))
}

#[derive(Debug)]
pub struct PageRevisionChanged;

impl std::fmt::Display for PageRevisionChanged {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("页面已更新，请重新载入后再试。")
    }
}

impl std::error::Error for PageRevisionChanged {}

fn upgrade_original_selections(page: &mut Value, image: &Path, cache: &Path) -> Result<bool> {
    let ids = preserved_bubbles::legacy_ids(page);
    if ids.is_empty() { return Ok(false); }
    let original = read_page_image(image, cache)?;
    let image = page_source::source_canvas(page, &original)?;
    for id in ids { preserved_bubbles::set(page, &id, true, &image, &[])?; }
    crate::translation_cache::mark_manual(page);
    Ok(true)
}

fn ocr_review_current(page: &Value, cache: &Path) -> bool {
    cache_source_digest(cache).is_ok_and(|digest| crate::ocr_review::source_current(page, digest))
}

fn review_ocr(page: &mut Value, image: &Path, cache: &Path) -> Result<bool> {
    // Processing can spend time on OCR and structure analysis. Verify the
    // current file before trusting a receipt, even when its path is unchanged.
    let original = read_page_image(image, cache)?;
    crate::ocr_review::ensure_for_source(page, cache_source_digest(cache)?, |page| {
        let pixels = page_source::source_canvas(page, &original)?;
        let color = crate::imaging::decode_bytes(&pixels, opencv::imgcodecs::IMREAD_COLOR)?;
        crate::imaging::color_to_gray(&color)
    })
}

impl Runtime{
    pub fn new(cache_root:&Path,assets_root:&Path,seed_root:Option<&Path>,python:&Path,worker:&Path,weights:&Path,ollama:Option<PathBuf>)->Result<Self>{
        std::fs::create_dir_all(cache_root)?;std::fs::create_dir_all(assets_root)?;
        let cache_root=cache_root.canonicalize()?;let assets_root=assets_root.canonicalize()?;
        let seed_root=seed_root.map(Path::canonicalize).transpose()?;
        if let Some(seed)=&seed_root{
            ensure!(!cache_root.starts_with(seed)&&!seed.starts_with(&cache_root),"candidate and stable page caches must be separate");
            ensure!(!assets_root.starts_with(seed)&&!seed.starts_with(&assets_root),"display assets cannot overwrite stable caches");
        }
        ensure!(!assets_root.starts_with(&cache_root)&&!cache_root.starts_with(&assets_root),"page and display caches must be separate");
        let worker_absolute=worker.canonicalize()?;
        let project=worker_absolute.ancestors().nth(3).context("worker path has no project root")?;
        let sr=crate::super_resolution::Store{root:cache_root.parent().context("cache root has no parent")?.join("super-resolution-v1"),gate:Mutex::new(()),helpers:project.join("macos/.build")};
        std::fs::create_dir_all(&sr.root)?;
        ensure!(!sr.root.starts_with(&cache_root)&&!cache_root.starts_with(&sr.root),"增强缓存与译文目录不能重叠");
        if let Some(seed)=&seed_root {ensure!(!sr.root.starts_with(seed)&&!seed.starts_with(&sr.root),"增强缓存与稳定版缓存不能重叠");}
        let cache_policy=crate::cache_policy::Manager::new(&sr.root,&assets_root,&cache_root)?;
        Ok(Self{sr,cache_policy,cache_root,seed_root,display:display_cache::Store{root:assets_root,weights:weights.to_owned()},
            models:Mutex::new(ImageModels::new(python,worker,weights)?),
            engine:crate::local_engine::LocalEngine::new(ollama,weights.join("ollama"))})
    }
    pub fn cache_for(&self,image:&Path,mode:&str)->Result<PathBuf>{
        ensure!(["text","vision"].contains(&mode),"未知翻译方式");
        let root = if mode == "vision" {self.cache_root.join("vision-v1")} else {self.cache_root.clone()};
        persistence::cache_path(image, &root)
    }
    fn seed_page(&self, image: &Path, effective: &Path, mode: &str, target: &Path) -> Result<Option<Value>> {
        if target.exists() {
            let page = persistence::parse_page(&std::fs::read(target)?)?;
            let _ = crate::cache_policy::touch(target);
            return Ok(Some(page));
        }
        if let Some(seed) = &self.seed_root {
            let directory = if mode == "vision" { seed.join("vision-v1") } else { seed.clone() };
            let source = directory.join(target.file_name().unwrap());
            if source.is_file() {
                let bytes = std::fs::read(source)?;
                let page = persistence::parse_page(&bytes)?;
                return Ok(Some(persistence::seed_page(target, &bytes, page)?));
            }
        }
        let legacy = |mode| {
            page_source::legacy_enhanced_page(image, effective, target, &self.sr.root, &self.cache_root, mode)
        };
        let mut page = legacy(mode)?;
        if page.is_none() && mode == "vision" {
            page = vision_pipeline::prepare(image, target, &self.cache_root)?;
            if page.is_some() {
                let _ = crate::cache_policy::touch(&self.cache_for(image, "text")?);
            } else if let Some(seed) = &self.seed_root {
                page = vision_pipeline::prepare(image, target, seed)?;
            }
            if page.is_none() {
                page = legacy("text")?.map(vision_pipeline::from_text);
            }
        }
        page.map(|page| persistence::seed_page(target, &serde_json::to_vec_pretty(&page)?, page)).transpose()
    }

    pub async fn load_page(&self, image: &Path, effective: &Path, mode: &str) -> Result<Value> {
        let cache = self.cache_for(image, mode)?;
        let _pin = self.cache_policy.pin_page(&cache)?;
        let mut waited = false;
        let snapshot = loop {
            let committed = committed_page(&cache)?;
            if committed.as_ref().is_some_and(|(page, _)|
                ocr_review_current(page, &cache) && preserved_bubbles::legacy_ids(page).is_empty()) {
                break committed;
            }
            if let Some(_writer) = self.cache_policy.try_lock_page(&cache)? {
                // Re-read under the writer lock: a checkpoint or edit may have
                // won the race after the optimistic read.
                if waited { read_page_image(image, &cache)?; }
                if let Some(mut page) = self.seed_page(image, effective, mode, &cache)? {
                    let upgraded = upgrade_original_selections(&mut page, image, &cache)?;
                    let reviewed = review_ocr(&mut page, image, &cache)?;
                    if upgraded || reviewed { persistence::atomic_json(&cache, &page)?; }
                }
                break committed_page(&cache)?;
            }
            if committed.is_none() { break None; } // First OCR has not published yet.
            // The writer may migrate this legacy revision and publish it
            // before it starts waiting for the translation model.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            waited = true;
        };
        if waited { read_page_image(image, &cache)?; }
        let (mut page, revision) = snapshot.unzip();
        if let Some(page) = page.as_mut() {
            if crate::translation::audit_cache(page, None) { structure::sync(page)?; }
            let _ = crate::cache_policy::touch(&cache);
        }
        Ok(json!({"cache":cache,"page":page,"revision":revision,"layout_image":image}))
    }

    pub async fn process_page(&self, source: &Path, settings: &crate::super_resolution::Settings, cache: &Path,
        options: &ProcessOptions<'_>, service: &TranslationService, observer: &mut impl Observer) -> Result<Value> {
        ensure!(["ocr","translate","both"].contains(&options.operation),"未知处理操作");
        let _guard = self.cache_policy.lock_page(cache).await?;
        let target = if options.block_id.is_some() || options.retry {
            ensure!(options.operation != "ocr", "单句重试需要翻译操作");
            let id = options.block_id.context("单句重试缺少对白编号")?;
            let version = options.expected_page.ok_or(PageRevisionChanged)
                .context("单句重译缺少页面缓存或版本，请重新载入后再试。")?;
            Some((id, version))
        } else { None };
        let (opened_page, snapshot) = target.map(|(_, version)| version.open(source, cache)).transpose()?.unzip();
        let input = snapshot.as_ref().map_or(source, |file| file.path());
        let (effective, warning) = match self.sr.prepare(input, settings).await {
            Ok(image) => (image, None),
            Err(error) => {
                let warning = service.safe_error(error);
                observer.progress(&format!("超分失败，使用原图继续处理：{warning}"))?;
                (input.to_owned(), Some(warning))
            }
        };
        let existing = match opened_page {
            Some(page) => Some(page),
            None => self.seed_page(input, &effective, options.mode, cache)?,
        };
        let fresh_ocr = existing.is_none();
        // A legacy import retains its established segmentation and dialogue layout.
        let layout_image = if existing.as_ref().is_some_and(|page| page["layout_provenance"]["version"] == 1) {
            None
        } else { Some(input) };
        let mut page = if let Some(mut page) = existing {
            upgrade_original_selections(&mut page, input, cache)?;
            if options.mode == "text" {
                if layout_image.is_some() { overlay::ensure(&mut page, layout_image)?; }
                structure::ensure(&mut page, layout_image)?;
            }
            page
        } else {
            observer.progress("正在自动寻找文字并识别日文，首次处理会稍慢…")?;
            let mut models = self.models.lock().await;
            let mut page = if input == effective {
                ocr::recognize(input, &mut models).await?
            } else {
                let raw = models.ocr(&effective).await?;
                let mut page = ocr::assemble(&raw)?;
                page_source::rebase_ocr(&mut page, input)?;
                overlay::ensure(&mut page, Some(input))?;
                structure::ensure(&mut page, Some(input))?;
                page
            };
            ensure!(self.cache_for(source, options.mode)? == cache, "识别期间原文件发生变化，请重试");
            crate::translation_cache::mark_automatic(&mut page);
            persistence::atomic_json(cache, &page)?;
            page
        };
        review_ocr(&mut page, input, cache)?;
        if let Some((id, _)) = target.filter(|_| options.retry) {
            let block=page["blocks"].as_array_mut().context("invalid page blocks")?.iter_mut().find(|b|b["id"]==id).context("缓存中没有找到这条对白")?;
            ensure!(!keeps_original(block), "请恢复这个对话框的翻译后再重试。");
            ensure!(!crate::ocr_review::suppressed(block), "这处疑似非文字，请核对原图并确认为文字后再翻译。");
            // Older manually edited pages predate ocr_confirmed. Preserve that
            // confirmation when an explicit retry replaces the manual text.
            if truth(&block["edited"]) && block["ocr_review"]["status"] == "suspect" {
                block["ocr_confirmed"] = json!(true);
            }
            block["translation"]=json!("");block["error"]=json!("");block["edited"]=json!(false);
            block.as_object_mut().unwrap().remove("retranslation_error");
        }
        let mut checkpoints = Checkpoints { observer, retry_source: target.map(|_| source) };
        // The observer owns checkpointing for the destination selected above.
        checkpoints.save(&page)?;
        if options.operation != "ocr" {
            self.translate_with_ocr_policy(options.reuse_ocr && fresh_ocr, async {
                if array(&page["blocks"]).iter().any(|b| !keeps_original(b) && !crate::ocr_review::suppressed(b)) {
                    if options.mode == "vision" {
                        vision_pipeline::translate(&mut page,&effective,layout_image,options.previous_image,
                            options.model,service,options.block_id,&mut checkpoints).await?;
                    } else {
                        translate_pipeline::translate(&mut page,service,&translate_pipeline::Options{
                            model:options.model,unit:options.unit,batch_size:options.batch_size,block_id:options.block_id,image:layout_image},&mut checkpoints).await?;
                    }
                }
                Ok(())
            }).await?;
        }
        if let Some((id, version)) = target {
            let block = array(&page["blocks"]).iter().find(|b| b["id"] == id).context("重译目标已丢失")?;
            if let Some(error) = ["error", "retranslation_error"].iter()
                .filter_map(|key| block[*key].as_str()).find(|s| !s.is_empty()) {
                anyhow::bail!("{error}");
            }
            ensure!(block["translation"].as_str().is_some_and(|s| !s.trim().is_empty()), "未生成有效的新译文");
            // The writer lock serializes Koma edits. The image and cache can
            // still be replaced externally while waiting for the model.
            read_page_image(source, cache)?;
            version.verified_bytes(cache)?;
            observer.save(&page)?;
        }
        let blocks = array(&page["blocks"]);
        let issues = blocks.iter().filter(|b| !keeps_original(b) && !crate::ocr_review::suppressed(b)
            && crate::compat::has_translation_issue(b)).count();
        let mut result = json!({"cache":cache,"blocks":blocks.len(),"issues":issues});
        if let Some(warning) = warning { result["enhancement_warning"] = json!(warning); }
        Ok(result)
    }
    pub async fn display_page(&self,image:&Path,cache:&Path,revision:Option<&str>)->Result<Value>{
        let cache=cache.canonicalize()?;
        ensure!(cache.starts_with(&self.cache_root),"只能显示候选版本自己的缓存");
        let _pin = self.cache_policy.pin_page(&cache)?;
        let image_bytes=read_page_image(image, &cache)?;let cache_bytes=std::fs::read(&cache)?;
        if let Some(revision)=revision{ensure!(format!("{:x}",Sha256::digest(&cache_bytes))==revision,PageRevisionChanged);}
        let key=self.display.key(&image_bytes,&cache_bytes)?;
        let _=crate::cache_policy::touch(&cache);
        if let Some(reply)=self.display.load(&key){return Ok(json!({"reply":reply,"cache_hit":true}));}
        let page=persistence::parse_page(&cache_bytes)?;
        let pixels = page_source::source_canvas(&page, &image_bytes)?;
        let mut models=self.models.lock().await;
        let reply=display_cache::generate(&pixels,&page,None,Some(&mut models),Some(&self.display)).await?;
        self.display.save(&key,&reply)?;
        Ok(json!({"reply":reply,"cache_hit":false}))
    }

    pub async fn chapter_export_page(&self, source: &Path, mode: &str,
        settings: &crate::super_resolution::Settings) -> Result<Value> {
        let effective = self.sr.prepare(source, settings).await?;
        let mut snapshot = self.load_page(source, &effective, mode).await?;
        ensure!(!snapshot["page"].is_null(), "这页尚未处理，请先翻译整个文件夹。");
        let cache = Path::new(snapshot["cache"].as_str().context("missing cache path")?);
        let revision = snapshot["revision"].as_str().context("missing page revision")?;
        snapshot["reply"] = self.display_page(source, cache, Some(revision)).await?["reply"].take();
        snapshot["effective_image"] = json!(effective);
        Ok(snapshot)
    }

    pub async fn edit(&self,cache:&Path,id:&str,values:&Value,revision:Option<&str>,image:Option<&Path>)->Result<Value>{
        let cache=cache.canonicalize()?;
        ensure!(cache.starts_with(&self.cache_root),"只能修改候选版本自己的缓存");
        let _guard = self.cache_policy.lock_page(&cache).await?;
        let bytes=std::fs::read(&cache)?;
        if let Some(revision)=revision{ensure!(format!("{:x}",Sha256::digest(&bytes))==revision,"页面已更新，请重新打开后再保存修改");}
        let mut page=persistence::parse_page(&bytes)?;
        let image = image.map(|path| read_page_image(path, &cache)).transpose()?;
        let excluded = |block: &Value| keeps_original(block) || crate::ocr_review::suppressed(block);
        let previously_excluded: Vec<_> = array(&page["blocks"]).iter().map(excluded).collect();
        let fields = values.as_object().context("invalid edit fields")?;
        if let Some(keep) = values["keep_original"].as_bool() {
            ensure!(fields.len() == 1, "invalid preservation edit fields");
            if keep {
                let image = image.context("请重新打开漫画页面再保留对话框。")?;
                let display = self.display.key(&image, &bytes).ok().and_then(|key| self.display.load(&key)).unwrap_or(Value::Null);
                let pixels = page_source::source_canvas(&page, &image)?;
                preserved_bubbles::set(&mut page, id, true, &pixels, array(&display["regions"]))?;
            } else {
                preserved_bubbles::set(&mut page, id, false, &[], &[])?;
            }
        } else {
            let block=page["blocks"].as_array_mut().context("invalid page blocks")?.iter_mut().find(|b|b["id"]==id).context("缓存中没有找到这条对白，请重新打开图片。")?;
            for (key,value) in fields {
                let valid=match key.as_str(){"source"|"translation"|"error"|"vision_source"=>value.is_string(),"edited"|"ocr_confirmed"=>value.is_boolean(),
                    "quality_warnings"=>value.as_array().is_some_and(|a|a.iter().all(Value::is_string)),_=>false};
                ensure!(valid,"invalid edit field");block[key]=value.clone();
            }
            if fields.contains_key("source") || fields.contains_key("translation") {
                block["ocr_confirmed"] = json!(true);
                block.as_object_mut().unwrap().remove("retranslation_error");
            }
        }
        let config = page["translation_config"].clone();
        for (block, was_excluded) in page["blocks"].as_array_mut().expect("validated blocks").iter_mut().zip(previously_excluded) {
            let now_excluded = excluded(block);
            if now_excluded != was_excluded
                && !truth(&block["edited"]) && truth(&block["translation"])
                && block.get("translation_config_override").is_none() {
                // Remember the current provenance on hide. Restoring an older
                // untracked block leaves its text visible until translation is requested.
                block["translation_config_override"] = if now_excluded { config.clone() } else { Value::Null };
            }
        }
        crate::translation_cache::mark_manual(&mut page);
        persistence::atomic_json(&cache,&page)?;
        if crate::translation::audit_cache(&mut page, None) { structure::sync(&mut page)?; }
        Ok(json!({"cache":cache,"page":page,"revision":format!("{:x}",Sha256::digest(std::fs::read(&cache)?))}))
    }
    pub async fn maintain_cache(&self) {
        if let Err(error)=self.cache_policy.maintain().await { eprintln!("缓存清理未完成：{error}"); }
    }
    pub async fn release_ocr(&self)->Result<()>{self.models.lock().await.release_ocr().await}

    async fn translate_with_ocr_policy(&self, reuse: bool, translation: impl Future<Output = Result<()>>) -> Result<()> {
        tokio::pin!(translation);
        if reuse && ocr_residency::normal_memory_pressure() {
            tokio::select! {
                result = &mut translation => return result,
                () = ocr_residency::wait_for_release() => {},
            }
        }
        self.release_ocr().await?;
        translation.await
    }
    pub async fn close(&self)->Result<()>{
        let models=self.models.lock().await.stop().await;
        let engine=self.engine.release().await;
        models.and(engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_runtime(root: &Path) -> Result<Runtime> {
        let worker = root.join("core/python/model_worker.py");
        std::fs::create_dir_all(worker.parent().unwrap())?;
        std::fs::write(&worker, "")?;
        Runtime::new(&root.join("pages"), &root.join("display"), None,
            Path::new("python3"), &worker, root, None)
    }

    #[tokio::test]
    async fn readers_observe_commits_while_the_page_writer_waits() -> Result<()> {
        use tokio::time::{Duration, sleep, timeout};
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        for name in ["comictextdetector.pt", "anime-manga-big-lama.pt"] {
            std::fs::write(root.join(name), "test")?;
        }
        let image = root.join("page.pgm");
        std::fs::write(&image, [b"P5\n32 32\n255\n".as_slice(), &[255; 1024]].concat())?;
        let cache = runtime.cache_for(&image, "text")?;
        let writer = runtime.cache_policy.lock_page(&cache).await?;
        let limit = Duration::from_secs(2);
        let missing = timeout(limit, runtime.load_page(&image, &image, "text")).await??;
        assert!(missing["page"].is_null() && missing["revision"].is_null() && !cache.exists());

        let mut page = json!({"width":32,"height":32,"blocks":[
            {"id":"1","box":[4,4,18,22],"source":"原文","translation":"已保存"}]});
        persistence::atomic_json(&cache, &page)?;
        let load = runtime.load_page(&image, &image, "text");
        tokio::pin!(load);
        // This legacy record needs a write; the reader may neither migrate it
        // under someone else's lock nor miss a new, ready checkpoint.
        tokio::select! {
            result = &mut load => anyhow::bail!("premature legacy read: {result:?}"),
            _ = sleep(Duration::from_millis(20)) => (),
        }
        assert_eq!(persistence::parse_page(&std::fs::read(&cache)?)?, page);
        review_ocr(&mut page, &image, &cache)?;
        let mut bytes = serde_json::to_vec(&page)?;
        bytes.extend_from_slice(b" \n"); // The exact bytes determine the revision.
        persistence::atomic_bytes(&cache, &bytes)?;
        let loaded = timeout(limit, load).await??;
        let revision = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(loaded["page"], page);
        assert_eq!(loaded["revision"], revision);
        let rendered = timeout(limit, runtime.display_page(&image, &cache, Some(&revision))).await??;
        assert_eq!(rendered["reply"]["version"], 3);

        page["blocks"][0]["translation"] = json!("最新译文");
        persistence::atomic_json(&cache, &page)?;
        let stale = runtime.display_page(&image, &cache, Some(&revision)).await.unwrap_err();
        assert!(stale.is::<PageRevisionChanged>());
        let fields = json!({"translation":"过期修改"});
        let edit = runtime.edit(&cache, "1", &fields, Some(&revision), Some(&image));
        tokio::pin!(edit);
        tokio::select! {
            result = &mut edit => anyhow::bail!("edit bypassed writer lock: {result:?}"),
            _ = sleep(Duration::from_millis(20)) => (),
        }
        drop(writer);
        assert!(timeout(limit, edit).await?.is_err());
        assert_eq!(persistence::parse_page(&std::fs::read(&cache)?)?, page);
        Ok(())
    }

    #[tokio::test]
    async fn color_page_ocr_review_agrees_with_compositing() -> Result<()> {
        use opencv::{core::{Mat, Rect, Scalar, Vector, CV_8UC3}, imgcodecs, imgproc};
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        for name in ["comictextdetector.pt", "anime-manga-big-lama.pt"] {
            std::fs::write(root.join(name), "test")?;
        }
        // For this PNG color the grayscale codec yields 139 while converting
        // decoded BGR yields 140, on opposite sides of the ink threshold.
        let mut pixels = Mat::new_rows_cols_with_default(64, 64, CV_8UC3, Scalar::all(175.))?;
        imgproc::rectangle(&mut pixels, Rect::new(11,11,12,14), Scalar::new(255.,117.,140.,0.), -1, 8, 0)?;
        let image = root.join("color.png");
        ensure!(imgcodecs::imwrite(image.to_str().unwrap(), &pixels, &Vector::new())?);
        let cache = runtime.cache_for(&image, "text")?;
        persistence::atomic_json(&cache, &json!({"width":64,"height":64,"blocks":[
            {"id":"1","box":[4,4,32,32],"source":"こんにちは","translation":""}]}))?;
        let loaded = runtime.load_page(&image, &image, "text").await?;
        let display = runtime.display_page(&image, &cache, loaded["revision"].as_str()).await?;
        assert!(crate::ocr_review::suppressed(&loaded["page"]["blocks"][0]));
        assert!(display["reply"]["preserved"].get("1").is_some());
        Ok(())
    }

    #[tokio::test]
    async fn restored_legacy_automatic_text_remains_visible_with_unknown_provenance() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let runtime = isolated_runtime(temp.path())?;
        let cache = runtime.cache_root.join(format!("{:064x}.json", 1));
        let config = json!({"model":"B"});
        persistence::atomic_json(&cache, &json!({"width":32,"height":32,"translation_config":config,
            "preserved_bubbles":[{"block_ids":["a","b"],"bounds":null}],"blocks":[
                {"id":"a","keep_original":true,"translation":"旧机翻"},
                {"id":"b","keep_original":true,"translation":"手改","edited":true}]}))?;
        let edited = runtime.edit(&cache, "a", &json!({"keep_original":false}), None, None).await?;
        let page = &edited["page"];
        assert_eq!(page["blocks"][0]["translation"], "旧机翻");
        assert_eq!(page["blocks"][0].get("translation_config_override"), Some(&Value::Null));
        assert!(translate_pipeline::needs_translation(&page["blocks"][0], &config, false));
        assert_eq!(page["blocks"][1]["translation"], "手改");
        Ok(())
    }

    #[tokio::test]
    async fn replaced_source_cannot_display_or_edit_the_open_page() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        for name in ["comictextdetector.pt", "anime-manga-big-lama.pt"] {
            std::fs::write(root.join(name), "test")?;
        }
        let image = root.join("page.pgm");
        let write_image = |tone: u8| std::fs::write(&image, [b"P5\n32 32\n255\n".as_slice(), &[tone; 1024]].concat());
        write_image(255)?;
        let cache = runtime.cache_for(&image, "text")?;
        persistence::atomic_json(&cache, &json!({"width":32,"height":32,
            "blocks":[{"id":"1","box":[4,4,18,22],"source":"こんにちは","translation":""}]}))?;
        let loaded = runtime.load_page(&image, &image, "text").await?;
        let revision = loaded["revision"].as_str();
        runtime.display_page(&image, &cache, revision).await?;
        let original = std::fs::read(&cache)?;
        write_image(100)?;
        let mut reviewed = loaded["page"].clone();
        assert!(ocr_review_current(&reviewed, &cache));
        assert!(review_ocr(&mut reviewed, &image, &cache).is_err());
        assert!(runtime.display_page(&image, &cache, revision).await.is_err());
        for fields in [json!({"translation":"不应写入","edited":true}), json!({"keep_original":false})] {
            assert!(runtime.edit(&cache, "1", &fields, revision, Some(&image)).await.is_err());
        }
        assert_eq!(std::fs::read(cache)?, original);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_cached_page_returns_an_error_before_processing_or_rewriting_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let runtime = isolated_runtime(temp.path())?;
        let image = temp.path().join("page.pgm");
        std::fs::write(&image, [b"P5\n32 32\n255\n".as_slice(), &[255; 1024]].concat())?;
        let cache = runtime.cache_for(&image, "text")?;
        let service = TranslationService::new("ollama", None, None)?;
        let options = ProcessOptions { operation:"ocr", mode:"text", model:"unused", block_id:None,
            previous_image:None, expected_page:None, unit:"blocks", batch_size:8, retry:false, reuse_ocr:false };
        let vision_cache = runtime.cache_for(&image, "vision")?;
        for invalid in [
            json!("invalid page"),
            json!({"width":0,"height":32,"blocks":[]}),
            json!({"width":32,"height":32,"blocks":["invalid block"]}),
            json!({"width":32,"height":32,"blocks":[{"id":"1"},{"id":"1"}]}),
            json!({"width":32,"height":32,"blocks":[],"dialogues":["invalid dialogue"]}),
        ] {
            persistence::atomic_json(&cache, &invalid)?;
            let before = std::fs::read(&cache)?;
            let mut writer = persistence::FileObserver { destination:&cache, status:None, metrics:None };
            assert!(runtime.process_page(&image, &Default::default(), &cache, &options, &service, &mut writer).await.is_err());
            assert!(runtime.load_page(&image, &image, "vision").await.is_err());
            assert!(!vision_cache.exists());
            assert_eq!(std::fs::read(&cache)?, before);
        }
        Ok(())
    }

    #[tokio::test]
    async fn queued_block_retry_rechecks_the_view_after_acquiring_the_writer_lock() -> Result<()> {
        use tokio::time::{sleep, Duration};

        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        let image = root.join("page.pgm");
        let pixels = [b"P5\n32 32\n255\n".as_slice(), &[255; 1024]].concat();
        let page = json!({"width":32,"height":32,"blocks":[{"id":"1","source":"原文",
            "translation":"人工译文","edited":true}]});
        let service = TranslationService::new("ollama", None, None)?;
        for replace_image in [false, true] {
            std::fs::write(&image, &pixels)?;
            let cache = runtime.cache_for(&image, "text")?;
            persistence::atomic_json(&cache, &page)?;
            let revision = format!("{:x}", Sha256::digest(std::fs::read(&cache)?));
            let options = ProcessOptions { operation:"translate", mode:"text", model:"unused", block_id:Some("1"),
                previous_image:None, expected_page:Some(PageVersion { cache:&cache, revision:&revision }),
                unit:"blocks", batch_size:8, retry:true, reuse_ocr:false };
            let mut observer = persistence::FileObserver { destination:&cache, status:None, metrics:None };
            let guard = runtime.cache_policy.lock_page(&cache).await?;
            let settings = crate::super_resolution::Settings::default();
            let processing = runtime.process_page(&image, &settings, &cache, &options, &service, &mut observer);
            tokio::pin!(processing);
            tokio::select! {
                result = &mut processing => anyhow::bail!("retry bypassed writer lock: {result:?}"),
                _ = sleep(Duration::from_millis(20)) => (),
            }
            if replace_image {
                std::fs::write(&image, b"a different file at the same path")?;
            } else {
                let mut edited = page.clone();
                edited["blocks"][0]["translation"] = json!("刚保存的人工译文");
                persistence::atomic_json(&cache, &edited)?;
            }
            let expected = std::fs::read(&cache)?;
            drop(guard);
            assert!(processing.await.unwrap_err().is::<PageRevisionChanged>());
            assert_eq!(std::fs::read(&cache)?, expected);
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_retry_preserves_previous_translation_and_edit_protection() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let runtime = isolated_runtime(root)?;
        let image = root.join("page.pgm");
        let pixels = [b"P5\n64 64\n255\n".as_slice(), &[255; 64 * 64]].concat();
        std::fs::write(&image, pixels)?;
        let page = json!({"version":2, "width":64,"height":64, "blocks":[{
            "id":"1","box":[8,8,48,48],"source":"こんにちは","translation":"人工译文",
            "edited":true,"ocr_confirmed":true,"error":""}]});
        let service = TranslationService::new("ollama", None, None)?;
        for mode in ["text", "vision"] {
            let cache = runtime.cache_for(&image, mode)?;
            persistence::atomic_json(&cache, &page)?;
            let mut writer = persistence::FileObserver { destination: &cache, status: None, metrics: None };
            let mut options = ProcessOptions { operation: "ocr", mode, model: "unused", block_id: None,
                previous_image: None, expected_page: None, unit: "blocks", batch_size: 8, retry: false, reuse_ocr: false };
            runtime.process_page(&image, &Default::default(), &cache, &options, &service, &mut writer).await?;
            let before = std::fs::read(&cache)?;
            options.operation = "translate";
            options.block_id = Some("1");
            options.retry = true;
            let revision = format!("{:x}", Sha256::digest(&before));
            options.expected_page = Some(PageVersion { cache: &cache, revision: &revision });
            options.unit = "invalid"; // ordinary-mode setup fails, vision rejects Ollama.
            let error = runtime.process_page(&image, &Default::default(), &cache, &options, &service, &mut writer).await.unwrap_err();
            assert!(error.to_string().contains(if mode == "text" { "翻译单位" } else { "oMLX" }));
            assert_eq!(std::fs::read(&cache)?, before);
        }
        Ok(())
    }
}
