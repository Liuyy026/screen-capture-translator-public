import base64
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("macos_vision_worker", ROOT / "macos/backend/vision_worker.py")
v = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v)


class FakeService:
    provider = "omlx"
    identity = {"provider": "omlx", "base_url": "http://localhost:8000/v1"}
    chat_route = "/chat/completions"

    def __init__(self, content=None):
        self.payloads = []
        self.content = content

    def list_models(self):
        return ["test", "other"]

    def translation_payload(self, model, messages, schema):
        return {"model": model, "messages": messages, "schema": schema}

    def request(self, route, payload):
        self.payloads.append(payload)
        if isinstance(self.content, Exception):
            raise self.content
        ids = payload["schema"]["properties"]["translations"]["required"]
        return {"content": self.content or json.dumps({"translations": {key: "你好" for key in ids},
                                          "source_suggestions": {key: "" for key in ids}})}

    def translation_content(self, response):
        return response["content"]

    def safe_error(self, error):
        return str(error).replace("private-key", "[已隐藏]")


class VisionWorkerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.image = self.root / "page.png"
        Image.new("RGB", (2400, 3000), "white").save(self.image)
        self.page = {"version": 2, "width": 2400, "height": 3000, "model": "", "ocr": "test",
            "blocks": [{"id": str(i), "box": [10*i, 20, 10*i+8, 60], "source": "こんにちは",
                "translation": "", "error": "", "vertical": True, "edited": False, "reading_order": i}
                for i in range(1, 4)]}
        self.addCleanup(patch.stopall)
        patch.object(v.shared, "CACHE", self.root / "text").start()
        patch.object(v.shared._structure, "ensure_dialogues").start()
        patch.object(v.shared._structure, "sync_dialogue_translations").start()
        patch.object(v.shared._overlay, "ensure_page_overlays").start()

    def run_translation(self, service=None, block_id=None, model="test"):
        service = service or FakeService()
        v.translate(self.page, self.image, model, service, lambda _: None,
                    lambda p: None, block_id)
        return service

    def test_image_is_valid_bounded_and_payload_keeps_original_coordinates(self):
        url, size = v.encode_image(self.image)
        self.assertEqual(size, (1420, 1775))
        decoded = Image.open(io.BytesIO(base64.b64decode(url.split(",", 1)[1])))
        self.assertEqual(decoded.size, size)
        payload = v.make_payload(FakeService(), "test", self.page, self.page["blocks"][:1], url)
        parts = payload["messages"][1]["content"]
        self.assertEqual(parts[1]["image_url"]["url"], url)
        context = json.loads(parts[0]["text"])
        self.assertEqual(context["image_width"], 2400)
        self.assertEqual(context["page"][0]["box"], [10, 20, 18, 60])
        self.assertEqual(context["target_ids"], ["1"])

    def test_separate_cache_preserves_original_and_manual_edits(self):
        original = v.shared.cache_path(self.image)
        self.page["blocks"][0].update(translation="人工译文", edited=True)
        self.page["blocks"][1]["translation"] = "普通机器译文"
        self.page["unknown_metadata"] = {"keep": True}
        v.shared.atomic_json(original, self.page)
        before = original.read_bytes()
        dest = v.cache_path(self.image)
        result = v.prepare_page(self.image, dest)
        self.assertNotEqual(dest, original)
        self.assertEqual(dest.parent, original.parent / "vision-v1")
        self.assertEqual(result["blocks"][0]["translation"], "人工译文")
        self.assertEqual(result["blocks"][1]["translation"], "")
        self.assertEqual(result["unknown_metadata"], {"keep": True})
        self.assertEqual(original.read_bytes(), before)
        v.shared.atomic_json(dest, result)
        self.assertEqual(v.prepare_page(self.image, dest), result)

    def test_visual_suggestion_never_replaces_source(self):
        s = FakeService(json.dumps({"translations": {"1": "你好"}, "source_suggestions": {"1": "こんばんは"}}))
        self.run_translation(s, "1")
        self.assertEqual(self.page["blocks"][0]["source"], "こんにちは")
        self.assertEqual(self.page["blocks"][0]["vision_source"], "こんばんは")
        self.assertEqual(self.page["blocks"][1]["translation"], "")

    def test_model_change_invalidates_machine_results_but_not_manual(self):
        self.page["blocks"][0].update(translation="人工译文", edited=True)
        self.run_translation()
        self.assertEqual(self.page["blocks"][0]["translation"], "人工译文")
        second = self.run_translation(model="other")
        self.assertEqual(len(second.payloads), 1)
        ids = second.payloads[0]["schema"]["properties"]["translations"]["required"]
        self.assertNotIn("1", ids)
        self.assertEqual(self.page["blocks"][0]["translation"], "人工译文")
        self.assertEqual(self.page["translation_config"]["model"], "other")

    def test_complete_cache_avoids_inference_and_source_edit_invalidates_it(self):
        self.run_translation()
        self.assertEqual(len(self.run_translation().payloads), 0)
        self.page["blocks"][1]["source"] = "こんばんは"
        self.assertEqual(len(self.run_translation().payloads), 1)

    def test_missing_and_duplicate_ids_do_not_look_successful(self):
        self.run_translation(FakeService('{"translations":{"1":"你好","1":"您好"},"source_suggestions":{}}'))
        self.assertTrue(all(b["error"] for b in self.page["blocks"]))
        self.run_translation(FakeService('{"translations":{"1":"你好"},"source_suggestions":{}}'))
        self.assertEqual(self.page["blocks"][0]["translation"], "你好")
        self.assertTrue(self.page["blocks"][1]["error"])
        self.assertTrue(self.page["blocks"][2]["error"])

    def test_failure_redacts_secret_and_removes_stale_suggestion(self):
        self.page["blocks"][0]["vision_source"] = "stale"
        self.run_translation(FakeService(ValueError("private-key")))
        self.assertNotIn("private-key", json.dumps(self.page))
        self.assertNotIn("vision_source", self.page["blocks"][0])
        self.assertTrue(self.page["blocks"][0]["error"])

    def test_multiple_batches_and_skipped_blocks(self):
        seed = self.page["blocks"][0]
        self.page["blocks"] = [{**copy.deepcopy(seed), "id": str(i), "reading_order": i} for i in range(20)]
        self.page["blocks"][0]["overlay"] = "skip"
        self.page["blocks"][1]["text_kind"] = "noise"
        service = self.run_translation()
        self.assertEqual([len(p["schema"]["properties"]["translations"]["required"]) for p in service.payloads], [8, 8, 2])
        self.assertEqual(self.page["blocks"][0]["translation"], "")
        self.assertEqual(self.page["blocks"][1]["translation"], "")

    def test_ollama_cannot_silently_run_text_in_vision_mode(self):
        service = FakeService()
        service.provider = "ollama"
        with self.assertRaisesRegex(ValueError, "oMLX"):
            self.run_translation(service)

    def test_shared_han_word_is_kept_with_warning_but_japanese_echo_is_error(self):
        self.page["blocks"][0]["source"] = "当然"
        self.run_translation(FakeService(json.dumps({"translations": {"1": "当然", "2": "こんにちは"}})))
        self.assertEqual(self.page["blocks"][0]["translation"], "当然")
        self.assertEqual(self.page["blocks"][0]["error"], "")
        self.assertTrue(self.page["blocks"][0]["quality_warnings"])
        self.assertTrue(self.page["blocks"][1]["error"])


if __name__ == "__main__":
    unittest.main()
