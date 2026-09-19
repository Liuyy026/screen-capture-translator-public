import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from scripts.annotate_structure import score, template, refresh_predictions


def annotation():
    page = {'width': 100, 'height': 100, 'blocks': [
        {'id': str(i), 'box': [i * 10, 0, i * 10 + 5, 20], 'source': str(i),
         'vertical': True, 'text_kind': 'dialogue'} for i in range(1, 4)],
        'dialogues': [{'block_ids': ['1', '2']}, {'block_ids': ['3']}],
        'panels': [{'block_ids': ['1', '2', '3'], 'reading_order': 1}]}
    a = template(page)
    a.update(expected_dialogues=[['1', '2'], ['3']],
             label_provenance={'reviewer_type': 'human', 'evidence': 'synthetic fixture'})
    return page, a


class AnnotationTests(unittest.TestCase):
    def test_init_does_not_overwrite_existing_annotation_without_force(self):
        with tempfile.TemporaryDirectory() as temp:
            cache = Path(temp) / 'page.json'; label = Path(temp) / 'annotation.json'
            cache.write_text(json.dumps({'blocks': []}), encoding='utf8')
            label.write_text('{"keep": true}', encoding='utf8')
            result = subprocess.run([sys.executable, 'scripts/annotate_structure.py', str(cache), str(label), '--init'], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('keep', label.read_text(encoding='utf8'))

    def test_internal_dialogue_order_is_scored(self):
        _, a = annotation()
        a['expected_dialogues'][0].reverse()
        result = score(a)
        self.assertEqual(result['dialogue_exact'], .5)
        self.assertEqual(result['dialogue_coverage'], 1)

    def test_wrong_membership_and_missing_prediction_reduce_scores(self):
        page, a = annotation()
        a['expected_dialogues'] = [['1'], ['2', '3']]
        self.assertEqual(score(a)['dialogue_exact'], 0)
        self.assertAlmostEqual(score(a)['dialogue_coverage'], .5)
        page['dialogues'] = []
        result = score(refresh_predictions(a, page))
        self.assertTrue(result['annotated'])
        self.assertEqual(result['dialogue_exact'], 0)
        self.assertEqual(result['dialogue_coverage'], 0)

    def test_invalid_labels_are_excluded_not_scored(self):
        for invalid in (['1', '2', '3'], [['1', '1', '2'], ['3']],
                        [['1', '2'], ['2', '3']], [[], ['1', '2', '3']],
                        [['1', '2'], ['9']], [['1', '2']], {'group': ['1']}):
            with self.subTest(invalid=invalid):
                _, a = annotation()
                a['expected_dialogues'] = invalid
                result = score(a)
                self.assertTrue(result['excluded'])
                self.assertFalse(result['annotated'])
                self.assertNotIn('dialogue_exact', result)

    def test_predictions_cannot_be_replaced_with_labels(self):
        _, a = annotation()
        a['predicted_dialogues'] = [['3'], ['1', '2']]
        self.assertTrue(score(a)['excluded'])

    def test_refresh_uses_cache_and_preserves_labels(self):
        page, a = annotation()
        before = copy.deepcopy(a)
        page['dialogues'].reverse()
        refreshed = refresh_predictions(a, page)
        self.assertEqual(refreshed['expected_dialogues'], before['expected_dialogues'])
        self.assertEqual(refreshed['predicted_dialogues'], [['3'], ['1', '2']])
        self.assertEqual(score(refreshed)['dialogue_exact'], 0)
        self.assertEqual(a, before)
        page['blocks'][0]['source'] = 'changed'
        with self.assertRaisesRegex(ValueError, 'OCR fingerprint mismatch'):
            refresh_predictions(a, page)

    def test_cli_score_reads_actual_cache_without_rewriting_labels(self):
        page, a = annotation()
        page['dialogues'].reverse()
        with tempfile.TemporaryDirectory() as temp:
            cache, label = Path(temp) / 'page.json', Path(temp) / 'annotation.json'
            cache.write_text(json.dumps(page), encoding='utf8')
            label.write_text(json.dumps(a), encoding='utf8')
            before = label.read_bytes()
            result = subprocess.run([sys.executable, 'scripts/annotate_structure.py', str(cache), str(label), '--score'], capture_output=True, check=True)
            self.assertEqual(json.loads(result.stdout)['dialogue_exact'], 0)
            self.assertEqual(label.read_bytes(), before)

    def test_attributes_include_text_kind_and_panel_member_order_is_irrelevant(self):
        _, a = annotation()
        a['expected_panel_order'] = [['3', '1', '2']]
        a['expected_block_attributes'] = {'1': {'vertical': True, 'text_kind': 'caption'}}
        self.assertEqual(score(a)['panel_exact'], 1)
        self.assertEqual(score(a)['block_attribute_exact'], .5)

    def test_unfilled_template_is_not_scored(self):
        self.assertEqual(score({'predicted_dialogues': [['1']]}), {'annotated': False})

    def test_legacy_labels_require_explicit_migration(self):
        page, a = annotation()
        del a['prediction_provenance']
        self.assertTrue(score(a)['excluded'])
        with self.assertRaisesRegex(ValueError, 'Legacy'):
            refresh_predictions(a, page)
