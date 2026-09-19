import json
from pathlib import Path
import tempfile
import unittest
from scripts.compare_translation_units import compare


class ComparisonTests(unittest.TestCase):
    def test_empty_without_error_is_not_valid_and_changed_ocr_rejects_comparison(self):
        with tempfile.TemporaryDirectory() as temp:
            a, b = Path(temp)/'a', Path(temp)/'b'
            a.mkdir(); b.mkdir()
            page = {'model': 'test-local', 'blocks': [
                {'id': '1', 'box': [0, 0, 20, 20], 'source': '原文', 'translation': '', 'error': ''}]}
            for directory in (a, b):
                (directory/'page.json').write_text(json.dumps(page), encoding='utf-8')
            result = compare(a, b)
            self.assertTrue(result['comparable'])
            self.assertEqual(result['left']['valid_blocks'], 0)
            page['blocks'][0]['source'] = '別の原文'
            (b/'page.json').write_text(json.dumps(page), encoding='utf-8')
            self.assertFalse(compare(a, b)['comparable'])

    def test_ambiguous_model_directory_fails(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'model-caches'/'one').mkdir(parents=True)
            (root/'model-caches'/'two').mkdir()
            with self.assertRaises(ValueError):
                compare(root, root)
