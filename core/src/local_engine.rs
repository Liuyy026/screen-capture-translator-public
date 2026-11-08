//! Own only a local server that this application started. Existing endpoints
//! are queried read-only; their processes are never adopted or terminated.
use crate::service::TranslationService;
use anyhow::{Result,Context,bail};
use std::{path::{Path,PathBuf},process::Stdio,time::Duration};
use std::os::unix::process::CommandExt;
use tokio::{process::{Command,Child},sync::Mutex};

struct OwnedProcess { child:Child, group:i32, active:bool }
impl OwnedProcess {
    fn spawn(executable:&Path,models:&Path)->Result<Self>{
        let mut command=Command::new(executable);
        command.arg("serve").env_remove("KOMA_TRANSLATION_API_KEY")
            .env("OLLAMA_HOST","127.0.0.1:11439").env("OLLAMA_MODELS",models)
            .env("OLLAMA_NO_CLOUD","1").env("OLLAMA_NUM_PARALLEL","1").env("OLLAMA_MAX_LOADED_MODELS","1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        // Runner descendants share this private process group. Never signal an
        // endpoint discovered through HTTP, or another application's group.
        command.as_std_mut().process_group(0);
        let child=command.spawn().context("无法启动本机 Ollama，请检查安装或配置其他接口。")?;
        let group=child.id().context("missing owned process id")? as i32;
        Ok(Self{child,group,active:true})
    }
    async fn stop(mut self)->Result<()>{
        unsafe { libc::kill(-self.group,libc::SIGTERM); }
        let result=tokio::time::timeout(Duration::from_secs(3),self.child.wait()).await;
        // Clean up any runner left after the parent exits, or a stuck parent.
        unsafe { libc::kill(-self.group,libc::SIGKILL); }
        if result.is_err(){self.child.wait().await?;}else{result??;}
        self.active=false;
        Ok(())
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self){
        if self.active {unsafe { libc::kill(-self.group,libc::SIGKILL); }}
    }
}

pub struct LocalEngine { executable:Option<PathBuf>,models:PathBuf,owned:Mutex<Option<OwnedProcess>> }
impl LocalEngine {
    pub fn new(executable:Option<PathBuf>,models:PathBuf)->Self{Self{executable,models,owned:Mutex::new(None)}}
    pub async fn discover(&self,service:&TranslationService,allow_start:bool)->Result<Vec<String>>{
        self.discover_with_permission(service,allow_start&&service.allows_local_start()).await
    }
    async fn discover_with_permission(&self,service:&TranslationService,may_start:bool)->Result<Vec<String>>{
        // Serializes simultaneous probes with release. A provisional child is
        // local to this future, so cancellation kills it before another spawn.
        let mut owned=self.owned.lock().await;
        let error=match service.list_models().await{Ok(models)=>return Ok(models),Err(error)=>error};
        if !may_start{return Err(error);}
        let executable=self.executable.as_ref().filter(|p|p.is_file())
            .context("本机 Ollama 尚未安装，请运行 macos/安装.command 或配置其他接口。")?;
        if let Some(process)=owned.take(){process.stop().await?;}
        let mut process=OwnedProcess::spawn(executable,&self.models)?;
        for _ in 0..24{
            tokio::time::sleep(Duration::from_millis(250)).await;
            if process.child.try_wait()?.is_some(){break;}
            if let Ok(models)=service.list_models().await{
                // Successful probes retain the server until settings release
                // it or the Rust service exits. No language model is loaded here.
                *owned=Some(process);return Ok(models);
            }
        }
        process.stop().await?;
        bail!("本机 Ollama 未能连接，请检查端口或接口设置。")
    }
    pub async fn release(&self)->Result<()>{
        if let Some(process)=self.owned.lock().await.take(){process.stop().await?;}
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[tokio::test]
    async fn stop_and_cancel_only_owned_process_groups()->Result<()>{
        let dir=tempfile::tempdir()?;
        let executable=dir.path().join("fake-ollama");
        std::fs::write(&executable,"#!/bin/sh\nsleep 60 &\necho $! > \"$OLLAMA_MODELS/runner.pid\"\nwait\n")?;
        std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700))?;
        for cancel in [false,true]{
            let process=OwnedProcess::spawn(&executable,dir.path())?;
            let parent=process.group;
            for _ in 0..100{if dir.path().join("runner.pid").exists(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
            let runner=std::fs::read_to_string(dir.path().join("runner.pid"))?.trim().parse::<i32>()?;
            if cancel {drop(process);} else {process.stop().await?;}
            for _ in 0..100{
                if unsafe{libc::kill(parent,0)}!=0&&unsafe{libc::kill(runner,0)}!=0{break;}
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_ne!(unsafe{libc::kill(parent,0)},0);
            assert_ne!(unsafe{libc::kill(runner,0)},0);
            std::fs::remove_file(dir.path().join("runner.pid"))?;
        }
        Ok(())
    }
    #[test]
    fn remote_authenticated_and_omlx_connections_never_start_local_servers()->Result<()>{
        for (provider,url,key) in [("omlx","http://127.0.0.1:11439",""),("ollama","http://localhost:11439",""),
            ("ollama","http://127.0.0.1:11434",""),("ollama","http://127.0.0.1:11439","secret")]{
            assert!(!TranslationService::new(provider,Some(url),Some(key))?.allows_local_start());
        }
        assert!(TranslationService::new("ollama",None,Some(""))?.allows_local_start());Ok(())
    }
    #[tokio::test]
    async fn probe_reuse_external_ownership_failure_and_cancel()->Result<()>{
        let dir=tempfile::tempdir()?;
        let listener=std::net::TcpListener::bind("127.0.0.1:0")?;
        let port=listener.local_addr()?.port();drop(listener);
        let executable=dir.path().join("fake-server");
        let script=format!(r#"#!/usr/bin/env python3
import json, os, time
from pathlib import Path
from http.server import BaseHTTPRequestHandler, HTTPServer
directory=Path(os.environ['OLLAMA_MODELS'])
assert 'KOMA_TRANSLATION_API_KEY' not in os.environ
(directory/'server.pid').write_text(str(os.getpid()))
if (directory/'fail').exists():raise SystemExit(21)
if (directory/'hang').exists():time.sleep(60)
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_GET(self):
        body=b'{{"models":[{{"name":"fake-local"}}]}}'
        self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=HTTPServer(('127.0.0.1',{port}),Handler)
server.serve_forever()
"#);
        std::fs::write(&executable,script)?;
        std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700))?;
        let engine=LocalEngine::new(Some(executable.clone()),dir.path().to_owned());
        let service=TranslationService::new("ollama",Some(&format!("http://127.0.0.1:{port}")),Some(""))?;
        // A test-only call to the private post-authorization path uses an
        // ephemeral port; production authorization remains fixed and tested.
        assert_eq!(engine.discover_with_permission(&service,true).await?,vec!["fake-local"]);
        let pid=std::fs::read_to_string(dir.path().join("server.pid"))?;
        assert_eq!(engine.discover_with_permission(&service,true).await?,vec!["fake-local"]);
        assert_eq!(std::fs::read_to_string(dir.path().join("server.pid"))?,pid);
        engine.release().await?;
        assert_ne!(unsafe{libc::kill(pid.parse()?,0)},0);

        let external=OwnedProcess::spawn(&executable,dir.path())?;
        for _ in 0..100{if service.list_models().await.is_ok(){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        assert_eq!(engine.discover(&service,true).await?,vec!["fake-local"]);
        engine.release().await?;
        assert_eq!(unsafe{libc::kill(external.group,0)},0);
        external.stop().await?;

        std::fs::write(dir.path().join("fail"),"")?;
        assert!(engine.discover_with_permission(&service,true).await.is_err());
        assert!(engine.owned.lock().await.is_none());
        std::fs::remove_file(dir.path().join("fail"))?;
        std::fs::write(dir.path().join("hang"),"")?;
        assert!(tokio::time::timeout(Duration::from_millis(150),engine.discover_with_permission(&service,true)).await.is_err());
        let pid=std::fs::read_to_string(dir.path().join("server.pid"))?.parse::<i32>()?;
        for _ in 0..100{if unsafe{libc::kill(pid,0)}!=0{break;}tokio::time::sleep(Duration::from_millis(10)).await;}
        assert_ne!(unsafe{libc::kill(pid,0)},0);
        assert!(engine.owned.lock().await.is_none());
        Ok(())
    }
}
