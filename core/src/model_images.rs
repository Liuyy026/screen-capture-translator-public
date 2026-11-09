//! Image-only model client. Raw arrays remain inside a task-owned directory.
use crate::models::ModelWorker;
use anyhow::{Context, Result, ensure};
use opencv::{
    core::{CV_8UC1, CV_8UC3, Mat, Scalar},
    prelude::*,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct ImageModels {
    worker: Option<ModelWorker>,
    workspace: tempfile::TempDir,
    python: PathBuf,
    script: PathBuf,
    weights: PathBuf,
    serial: u64,
}

impl ImageModels {
    async fn prepare_request(&mut self)->Result<()>{
        if self.worker.as_ref().is_some_and(ModelWorker::interrupted){
            // Reap the old process before reusing GPU memory or deleting files
            // it may still be accessing after cancellation.
            if let Some(worker)=self.worker.take(){worker.shutdown().await?;}
            self.workspace=tempfile::tempdir()?;
        }
        Ok(())
    }
    pub fn new(python: &Path, script: &Path, weights: &Path) -> Result<Self> {
        Ok(Self {
            worker: None,
            workspace: tempfile::tempdir()?,
            python: python.into(),
            script: script.into(),
            weights: weights.into(),
            serial: 0,
        })
    }

    fn write_array(&self, name: &str, image: &Mat) -> Result<Value> {
        ensure!(
            matches!(image.typ(), CV_8UC1 | CV_8UC3) && !image.empty(),
            "invalid model image"
        );
        let image = image.try_clone()?;
        let bytes = image.data_bytes()?;
        ensure!(
            bytes.len() <= 128 * 1024 * 1024,
            "model image exceeds size limit"
        );
        let mut handle = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.workspace.path().join(name))?;
        handle.write_all(bytes)?;
        let shape = if image.channels() == 1 {
            json!([image.rows(), image.cols()])
        } else {
            json!([image.rows(), image.cols(), 3])
        };
        Ok(json!({"path":name,"shape":shape}))
    }

    fn read_array(
        &self,
        value: &Value,
        name: &str,
        width: i32,
        height: i32,
        channels: i32,
    ) -> Result<Mat> {
        ensure!(
            value["path"] == name,
            "model returned unexpected output path"
        );
        let shape = if channels == 1 {
            json!([height, width])
        } else {
            json!([height, width, 3])
        };
        ensure!(
            value["shape"] == shape,
            "model returned unexpected output shape"
        );
        let path = self.workspace.path().join(name).canonicalize()?;
        ensure!(
            path.starts_with(self.workspace.path().canonicalize()?),
            "model output escaped workspace"
        );
        let size = width as u64 * height as u64 * channels as u64;
        ensure!(
            size <= 128 * 1024 * 1024 && std::fs::metadata(&path)?.len() == size,
            "model output has invalid size"
        );
        let data = std::fs::read(path)?;
        let mut image = Mat::new_rows_cols_with_default(
            height,
            width,
            if channels == 1 { CV_8UC1 } else { CV_8UC3 },
            Scalar::all(0.0),
        )?;
        image.data_bytes_mut()?.copy_from_slice(&data);
        Ok(image)
    }

    async fn call(&mut self, request: Value) -> Result<Value> {
        if self.worker.is_none() {
            self.worker = Some(
                ModelWorker::spawn(
                    &self.python,
                    &self.script,
                    self.workspace.path(),
                    &self.weights,
                )
                .await?,
            );
        }
        self.worker
            .as_mut()
            .unwrap()
            .request(request, Duration::from_secs(300))
            .await
    }

    pub async fn restore(&mut self, image: &Mat, mask: &Mat) -> Result<Mat> {
        self.prepare_request().await?;
        ensure!(
            image.size()? == mask.size()? && mask.typ() == CV_8UC1,
            "restoration mask size mismatch"
        );
        self.serial += 1;
        let prefix = format!("restore-{}", self.serial);
        let image_name = format!("{prefix}-image.bin");
        let mask_name = format!("{prefix}-mask.bin");
        let output_name = format!("{prefix}-output.bin");
        let input = self.write_array(&image_name, image)?;
        let mask_input = self.write_array(&mask_name, mask)?;
        let reply = self
            .call(json!({"op":"restore","image":input,"mask":mask_input,"output":output_name}))
            .await
            .context("restore background model")?;
        let output = self.read_array(
            &reply["image"],
            &output_name,
            image.cols(),
            image.rows(),
            image.channels(),
        )?;
        for name in [&image_name, &mask_name, &output_name] {
            std::fs::remove_file(self.workspace.path().join(name))?;
        }
        Ok(output)
    }

    pub async fn detect(&mut self, image: &Mat) -> Result<Mat> {
        self.prepare_request().await?;
        self.serial += 1;
        let prefix = format!("detect-{}", self.serial);
        let image_name = format!("{prefix}-image.bin");
        let raw_name = format!("{prefix}-raw.bin");
        let refined_name = format!("{prefix}-refined.bin");
        let input = self.write_array(&image_name, image)?;
        let reply = self.call(json!({"op":"detect","image":input,"raw_output":raw_name,"refined_output":refined_name})).await.context("detect text model")?;
        let raw = self.read_array(&reply["raw"], &raw_name, image.cols(), image.rows(), 1)?;
        let mut refined = self.read_array(
            &reply["refined"],
            &refined_name,
            image.cols(),
            image.rows(),
            1,
        )?;
        for (v, &r) in refined.data_bytes_mut()?.iter_mut().zip(raw.data_bytes()?) {
            *v = if *v > 0 || r > 160 { 255 } else { 0 };
        }
        for name in [&image_name, &raw_name, &refined_name] {
            std::fs::remove_file(self.workspace.path().join(name))?;
        }
        Ok(refined)
    }

    pub async fn ocr(&mut self,image:&Path)->Result<Value>{
        self.prepare_request().await?;
        self.serial+=1;
        let extension=image.extension().and_then(|s|s.to_str()).unwrap_or("image");
        let name=format!("ocr-{}.{}",self.serial,extension);
        let temporary=self.workspace.path().join(&name);
        std::fs::copy(image,&temporary).context("copy OCR input into model workspace")?;
        let result=self.call(json!({"op":"ocr","image_path":name})).await.context("recognize Japanese text model");
        let _=std::fs::remove_file(temporary);
        result
    }

    pub async fn release_ocr(&mut self)->Result<()>{
        self.prepare_request().await?;
        if self.worker.is_some(){self.call(json!({"op":"release_ocr"})).await?;}
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.stop().await
    }

    pub async fn stop(&mut self)->Result<()>{
        if let Some(worker) = self.worker.take() {
            worker.shutdown().await?;
        }
        Ok(())
    }
}
