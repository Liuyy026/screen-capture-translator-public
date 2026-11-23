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
    ocr_requested: bool,
    workspace: tempfile::TempDir,
    python: PathBuf,
    script: PathBuf,
    weights: PathBuf,
}

impl ImageModels {
    async fn prepare_request(&mut self)->Result<()>{
        if self.worker.as_ref().is_some_and(ModelWorker::interrupted){
            // Reap the old process before reusing GPU memory or deleting files
            // it may still be accessing after cancellation.
            self.ocr_requested = false;
            if let Some(worker)=self.worker.take(){worker.shutdown().await?;}
            self.workspace=tempfile::tempdir()?;
        }
        Ok(())
    }
    pub fn new(python: &Path, script: &Path, weights: &Path) -> Result<Self> {
        Ok(Self {
            worker: None,
            ocr_requested: false,
            workspace: tempfile::tempdir()?,
            python: python.into(),
            script: script.into(),
            weights: weights.into(),
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

    // A cancelled worker may still be reading or writing. Leave its directory in
    // place until prepare_request has reaped it. Ordinary model errors can clean up now.
    async fn with_job_files<T>(&mut self, operation: impl AsyncFnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.prepare_request().await?;
        let job = self.workspace.path().join("job");
        if job.try_exists()? {
            std::fs::remove_dir_all(&job)?;
        }
        std::fs::create_dir(&job)?;
        let result = operation(self).await;
        if !self.worker.as_ref().is_some_and(ModelWorker::interrupted) {
            std::fs::remove_dir_all(job).context("clean up model job files")?;
        }
        result
    }

    pub async fn restore(&mut self, image: &Mat, mask: &Mat) -> Result<Mat> {
        ensure!(
            image.size()? == mask.size()? && mask.typ() == CV_8UC1,
            "restoration mask size mismatch"
        );
        self.with_job_files(async |models| {
            let input = models.write_array("job/image.bin", image)?;
            let mask_input = models.write_array("job/mask.bin", mask)?;
            let output = "job/output.bin";
            let reply = models.call(json!({"op":"restore","image":input,"mask":mask_input,"output":output}))
                .await.context("restore background model")?;
            models.read_array(&reply["image"], output, image.cols(), image.rows(), image.channels())
        }).await
    }

    pub async fn detect(&mut self, image: &Mat) -> Result<Mat> {
        self.with_job_files(async |models| {
            let input = models.write_array("job/image.bin", image)?;
            let raw_name = "job/raw.bin";
            let refined_name = "job/refined.bin";
            let reply = models.call(json!({"op":"detect","image":input,"raw_output":raw_name,"refined_output":refined_name}))
                .await.context("detect text model")?;
            let raw = models.read_array(&reply["raw"], raw_name, image.cols(), image.rows(), 1)?;
            let mut refined = models.read_array(&reply["refined"], refined_name, image.cols(), image.rows(), 1)?;
            for (value, &r) in refined.data_bytes_mut()?.iter_mut().zip(raw.data_bytes()?) {
                *value = if *value > 0 || r > 160 { 255 } else { 0 };
            }
            Ok(refined)
        }).await
    }

    pub async fn ocr(&mut self, image: &Path) -> Result<Value> {
        self.with_job_files(async |models| {
            let extension = image.extension().and_then(|s| s.to_str()).unwrap_or("image");
            let name = format!("job/input.{extension}");
            std::fs::copy(image, models.workspace.path().join(&name)).context("copy OCR input into model workspace")?;
            // The worker can load weights and then fail on the image.
            models.ocr_requested = true;
            models.call(json!({"op":"ocr","image_path":name})).await.context("recognize Japanese text model")
        }).await
    }

    pub async fn ocr_crop(&mut self, image: &Mat) -> Result<String> {
        self.with_job_files(async |models| {
            let input = models.write_array("job/crop.bin", image)?;
            models.ocr_requested = true;
            let response = models.call(json!({"op":"ocr_crop","image":input})).await
                .context("无法识别选区，请检查本地 Manga OCR 模型")?;
            Ok(response["source"].as_str().context("选区识别结果格式无效")?.trim().to_owned())
        }).await
    }

    pub async fn release_ocr(&mut self)->Result<()>{
        self.prepare_request().await?;
        if self.ocr_requested && self.worker.is_some() {
            if let Err(error) = self.call(json!({"op":"release_ocr"})).await {
                self.stop().await.context("stop worker after OCR release failed")?;
                return Err(error);
            }
        }
        self.ocr_requested = false;
        Ok(())
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.stop().await
    }

    pub async fn stop(&mut self)->Result<()>{
        self.ocr_requested = false;
        if let Some(worker) = self.worker.take() {
            worker.shutdown().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_model_jobs_leave_no_partial_arrays() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let script = directory.path().join("model.py");
        std::fs::write(&script, r#"
import json, pathlib, sys
workspace = pathlib.Path(sys.argv[sys.argv.index('--workspace') + 1])
for line in sys.stdin:
    request = json.loads(line)
    op = request['op']
    reply = {'id': request['id'], 'ok': True, 'result': {'protocol': 1}}
    if op == 'detect':
        (workspace / request['raw_output']).write_bytes(b'partial')
        reply.update(ok=False, error='detection failed')
    elif op == 'restore':
        (workspace / request['output']).write_bytes(b'partial')
        reply['result'] = {'image': {'path': request['output'], 'shape': [1, 1]}}
    print(json.dumps(reply), flush=True)
    if op == 'shutdown':
        break
"#)?;
        let mut models = ImageModels::new(Path::new("python3"), &script, directory.path())?;
        let image = Mat::new_rows_cols_with_default(2, 2, CV_8UC1, Scalar::all(0.0))?;
        assert!(models.detect(&image).await.is_err());
        assert_eq!(models.workspace.path().read_dir()?.count(), 0);
        assert!(models.restore(&image, &image).await.is_err());
        assert_eq!(models.workspace.path().read_dir()?.count(), 0);
        models.shutdown().await
    }
}
