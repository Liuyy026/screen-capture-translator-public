//! Translation transport. Persist endpoint identity only; credentials never
//! enter request logs, page metadata, or server-error messages.
use anyhow::{Result, anyhow, bail, ensure};
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::time::Duration;

const INVALID_URL: &str = "服务地址无效：请填写不含账号、密码、查询参数或片段的 HTTP/HTTPS 地址";

pub fn normalize_base_url(provider: &str, base: Option<&str>) -> Result<String> {
    let default = match provider {
        "ollama" => "http://127.0.0.1:11439",
        "omlx" => "http://127.0.0.1:8000/v1",
        _ => bail!("未知翻译服务"),
    };
    let value = base.unwrap_or(default).trim();
    let invalid = || anyhow!(INVALID_URL);
    if value.chars().any(|c| c.is_whitespace() || (c as u32) < 32) || value.contains(['?', '#']) {
        return Err(invalid());
    }
    let (scheme, rest) = value.split_once("://").ok_or_else(invalid)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(invalid());
    }
    let (authority, path) = rest
        .split_once('/')
        .map(|(a, p)| (a, format!("/{p}")))
        .unwrap_or((rest, String::new()));
    if authority.is_empty() || authority.contains('@') {
        return Err(invalid());
    }
    let (host, port) = if authority.starts_with('[') {
        let end = authority.find(']').ok_or_else(invalid)?;
        let host = &authority[..=end];
        let suffix = &authority[end + 1..];
        if suffix.is_empty() {
            (host, None)
        } else {
            (host, Some(suffix.strip_prefix(':').ok_or_else(invalid)?))
        }
    } else {
        authority
            .split_once(':')
            .map(|(h, p)| (h, Some(p)))
            .unwrap_or((authority, None))
    };
    if host.is_empty() {
        return Err(invalid());
    }
    let port = match port.filter(|p| !p.is_empty()) {
        Some(p) => Some(p.parse::<u16>().map_err(|_| invalid())?),
        None => None,
    };
    let default_port = if scheme == "http" { 80 } else { 443 };
    let netloc = if let Some(port) = port.filter(|p| *p != default_port) {
        format!("{}:{port}", host.to_lowercase())
    } else {
        host.to_lowercase()
    };
    let mut path = path.trim_end_matches('/').to_owned();
    if provider == "omlx" && !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    let normalized = format!("{scheme}://{netloc}{path}");
    let parsed = reqwest::Url::parse(&normalized).map_err(|_| invalid())?;
    if parsed.host_str().is_none() {
        return Err(invalid());
    }
    Ok(normalized)
}

pub struct TranslationService {
    pub provider: String,
    pub base_url: String,
    api_key: String,
    client: Client,
}

impl TranslationService {
    pub fn allows_local_start(&self) -> bool {
        self.provider == "ollama" && self.base_url == "http://127.0.0.1:11439" && self.api_key.is_empty()
    }
    pub fn new(provider: &str, base_url: Option<&str>, key: Option<&str>) -> Result<Self> {
        let base_url = normalize_base_url(provider, base_url)?;
        let api_key = key
            .map(str::to_owned)
            .unwrap_or_else(|| std::env::var("KOMA_TRANSLATION_API_KEY").unwrap_or_default())
            .trim()
            .to_owned();
        ensure!(
            !api_key.chars().any(|c| (c as u32) < 32 || c == '\u{7f}'),
            "API 密钥含有无效字符"
        );
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        if !api_key.is_empty() {
            let mut value = HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|_| anyhow!("API 密钥含有无效字符"))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .build()
            .map_err(|_| anyhow!("无法初始化翻译服务连接"))?;
        Ok(Self {
            provider: provider.into(),
            base_url,
            api_key,
            client,
        })
    }

    pub fn identity(&self) -> Value {
        json!({"provider":self.provider,"base_url":self.base_url})
    }
    pub fn chat_route(&self) -> &'static str {
        if self.provider == "ollama" {
            "/api/chat"
        } else {
            "/chat/completions"
        }
    }
    pub fn safe_error(&self, error: impl std::fmt::Display) -> String {
        let message = error.to_string();
        if self.api_key.is_empty() {
            message
        } else {
            message.replace(&self.api_key, "[已隐藏]")
        }
    }

    pub async fn request(
        &self,
        route: &str,
        payload: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value> {
        let url = format!("{}{route}", self.base_url);
        let request = if let Some(payload) = payload {
            self.client.post(url).json(payload)
        } else {
            self.client.get(url)
        };
        let network = || anyhow!("无法连接翻译服务或服务未返回有效 JSON，请检查地址和服务状态");
        let mut response = request
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| network())?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            bail!("翻译服务拒绝访问，请检查 API 密钥及访问权限");
        }
        if !(200..300).contains(&status) {
            bail!("翻译服务请求失败（HTTP {status}）");
        }
        // Bound unexpected server output without ever including it in errors.
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(|_| network())? {
            if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                return Err(network());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| network())?;
        ensure!(value.is_object(), "翻译服务返回了无效响应对象");
        ensure!(
            value.get("error").is_none(),
            "翻译服务返回错误，请检查所选模型及服务状态"
        );
        Ok(value)
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let route = if self.provider == "ollama" {
            "/api/tags"
        } else {
            "/models"
        };
        self.models_from_response(&self.request(route, None, Duration::from_secs(5)).await?)
    }

    pub fn models_from_response(&self, response: &Value) -> Result<Vec<String>> {
        let key = if self.provider == "ollama" {
            "models"
        } else {
            "data"
        };
        let rows = response[key]
            .as_array()
            .ok_or_else(|| anyhow!("翻译服务未返回有效模型列表"))?;
        let mut models = vec![];
        for row in rows {
            ensure!(row.is_object(), "翻译服务返回的模型列表格式无效");
            let key = if self.provider == "ollama" {
                "name"
            } else {
                "id"
            };
            let model = row[key]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| anyhow!("翻译服务返回的模型编号无效"))?;
            if self.provider == "ollama"
                && (crate::compat::truth(&row["remote_host"])
                    || crate::compat::truth(&row["remote_model"])
                    || model.to_lowercase().contains("cloud"))
            {
                continue;
            }
            if !models.iter().any(|m| m == model) {
                models.push(model.into());
            }
        }
        Ok(models)
    }

    pub fn translation_payload(&self, model: &str, messages: Value, schema: Value) -> Value {
        if self.provider == "ollama" {
            json!({"model":model,"messages":messages,"stream":false,"think":false,"keep_alive":"5m","format":schema,
                "options":{"temperature":0.15,"num_ctx":4096,"num_predict":2048}})
        } else {
            json!({"model":model,"messages":messages,"stream":false,"temperature":0.15,"max_tokens":2048,
                "response_format":{"type":"json_schema","json_schema":{"name":"translation","schema":schema,"strict":true}},
                "chat_template_kwargs":{"enable_thinking":false}})
        }
    }

    pub fn translation_content<'a>(&self, response: &'a Value) -> Result<&'a str> {
        ensure!(
            response.is_object() && response.get("error").is_none(),
            "翻译服务返回了无效翻译响应"
        );
        let message = if self.provider == "ollama" {
            ensure!(response["done_reason"] != "length", "模型输出被截断");
            &response["message"]
        } else {
            let choice = response["choices"]
                .as_array()
                .and_then(|a| a.first())
                .filter(|v| v.is_object())
                .ok_or_else(|| anyhow!("翻译服务未返回有效译文内容"))?;
            ensure!(choice["finish_reason"] != "length", "模型输出被截断");
            &choice["message"]
        };
        let content = message
            .get("content")
            .ok_or_else(|| anyhow!("翻译服务未返回有效译文内容"))?;
        content
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow!("模型返回了空译文"))
    }
}

pub async fn probe(request: &Value) -> Result<Value> {
    let provider = request["provider"].as_str().unwrap_or("ollama");
    let base = request["base_url"].as_str();
    if request["op"] == "normalize" {
        return Ok(json!(normalize_base_url(provider, base)?));
    }
    let service = TranslationService::new(provider, base, request["key"].as_str())?;
    match request["op"].as_str().unwrap_or("") {
        "identity" => Ok(service.identity()),
        "models" => Ok(json!(service.models_from_response(&request["response"])?)),
        "content" => Ok(json!(service.translation_content(&request["response"])?)),
        "safe_error" => Ok(json!(
            service.safe_error(request["message"].as_str().unwrap_or(""))
        )),
        "payload" => Ok(service.translation_payload(
            request["model"].as_str().unwrap_or("model"),
            request["messages"].clone(),
            request["schema"].clone(),
        )),
        "vision_payload" => Ok(crate::translation::vision_payload(
            &service,
            request["model"].as_str().unwrap_or("model"),
            &request["page"],
            crate::compat::array(&request["batch"]),
            request["image_url"]
                .as_str()
                .unwrap_or("data:image/jpeg;base64,test"),
        )),
        "block_payload" => {
            let ids: Vec<String> = serde_json::from_value(request["ids"].clone())?;
            Ok(crate::translation::block_payload(
                &service,
                request["model"].as_str().unwrap_or("model"),
                request["context"].clone(),
                &ids,
                crate::compat::truth(&request["dialogue"]),
            ))
        }
        "request" => {
            service
                .request(
                    request["route"].as_str().unwrap_or("/models"),
                    request.get("payload"),
                    Duration::from_secs_f64(request["timeout"].as_f64().unwrap_or(300.0)),
                )
                .await
        }
        "list_models" => Ok(json!(service.list_models().await?)),
        _ => bail!("unknown service probe"),
    }
}
