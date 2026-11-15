//! Application-owned processing state. Candidate caches are writable; the
//! stable seed caches and source images are always read-only.
use crate::{compat::{array,keeps_original,truth},display_cache,model_images::ImageModels,ocr,overlay,persistence,preserved_bubbles,
    service::TranslationService,structure,translate_pipeline::{self,Observer},vision_pipeline};
use anyhow::{Result,ensure,Context};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{collections::HashMap,path::{Path,PathBuf},sync::Arc};
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
    pages:Mutex<HashMap<PathBuf,Arc<Mutex<()>>>>,
    pub engine:crate::local_engine::LocalEngine,
}

pub struct ProcessOptions<'a>{
    pub operation:&'a str,pub mode:&'a str,pub model:&'a str,pub block_id:Option<&'a str>,
    pub unit:&'a str,pub batch_size:usize,pub retry:bool,
}

fn apply_display_warnings(page: &mut Value) {
    if let Some(blocks) = page["blocks"].as_array_mut() {
        for block in blocks {
            if !truth(&block["edited"]) && !keeps_original(block) {
                let reason = crate::translation::translation_error(&block["translation"]);
                if reason == "模型返回了占位文字，没有提供实际译文" {
                    block["error"] = json!(reason);
                }
            }
        }
    }
}

fn upgrade_original_selections(page: &mut Value, image: &Path, cache: &Path) -> Result<()> {
    let ids = preserved_bubbles::legacy_ids(page);
    if ids.is_empty() { return Ok(()); }
    let original = std::fs::read(image)?;
    let image = page_source::source_canvas(page, &original)?;
    for id in ids { preserved_bubbles::set(page, &id, true, &image, &[])?; }
    crate::translation_cache::mark_manual(page);
    persistence::atomic_json(cache, page)
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
            models:Mutex::new(ImageModels::new(python,worker,weights)?),pages:Mutex::new(HashMap::new()),
            engine:crate::local_engine::LocalEngine::new(ollama,weights.join("ollama"))})
    }
    pub fn cache_for(&self,image:&Path,mode:&str)->Result<PathBuf>{
        ensure!(["text","vision"].contains(&mode),"未知翻译方式");
        let root = if mode == "vision" {self.cache_root.join("vision-v1")} else {self.cache_root.clone()};
        persistence::cache_path(image, &root)
    }
    async fn page_lock(&self,path:&Path)->Arc<Mutex<()>>{
        let mut pages=self.pages.lock().await;
        // Keep only locks held by an active request; no per-book growth.
        pages.retain(|_,lock|Arc::strong_count(lock)>1);
        pages.entry(path.to_owned()).or_insert_with(||Arc::new(Mutex::new(()))).clone()
    }
    fn seed_page(&self, image: &Path, effective: &Path, mode: &str, target: &Path) -> Result<Option<Value>> {
        if target.exists() {
            let page = serde_json::from_slice(&std::fs::read(target)?)?;
            let _ = crate::cache_policy::touch(target);
            return Ok(Some(page));
        }
        if let Some(seed) = &self.seed_root {
            let directory = if mode == "vision" { seed.join("vision-v1") } else { seed.clone() };
            let source = directory.join(target.file_name().unwrap());
            if source.is_file() {
                let bytes = std::fs::read(source)?;
                let page = serde_json::from_slice(&bytes)?;
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
        let lock = self.page_lock(&cache).await;
        let _guard = lock.lock().await;
        let mut page = self.seed_page(image, effective, mode, &cache)?;
        if let Some(page) = page.as_mut() {
            upgrade_original_selections(page, image, &cache)?;
            apply_display_warnings(page);
        }
        let revision = if cache.exists() {
            Some(format!("{:x}", Sha256::digest(std::fs::read(&cache)?)))
        } else { None };
        Ok(json!({"cache":cache,"page":page,"revision":revision,"layout_image":image}))
    }

    pub async fn process_page(&self, source: &Path, effective: &Path, cache: &Path,
        options: &ProcessOptions<'_>, service: &TranslationService, observer: &mut impl Observer) -> Result<Value> {
        ensure!(["ocr","translate","both"].contains(&options.operation),"未知处理操作");
        let lock = self.page_lock(cache).await;
        let _guard = lock.lock().await;
        let existing = self.seed_page(source, effective, options.mode, cache)?;
        // A legacy import retains its established segmentation and dialogue layout.
        let layout_image = if existing.as_ref().is_some_and(|page| page["layout_provenance"]["version"] == 1) {
            None
        } else { Some(source) };
        let mut page = if let Some(mut page) = existing {
            upgrade_original_selections(&mut page, source, cache)?;
            if options.mode == "text" {
                let changed = match layout_image {
                    Some(image) => overlay::ensure(&mut page, Some(image))?,
                    None => false,
                };
                let before = page["structure"].clone();
                structure::ensure(&mut page, layout_image)?;
                if changed || before != page["structure"] { persistence::atomic_json(cache, &page)?; }
            }
            page
        } else {
            observer.progress("正在自动寻找文字并识别日文，首次处理会稍慢…")?;
            let mut models = self.models.lock().await;
            let mut page = if source == effective {
                ocr::recognize(source, &mut models).await?
            } else {
                let raw = models.ocr(effective).await?;
                let mut page = ocr::assemble(&raw)?;
                page_source::rebase_ocr(&mut page, source)?;
                overlay::ensure(&mut page, Some(source))?;
                structure::ensure(&mut page, Some(source))?;
                page
            };
            ensure!(self.cache_for(source, options.mode)? == cache, "识别期间原文件发生变化，请重试");
            crate::translation_cache::mark_automatic(&mut page);
            persistence::atomic_json(cache, &page)?;
            page
        };
        if options.retry{
            let id=options.block_id.context("单句重试缺少对白编号")?;
            let block=page["blocks"].as_array_mut().context("invalid page blocks")?.iter_mut().find(|b|b["id"]==id).context("缓存中没有找到这条对白")?;
            ensure!(!keeps_original(block), "请恢复这个对话框的翻译后再重试。");
            block["translation"]=json!("");block["error"]=json!("");block["edited"]=json!(false);
            persistence::atomic_json(cache,&page)?;
        }
        // The observer owns checkpointing for the destination selected above.
        observer.save(&page)?;
        if options.operation!="ocr" && array(&page["blocks"]).iter().any(|b| !keeps_original(b)) {
            self.models.lock().await.release_ocr().await?;
            if options.mode=="vision"{
                vision_pipeline::translate(&mut page,effective,layout_image,options.model,service,options.block_id,observer).await?;
            }else{
                translate_pipeline::translate(&mut page,service,&translate_pipeline::Options{
                    model:options.model,unit:options.unit,batch_size:options.batch_size,block_id:options.block_id,image:layout_image},observer).await?;
            }
        }
        Ok(json!({"cache":cache,"blocks":array(&page["blocks"]).len(),"issues":array(&page["blocks"]).iter().filter(|b|!keeps_original(b) && truth(&b["error"])).count()}))
    }
    pub async fn display_page(&self,image:&Path,cache:&Path,revision:Option<&str>)->Result<Value>{
        let cache=cache.canonicalize()?;
        ensure!(cache.starts_with(&self.cache_root),"只能显示候选版本自己的缓存");
        let lock=self.page_lock(&cache).await;let _guard=lock.lock().await;
        let image_bytes=std::fs::read(image)?;let cache_bytes=std::fs::read(&cache)?;
        if let Some(revision)=revision{ensure!(format!("{:x}",Sha256::digest(&cache_bytes))==revision,"页面已更新，请重试。");}
        let key=self.display.key(&image_bytes,&cache_bytes)?;
        let _=crate::cache_policy::touch(&cache);
        if let Some(reply)=self.display.load(&key){return Ok(json!({"reply":reply,"cache_hit":true}));}
        let page=serde_json::from_slice(&cache_bytes)?;
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
        let lock=self.page_lock(&cache).await;let _guard=lock.lock().await;
        let bytes=std::fs::read(&cache)?;
        if let Some(revision)=revision{ensure!(format!("{:x}",Sha256::digest(&bytes))==revision,"页面已更新，请重新打开后再保存修改");}
        let mut page:Value=serde_json::from_slice(&bytes)?;
        let fields = values.as_object().context("invalid edit fields")?;
        if let Some(keep) = values["keep_original"].as_bool() {
            ensure!(fields.len() == 1, "invalid preservation edit fields");
            if keep {
                let image = std::fs::read(image.context("请重新打开漫画页面再保留对话框。")?)?;
                let digest = format!("{:x}", Sha256::digest(&image));
                ensure!(cache.file_stem().and_then(|stem| stem.to_str()) == Some(digest.as_str()), "漫画页面已更新，请重新打开后再修改。");
                let display = self.display.key(&image, &bytes).ok().and_then(|key| self.display.load(&key)).unwrap_or(Value::Null);
                let pixels = page_source::source_canvas(&page, &image)?;
                preserved_bubbles::set(&mut page, id, true, &pixels, array(&display["regions"]))?;
            } else {
                preserved_bubbles::set(&mut page, id, false, &[], &[])?;
            }
        } else {
            let block=page["blocks"].as_array_mut().context("invalid page blocks")?.iter_mut().find(|b|b["id"]==id).context("缓存中没有找到这条对白，请重新打开图片。")?;
            for (key,value) in fields {
                let valid=match key.as_str(){"source"|"translation"|"error"|"vision_source"=>value.is_string(),"edited"=>value.is_boolean(),
                    "quality_warnings"=>value.as_array().is_some_and(|a|a.iter().all(Value::is_string)),_=>false};
                ensure!(valid,"invalid edit field");block[key]=value.clone();
            }
        }
        crate::translation_cache::mark_manual(&mut page);
        persistence::atomic_json(&cache,&page)?;
        apply_display_warnings(&mut page);
        Ok(json!({"cache":cache,"page":page,"revision":format!("{:x}",Sha256::digest(std::fs::read(&cache)?))}))
    }
    pub async fn maintain_cache(&self) {
        if let Err(error)=self.cache_policy.maintain().await { eprintln!("缓存清理未完成：{error}"); }
    }
    pub async fn release_ocr(&self)->Result<()>{self.models.lock().await.release_ocr().await}
    pub async fn close(&self)->Result<()>{
        let models=self.models.lock().await.stop().await;
        let engine=self.engine.release().await;
        models.and(engine)
    }
}
