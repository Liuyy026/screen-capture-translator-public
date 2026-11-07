import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from backend.translation_service import (
    API_KEY_ENV, DEFAULT_URLS, TranslationService, TranslationServiceError, normalize_base_url,
)

spec = importlib.util.spec_from_file_location("service_test_worker", ROOT / "backend/worker.py")
w = importlib.util.module_from_spec(spec)
spec.loader.exec_module(w)


class FakeService:
    """Actual HTTP transport with no dependency on installed model services."""
    def __init__(self):
        self.calls = []
        self.response = None
        self.translation = "早上好"
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.answer()

            def do_POST(self):
                self.answer()

            def answer(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
                payload = json.loads(raw) if raw else None
                owner.calls.append({"method": self.command, "path": self.path,
                                    "authorization": self.headers.get("Authorization"), "payload": payload})
                if owner.response is not None:
                    status, response, headers = owner.response
                elif self.path == "/api/tags":
                    status, response, headers = 200, {"models": [
                        {"name": "same-model"}, {"name": "remote", "remote_host": "elsewhere"},
                        {"name": "test-cloud"}]}, {}
                elif self.path.endswith("/models"):
                    status, response, headers = 200, {"data": [{"id": "same-model"}]}, {}
                else:
                    schema = (payload.get("format") or payload["response_format"]["json_schema"]["schema"])
                    ids = schema["properties"]["translations"]["required"]
                    content = json.dumps({"translations": {key: owner.translation for key in ids}})
                    if self.path == "/api/chat":
                        response = {"message": {"content": content}, "done_reason": "stop"}
                    else:
                        response = {"choices": [{"message": {"content": content}, "finish_reason": "stop"}]}
                    status, headers = 200, {}
                data = response.encode() if isinstance(response, str) else json.dumps(response).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                for key, value in headers.items():
                    self.send_header(key, value)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = "http://127.0.0.1:" + str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class TranslationServiceTests(unittest.TestCase):
    def setUp(self):
        self.env_patch = patch.dict(os.environ, {API_KEY_ENV: ""})
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)

    def test_url_normalization_and_validation(self):
        self.assertEqual(normalize_base_url("omlx"), DEFAULT_URLS["omlx"])
        self.assertEqual(normalize_base_url("ollama"), DEFAULT_URLS["ollama"])
        for value in ["http://LOCALHOST:80", "http://localhost/v1", "http://localhost/v1/"]:
            self.assertEqual(normalize_base_url("omlx", value), "http://localhost/v1")
        self.assertEqual(normalize_base_url("omlx", "https://example.com/prefix/"), "https://example.com/prefix/v1")
        self.assertEqual(normalize_base_url("ollama", "http://[::1]:11439/"), "http://[::1]:11439")
        for value in ["", "ftp://localhost", "localhost:8000", "http://u:secret@host", "http://host?key=secret",
                      "http://host#secret", "http://host?", "http://host#", "http://host:99999", "http://ho st"]:
            with self.subTest(value=value), self.assertRaises(TranslationServiceError) as error:
                normalize_base_url("omlx", value)
            self.assertNotIn("secret", str(error.exception))

    def test_omlx_authenticated_model_discovery_uses_get_only(self):
        with FakeService() as server, patch.dict(os.environ, {API_KEY_ENV: "fake-secret"}):
            service = TranslationService("omlx", server.url + "/v1/")
            self.assertEqual(service.list_models(), ["same-model"])
            self.assertEqual(service.identity, {"provider": "omlx", "base_url": server.url + "/v1"})
            self.assertEqual(len(server.calls), 1)
            self.assertEqual(server.calls[0], {"method": "GET", "path": "/v1/models",
                                             "authorization": "Bearer fake-secret", "payload": None})

    def test_ollama_discovery_filters_remote_models_and_omits_empty_auth(self):
        with FakeService() as server:
            self.assertEqual(TranslationService("ollama", server.url).list_models(), ["same-model"])
            self.assertEqual(server.calls[0]["path"], "/api/tags")
            self.assertIsNone(server.calls[0]["authorization"])

    def test_errors_never_echo_response_body_or_credentials(self):
        with FakeService() as server, patch.dict(os.environ, {API_KEY_ENV: "fake-secret"}):
            service = TranslationService("omlx", server.url)
            for status, body in [(401, {"error": "fake-secret private source"}),
                                 (403, "fake-secret"), (500, "private source"),
                                 (200, "bad json fake-secret private source"),
                                 (200, {"error": "private source fake-secret"}),
                                 (200, []), (200, {"data": "fake-secret"}),
                                 (200, {"data": [{"wrong": "fake-secret"}]})]:
                server.response = (status, body, {})
                with self.subTest(status=status, body=body), self.assertRaises(TranslationServiceError) as error:
                    service.list_models()
                self.assertNotIn("fake-secret", str(error.exception))
                self.assertNotIn("private source", str(error.exception))
            self.assertNotIn("fake-secret", service.safe_error(ValueError("unexpected fake-secret")))

    def test_redirect_is_rejected_without_forwarding_auth(self):
        with FakeService() as server, FakeService() as destination, patch.dict(os.environ, {API_KEY_ENV: "fake-secret"}):
            server.response = (302, {}, {"Location": destination.url + "/stolen"})
            with self.assertRaises(TranslationServiceError):
                TranslationService("omlx", server.url).list_models()
            self.assertEqual(len(server.calls), 1)
            self.assertEqual(destination.calls, [])

    def test_completion_shapes_and_truncation_are_validated(self):
        for provider in ["ollama", "omlx"]:
            service = TranslationService(provider)
            bad = [{}, {"error": "private source fake-secret"}, {"choices": []},
                   {"choices": [{"message": {"content": None}}]},
                   {"message": {"content": None}},
                   {"message": {"content": "{}"}, "done_reason": "length"} if provider == "ollama"
                   else {"choices": [{"message": {"content": "{}"}, "finish_reason": "length"}]}]
            for response in bad:
                with self.subTest(provider=provider, response=response), self.assertRaises(TranslationServiceError):
                    service.translation_content(response)

    def test_cli_discovery_needs_no_image_or_status_and_emits_one_reply(self):
        with FakeService() as server:
            for provider, suffix in [("omlx", "/v1"), ("ollama", "")]:
                result = subprocess.run([sys.executable, str(ROOT / "backend/worker.py"), "--list-models",
                    "--provider", provider, "--base-url", server.url + suffix],
                    cwd=tempfile.gettempdir(), capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {"ok": True, "models": ["same-model"]})
                self.assertEqual(len(result.stdout.splitlines()), 1)
            self.assertTrue(all(call["method"] == "GET" for call in server.calls))
            server.response = (401, {"error": "fake-secret"}, {})
            result = subprocess.run([sys.executable, str(ROOT / "backend/worker.py"), "--list-models",
                "--provider", "omlx", "--base-url", server.url],
                capture_output=True, text=True, env={**os.environ, API_KEY_ENV: "fake-secret"})
            self.assertEqual(result.returncode, 1)
            self.assertFalse(json.loads(result.stdout)["ok"])
            self.assertNotIn("fake-secret", result.stdout + result.stderr)


class WorkerProviderTests(unittest.TestCase):
    def setUp(self):
        self.env_patch = patch.dict(os.environ, {API_KEY_ENV: "fake-secret"})
        self.env_patch.start()
        self.addCleanup(self.env_patch.stop)

    def page(self):
        return {"model": "same-model", "blocks": [
            {"id": "1", "source": "おはよう", "translation": "", "edited": False, "error": ""},
            {"id": "2", "source": "こんにちは", "translation": "手动译文", "edited": True, "error": ""}]}

    def translate(self, page, server, provider="omlx", **kwargs):
        w.translate(page, "same-model", lambda _: None, lambda _: None,
                    provider=provider, base_url=server.url, **kwargs)

    def test_omlx_http_translation_schema_auth_and_cache_identity(self):
        with FakeService() as server:
            page = self.page()
            self.translate(page, server)
            self.assertEqual(page["blocks"][0]["translation"], "早上好")
            self.assertEqual(page["blocks"][1]["translation"], "手动译文")
            chat = server.calls[1]
            self.assertEqual(chat["path"], "/v1/chat/completions")
            self.assertEqual(chat["authorization"], "Bearer fake-secret")
            payload = chat["payload"]
            self.assertFalse(payload["stream"])
            self.assertEqual(payload["temperature"], 0.15)
            self.assertEqual(payload["max_tokens"], 2048)
            self.assertEqual(payload["chat_template_kwargs"], {"enable_thinking": False})
            self.assertNotIn("options", payload)
            schema = payload["response_format"]["json_schema"]
            self.assertEqual(schema["name"], "translation")
            self.assertTrue(schema["strict"])
            self.assertEqual(schema["schema"]["properties"]["translations"]["required"], ["1"])
            self.assertFalse(schema["schema"]["additionalProperties"])
            self.assertEqual(page["translation_config"]["provider"], "omlx")
            self.assertEqual(page["translation_config"]["base_url"], server.url + "/v1")
            self.assertNotIn("fake-secret", json.dumps(page))
            self.translate(page, server)
            self.assertEqual(len([call for call in server.calls if call["method"] == "POST"]), 1)

    def test_provider_and_endpoint_switch_retranslate_machine_output_preserve_edits(self):
        with FakeService() as server, FakeService() as other:
            page = self.page()
            page["blocks"][0]["translation"] = "旧缓存"
            self.translate(page, server, provider="ollama")
            self.assertEqual(page["blocks"][0]["translation"], "早上好")
            self.assertEqual(server.calls[1]["path"], "/api/chat")
            self.assertIn("format", server.calls[1]["payload"])
            self.assertFalse(server.calls[1]["payload"]["think"])
            server.translation = "早安"
            self.translate(page, server, provider="omlx")
            self.assertEqual(page["blocks"][0]["translation"], "早安")
            self.assertEqual(page["blocks"][1]["translation"], "手动译文")
            other.translation = "早啊"
            self.translate(page, other, provider="omlx")
            self.assertEqual(page["blocks"][0]["translation"], "早啊")
            self.assertEqual(page["blocks"][1]["translation"], "手动译文")
            self.assertEqual(page["translation_config"]["base_url"], other.url + "/v1")
            self.assertEqual(len([call for call in server.calls if call["method"] == "POST"]), 2)
            self.assertEqual(len([call for call in other.calls if call["method"] == "POST"]), 1)

    def test_legacy_default_ollama_cache_remains_compatible(self):
        page = self.page()
        requests = []
        def fake(route, payload=None, timeout=300):
            if route == "/api/tags":
                return {"models": [{"name": "same-model"}]}
            requests.append(payload)
            return {"message": {"content": '{"translations":{"1":"早上好"}}'}}
        with patch.object(w, "api", side_effect=fake):
            w.translate(page, "same-model", lambda _: None, lambda _: None)
            page["translation_config"].pop("provider")
            page["translation_config"].pop("base_url")
            w.translate(page, "same-model", lambda _: None, lambda _: None)
        self.assertEqual(len(requests), 1)

    def test_omlx_failures_keep_source_and_do_not_log_response_text(self):
        with FakeService() as server, tempfile.TemporaryDirectory() as directory:
            metrics = Path(directory) / "metrics.jsonl"
            model_list = {"data": [{"id": "same-model"}]}
            service_request = TranslationService.request
            for response in [
                {"choices": [{"message": {"content": '{"translations":{"1":"早上好"}}'}, "finish_reason": "length"}]},
                {"choices": [{"message": {"content": '{"translations":{"1":"おはよう"}}'}, "finish_reason": "stop"}]},
                {"choices": [{"message": {"content": "private source fake-secret"}, "finish_reason": "stop"}]},
                {"choices": []}, {"error": "private source fake-secret"}]:
                page = self.page()
                def request(service, route, payload=None, timeout=300):
                    if route == "/models":
                        return model_list
                    server.response = (200, response, {})
                    return service_request(service, route, payload, timeout)
                with patch.object(TranslationService, "request", request), patch.dict(os.environ, {"KOMA_BENCHMARK_METRICS": str(metrics)}):
                    self.translate(page, server)
                self.assertEqual(page["blocks"][0]["source"], "おはよう")
                self.assertFalse(page["blocks"][0]["translation"])
                self.assertTrue(page["blocks"][0]["error"])
                self.assertEqual(page["blocks"][1]["translation"], "手动译文")
                self.assertNotIn("fake-secret", json.dumps(page))
            logged = metrics.read_text()
            self.assertNotIn("fake-secret", logged)
            self.assertNotIn("private source", logged)
            self.assertNotIn("おはよう", logged)


if __name__ == "__main__":
    unittest.main()
