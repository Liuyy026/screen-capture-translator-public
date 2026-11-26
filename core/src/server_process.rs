//! Chapter jobs deliver the first readable page, then separate OCR and translation
//! in bounded groups. An OCR checkpoint is never a completed-page receipt.
use super::{
    Events, Job, PageVersion, ProcessOptions, Runtime, TranslationService, cleanup_ocr,
    previous_for_page, required, service,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
use tokio::sync::mpsc;

// The two measured books needed roughly 2.5–3 seconds of OCR per page.
// Twelve keeps each OCR-only stretch short and amortizes weight loading.
const CHAPTER_GROUP: usize = 12;

struct Queue<'a> {
    runtime: &'a Runtime,
    job: &'a Job,
    output: &'a mpsc::Sender<Value>,
    images: Vec<&'a str>,
    operation: &'a str,
    chapter: bool,
    settings: crate::super_resolution::Settings,
    processing: Option<(ProcessOptions<'a>, TranslationService)>,
    image_cache: crate::vision_image::EncodingCache,
}

impl Queue<'_> {
    async fn event(&self, mut event: Value) {
        event["id"] = json!(self.job.id);
        let _ = self.output.send(event).await;
    }

    async fn start(&self, index: usize) {
        self.event(json!({"event":"page_started","image":self.images[index],"index":index,"total":self.images.len()})).await;
    }

    async fn attempt(&self, index: usize, stage: &str, ocr_prepass: bool) -> Result<Value> {
        let _activity = self.runtime.cache_policy.activity().await?;
        let image = self.images[index];
        let source = Path::new(image);
        let Some((options, service)) = &self.processing else {
            ensure!(self.settings.engine != "off", "请选择超分引擎");
            return Ok(
                json!({"skipped":self.runtime.sr.prepare(source, &self.settings).await? == source}),
            );
        };
        let cache = self.runtime.cache_for(source, options.mode)?;
        // A filename check is sufficient here: the later both pass reopens
        // and validates the current source and cache under the page lock. It
        // also completes partial manual OCR and recovers a disappeared cache.
        if ocr_prepass && cache.symlink_metadata().is_ok_and(|m| m.is_file()) {
            return Ok(Value::Null);
        }
        let mut events = Events {
            id: self.job.id.clone(),
            image: image.into(),
            cache: cache.clone(),
            output: self.output.clone(),
        };
        let mut options = *options;
        options.operation = stage;
        options = options.for_queue_page(index, self.images.len(), &self.settings.engine);
        if options.mode == "vision" && matches!(stage, "both" | "translate") {
            options.previous_image =
                previous_for_page(&self.job.request, image, self.images.len())?;
        }
        self.runtime
            .process_page_with_images(
                source,
                &self.settings,
                &cache,
                &options,
                service,
                &mut events,
                &self.image_cache,
            )
            .await
    }

    fn error_message(&self, error: &anyhow::Error) -> String {
        match &self.processing {
            Some((_, service)) => service.safe_error(error),
            None => error.to_string(),
        }
    }

    async fn finish(&self, index: usize, stage: &str, mut outcome: Result<Value>) -> Value {
        let image = self.images[index];
        let mut first_error = None;
        if self.chapter && matches!(stage, "both" | "translate") {
            first_error = match &outcome {
                Err(e) if e.is::<crate::service::TransientServiceError>() => {
                    Some(self.error_message(e))
                }
                Ok(v) if v["retryable_issues"].as_u64().unwrap_or(0) > 0 => {
                    Some(format!("{} 处译文可重试", v["retryable_issues"]))
                }
                _ => None,
            };
        }
        if let Some(reason) = &first_error {
            self.event(json!({"event":"page_retrying","index":index,"total":self.images.len(),"image":image,
                "attempt":2,"reason":reason})).await;
            // The outer job's cancellation select also interrupts this delay.
            tokio::time::sleep(Duration::from_millis(600)).await;
            // Both-mode translation has already committed OCR. The ordinary
            // translation plan respects the latest manual edits and skips
            // successful boxes tagged with the current configuration.
            let retry = self.attempt(index, stage, false).await;
            // A transport-level retry can fail before it sees the earlier block
            // errors. Their originals still remain in the checkpoint on disk.
            outcome = retry;
        }
        if stage == "both" && outcome.is_err() {
            cleanup_ocr(self.runtime).await;
        }
        let mut page = match outcome {
            Ok(result) => json!({"image":image,"ok":true,"result":result}),
            Err(error) => {
                let mut page = json!({"image":image,"ok":false,"error":self.error_message(&error)});
                if error.is::<crate::runtime::PageRevisionChanged>() {
                    page["code"] = json!("page_revision_changed");
                }
                page
            }
        };
        if let Some(reason) = first_error {
            page["retried"] = json!(true);
            page["first_error"] = json!(reason);
        }
        self.event(
            json!({"event":"page_finished","done":index+1,"total":self.images.len(),"page":page}),
        )
        .await;
        self.runtime.maintain_cache().await;
        page
    }

    async fn process(&self) -> Result<Value> {
        let mut results = Vec::with_capacity(self.images.len());
        // Enhancement opens source images and prepares its own workspaces.
        // Keep enhanced pages in one pass to avoid doubling that preparation.
        let grouped = self.chapter && self.operation == "both" && self.settings.engine == "off";
        for i in 0..self.images.len() {
            self.start(i).await;
            let page = self
                .finish(
                    i,
                    self.operation,
                    self.attempt(i, self.operation, false).await,
                )
                .await;
            let readable = page["result"]["translated_blocks"].as_u64().unwrap_or(0) > 0;
            results.push(page);
            if grouped && readable {
                break;
            }
        }
        while results.len() < self.images.len() {
            let start = results.len();
            let limit = (start + CHAPTER_GROUP).min(self.images.len());
            let mut prepared = Vec::with_capacity(limit - start);
            self.event(
                json!({"event":"chapter_phase","phase":"ocr","index":start,"count":limit-start}),
            )
            .await;
            for index in start..limit {
                self.start(index).await;
                prepared.push(self.attempt(index, "ocr", true).await);
                if !crate::runtime::ocr_residency::normal_memory_pressure() {
                    break;
                }
            }
            let end = start + prepared.len();
            // The last OCR in a run must give back its weights before the
            // first translation, even if every page was cached or failed.
            cleanup_ocr(self.runtime).await;
            self.event(json!({"event":"chapter_phase","phase":"translate","index":start,"count":end-start})).await;
            for (index, ocr) in (start..end).zip(prepared) {
                let (stage, outcome) = match ocr {
                    Ok(_) => ("both", self.attempt(index, "both", false).await),
                    Err(error) => ("ocr", Err(error)),
                };
                results.push(self.finish(index, stage, outcome).await);
            }
        }
        Ok(json!({"pages":results}))
    }
}

pub(super) async fn execute(
    runtime: &Runtime,
    job: &Job,
    output: &mpsc::Sender<Value>,
) -> Result<Value> {
    let r = &job.request;
    let values = r["images"].as_array().context("missing images")?;
    ensure!(
        !values.is_empty() && values.len() <= 4096,
        "invalid page queue size"
    );
    let images = values
        .iter()
        .map(|v| v.as_str().context("invalid image path"))
        .collect::<Result<Vec<_>>>()?;
    let operation = r["operation"].as_str().unwrap_or("both");
    let block_id = r["block_id"].as_str();
    let retry = r["retry"].as_bool().unwrap_or(false);
    ensure!(
        r["block_id"].is_null() || block_id.is_some_and(|id| !id.is_empty()),
        "无效的单句对白编号"
    );
    ensure!(
        r["retry"].is_null() || r["retry"].is_boolean(),
        "无效的单句重试选项"
    );
    ensure!(
        r["chapter"].is_null() || r["chapter"].is_boolean(),
        "无效的整章处理选项"
    );
    if block_id.is_some() || retry {
        ensure!(
            images.len() == 1 && matches!(operation, "translate" | "both"),
            "单句重试需要指定一页漫画并启用翻译"
        );
        ensure!(block_id.is_some(), "单句重试缺少对白编号");
    }
    let chapter = r["chapter"] == true && images.len() > 1;
    let settings = serde_json::from_value(r.get("super_resolution").cloned().unwrap_or(json!({})))?;
    let processing = if operation == "enhance" {
        None
    } else {
        let options = ProcessOptions {
            operation,
            mode: r["mode"].as_str().unwrap_or("text"),
            model: required(r, "model")?,
            block_id,
            previous_image: None,
            reuse_ocr: false,
            expected_page: r["cache"].as_str().zip(r["revision"].as_str()).map(
                |(cache, revision)| PageVersion {
                    cache: Path::new(cache),
                    revision,
                },
            ),
            unit: r["unit"].as_str().unwrap_or("blocks"),
            batch_size: r["batch_size"].as_u64().unwrap_or(8) as usize,
            retry,
        };
        Some((options, runtime.translation_models.track(service(r)?)))
    };
    Queue {
        runtime,
        job,
        output,
        images,
        operation,
        chapter,
        settings,
        processing,
        image_cache: Default::default(),
    }
    .process()
    .await
}
