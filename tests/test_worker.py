import importlib.util
import tempfile
import unittest
from pathlib import Path

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


if __name__ == '__main__': unittest.main()
