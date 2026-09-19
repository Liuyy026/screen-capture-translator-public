import unittest
from backend.structure import build_dialogues, ensure_dialogues, sync_dialogue_translations, classify_text


class StructureTests(unittest.TestCase):
    def test_text_kind_is_candidate_not_silent_exclusion(self):
        self.assertEqual(classify_text("ＦＡＴＡＬＰＵＬＳＥＯＮＬＹ"), "page_text_candidate")
        self.assertEqual(classify_text("ママ"), "sfx_candidate")
        self.assertEqual(classify_text("！？"), "utterance_candidate")
        self.assertEqual(classify_text("おはよう"), "dialogue")
    def test_horizontal_group_keeps_members_and_order(self):
        page = {"blocks": [
            {"id": "1", "box": [100, 30, 140, 50], "source": "二行"},
            {"id": "2", "box": [100, 5, 140, 25], "source": "一行"}],
            "overlay_regions": [{"id": "7", "block_ids": ["1", "2"], "vertical": False}]}
        result = build_dialogues(page)
        self.assertEqual(result[0]["block_ids"], ["2", "1"])
        self.assertEqual(result[0]["source"], "一行\n二行")
        self.assertEqual(page["blocks"][0]["dialogue_id"], "d1")

    def test_vertical_uses_right_to_left_columns(self):
        page = {"blocks": [
            {"id": "1", "box": [10, 10, 30, 60], "source": "左", "vertical": True},
            {"id": "2", "box": [80, 10, 100, 60], "source": "右", "vertical": True}],
            "overlay_regions": [{"id": "1", "block_ids": ["1", "2"], "vertical": True}]}
        self.assertEqual(build_dialogues(page)[0]["block_ids"], ["2", "1"])

    def test_partial_translation_marks_block_fallback(self):
        page = {"blocks": [{"id": "1", "box": [0, 0, 10, 10], "translation": "甲"},
                            {"id": "2", "box": [0, 10, 10, 20], "translation": "", "error": "失败"}],
                "dialogues": [{"id": "d1", "block_ids": ["1", "2"]}]}
        sync_dialogue_translations(page)
        self.assertEqual(page["blocks"][1]["layout_fallback"], "block")
        self.assertIn("failures", page["diagnostics"])
        self.assertIsNone(page["diagnostics"]["failures"]["layout"])
        self.assertEqual(page['diagnostics']['layout']['status'], 'not_measured')

    def test_failure_counts_deduplicate_dialogue_and_block_errors(self):
        page = {'blocks': [{'id': '1', 'source': 'one', 'box': [0, 0, 10, 10], 'error': 'timeout'},
                           {'id': '2', 'source': 'two', 'box': [0, 20, 10, 30], 'error': 'timeout'},
                           {'id': '3', 'source': 'three', 'box': [0, 40, 10, 50]}],
                'dialogues': [{'id': 'd1', 'block_ids': ['1', '2'], 'translation_error': 'timeout'},
                              {'id': 'd2', 'block_ids': ['3']}]}
        sync_dialogue_translations(page)
        self.assertEqual(page['diagnostics']['translation']['errors'], 2)
        self.assertEqual(page['diagnostics']['failures']['translation'], 1)
        self.assertEqual(page['diagnostics']['translation']['pending_dialogues'], 1)
        self.assertIsNone(page['diagnostics']['failures']['layout'])

    def test_unchanged_dialogue_preserves_quality_warnings_on_rebuild(self):
        page = {'blocks': [{'id': '1', 'source': 'one', 'box': [0, 0, 10, 10]}]}
        build_dialogues(page)[0]['quality_warnings'] = ['review']
        self.assertEqual(build_dialogues(page)[0]['quality_warnings'], ['review'])
        page['blocks'][0]['source'] = 'changed'
        self.assertEqual(build_dialogues(page)[0]['quality_warnings'], [])

    def test_unassigned_close_lines_are_clustered(self):
        page = {"blocks": [
            {"id": "1", "box": [20, 10, 80, 30], "source": "上"},
            {"id": "2", "box": [20, 34, 80, 54], "source": "下"}], "overlay_regions": []}
        result = build_dialogues(page)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["block_ids"], ["1", "2"])

    def test_page_order_is_independent_of_detector_array(self):
        import copy
        page = {'blocks': [
            {'id': 'left', 'box': [10, 10, 40, 80], 'source': '左', 'vertical': True},
            {'id': 'bottom', 'box': [200, 150, 230, 200], 'source': '下'},
            {'id': 'right', 'box': [200, 10, 230, 80], 'source': '右', 'vertical': True}]}
        reverse = copy.deepcopy(page)
        reverse['blocks'].reverse()
        expected = [['right'], ['bottom'], ['left']]
        self.assertEqual([d['block_ids'] for d in build_dialogues(page)], expected)
        self.assertEqual([d['block_ids'] for d in build_dialogues(reverse)], expected)

    def test_diagnostics_expose_failure_attribution(self):
        page = {'blocks': [{'id': '1', 'box': [0, 0, 10, 10], 'source': '文本'}]}
        build_dialogues(page)
        self.assertIn('failures', page['diagnostics'])
        self.assertIn('grouping_review', page['diagnostics']['failures'])

    def test_source_change_invalidates_full_translation_and_cached_display(self):
        page = {'blocks': [{'id': '1', 'box': [0, 0, 20, 20], 'source': 'こんにちは', 'translation': '你好'}]}
        d = build_dialogues(page)[0]
        d.update(full_translation='你好', mapping={'1': '你好'})
        sync_dialogue_translations(page)
        self.assertTrue(page['blocks'][0]['dialogue_translation_complete'])
        page['blocks'][0]['source'] = 'さようなら'
        ensure_dialogues(page)
        self.assertNotIn('full_translation', page['dialogues'][0])
        self.assertFalse(page['blocks'][0]['dialogue_translation_complete'])

    def test_duplicate_members_preserved_but_source_sent_once(self):
        page = {'blocks': [
            {'id': '1', 'box': [10, 10, 50, 50], 'source': '私が', 'translation': ''},
            {'id': '2', 'box': [12, 12, 48, 48], 'source': '私が', 'translation': ''}]}
        d = build_dialogues(page)[0]
        self.assertEqual(len(d['block_ids']), 2)
        self.assertEqual(len(d['source_block_ids']), 1)
        self.assertEqual(d['source'], '私が')
        bid = d['source_block_ids'][0]
        d.update(full_translation='我', mapping={bid: '我'})
        next(b for b in page['blocks'] if b['id'] == bid)['translation'] = '我'
        sync_dialogue_translations(page)
        self.assertTrue(all(b['dialogue_translation_complete'] for b in page['blocks']))
        self.assertEqual([b['translation'] for b in page['blocks']], ['我', '我'])

    def test_distinct_regions_and_manual_speakers_are_not_merged(self):
        page = {'blocks': [
            {'id': '1', 'box': [0, 0, 40, 20], 'source': 'あ'},
            {'id': '2', 'box': [0, 22, 40, 42], 'source': 'い'}],
            'overlay_regions': [{'id': 'r1', 'block_ids': ['1']}, {'id': 'r2', 'block_ids': ['2']}]}
        self.assertEqual(len(build_dialogues(page)), 2)
        page['overlay_regions'] = [{'id': 'r', 'block_ids': ['1', '2']}]
        page['blocks'][0]['dialogue_group'] = 'speaker1'
        page['blocks'][1]['dialogue_group'] = 'speaker2'
        self.assertEqual(len(ensure_dialogues(page)), 2)


if __name__ == "__main__":
    unittest.main()
