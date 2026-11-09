//! Disposable native display cache, independent from OCR/translation storage.
use crate::{compat,imaging,ink,model_images::ImageModels,persistence,reading};
use anyhow::{Result,ensure};
use opencv::{core::{Mat,Vector},imgcodecs,imgproc,prelude::*};
use serde_json::{Value,json};
use sha2::{Digest,Sha256};
use std::{path::{Path,PathBuf},time::UNIX_EPOCH};

pub struct Store {pub root:PathBuf,pub weights:PathBuf}

fn model_identity(digest:&mut Sha256,path:&Path)->Result<()>{
    let path=path.canonicalize()?;let metadata=std::fs::metadata(&path)?;
    digest.update(path.to_string_lossy().as_bytes());
    digest.update(metadata.len().to_le_bytes());
    digest.update(metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos().to_le_bytes());
    Ok(())
}

impl Store{
    pub fn key(&self,image:&[u8],cache:&[u8])->Result<String>{
        let mut digest=Sha256::new();digest.update(b"koma-native-display-v1\0");digest.update(image);digest.update([0]);digest.update(cache);
        for code in [include_bytes!("reading.rs").as_slice(),include_bytes!("ink.rs").as_slice(),include_bytes!("geometry.rs").as_slice(),
            include_bytes!("background.rs").as_slice(),include_bytes!("texture.rs").as_slice(),include_bytes!("raster.rs").as_slice(),
            include_bytes!("imaging.rs").as_slice(),include_bytes!("display_cache.rs").as_slice(),include_bytes!("compat.rs").as_slice(),
            include_bytes!("model_images.rs").as_slice(),include_bytes!("../python/model_worker.py").as_slice()]{digest.update(code);}
        for name in ["comictextdetector.pt","anime-manga-big-lama.pt"]{model_identity(&mut digest,&self.weights.join(name))?;}
        Ok(format!("{:x}",digest.finalize()))
    }
    fn page_path(&self,key:&str)->PathBuf{self.root.join("pages").join(format!("{key}.json"))}
    pub fn load(&self,key:&str)->Option<Value>{
        let wrapper:Value=serde_json::from_slice(&std::fs::read(self.page_path(key)).ok()?).ok()?;
        if wrapper["cache_version"]!=1||wrapper["key"]!=key{return None;}
        let reply=wrapper.get("payload")?;
        if reply["version"]!=3||!reply["regions"].is_array()||!reply["preserved"].is_object(){return None;}
        let hash=format!("{:x}",Sha256::digest(serde_json::to_vec(reply).ok()?));
        if wrapper["payload_sha256"]!=hash{return None;}
        let _=crate::cache_policy::touch(&self.page_path(key));
        Some(reply.clone())
    }
    pub fn save(&self,key:&str,reply:&Value)->Result<()>{
        let hash=format!("{:x}",Sha256::digest(serde_json::to_vec(reply)?));
        persistence::atomic_json(&self.page_path(key),&json!({"cache_version":1,"key":key,"payload_sha256":hash,"payload":reply}))
    }
    fn mask_path(&self,image:&[u8])->Result<PathBuf>{
        let mut digest=Sha256::new();digest.update(b"koma-native-ctd-mask-v1\0");digest.update(image);
        digest.update(include_bytes!("model_images.rs"));digest.update(include_bytes!("../python/model_worker.py"));
        digest.update(include_bytes!("imaging.rs"));
        model_identity(&mut digest,&self.weights.join("comictextdetector.pt"))?;
        Ok(self.root.join("masks").join(format!("{:x}.png",digest.finalize())))
    }
    pub fn mask(&self,image:&[u8],width:i32,height:i32)->Option<Mat>{
        let path=self.mask_path(image).ok()?;
        let mask=imaging::decode_gray(&path).ok()?;
        if mask.cols()!=width||mask.rows()!=height{return None;}
        let _=crate::cache_policy::touch(&path);
        Some(mask)
    }
    pub fn save_mask(&self,image:&[u8],mask:&Mat)->Result<()>{
        let path=self.mask_path(image)?;
        let mut encoded=Vector::new();
        ensure!(imgcodecs::imencode(".png",mask,&mut encoded,&Vector::new())?,"cannot encode detector mask");
        persistence::atomic_bytes(&path,encoded.as_slice())
    }
}

pub async fn generate(image_bytes:&[u8],page:&Value,source:Option<Mat>,mut models:Option<&mut ImageModels>,store:Option<&Store>)->Result<Value>{
    let color=imaging::decode_bytes(image_bytes,imgcodecs::IMREAD_COLOR)?;
    let mut gray=Mat::default();imgproc::cvt_color_def(&color,&mut gray,imgproc::COLOR_BGR2GRAY)?;
    let has_color=color.data_bytes()?.chunks_exact(3).any(|p|p.iter().max().unwrap()-p.iter().min().unwrap()>8);
    let mut source=source;
    if source.is_none()&&compat::array(&page["blocks"]).iter().any(ink::ready){
        if let Some(store)=store{source=store.mask(image_bytes,color.cols(),color.rows());}
        if source.is_none()&&let Some(models)=models.as_mut(){
            let mask=models.detect(&color).await?;
            if let Some(store)=store{store.save_mask(image_bytes,&mask)?;}
            source=Some(mask);
        }
    }
    let regions=reading::regions(page,&gray,if has_color{Some(&color)}else{None},source.as_ref(),models).await?;
    Ok(json!({"version":3,"regions":regions,"preserved":ink::preserved_markers(page)}))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn corrupt_payload_is_not_a_cache_hit()->Result<()>{
        let dir=tempfile::tempdir()?;let store=Store{root:dir.path().to_owned(),weights:dir.path().to_owned()};
        let reply=json!({"version":3,"regions":[],"preserved":{}});
        store.save("test",&reply)?;assert_eq!(store.load("test"),Some(reply));
        let path=store.page_path("test");let mut saved:Value=serde_json::from_slice(&std::fs::read(&path)?)?;
        saved["payload"]["regions"]=json!([{"wrong":true}]);persistence::atomic_json(&path,&saved)?;
        assert!(store.load("test").is_none());Ok(())
    }
}
