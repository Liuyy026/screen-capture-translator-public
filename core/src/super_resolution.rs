//! Optional image preparation. The source is immutable; only validated private PNGs are published.
use anyhow::{Result,ensure,Context};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{path::{Path,PathBuf},process::Stdio};
use opencv::prelude::*;

#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(default,deny_unknown_fields)]
pub struct Settings{pub engine:String,pub target:u32,pub axis:String,pub preset:String}
impl Default for Settings{fn default()->Self{Self{engine:"off".into(),target:1440,axis:"long_edge".into(),preset:"standard".into()}}}
pub fn dimensions(w:u32,h:u32,s:&Settings)->Result<Option<(u32,u32)>>{
    ensure!(["off","metalfx","realcugan"].contains(&s.engine.as_str()),"未知增强引擎");
    ensure!(["long_edge","height"].contains(&s.axis.as_str())&&[1440,2160].contains(&s.target),"无效增强尺寸设置");
    ensure!(w>0&&h>0,"无效图片尺寸");
    let edge=if s.axis=="height"{h}else{w.max(h)};
    if s.engine=="off"||edge>=s.target{return Ok(None)}
    let scale=s.target as f64/edge as f64;
    let result=((w as f64*scale).round() as u32,(h as f64*scale).round() as u32);
    ensure!(result.0<=16384&&result.1<=16384,"增强尺寸超过限制");Ok(Some(result))
}
// A dedicated process group prevents helper descendants surviving cancellation.
struct Group{pid:i32}
impl Drop for Group{fn drop(&mut self){unsafe{libc::kill(-self.pid,libc::SIGKILL);}}}
async fn invoke(exe:&Path,args:&[String])->Result<Value>{
    let mut cmd=tokio::process::Command::new(exe);
    cmd.env_remove("KOMA_TRANSLATION_API_KEY").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)] {cmd.process_group(0);}
    let child=cmd.spawn().context("增强工具尚未准备，请先构建所选后端")?;
    let _group=Group{pid:child.id().context("missing helper pid")? as i32};
    let output=tokio::time::timeout(std::time::Duration::from_secs(180),child.wait_with_output()).await.context("增强工具超时")??;
    ensure!(output.status.success(),"增强失败：{}",String::from_utf8_lossy(&output.stderr).chars().take(2000).collect::<String>());
    ensure!(output.stdout.len()<65536,"增强工具响应过大");
    Ok(serde_json::from_slice(&output.stdout).context("无效增强工具响应")?)
}
pub struct Store{pub root:PathBuf,pub helpers:PathBuf,pub gate:tokio::sync::Mutex<()>}
impl Store{
    pub async fn capabilities(&self,engine:&str)->Value{
        if !["metalfx","realcugan"].contains(&engine){return json!({"engine":engine,"available":false,"presets":[],"reason":"unknown engine"})}
        match invoke(&self.helpers.join(format!("koma-sr-{engine}")),&["--capabilities".into()]).await{
            Ok(v)=>v,Err(e)=>json!({"engine":engine,"available":false,"presets":[],"reason":e.to_string()}),
        }
    }
    pub async fn prepare(&self,input:&Path,s:&Settings)->Result<PathBuf>{
        if s.engine=="off"{return Ok(input.to_owned())}
        ensure!(["conservative","standard","denoise"].contains(&s.preset.as_str()),"未知增强预设");
        ensure!(s.engine!="metalfx"||s.preset=="standard","MetalFX 仅支持 standard 预设");
        let _guard=self.gate.lock().await;
        let image=crate::imaging::decode_color(input)?;
        let Some((w,h))=dimensions(image.cols() as u32,image.rows() as u32,s)? else{return Ok(input.to_owned())};
        let exe=self.helpers.join(format!("koma-sr-{}",s.engine));
        let capability=invoke(&exe,&["--capabilities".into()]).await?;
        ensure!(capability["engine"]==s.engine&&capability["available"]==true,"增强引擎不可用：{}",capability["reason"]);
        ensure!(capability["presets"].as_array().is_some_and(|p|p.iter().any(|p|p==&s.preset)),"所选增强预设不受支持");
        let identity=json!({"schema":1,"source_sha256":format!("{:x}",Sha256::digest(std::fs::read(input)?)),"settings":s,"width":w,"height":h,"capability":capability,"executable_sha256":format!("{:x}",Sha256::digest(std::fs::read(&exe)?))});
        let key=format!("{:x}",Sha256::digest(serde_json::to_vec(&identity)?));
        let directory=self.root.join(key);std::fs::create_dir_all(&directory)?;
        let target=directory.join("effective.png");
        if target.exists(){
            let valid=(||->Result<bool>{
                let metadata:Value=serde_json::from_slice(&std::fs::read(directory.join("identity.json"))?)?;
                let hash=format!("{:x}",Sha256::digest(std::fs::read(&target)?));
                let check=crate::imaging::decode_color(&target)?;
                Ok(metadata["identity"]==identity&&metadata["output_sha256"]==hash&&check.cols()==w as i32&&check.rows()==h as i32)
            })().unwrap_or(false);
            if valid{return Ok(target)}
        }
        let temp=tempfile::Builder::new().prefix("inference-").tempdir_in(&directory)?;
        let output=temp.path().join("output.png");
        let result=invoke(&exe,&["--input".into(),input.canonicalize()?.to_string_lossy().into(),"--output".into(),output.to_string_lossy().into(),"--width".into(),w.to_string(),"--height".into(),h.to_string(),"--preset".into(),s.preset.clone()]).await?;
        ensure!(result["engine"]==s.engine&&result["width"]==w&&result["height"]==h&&result["elapsed_ms"].is_number(),"增强响应与请求不匹配");
        ensure!(identity["source_sha256"]==format!("{:x}",Sha256::digest(std::fs::read(input)?)),"增强期间原文件发生变化，请重试");
        let check=crate::imaging::decode_color(&output)?;ensure!(check.cols()==w as i32&&check.rows()==h as i32,"增强输出尺寸不匹配");
        crate::persistence::atomic_json(&directory.join("identity.json"),&json!({"identity":identity,"result":result,"output_sha256":format!("{:x}",Sha256::digest(std::fs::read(&output)?))}))?;
        std::fs::rename(output,&target)?;Ok(target)
    }
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn sizes_and_skip(){let mut s=Settings{engine:"metalfx".into(),..Default::default()};assert_eq!(dimensions(540,720,&s).unwrap(),Some((1080,1440)));assert_eq!(dimensions(2160,1440,&s).unwrap(),None);s.axis="height".into();assert_eq!(dimensions(2160,720,&s).unwrap(),Some((4320,1440)));s.engine="off".into();assert_eq!(dimensions(540,720,&s).unwrap(),None);s.target=0;assert!(dimensions(540,720,&s).is_err());}
}
