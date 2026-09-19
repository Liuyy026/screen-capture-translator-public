import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from scripts.annotate_structure import template


class AnnotationScoreTests(unittest.TestCase):
    def test_batch_excludes_invalid_pages_and_empty_optional_attributes(self):
        page = {'blocks': [{'id': '1', 'source': 'one', 'box': [0, 0, 10, 10]}],
                'dialogues': [{'block_ids': ['1']}]}
        good = template(page)
        good.update(expected_dialogues=[['1']], expected_block_attributes={},
                    label_provenance={'reviewer_type': 'assistant_visual_audit'})
        invalid = template(page)
        invalid.update(expected_dialogues=['1'], label_provenance={'reviewer_type': 'human'})
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for name, data in [('good', good), ('invalid', invalid), ('unknown', template(page))]:
                (directory / (name + '.json')).write_text(json.dumps(data), encoding='utf8')
            result = subprocess.run([sys.executable, 'scripts/score_structure_annotations.py', str(directory)], capture_output=True, check=True)
            report = json.loads(result.stdout)
            self.assertEqual(report['annotated_pages'], 1)
            self.assertEqual(report['excluded_pages'], 1)
            self.assertEqual(report['unannotated_pages'], 1)
            self.assertEqual(report['human_labeled_pages'], 0)
            self.assertEqual(report['assistant_audited_pages'], 1)
            self.assertEqual(report['dialogue_exact_mean'], 1)
            self.assertIsNone(report['block_attribute_exact_mean'])
            self.assertEqual(report['metric_pages']['block_attribute_exact'], 0)
