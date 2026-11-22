//! Track only models named in a chat request made by this daemon. A server's
//! model list also includes models installed or loaded by other clients.
use crate::service::TranslationService;
use anyhow::{Context, Result};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::Mutex, task::JoinSet};

#[derive(Clone, PartialEq, Eq, Hash)]
struct ModelKey {
    provider: String,
    base_url: String,
    model: String,
}

impl ModelKey {
    fn new(service: &TranslationService, model: &str) -> Self {
        Self {
            provider: service.provider.clone(),
            base_url: service.base_url.clone(),
            model: model.to_owned(),
        }
    }
}

#[derive(Default)]
pub struct ModelSession {
    used: Mutex<HashMap<ModelKey, TranslationService>>,
}

impl TranslationService {
    pub async fn chat(&self, payload: &Value) -> Result<Value> {
        let model = payload["model"].as_str().context("翻译请求缺少模型名称")?;
        if let Some(session) = self.session.as_ref().and_then(|session| session.upgrade()) {
            // The server may finish loading even when cancellation drops the
            // HTTP future before its first response. Remember before sending.
            session
                .used
                .lock()
                .await
                .insert(ModelKey::new(self, model), self.clone());
        }
        self.request(self.chat_route(), Some(payload), Duration::from_secs(300))
            .await
    }
}

impl ModelSession {
    pub fn track(self: &Arc<Self>, mut service: TranslationService) -> TranslationService {
        service.session = Some(Arc::downgrade(self));
        service
    }

    /// The daemon queues this operation alongside process jobs. A queued
    /// release cannot interrupt inference or race a subsequent use of the model.
    pub async fn unload(
        &self,
        requested: &TranslationService,
        model: &str,
        explicit: bool,
        key_provided: bool,
    ) -> Result<bool> {
        let key = ModelKey::new(requested, model);
        let mut used = self.used.lock().await;
        let service = match used.get(&key) {
            Some(original) if !(explicit && key_provided) => original,
            _ if explicit => requested,
            _ => return Ok(false),
        };
        service.unload_model(model).await?;
        used.remove(&key);
        Ok(true)
    }

    /// Called after all jobs have joined. Each endpoint has its own timeout,
    /// so one offline endpoint doesn't delay releasing every other model.
    pub async fn close(&self) -> Option<String> {
        let mut releases = JoinSet::new();
        let mut used = self.used.lock().await;
        for (key, service) in used.iter() {
            let (key, service) = (key.clone(), service.clone());
            releases.spawn(async move {
                let mut result = service.unload_model(&key.model).await;
                if result.is_err() {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    result = service.unload_model(&key.model).await;
                }
                (key, result.map_err(|error| service.safe_error(error)))
            });
        }
        let mut errors = Vec::new();
        while let Some(result) = releases.join_next().await {
            match result {
                Ok((key, Ok(()))) => {
                    used.remove(&key);
                }
                Ok((_, Err(error))) => errors.push(error),
                Err(_) => errors.push("模型卸载任务意外结束".to_owned()),
            }
        }
        errors
            .first()
            .map(|reason| format!("有 {} 个翻译模型未能确认卸载。{reason}", errors.len()))
    }
}
