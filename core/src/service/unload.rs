use super::{INVALID_URL, TranslationService, check_status, network_error, read_object};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::time::Duration;

#[cfg(test)]
#[path = "unload_tests.rs"]
mod tests;

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_INTERVAL: Duration = Duration::from_millis(200);

impl TranslationService {
    pub async fn unload_model(&self, model: &str) -> Result<()> {
        ensure!(!model.trim().is_empty(), "缺少要卸载的模型名称");
        tokio::time::timeout(RELEASE_TIMEOUT, async {
            if self.provider == "ollama" {
                self.unload_ollama(model).await
            } else {
                self.unload_omlx(model).await
            }
        })
        .await
        .map_err(|_| anyhow!("等待模型卸载超时，请稍后重试"))?
    }

    async fn unload_ollama(&self, model: &str) -> Result<()> {
        let payload = json!({"model":model, "keep_alive":0, "stream":false});
        let ack = self
            .request("/api/generate", Some(&payload), HTTP_TIMEOUT)
            .await?;
        ensure!(
            ack["done"] == true && ack["done_reason"] == "unload",
            "Ollama 未确认卸载请求"
        );
        // Ollama acknowledges as soon as the runner is scheduled for expiry.
        let target = ollama_name(model);
        loop {
            let status = self.request("/api/ps", None, HTTP_TIMEOUT).await?;
            let rows = status["models"]
                .as_array()
                .ok_or_else(|| anyhow!("Ollama 未返回运行模型列表"))?;
            let mut loaded = false;
            for row in rows {
                let name = row["name"]
                    .as_str()
                    .or_else(|| row["model"].as_str())
                    .ok_or_else(|| anyhow!("Ollama 返回的运行模型名称无效"))?;
                loaded |= ollama_name(name) == target;
            }
            if !loaded {
                return Ok(());
            }
            tokio::time::sleep(STATUS_INTERVAL).await;
        }
    }

    async fn unload_omlx(&self, model: &str) -> Result<()> {
        let mut status = self.request("/models/status", None, HTTP_TIMEOUT).await?;
        let rows = omlx_rows(&status)?;
        // Public inference accepts aliases and profiles. The unload route in
        // oMLX 0.7.3 takes the underlying engine's physical ID exclusively.
        let id = if let Some(id) = omlx_engine_id(rows, model) {
            id.to_owned()
        } else {
            // Claude tier aliases expose their physical ID only in /v1/models.
            let catalog = self.request("/models", None, HTTP_TIMEOUT).await?;
            catalog["data"]
                .as_array()
                .and_then(|models| {
                    models
                        .iter()
                        .find(|row| row["id"] == model && row["anthropic_family_tier"].is_string())
                })
                .and_then(|row| row["display_name"].as_str())
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("oMLX 未提供该模型对应的运行标识"))?
        };
        ensure!(
            !id.is_empty() && id != "." && id != "..",
            "oMLX 返回的模型标识无效"
        );
        let mut url = reqwest::Url::parse(&self.base_url).map_err(|_| anyhow!(INVALID_URL))?;
        url.path_segments_mut()
            .map_err(|_| anyhow!(INVALID_URL))?
            .extend(["models", &id, "unload"]);
        let mut queued = false;
        let mut first_status = true;
        loop {
            let rows = omlx_rows(&status)?;
            let row = rows
                .iter()
                .find(|row| row["id"] == id)
                .ok_or_else(|| anyhow!("oMLX 运行状态中找不到目标模型"))?;
            let loaded = row["loaded"]
                .as_bool()
                .ok_or_else(|| anyhow!("oMLX 未返回模型加载状态"))?;
            let loading = row["is_loading"]
                .as_bool()
                .ok_or_else(|| anyhow!("oMLX 未返回模型加载状态"))?;
            // An initially absent target released nothing in this operation.
            // Any outstanding global charge could belong to another engine.
            if !loading && !loaded && (first_status || !omlx_memory_pending(&status, rows)) {
                return Ok(());
            }
            first_status = false;
            if loaded && !loading && !queued {
                let response = match self
                    .client
                    .post(url.clone())
                    .timeout(HTTP_TIMEOUT)
                    .send()
                    .await
                {
                    Ok(response) => response,
                    Err(error) if error.is_timeout() && !error.is_connect() => {
                        // An accepted idle-model POST may withhold its 200 until
                        // Metal finishes cleanup. Confirm its outcome by reading
                        // status without issuing another destructive request.
                        queued = true;
                        status = self.request("/models/status", None, HTTP_TIMEOUT).await?;
                        continue;
                    }
                    Err(_) => return Err(network_error()),
                };
                let code = response.status().as_u16();
                match code {
                    200 | 202 => {
                        let ack = read_object(response).await?;
                        ensure!(ack["model_id"] == id, "oMLX 确认的卸载模型与请求不符");
                        if code == 200 && ack["status"] == "ok" {
                            return Ok(());
                        }
                        ensure!(
                            code == 202 && ack["status"] == "unloading",
                            "oMLX 未确认卸载请求"
                        );
                        queued = true;
                    }
                    400 | 409 => {
                        let body = read_object(response).await?;
                        let expected = if code == 400 {
                            "Model not loaded"
                        } else {
                            "Model still loading"
                        };
                        let detail = body["error"]["message"]
                            .as_str()
                            .or_else(|| body["detail"].as_str());
                        if detail != Some(format!("{expected}: {id}").as_str()) {
                            check_status(code)?;
                        }
                        // The engine changed between our status check and POST.
                    }
                    _ => {
                        check_status(code)?;
                        bail!("oMLX 未确认卸载请求");
                    }
                }
            }
            tokio::time::sleep(STATUS_INTERVAL).await;
            status = self.request("/models/status", None, HTTP_TIMEOUT).await?;
        }
    }
}

fn ollama_name(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    let name = name.strip_prefix("registry.ollama.ai/").unwrap_or(&name);
    let name = name.strip_prefix("library/").unwrap_or(name);
    if name
        .rsplit('/')
        .next()
        .is_some_and(|part| part.contains(':'))
    {
        name.to_owned()
    } else {
        format!("{name}:latest")
    }
}

fn omlx_rows(status: &Value) -> Result<&Vec<Value>> {
    status["models"]
        .as_array()
        .ok_or_else(|| anyhow!("oMLX 未返回运行模型列表"))
}

// oMLX sets loaded=false before clearing Metal buffers. Its admission budget
// drops only after that wait. Profiles mirror physical entries, and built-in
// tools share no model weight budget. Other models can stay resident.
fn omlx_memory_pending(status: &Value, rows: &[Value]) -> bool {
    let Some(total) = status["current_model_memory"].as_u64() else {
        return false;
    };
    let remaining = rows
        .iter()
        .filter(|row| {
            row["loaded"] == true
                && row["source_model_id"].is_null()
                && row["source_type"] != "builtin"
        })
        .try_fold(0_u64, |sum, row| {
            sum.checked_add(row["resident_estimated_size"].as_u64()?)
        });
    remaining.is_some_and(|resident| total > resident)
}

fn omlx_engine_id<'a>(rows: &'a [Value], model: &str) -> Option<&'a str> {
    let lookup = |name: &str| {
        rows.iter()
            .find(|row| row["id"] == name)
            .or_else(|| {
                rows.iter().find(|row| {
                    row["id"]
                        .as_str()
                        .is_some_and(|id| id.eq_ignore_ascii_case(name))
                })
            })
            .or_else(|| rows.iter().find(|row| row["model_alias"] == name))
            .and_then(|row| {
                row["source_model_id"]
                    .as_str()
                    .or_else(|| row["id"].as_str())
            })
    };
    lookup(model).or_else(|| model.split_once('/').and_then(|(_, bare)| lookup(bare)))
}
