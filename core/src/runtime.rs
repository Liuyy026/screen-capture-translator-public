//! Application-owned processing state. Candidate caches are writable; the
//! stable seed caches and source images are always read-only.
use crate::{compat::{array,truth},display_cache,model_images::ImageModels,ocr,overlay,persistence,
    service::TranslationService,structure,translate_pipeline::{self,Observer},vision_pipeline};
use anyhow::{Result,ensure,Context};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{collections::HashMap,path::{Path,PathBuf},sync::Arc};
use tokio::sync::Mutex;

pub struct Runtime{
    pub cache_root:PathBuf,
    pub seed_root:Option<PathBuf>,
    pub display:display_cache::Store,
    models:Mutex<ImageModels>,
    pages:Mutex<HashMap<PathBuf,Arc<Mutex<()>>>>,
}

pub struct ProcessOptions<'a>{
    pub operation:&'a str,pub mode:&'a str,pub model:&'a str,pub block_id:Option<&'a str>,
    pub unit:&'a str,pub batch_size:usize,pub retry:bool,
}

impl Runtime{
    pub fn new(cache_root:&Path,assets_root:&Path,seed_root:Option<&Path>,python:&Path,worker:&Path,weights:&Path)->Result<Self>{
        std::fs::create_dir_all(cache_root)?;std::fs::create_dir_all(assets_root)?;
        let cache_root=cache_root.canonicalize()?;let assets_root=assets_root.canonicalize()?;
        let seed_root=seed_root.map(Path::canonicalize).transpose()?;
        if let Some(seed)=&seed_root{
            ensure!(!cache_root.starts_with(seed)&&!seed.starts_with(&cache_root),"candidate and stable page caches must be separate");
            ensure!(!assets_root.starts_with(seed)&&!seed.starts_with(&assets_root),"display assets cannot overwrite stable caches");
        }
        ensure!(!assets_root.starts_with(&cache_root)&&!cache_root.starts_with(&assets_root),"page and display caches must be separate");
        Ok(Self{cache_root,seed_root,display:display_cache::Store{root:assets_root,weights:weights.to_owned()},
            models:Mutex::new(ImageModels::new(python,worker,weights)?),pages:Mutex::new(HashMap::new())})
    }
    pub fn cache_for(&self,image:&Path,mode:&str)->Result<PathBuf>{
        ensure!(["text","vision"].contains(&mode),"未知翻译方式");
        persistence::cache_path(image,&if mode=="vision"{self.cache_root.join("vision-v1")}else{self.cache_root.clone()})
    }
    async fn page_lock(&self,path:&Path)->Arc<Mutex<()>>{
        let mut pages=self.pages.lock().await;
        // Keep only locks held by an active request; no per-book growth.
        pages.retain(|_,lock|Arc::strong_count(lock)>1);
        pages.entry(path.to_owned()).or_insert_with(||Arc::new(Mutex::new(()))).clone()
    }
    fn seed_page(&self,image:&Path,mode:&str,target:&Path)->Result<Option<Value>>{
        if target.exists(){return Ok(Some(serde_json::from_slice(&std::fs::read(target)?)?));}
        if let Some(seed)=&self.seed_root{
            let source=seed.join(if mode=="vision"{"vision-v1"}else{""}).join(target.file_name().unwrap());
            if source.is_file(){
                let bytes=std::fs::read(source)?;let page=serde_json::from_slice(&bytes)?;
                persistence::atomic_bytes(target,&bytes)?;return Ok(Some(page));
            }
        }
        if mode=="vision"{
            let mut page=vision_pipeline::prepare(image,target,&self.cache_root)?;
            if page.is_none()&&let Some(seed)=&self.seed_root{page=vision_pipeline::prepare(image,target,seed)?;}
            if let Some(page)=&page{persistence::atomic_json(target,page)?;}
            return Ok(page);
        }
        Ok(None)
    }
    pub async fn load_page(&self,image:&Path,mode:&str)->Result<Value>{
        let cache=self.cache_for(image,mode)?;let lock=self.page_lock(&cache).await;let _guard=lock.lock().await;
        let page=self.seed_page(image,mode,&cache)?;
        let revision=if cache.exists(){Some(format!("{:x}",Sha256::digest(std::fs::read(&cache)?)))}else{None};
        Ok(json!({"cache":cache,"page":page,"revision":revision}))
    }
    pub async fn process_page(&self,image:&Path,options:&ProcessOptions<'_>,service:&TranslationService,observer:&mut impl Observer)->Result<Value>{
        ensure!(["ocr","translate","both"].contains(&options.operation),"未知处理操作");
        let cache=self.cache_for(image,options.mode)?;let lock=self.page_lock(&cache).await;let _guard=lock.lock().await;
        let existing=self.seed_page(image,options.mode,&cache)?;
        let mut page=if let Some(mut page)=existing{
            if options.mode=="text"{
                let changed=overlay::ensure(&mut page,Some(image))?;
                let before=page["structure"].clone();structure::ensure(&mut page,Some(image))?;
                if changed||before!=page["structure"]{persistence::atomic_json(&cache,&page)?;}
            }
            page
        }else{
            observer.progress("正在自动寻找文字并识别日文，首次处理会稍慢…")?;
            let mut models=self.models.lock().await;
            let page=ocr::recognize(image,&mut models).await?;
            persistence::atomic_json(&cache,&page)?;page
        };
        if options.retry{
            let id=options.block_id.context("单句重试缺少对白编号")?;
            let block=page["blocks"].as_array_mut().context("invalid page blocks")?.iter_mut().find(|b|b["id"]==id).context("缓存中没有找到这条对白")?;
            block["translation"]=json!("");block["error"]=json!("");block["edited"]=json!(false);
            persistence::atomic_json(&cache,&page)?;
        }
        // The observer owns checkpointing for the destination selected above.
        observer.save(&page)?;
        if options.operation!="ocr"{
            self.models.lock().await.release_ocr().await?;
            if options.mode=="vision"{
                vision_pipeline::translate(&mut page,image,options.model,service,options.block_id,observer).await?;
            }else{
                translate_pipeline::translate(&mut page,service,&translate_pipeline::Options{
                    model:options.model,unit:options.unit,batch_size:options.batch_size,block_id:options.block_id,image:Some(image)},observer).await?;
            }
        }
        Ok(json!({"cache":cache,"blocks":array(&page["blocks"]).len(),"issues":array(&page["blocks"]).iter().filter(|b|truth(&b["error"])).count()}))
    }
    pub async fn display_page(&self,image:&Path,cache:&Path)->Result<Value>{
        let cache=cache.canonicalize()?;
        ensure!(cache.starts_with(&self.cache_root),"只能显示候选版本自己的缓存");
        let lock=self.page_lock(&cache).await;let _guard=lock.lock().await;
        let image_bytes=std::fs::read(image)?;let cache_bytes=std::fs::read(&cache)?;
        let key=self.display.key(&image_bytes,&cache_bytes)?;
        if let Some(reply)=self.display.load(&key){return Ok(json!({"reply":reply,"cache_hit":true}));}
        let page=serde_json::from_slice(&cache_bytes)?;
        let mut models=self.models.lock().await;
        let reply=display_cache::generate(&image_bytes,&page,None,Some(&mut models),Some(&self.display)).await?;
        self.display.save(&key,&reply)?;
        Ok(json!({"reply":reply,"cache_hit":false}))
    }
    pub async fn edit(&self,cache:&Path,id:&str,values:&Value,revision:Option<&str>)->Result<Value>{
        let cache=cache.canonicalize()?;
        ensure!(cache.starts_with(&self.cache_root),"只能修改候选版本自己的缓存");
        let lock=self.page_lock(&cache).await;let _guard=lock.lock().await;
        let bytes=std::fs::read(&cache)?;
        if let Some(revision)=revision{ensure!(format!("{:x}",Sha256::digest(&bytes))==revision,"页面已更新，请重新打开后再保存修改");}
        let mut page:Value=serde_json::from_slice(&bytes)?;
        let block=page["blocks"].as_array_mut().context("invalid page blocks")?.iter_mut().find(|b|b["id"]==id).context("缓存中没有找到这条对白，请重新打开图片。")?;
        for (key,value) in values.as_object().context("invalid edit fields")?{
            let valid=match key.as_str(){"source"|"translation"|"error"|"vision_source"=>value.is_string(),"edited"=>value.is_boolean(),
                "quality_warnings"=>value.as_array().is_some_and(|a|a.iter().all(Value::is_string)),_=>false};
            ensure!(valid,"invalid edit field");block[key]=value.clone();
        }
        persistence::atomic_json(&cache,&page)?;
        Ok(json!({"cache":cache,"page":page,"revision":format!("{:x}",Sha256::digest(std::fs::read(&cache)?))}))
    }
    pub async fn release_ocr(&self)->Result<()>{self.models.lock().await.release_ocr().await}
    pub async fn close(&self)->Result<()>{self.models.lock().await.stop().await}
}
