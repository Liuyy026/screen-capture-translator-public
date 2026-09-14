import importlib.util
import json
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('worker', Path(__file__).parents[1] / 'backend/worker.py')
w = importlib.util.module_from_spec(spec)
spec.loader.exec_module(w)


class WorkerTests(unittest.TestCase):
    def test_missing_id_is_not_silently_mapped(self):
        found, errors = w.parse_translations('{"translations":[{"id":"2","text":"你好"}]}', {'1', '2'})
        self.assertEqual(found, {'2': '你好'})
        self.assertIn('1', errors)

    def test_placeholder_is_not_translation(self):
        for text in ['（此处应为对白翻译）', ' [ insert translation here ] ', '待翻译']:
            self.assertTrue(w.translation_error(text))
        self.assertFalse(w.translation_error('请稍等，我正在翻译。'))

    def test_bad_line_does_not_discard_good_lines(self):
        found, errors = w.parse_translations('{"translations":[{"id":"1","text":"（此处应为对白翻译）"},{"id":"2","text":"早上好"},{"id":"3","text":"甲"},{"id":"3","text":"乙"},{"id":"99","text":"其他内容"}]}', {'1','2','3'})
        self.assertEqual(found, {'2': '早上好'})
        self.assertEqual(set(errors), {'1','3'})

    def test_exact_key_schema_response_and_duplicate_json(self):
        self.assertEqual(w.validate_translations('{"translations":{"58":"你好"}}', {'58'}), {'58':'你好'})
        with self.assertRaises(ValueError):
            w.parse_translations('{"translations":{"58":"甲","58":"乙"}}', {'58'})

    def test_old_placeholder_cache_is_flagged_but_manual_edit_preserved(self):
        page = {'blocks': [
            {'translation':'（此处应为对白翻译）','edited':False,'error':''},
            {'translation':'（此处应为对白翻译）','edited':True,'error':''}]}
        w.audit_cache(page)
        self.assertTrue(page['blocks'][0]['error'])
        self.assertFalse(page['blocks'][1]['error'])

    def test_single_retry_uses_actual_id_and_leaves_other_failure_alone(self):
        page = {'model':'qwen3:4b', 'blocks': [
            {'id':'1','source':'おはよう','translation':'','edited':False,'error':'之前的失败'},
            {'id':'58','source':'こんにちは','translation':'','edited':False,'error':''}]}
        original, requests = w.api, []
        def fake(route, payload=None, timeout=300):
            if route == '/api/tags': return {'models':[{'name':'qwen3:4b'}]}
            requests.append(payload)
            return {'message':{'content':'{"translations":{"58":"你好"}}'}}
        w.api = fake
        try:
            w.translate(page, 'qwen3:4b', lambda _:None, lambda _:None, block_id='58')
        finally:
            w.api = original
        self.assertEqual(requests[0]['format']['properties']['translations']['required'], ['58'])
        self.assertEqual(page['blocks'][0]['error'], '之前的失败')
        self.assertEqual(page['blocks'][1]['translation'], '你好')

    def test_duplicate_or_unknown_ids_rejected(self):
        for content in ['{"translations":[{"id":"1","text":"a"},{"id":"1","text":"b"}]}',
                        '{"translations":[{"id":"9","text":"a"}]}']:
            with self.assertRaises(ValueError):
                w.validate_translations(content, {'1'})

    def test_empty_translation_rejected(self):
        with self.assertRaises(ValueError):
            w.validate_translations('{"translations":[{"id":"1","text":"  "}]}', {'1'})

    def test_content_addressed_cache(self):
        with tempfile.TemporaryDirectory() as d:
            first, second = Path(d)/'1.webp', Path(d)/'renamed.webp'
            first.write_bytes(b'original'); second.write_bytes(b'original')
            self.assertEqual(w.cache_path(first), w.cache_path(second))
            second.write_bytes(b'changed')
            self.assertNotEqual(w.cache_path(first), w.cache_path(second))

    def test_hand_edits_preserved_and_partial_output_marked(self):
        page = {'model':'qwen3:4b', 'blocks': [
            {'id':'1', 'source':'a', 'translation':'手改', 'edited':True, 'error':''},
            {'id':'2', 'source':'b', 'translation':'', 'edited':False, 'error':''},
            {'id':'3', 'source':'c', 'translation':'', 'edited':False, 'error':''}]}
        original = w.api
        def fake(route, payload=None, timeout=300):
            if route == '/api/tags': return {'models':[{'name':'qwen3:4b'}]}
            return {'message':{'content':'{"translations":[{"id":"2","text":"译文"}]}'} }
        w.api = fake
        try:
            w.translate(page, 'qwen3:4b', lambda _: None, lambda _: None)
        finally:
            w.api = original
        self.assertEqual(page['blocks'][0]['translation'], '手改')
        self.assertEqual(page['blocks'][1]['translation'], '译文')
        self.assertTrue(page['blocks'][2]['error'])

    def test_overlay_classifier_integration_preserves_manual_text(self):
        page = {'version': 1, 'blocks': [
            {'source': 'ん', 'translation': '', 'edited': False},
            {'source': 'そう', 'translation': '手动译文', 'edited': True},
        ]}
        calls = []
        original = w._overlay.ensure_page_overlays
        def fake(value, gray=None, image=None):
            calls.append((gray, image))
            value['version'] = 2
            value['blocks'][0].update({'overlay': 'skip', 'overlay_score': 0.0,
                                       'overlay_reason': '无背景'})
            return True
        w._overlay.ensure_page_overlays = fake
        try:
            w._overlay.ensure_page_overlays(page, image='page.png')
        finally:
            w._overlay.ensure_page_overlays = original
        self.assertEqual(page['version'], 2)
        self.assertEqual(page['blocks'][0]['overlay'], 'skip')
        self.assertEqual(page['blocks'][1]['translation'], '手动译文')
        self.assertEqual(calls, [(None, 'page.png')])

    def test_recognize_passes_source_image_to_overlay_classifier(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / '.models' / 'manga-ocr').mkdir(parents=True)
            (root / '.models' / 'manga-ocr' / 'pytorch_model.bin').write_bytes(b'weights')
            (root / '.models' / 'comictextdetector.pt').write_bytes(b'weights')
            fake_torch = types.ModuleType('torch')
            fake_torch.set_num_threads = lambda _: None
            fake_ocr = types.ModuleType('mokuro.manga_page_ocr')
            class FakeOcr:
                def __init__(self, **kwargs): pass
                def __call__(self, image):
                    return {'blocks': [{'lines': ['おはよう'], 'box': [10, 10, 30, 30], 'vertical': False}],
                            'img_width': 100, 'img_height': 100}
            fake_ocr.MangaPageOcr = FakeOcr
            fake_cache_module = types.ModuleType('mokuro.cache')
            fake_cache_module.cache = types.SimpleNamespace(root=None)
            fake_mokuro = types.ModuleType('mokuro')
            calls = []
            original = w._overlay.ensure_page_overlays
            def fake_overlay(page, gray=None, image=None):
                calls.append(image)
                page['version'] = 2
                return True
            w._overlay.ensure_page_overlays = fake_overlay
            try:
                with patch.object(w, 'ROOT', root), patch.dict(sys.modules, {
                        'torch': fake_torch, 'mokuro': fake_mokuro,
                        'mokuro.manga_page_ocr': fake_ocr,
                        'mokuro.cache': fake_cache_module}):
                    page = w.recognize('scan.png', lambda _: None)
            finally:
                w._overlay.ensure_page_overlays = original
            self.assertEqual(calls, ['scan.png'])
            self.assertEqual(page['blocks'][0]['source'], 'おはよう')

    def test_skip_blocks_are_excluded_from_translation_targets(self):
        page = {'model': 'qwen3:4b', 'version': 2, 'blocks': [
            {'id': '1', 'source': 'ん', 'translation': '', 'edited': False,
             'overlay': 'skip', 'error': ''},
            {'id': '2', 'source': 'おはよう', 'translation': '', 'edited': False,
             'overlay': 'uncertain', 'error': ''},
        ]}
        original, requests = w.api, []
        def fake(route, payload=None, timeout=300):
            if route == '/api/tags': return {'models': [{'name': 'qwen3:4b'}]}
            requests.append(payload)
            return {'message': {'content': '{"translations":{"2":"早上好"}}'}}
        w.api = fake
        try:
            w.translate(page, 'qwen3:4b', lambda _: None, lambda _: None)
        finally:
            w.api = original
        user_payload = json.loads(requests[0]['messages'][1]['content'].split('\n', 1)[0])
        self.assertEqual(user_payload['target_ids'], ['2'])
        self.assertEqual(page['blocks'][0]['translation'], '')

    def test_skip_only_page_does_not_query_model(self):
        page = {'model': '', 'version': 2, 'blocks': [
            {'id': '1', 'source': 'ん', 'translation': '', 'edited': False,
             'overlay': 'skip', 'error': ''},
        ]}
        original = w.api
        calls = []
        w.api = lambda *args, **kwargs: calls.append(args) or {'models': []}
        try:
            w.translate(page, 'qwen3:4b', lambda _: None, lambda _: None)
        finally:
            w.api = original
        self.assertEqual(calls, [])

    def test_translation_save_refreshes_overlay_with_source_image(self):
        page = {'model': 'qwen3:4b', 'blocks': [
            {'id': '1', 'source': 'おはよう', 'translation': '', 'edited': False,
             'overlay': 'uncertain', 'error': ''},
        ]}
        calls, saved = [], []
        def fake_api(route, payload=None, timeout=300):
            if route == '/api/tags':
                return {'models': [{'name': 'qwen3:4b'}]}
            return {'message': {'content': '{"translations":{"1":"早上好"}}'}}
        def fake_overlay(value, gray=None, image=None):
            calls.append((value['blocks'][0]['translation'], image))
            value['blocks'][0]['overlay'] = 'rectangle'
            return True
        with patch.object(w, 'api', side_effect=fake_api), \
                patch.object(w._overlay, 'ensure_page_overlays', side_effect=fake_overlay):
            w.translate(page, 'qwen3:4b', lambda _: None,
                        lambda value: saved.append(value['blocks'][0]['overlay']), image='page.webp')
        self.assertEqual(calls, [('早上好', 'page.webp')])
        self.assertEqual(saved, ['rectangle'])


if __name__ == '__main__': unittest.main()
