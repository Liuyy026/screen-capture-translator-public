//! Bounded JSONL application service. Rust owns processing queues, cancellation,
//! per-page retries, model lifetime and cache writes. No credentials are logged.
use crate::{persistence,protocol,runtime::{Runtime,ProcessOptions,PageVersion},service::TranslationService,translate_pipeline::Observer};
use anyhow::{Result,Context,ensure,bail};
use serde_json::{Value,json};
use std::{collections::HashMap,path::{Path,PathBuf},sync::Arc};
use tokio::{io::{AsyncWriteExt,BufReader},sync::{mpsc,watch,Mutex,Semaphore},task::JoinSet};

struct RegisteredJob {
    cancel: watch::Sender<bool>,
    finish_on_shutdown: bool,
}
type Registry=Arc<Mutex<HashMap<String, RegisteredJob>>>;
struct Job {id:String,request:Value,cancel:watch::Receiver<bool>}

struct Events {id:String,image:String,cache:PathBuf,output:mpsc::Sender<Value>}
impl Events{
    fn emit(&self,event:Value){let _=self.output.try_send(event);}
}
impl Observer for Events{
    fn progress(&mut self,message:&str)->Result<()>{
        self.emit(json!({"id":self.id,"event":"progress","image":self.image,"message":message}));Ok(())
    }
    fn save(&mut self,page:&Value)->Result<()>{
        persistence::atomic_json(&self.cache,page)?;
        self.emit(json!({"id":self.id,"event":"checkpoint","image":self.image,"cache":self.cache}));Ok(())
    }
    fn metrics(&mut self,event:&Value)->Result<()>{
        self.emit(json!({"id":self.id,"event":"metrics","metrics":event}));Ok(())
    }
}

fn required<'a>(request:&'a Value,key:&str)->Result<&'a str>{request[key].as_str().with_context(||format!("missing {key}"))}
fn service(request:&Value)->Result<TranslationService>{
    TranslationService::new(request["provider"].as_str().unwrap_or("omlx"),request["base_url"].as_str(),request["key"].as_str())
}

fn previous_for_page<'a>(request: &'a Value, image: &str, total: usize) -> Result<Option<&'a Path>> {
    let value = if let Some(single) = request.get("previous_image").filter(|v| !v.is_null()) {
        ensure!(total == 1 && request["previous_images"].is_null(),
            "多页翻译请按当前页分别指定上一页参照。");
        Some(single)
    } else if let Some(mapping) = request.get("previous_images").filter(|v| !v.is_null()) {
        mapping.as_object().context("上一页参照映射格式无效。")?.get(image)
    } else { None };
    value.map(|v| v.as_str().map(Path::new).context("上一页参照路径无效。")).transpose()
}

#[cfg(test)]
#[test]
fn chapter_references_follow_the_supplied_mapping_for_sparse_queues() -> Result<()> {
    let request = json!({"images":["/p1", "/p7"], "previous_images":{"/p7":"/p6"}});
    assert_eq!(previous_for_page(&request, "/p1", 2)?, None);
    assert_eq!(previous_for_page(&request, "/p7", 2)?, Some(Path::new("/p6")));
    let single = json!({"previous_image":"/p6"});
    assert_eq!(previous_for_page(&single, "/p7", 1)?, Some(Path::new("/p6")));
    assert!(previous_for_page(&single, "/p7", 2).is_err());
    Ok(())
}

async fn execute(runtime:&Runtime,job:&Job,output:&mpsc::Sender<Value>,display_gate:&Semaphore)->Result<Value>{
    let r=&job.request;
    match required(r,"op")?{
        "sr_capabilities"=>Ok(json!({"backends":[runtime.sr.capabilities("metalfx").await,runtime.sr.capabilities("realcugan").await]})),
        "cache_status"=>{runtime.maintain_cache().await;Ok(runtime.cache_policy.status())},
        "load_page"=>{
            let ticket=runtime.cache_policy.view_ticket();
            let _activity=runtime.cache_policy.activity().await?;
            let settings:crate::super_resolution::Settings=serde_json::from_value(r.get("super_resolution").cloned().unwrap_or(json!({})))?;
            let source = Path::new(required(r, "image")?);
            let (effective, warning) = match runtime.sr.prepare(source, &settings).await {
                Ok(image) => (image, None),
                Err(error) => (source.to_owned(), Some(error.to_string())),
            };
            let mut reply = runtime.load_page(source, &effective, r["mode"].as_str().unwrap_or("text")).await?;
            runtime.cache_policy.pin_view(ticket, &effective, Some(Path::new(reply["cache"].as_str().context("missing cache path")?)))?;
            reply["effective_image"] = json!(effective);
            reply["original_image"] = r["image"].clone();
            if let Some(warning) = warning { reply["enhancement_warning"] = json!(warning); }
            Ok(reply)
        },
        "models"=>Ok(json!({"models":runtime.engine.discover(&service(r)?,r["allow_local_start"].as_bool().unwrap_or(false)).await?})),
        "unload_model"=>{
            let model = required(r, "model")?;
            ensure!(!model.trim().is_empty(), "请选择需要卸载的翻译模型");
            ensure!(r["explicit"].is_null() || r["explicit"].is_boolean(), "无效的模型卸载选项");
            let released = runtime.translation_models.unload(&service(r)?, model,
                r["explicit"] == true, r["key"].is_string()).await?;
            Ok(json!({"released": released}))
        },
        "release_engine"=>{runtime.engine.release().await?;Ok(json!({"released":true}))},
        "display"=>{
            let _permit=display_gate.acquire().await?;
            let _activity=runtime.cache_policy.activity().await?;
            runtime.display_page(Path::new(required(r,"image")?),Path::new(required(r,"cache")?),r["revision"].as_str()).await
        },
        "export_preflight"=>{
            let settings = serde_json::from_value(r.get("super_resolution").cloned().unwrap_or(json!({})))?;
            runtime.export_preflight(Path::new(required(r,"image")?), r["mode"].as_str().unwrap_or("text"), &settings)
        },
        "chapter_export_page"=>{
            let _permit=display_gate.acquire().await?;
            let _activity=runtime.cache_policy.activity().await?;
            // Legacy clients export sequentially without a session identifier.
            let session = r["export_id"].as_str().unwrap_or("");
            ensure!(session.len() <= 128, "无效导出任务编号");
            let settings:crate::super_resolution::Settings=serde_json::from_value(r["super_resolution"].clone())?;
            ensure!(settings.engine != "off", "请选择超分引擎");
            let expected = r.get("expected_fingerprint").map(|v| v.as_str()
                .filter(|s| crate::cache_policy::hex_key(s)).context("无效导出预检凭据")).transpose()?;
            let reply = runtime.chapter_export_page(Path::new(required(r,"image")?),r["mode"].as_str().unwrap_or("text"),
                &settings, session, expected).await?;
            runtime.cache_policy.pin_export(session, Path::new(required(&reply, "effective_image")?))?;
            Ok(reply)
        },
        "release_export"=>{
            runtime.release_export(required(r, "export_id")?);
            Ok(json!({"released": true}))
        },
        "edit"=>{
            let _activity=runtime.cache_policy.activity().await?;
            runtime.edit(Path::new(required(r,"cache")?),required(r,"block_id")?,&r["values"],r["revision"].as_str(),r["image"].as_str().map(Path::new)).await
        },
        "ocr_crop_preview" | "ocr_crop_apply" => {
            let _activity = runtime.cache_policy.activity().await?;
            runtime.ocr_crop(r).await
        },
        "process"=>{
            let images=r["images"].as_array().context("missing images")?;
            ensure!(!images.is_empty()&&images.len()<=4096,"invalid page queue size");
            let operation = r["operation"].as_str().unwrap_or("both");
            let block_id = r["block_id"].as_str();
            let retry = r["retry"].as_bool().unwrap_or(false);
            ensure!(r["block_id"].is_null() || block_id.is_some_and(|id| !id.is_empty()), "无效的单句对白编号");
            ensure!(r["retry"].is_null() || r["retry"].is_boolean(), "无效的单句重试选项");
            if block_id.is_some() || retry {
                ensure!(images.len() == 1 && matches!(operation, "translate" | "both"), "单句重试需要指定一页漫画并启用翻译");
                ensure!(block_id.is_some(), "单句重试缺少对白编号");
            }
            let processing = if operation == "enhance" {
                None
            } else {
                let service = runtime.translation_models.track(service(r)?);
                let options = ProcessOptions {
                    operation, mode: r["mode"].as_str().unwrap_or("text"),
                    model: required(r, "model")?, block_id,
                    expected_page: r["cache"].as_str().zip(r["revision"].as_str())
                        .map(|(cache, revision)| PageVersion { cache: Path::new(cache), revision }),
                    unit: r["unit"].as_str().unwrap_or("blocks"),
                    batch_size: r["batch_size"].as_u64().unwrap_or(8) as usize,
                    retry,
                    reuse_ocr: false, previous_image: None,
                };
                Some((options, service))
            };
            let mut results=vec![];
            for (index,image) in images.iter().enumerate(){
                let image=image.as_str().context("invalid image path")?;
                let _=output.send(json!({"id":job.id,"event":"page_started","image":image,"index":index,"total":images.len()})).await;
                let page_result=async{
                    let _activity=runtime.cache_policy.activity().await?;
                    let settings:crate::super_resolution::Settings=serde_json::from_value(r.get("super_resolution").cloned().unwrap_or(json!({})))?;
                    ensure!(processing.is_some() || settings.engine != "off", "请选择超分引擎");
                    let source = Path::new(image);
                    if let Some((options, service)) = &processing {
                        let cache = runtime.cache_for(source, options.mode)?;
                        let mut events = Events { id: job.id.clone(), image: image.into(), cache: cache.clone(), output: output.clone() };
                        let mut options = options.for_queue_page(index, images.len(), &settings.engine);
                        if options.mode == "vision" && matches!(options.operation, "both" | "translate") {
                            options.previous_image = previous_for_page(r, image, images.len())?;
                        }
                        runtime.process_page(source, &settings, &cache, &options, service, &mut events).await
                    } else {
                        Ok(json!({"skipped":runtime.sr.prepare(source, &settings).await? == source}))
                    }
                }.await;
                if page_result.is_err() && operation == "both" {
                    cleanup_ocr(runtime).await;
                }
                let result=match page_result{
                    Ok(value)=>json!({"image":image,"ok":true,"result":value}),
                    Err(error) => {
                        let changed = error.is::<crate::runtime::PageRevisionChanged>();
                        let message = match &processing {
                            Some((_, service)) => service.safe_error(error),
                            None => error.to_string(),
                        };
                        let mut page = json!({"image":image,"ok":false,"error":message});
                        if changed { page["code"] = json!("page_revision_changed"); }
                        page
                    }
                };
                let _=output.send(json!({"id":job.id,"event":"page_finished","done":index+1,"total":images.len(),"page":result})).await;
                results.push(result);
                runtime.maintain_cache().await;
            }
            Ok(json!({"pages":results}))
        },
        _=>bail!("unknown operation"),
    }
}

async fn cleanup_ocr(runtime: &Runtime) {
    if let Err(error) = runtime.release_ocr().await {
        eprintln!("清理 OCR 工作进程时发生错误：{error}");
    }
}

async fn run(runtime:Arc<Runtime>,mut job:Job,output:mpsc::Sender<Value>,registry:Registry,display_gate:Arc<Semaphore>){
    let cancelled=|id:&str|json!({"id":id,"event":"cancelled"});
    let reply=if *job.cancel.borrow(){cancelled(&job.id)}else{
        let request=Job{id:job.id.clone(),request:job.request.clone(),cancel:job.cancel.clone()};
        let result=tokio::select!{biased;
            _=job.cancel.changed()=>None,
            result=execute(&runtime,&request,&output,&display_gate)=>Some(result),
        };
        if *job.cancel.borrow(){cancelled(&job.id)}else{match result{
            Some(Ok(value))=>json!({"id":job.id,"event":"result","result":value}),
            Some(Err(error))=>{
                let mut message=error.to_string();
                if let Some(key)=job.request["key"].as_str().filter(|k|!k.is_empty()){message=message.replace(key,"[已隐藏]");}
                let mut reply=json!({"id":job.id,"event":"error","error":message});
                if error.is::<crate::runtime::PageRevisionChanged>() {reply["code"]=json!("page_revision_changed");}
                reply
            },
            None=>cancelled(&job.id),
        }}
    };
    // Release owned OCR allocations before another translation job; a cancelled
    // model call also discards its poisoned channel and private array workspace.
    if job.request["op"] == "ocr_crop_preview" || (job.request["op"] == "process" && job.request["operation"] != "enhance") {
        cleanup_ocr(&runtime).await;
    }
    if job.request["op"] != "export_preflight" { runtime.maintain_cache().await; }
    registry.lock().await.remove(&job.id);
    let _=output.send(reply).await;
}

pub async fn serve(runtime:Runtime)->Result<()>{
    let runtime=Arc::new(runtime);
    runtime.maintain_cache().await;
    let registry:Registry=Arc::new(Mutex::new(HashMap::new()));
    let display_gate=Arc::new(Semaphore::new(1));
    let (output,mut replies)=mpsc::channel::<Value>(32);
    let writer=tokio::spawn(async move{
        let mut stdout=tokio::io::stdout();
        while let Some(reply)=replies.recv().await{
            let mut encoded=serde_json::to_vec(&reply)?;encoded.push(b'\n');stdout.write_all(&encoded).await?;stdout.flush().await?;
        }
        Ok::<(),anyhow::Error>(())
    });
    let (queue,mut pending)=mpsc::channel::<Job>(16);
    let worker={let (runtime,output,registry,display_gate)=(runtime.clone(),output.clone(),registry.clone(),display_gate.clone());
        tokio::spawn(async move{while let Some(job)=pending.recv().await{run(runtime.clone(),job,output.clone(),registry.clone(),display_gate.clone()).await;}})};
    let mut tasks=JoinSet::new();
    let mut input=BufReader::new(tokio::io::stdin());
    let mut maintenance=tokio::time::interval(std::time::Duration::from_secs(60));
    maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut shutdown_id = None;
    loop{
        while tasks.try_join_next().is_some(){}
        // Chapter requests may contain the current and preceding paths per page.
        let read=protocol::line(&mut input,4*1024*1024);
        tokio::pin!(read);
        let next=loop {tokio::select! {
            result=&mut read=>break result,
            _=maintenance.tick()=>{while tasks.try_join_next().is_some(){} let runtime=runtime.clone();tasks.spawn(async move {runtime.maintain_cache().await;});},
        }};
        let bytes=match next{
            Ok(Some(bytes))=>bytes,Ok(None)=>break,Err(error)=>{let _=output.send(json!({"id":null,"event":"error","error":error.to_string()})).await;break;}
        };
        let request:Value=match serde_json::from_slice(&bytes){Ok(value)=>value,Err(_)=>{
            let _=output.send(json!({"id":null,"event":"error","error":"invalid request JSON"})).await;continue;
        }};
        let id=match request["id"].as_str().filter(|s|!s.is_empty()&&s.len()<=128){Some(id)=>id.to_owned(),None=>{
            let _=output.send(json!({"id":null,"event":"error","error":"invalid request id"})).await;continue;
        }};
        match request["op"].as_str().unwrap_or(""){
            "health"=>{let active=registry.lock().await.len();let _=output.send(json!({"id":id,"event":"result","result":{"protocol":1,"active":active}})).await;continue;},
            "cancel"=>{let target=request["target"].as_str().unwrap_or("");
                let cancelled=registry.lock().await.get(target).is_some_and(|job|job.cancel.send(true).is_ok());
                let _=output.send(json!({"id":id,"event":"result","result":{"cancelled":cancelled}})).await;continue;
            },
            "shutdown"=>{shutdown_id = Some(id); break;},
            _=>{},
        }
        let (cancel,receiver)=watch::channel(false);
        {
            let mut registry=registry.lock().await;
            if registry.contains_key(&id)||registry.len()>=64{
                drop(registry);
                let _=output.send(json!({"id":id,"event":"error","error":"重复请求编号或队列已满，请稍后重试"})).await;continue;
            }
            registry.insert(id.clone(), RegisteredJob {
                cancel,
                finish_on_shutdown: request["op"] == "unload_model",
            });
        }
        let job=Job{id:id.clone(),request,cancel:receiver};
        if matches!(job.request["op"].as_str(), Some("process" | "unload_model" | "ocr_crop_preview" | "ocr_crop_apply")){
            if queue.try_send(job).is_err(){registry.lock().await.remove(&id);
                let _=output.send(json!({"id":id,"event":"error","error":"处理队列已满，请稍后重试"})).await;
            }
        }else{tasks.spawn(run(runtime.clone(),job,output.clone(),registry.clone(),display_gate.clone()));}
    }
    for job in registry.lock().await.values() {
        if !job.finish_on_shutdown { let _ = job.cancel.send(true); }
    }
    drop(queue);
    let worker_result = worker.await;
    while tasks.join_next().await.is_some(){}
    let unload_error = runtime.translation_models.close().await;
    if let Some(error) = &unload_error { eprintln!("{error}"); }
    let close_result = runtime.close().await;
    if let Some(id) = shutdown_id {
        let mut result = json!({"shutdown":true});
        if let Some(error) = unload_error { result["unload_error"] = json!(error); }
        let _ = output.send(json!({"id":id,"event":"result","result":result})).await;
    }
    drop(output);writer.await??;
    worker_result?;
    close_result
}
