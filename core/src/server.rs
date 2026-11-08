//! Bounded JSONL application service. Rust owns processing queues, cancellation,
//! per-page retries, model lifetime and cache writes. No credentials are logged.
use crate::{persistence,protocol,runtime::{Runtime,ProcessOptions},service::TranslationService,translate_pipeline::Observer};
use anyhow::{Result,Context,ensure,bail};
use serde_json::{Value,json};
use std::{collections::HashMap,path::{Path,PathBuf},sync::Arc};
use tokio::{io::{AsyncWriteExt,BufReader},sync::{mpsc,watch,Mutex,Semaphore},task::JoinSet};

type Registry=Arc<Mutex<HashMap<String,watch::Sender<bool>>>>;
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

async fn execute(runtime:&Runtime,job:&Job,output:&mpsc::Sender<Value>,display_gate:&Semaphore)->Result<Value>{
    let r=&job.request;
    match required(r,"op")?{
        "load_page"=>runtime.load_page(Path::new(required(r,"image")?),r["mode"].as_str().unwrap_or("text")).await,
        "models"=>Ok(json!({"models":service(r)?.list_models().await?})),
        "display"=>{
            let _permit=display_gate.acquire().await?;
            runtime.display_page(Path::new(required(r,"image")?),Path::new(required(r,"cache")?)).await
        },
        "edit"=>runtime.edit(Path::new(required(r,"cache")?),required(r,"block_id")?,&r["values"],r["revision"].as_str()).await,
        "process"=>{
            let images=r["images"].as_array().context("missing images")?;
            ensure!(!images.is_empty()&&images.len()<=4096,"invalid page queue size");
            let service=service(r)?;
            let options=ProcessOptions{operation:r["operation"].as_str().unwrap_or("both"),mode:r["mode"].as_str().unwrap_or("text"),
                model:required(r,"model")?,block_id:r["block_id"].as_str(),unit:r["unit"].as_str().unwrap_or("blocks"),
                batch_size:r["batch_size"].as_u64().unwrap_or(8) as usize,retry:r["retry"].as_bool().unwrap_or(false)};
            let mut results=vec![];
            for (index,image) in images.iter().enumerate(){
                let image=image.as_str().context("invalid image path")?;
                let _=output.send(json!({"id":job.id,"event":"page_started","image":image,"index":index,"total":images.len()})).await;
                let page_result=async{
                    let mut events=Events{id:job.id.clone(),image:image.into(),cache:runtime.cache_for(Path::new(image),options.mode)?,output:output.clone()};
                    runtime.process_page(Path::new(image),&options,&service,&mut events).await
                }.await;
                let result=match page_result{
                    Ok(value)=>json!({"image":image,"ok":true,"result":value}),
                    Err(error)=>json!({"image":image,"ok":false,"error":service.safe_error(error)}),
                };
                let _=output.send(json!({"id":job.id,"event":"page_finished","done":index+1,"total":images.len(),"page":result})).await;
                results.push(result);
            }
            Ok(json!({"pages":results}))
        },
        _=>bail!("unknown operation"),
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
                json!({"id":job.id,"event":"error","error":message})
            },
            None=>cancelled(&job.id),
        }}
    };
    // Release owned OCR allocations before another translation job; a cancelled
    // model call also discards its poisoned channel and private array workspace.
    if job.request["op"]=="process"{let _=runtime.release_ocr().await;}
    registry.lock().await.remove(&job.id);
    let _=output.send(reply).await;
}

pub async fn serve(runtime:Runtime)->Result<()>{
    let runtime=Arc::new(runtime);
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
    loop{
        while tasks.try_join_next().is_some(){}
        let bytes=match protocol::line(&mut input,1024*1024).await{
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
                let cancelled=registry.lock().await.get(target).is_some_and(|sender|sender.send(true).is_ok());
                let _=output.send(json!({"id":id,"event":"result","result":{"cancelled":cancelled}})).await;continue;
            },
            "shutdown"=>{let _=output.send(json!({"id":id,"event":"result","result":{"shutdown":true}})).await;break;},
            _=>{},
        }
        let (cancel,receiver)=watch::channel(false);
        {
            let mut registry=registry.lock().await;
            if registry.contains_key(&id)||registry.len()>=64{
                drop(registry);
                let _=output.send(json!({"id":id,"event":"error","error":"重复请求编号或队列已满，请稍后重试"})).await;continue;
            }
            registry.insert(id.clone(),cancel);
        }
        let job=Job{id:id.clone(),request,cancel:receiver};
        if job.request["op"]=="process"{
            if queue.try_send(job).is_err(){registry.lock().await.remove(&id);
                let _=output.send(json!({"id":id,"event":"error","error":"处理队列已满，请稍后重试"})).await;
            }
        }else{tasks.spawn(run(runtime.clone(),job,output.clone(),registry.clone(),display_gate.clone()));}
    }
    for cancel in registry.lock().await.values(){let _=cancel.send(true);}
    drop(queue);worker.await?;
    while tasks.join_next().await.is_some(){}
    runtime.close().await?;
    drop(output);writer.await??;
    Ok(())
}
