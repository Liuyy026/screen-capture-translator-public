"""Small, dependency-free clients for the supported local translation servers.

Only endpoint identity is suitable for persistence. Credentials stay in memory,
and server response bodies are never included in errors.
"""
import json
import http.client
import os
import urllib.error
import urllib.parse
import urllib.request


DEFAULT_URLS = {"ollama": "http://127.0.0.1:11439", "omlx": "http://127.0.0.1:8000/v1"}
API_KEY_ENV = "KOMA_TRANSLATION_API_KEY"


class TranslationServiceError(ValueError):
    """An error safe to display or persist without request/response contents."""


def normalize_base_url(provider, base_url=None):
    if provider not in DEFAULT_URLS:
        raise TranslationServiceError("未知翻译服务")
    value = DEFAULT_URLS[provider] if base_url is None else str(base_url).strip()
    try:
        parts = urllib.parse.urlsplit(value)
        # Accessing .port also validates malformed or out-of-range ports.
        port = parts.port
        if (parts.scheme.lower() not in {"http", "https"} or not parts.hostname
                or parts.username is not None or parts.password is not None
                or "?" in value or "#" in value
                or any(c.isspace() or ord(c) < 32 for c in value)):
            raise ValueError
        host = parts.hostname.lower()
        if ":" in host:
            host = "[" + host + "]"
        scheme = parts.scheme.lower()
        default_port = 80 if scheme == "http" else 443
        netloc = host if port is None or port == default_port else f"{host}:{port}"
        path = parts.path.rstrip("/")
        if provider == "omlx" and not path.endswith("/v1"):
            path += "/v1"
        return urllib.parse.urlunsplit((scheme, netloc, path, "", ""))
    except (ValueError, TypeError):
        raise TranslationServiceError("服务地址无效：请填写不含账号、密码、查询参数或片段的 HTTP/HTTPS 地址") from None


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Do not forward an Authorization header to another server.
        return None


class TranslationService:
    def __init__(self, provider="ollama", base_url=None, requester=None):
        self.provider = provider
        self.base_url = normalize_base_url(provider, base_url)
        self._api_key = os.environ.get(API_KEY_ENV, "").strip()
        if any(ord(c) < 32 or ord(c) == 127 for c in self._api_key):
            raise TranslationServiceError("API 密钥含有无效字符")
        self._requester = requester

    @property
    def identity(self):
        return {"provider": self.provider, "base_url": self.base_url}

    @property
    def chat_route(self):
        return "/api/chat" if self.provider == "ollama" else "/chat/completions"

    def request(self, route, payload=None, timeout=300):
        if self._requester is not None:
            return self._requester(route, payload, timeout=timeout)
        data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode("utf-8")
        headers = {"Content-Type": "application/json", "Accept": "application/json"}
        if self._api_key:
            headers["Authorization"] = "Bearer " + self._api_key
        req = urllib.request.Request(self.base_url + route, data=data, headers=headers)
        try:
            # Explicitly bypass environment proxies for local/LAN service traffic.
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())
            with opener.open(req, timeout=timeout) as response:
                result = json.load(response)
        except urllib.error.HTTPError as exc:
            exc.close()
            if exc.code in {401, 403}:
                raise TranslationServiceError("翻译服务拒绝访问，请检查 API 密钥及访问权限") from None
            raise TranslationServiceError(f"翻译服务请求失败（HTTP {exc.code}）") from None
        except (urllib.error.URLError, http.client.HTTPException, OSError, TimeoutError, ValueError):
            raise TranslationServiceError("无法连接翻译服务或服务未返回有效 JSON，请检查地址和服务状态") from None
        if not isinstance(result, dict):
            raise TranslationServiceError("翻译服务返回了无效响应对象")
        if "error" in result:
            raise TranslationServiceError("翻译服务返回错误，请检查所选模型及服务状态")
        return result

    def list_models(self):
        """Read model metadata only; never send a generation/load request."""
        route = "/api/tags" if self.provider == "ollama" else "/models"
        response = self.request(route, timeout=5)
        rows = response.get("models" if self.provider == "ollama" else "data")
        if not isinstance(rows, list):
            raise TranslationServiceError("翻译服务未返回有效模型列表")
        models = []
        for row in rows:
            if not isinstance(row, dict):
                raise TranslationServiceError("翻译服务返回的模型列表格式无效")
            key = "name" if self.provider == "ollama" else "id"
            model = row.get(key)
            if not isinstance(model, str) or not model.strip():
                raise TranslationServiceError("翻译服务返回的模型编号无效")
            if self.provider == "ollama" and (row.get("remote_host") or row.get("remote_model") or "cloud" in model.lower()):
                continue
            if model not in models:
                models.append(model)
        return models

    def translation_payload(self, model, messages, schema):
        common = {"model": model, "messages": messages, "stream": False}
        if self.provider == "ollama":
            return {**common, "think": False, "keep_alive": "5m", "format": schema,
                    "options": {"temperature": 0.15, "num_ctx": 4096, "num_predict": 2048}}
        return {**common, "temperature": 0.15, "max_tokens": 2048,
                "response_format": {"type": "json_schema", "json_schema": {
                    "name": "translation", "schema": schema, "strict": True}},
                "chat_template_kwargs": {"enable_thinking": False}}

    def translation_content(self, response):
        if not isinstance(response, dict) or "error" in response:
            raise TranslationServiceError("翻译服务返回了无效翻译响应")
        try:
            if self.provider == "ollama":
                if response.get("done_reason") == "length":
                    raise TranslationServiceError("模型输出被截断")
                content = response["message"]["content"]
            else:
                choices = response["choices"]
                if not isinstance(choices, list) or not choices or not isinstance(choices[0], dict):
                    raise KeyError
                if choices[0].get("finish_reason") == "length":
                    raise TranslationServiceError("模型输出被截断")
                content = choices[0]["message"]["content"]
        except (KeyError, IndexError, TypeError):
            raise TranslationServiceError("翻译服务未返回有效译文内容") from None
        if not isinstance(content, str) or not content.strip():
            raise TranslationServiceError("模型返回了空译文")
        return content

    def safe_error(self, exc):
        """Redact credentials even if a caller supplies an unexpected exception."""
        message = str(exc)
        return message.replace(self._api_key, "[已隐藏]") if self._api_key else message
