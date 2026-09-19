import copy
import hashlib
import json
from pathlib import Path
import unittest

import numpy as np

from backend.panels import ensure_panels
from backend.structure import ensure_dialogues


class PanelTests(unittest.TestCase):
    @staticmethod
    def fixture(gutter):
        image = np.full((320, 400), 120, dtype=np.uint8)
        image[190:204, :] = gutter
        image[:190, 160:174] = gutter
        blocks = [
            {'id': 'left', 'box': [40, 70, 90, 100], 'source': '左'},
            {'id': 'right2', 'box': [230, 140, 280, 170], 'source': '右下'},
            {'id': 'bottom', 'box': [100, 250, 150, 280], 'source': '下'},
            {'id': 'right1', 'box': [230, 20, 280, 50], 'source': '右上'}]
        return image, {'width': 400, 'height': 320, 'blocks': blocks}

    def test_black_and_white_gutters_keep_complete_panel_before_next(self):
        for gutter in (0, 255):
            with self.subTest(gutter=gutter):
                image, page = self.fixture(gutter)
                reverse = copy.deepcopy(page)
                reverse['blocks'].reverse()
                for current in (page, reverse):
                    ensure_dialogues(current, gray=image)
                    order = [b['id'] for b in sorted(current['blocks'], key=lambda b: b['reading_order'])]
                    self.assertEqual(order, ['right1', 'right2', 'left', 'bottom'])
                    self.assertEqual(len(current['panels']), 3)
                self.assertEqual(page['panels'], reverse['panels'])

    def test_gutter_cannot_slice_an_ocr_box(self):
        image, page = self.fixture(0)
        page['blocks'].append({'id': 'crossing', 'box': [130, 50, 200, 80], 'source': '跨界'})
        ensure_panels(page, gray=image)
        self.assertEqual(len(page['panels']), 2)
        self.assertEqual(set(page['panels'][0]['block_ids']), {'right1', 'right2', 'left', 'crossing'})

    def test_panel_constraints_split_even_an_incorrect_shared_region(self):
        image, page = self.fixture(255)
        page['overlay_regions'] = [{'id': 'bad-region', 'block_ids': ['left', 'right1']}]
        ensure_dialogues(page, gray=image)
        self.assertFalse(any({'left', 'right1'} <= set(d['block_ids']) for d in page['dialogues']))

    def test_cached_panels_survive_translation_but_not_ocr_geometry_change(self):
        image, page = self.fixture(0)
        ensure_dialogues(page, gray=image)
        signature = page['panel_signature']
        page['blocks'][0]['translation'] = '左边'
        ensure_dialogues(page)
        self.assertEqual(signature, page['panel_signature'])
        page['blocks'][0]['box'][0] += 1
        ensure_dialogues(page)
        self.assertNotIn('panels', page)
        self.assertTrue(all(b['panel_id'] is None for b in page['blocks']))

    def test_no_gutter_is_one_candidate_and_does_not_authorize_overlay(self):
        image, page = self.fixture(120)
        ensure_dialogues(page, gray=image)
        self.assertEqual(len(page['panels']), 1)
        self.assertEqual(page['panels'][0]['method'], 'page_fallback')
        self.assertNotIn('overlay_regions', page)

    def test_cached_geometry_needs_no_opencv_import(self):
        import builtins
        from unittest.mock import patch
        image, page = self.fixture(0)
        ensure_panels(page, gray=image)
        old = copy.deepcopy(page)
        original = builtins.__import__
        def without_opencv(name, *args, **kwargs):
            if name == 'cv2':
                raise ImportError('not installed')
            return original(name, *args, **kwargs)
        with patch('builtins.__import__', side_effect=without_opencv):
            self.assertFalse(ensure_panels(page))
            self.assertEqual(page, old)
            ensure_panels(page, image='new.png')
            self.assertIn('ImportError: not installed', page['panel_error'])
            self.assertNotIn('panels', page)

    def test_unreadable_new_image_does_not_reuse_old_panel_candidates(self):
        image, page = self.fixture(0)
        ensure_panels(page, gray=image)
        ensure_panels(page, image='missing-panel-image.png')
        self.assertNotIn('panels', page)
        self.assertIn('panel_error', page)

    def test_real_page8_panel_order_matches_visual_annotation(self):
        root = Path(__file__).parents[1]
        image = Path('C:/Users/18910/Downloads/1471793/00008.webp')
        annotation = json.loads((root/'tests/fixtures/page8-panels.json').read_text(encoding='utf-8'))
        if not image.exists():
            self.skipTest('Local fixed-set image is not installed')
        self.assertEqual(hashlib.sha256(image.read_bytes()).hexdigest(), annotation['image_sha256'])
        page = {'blocks': annotation['blocks']}
        ensure_dialogues(page, image=image)
        self.assertEqual([set(p['block_ids']) for p in page['panels']],
                         [set(ids) for ids in annotation['panel_order']])
        ordered = [b['id'] for b in sorted(page['blocks'], key=lambda b: b['reading_order'])]
        position = {bid: i for i, bid in enumerate(ordered)}
        for first, second in zip(annotation['panel_order'], annotation['panel_order'][1:]):
            self.assertLess(max(position[bid] for bid in first), min(position[bid] for bid in second))
