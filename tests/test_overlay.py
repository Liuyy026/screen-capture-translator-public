import unittest
import importlib.util
import json
from pathlib import Path

import numpy as np

spec = importlib.util.spec_from_file_location('overlay', Path(__file__).parents[1] / 'backend/overlay.py')
overlay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(overlay)
ensure_page_overlays = overlay.ensure_page_overlays


class OverlayTests(unittest.TestCase):
    def test_large_bright_panel_never_becomes_balloon_or_large_region(self):
        import cv2
        gray = np.full((600, 800), 40, dtype=np.uint8)
        cv2.rectangle(gray, (80, 40), (720, 560), 245, -1)
        box = [380, 275, 420, 325]
        page = {"blocks": [{"id": "1", "box": box, "source": "日本語", "translation": "中文"}]}
        ensure_page_overlays(page, gray=gray)
        region = page["overlay_regions"][0]
        self.assertEqual(region["overlay"], "rectangle")
        points = np.asarray(region["points"], dtype=float)
        width = points[:, 0].max() - points[:, 0].min()
        height = points[:, 1].max() - points[:, 1].min()
        self.assertLessEqual(width, (box[2] - box[0]) * 2.6)
        self.assertLessEqual(height, (box[3] - box[1]) * 2.6)

    def test_real_page_00012_regions_are_separated_and_safe(self):
        image = Path(r"C:\Users\18910\Downloads\1471793\00012.webp")
        cache = Path(r"C:\Users\18910\AppData\Local\KomaReader\pages\e63072ae9b114c2ae42bab145bec866236632e8c61b1ddde6f31d897aff71bb1.json")
        if not image.exists() or not cache.exists():
            self.skipTest("real-page regression assets are unavailable")
        page = json.loads(cache.read_text(encoding="utf-8"))
        page.pop("overlay_signature", None)
        ensure_page_overlays(page, image=image)
        blocks = {str(b["id"]): b for b in page["blocks"]}
        self.assertEqual(blocks["3"]["overlay"], "balloon")
        self.assertEqual(blocks["4"]["overlay"], "balloon")
        self.assertEqual(blocks["7"]["overlay"], "balloon")
        self.assertEqual(len({blocks[str(i)]["overlay_region_id"] for i in (3, 4, 7)}), 3)
        self.assertEqual(blocks["5"]["overlay_region_id"], blocks["6"]["overlay_region_id"])
        self.assertIsNotNone(blocks["5"]["overlay_region_id"])
        self.assertEqual(blocks["5"]["overlay"], "balloon")
        joint = next(region for region in page["overlay_regions"]
                     if region["id"] == blocks["5"]["overlay_region_id"])
        self.assertEqual(set(joint["block_ids"]), {"5", "6"})
        self.assertEqual(blocks["2"]["overlay"], "uncertain")
        self.assertEqual(blocks["9"]["overlay"], "uncertain")
        for region in page["overlay_regions"]:
            points = np.asarray(region.get("points") or [], dtype=float)
            if points.size == 0:
                continue
            region_width = points[:, 0].max() - points[:, 0].min()
            region_height = points[:, 1].max() - points[:, 1].min()
            member_boxes = np.asarray([blocks[i]["box"] for i in region["block_ids"]])
            union_width = member_boxes[:, 2].max() - member_boxes[:, 0].min()
            union_height = member_boxes[:, 3].max() - member_boxes[:, 1].min()
            self.assertLessEqual(region_width, union_width * 2.6 + 1)
            self.assertLessEqual(region_height, union_height * 2.6 + 1)

    def test_plain_background_is_uncertain(self):
        page = {"blocks": [{"id": "1", "box": [170, 125, 230, 175], "source": "これは文"}]}
        ensure_page_overlays(page, gray=np.full((300, 400), 245, dtype=np.uint8))
        self.assertEqual(page["blocks"][0]["overlay"], "uncertain")

    def test_translation_ready_recomputes_local_fill_after_ocr_only_pass(self):
        gray = np.full((300, 400), 245, dtype=np.uint8)
        page = {"blocks": [{"id": "1", "box": [170, 125, 230, 175],
                            "source": "これは文", "translation": "", "error": ""}]}
        self.assertTrue(ensure_page_overlays(page, gray=gray))
        self.assertEqual(page["blocks"][0]["overlay"], "uncertain")
        self.assertFalse(ensure_page_overlays(page, gray=gray))
        page["blocks"][0]["translation"] = "这是对白"
        self.assertTrue(ensure_page_overlays(page, gray=gray))
        self.assertEqual(page["blocks"][0]["overlay"], "rectangle")
        self.assertEqual(len(page["overlay_regions"]), 1)

    def test_closed_light_balloon_is_accepted(self):
        import cv2
        gray = np.full((300, 400), 40, dtype=np.uint8)
        cv2.ellipse(gray, (200, 150), (75, 55), 0, 0, 360, 245, -1)
        cv2.ellipse(gray, (200, 150), (75, 55), 0, 0, 360, 40, 3)
        page = {"blocks": [{"id": "1", "box": [155, 115, 245, 185], "source": "こんにちは"}]}
        ensure_page_overlays(page, gray=gray)
        self.assertEqual(page["blocks"][0]["overlay"], "balloon")
        self.assertEqual(page["overlay_regions"][0]["fill_color"], [245, 245, 245])

    def test_split_sentence_with_continuation_ink_is_grouped(self):
        import cv2
        gray = np.full((420, 360), 35, dtype=np.uint8)
        cv2.ellipse(gray, (180, 210), (65, 130), 0, 0, 360, 250, -1)
        cv2.line(gray, (177, 183), (177, 223), 25, 2)
        page = {"blocks": [
            {"id": "1", "box": [153, 125, 210, 205], "source": "これは", "vertical": True},
            {"id": "2", "box": [160, 210, 200, 295], "source": "続きです", "vertical": True},
        ]}
        ensure_page_overlays(page, gray=gray)
        self.assertEqual(len(page["overlay_regions"]), 1)
        self.assertEqual(page["overlay_regions"][0]["block_ids"], ["1", "2"])
        self.assertEqual(page["overlay_regions"][0]["overlay"], "balloon")

    def test_adjacent_balloons_remain_separate(self):
        import cv2
        gray = np.full((400, 620), 30, dtype=np.uint8)
        page = {"blocks": []}
        for index, center in enumerate((220, 400)):
            cv2.ellipse(gray, (center, 200), (85, 80), 0, 0, 360, 250, -1)
            page["blocks"].append({"id": str(index), "box": [center-60, 145, center+60, 255],
                                   "source": "これは文", "vertical": True})
        cv2.rectangle(gray, (305, 70), (315, 330), 0, -1)
        ensure_page_overlays(page, gray=gray)
        self.assertEqual(len(page["overlay_regions"]), 2)
        self.assertEqual({tuple(r["block_ids"]) for r in page["overlay_regions"]}, {("0",), ("1",)})
        masks = []
        for region in page["overlay_regions"]:
            self.assertEqual(region["overlay"], "balloon")
            mask = np.zeros_like(gray)
            cv2.fillPoly(mask, [np.asarray(region["points"], np.int32)], 1)
            self.assertFalse(mask[70:331, 305:316].any())
            masks.append(mask)
        self.assertFalse(np.any(masks[0] & masks[1]))

    def test_short_gap_panel_line_prevents_grouping(self):
        gray = np.full((160, 160), 250, dtype=np.uint8)
        first, second = (60, 20, 100, 70), (60, 80, 100, 130)
        self.assertTrue(overlay._adjacent_text(gray, first, second, True))
        gray[74:76, :] = 0
        self.assertFalse(overlay._adjacent_text(gray, first, second, True))

    def test_bounded_shapes_keep_erase_inside_text_neighborhood(self):
        import cv2
        for shape in ("square", "ellipse", "tail", "burst"):
            with self.subTest(shape=shape):
                gray = np.full((340, 440), 25, dtype=np.uint8)
                if shape == "square":
                    cv2.rectangle(gray, (150, 100), (290, 240), 248, -1)
                elif shape == "burst":
                    points = np.asarray([[220 + round((82 if i % 2 == 0 else 67) * np.cos(i*np.pi/8)),
                                          170 + round((82 if i % 2 == 0 else 67) * np.sin(i*np.pi/8))]
                                         for i in range(16)], np.int32)
                    cv2.fillPoly(gray, [points], 248)
                else:
                    cv2.ellipse(gray, (220, 170), (80, 72), 0, 0, 360, 248, -1)
                    if shape == "tail":
                        cv2.fillPoly(gray, [np.array([[230, 225], [252, 219], [265, 254]])], 248)
                cv2.line(gray, (182, 147), (220, 192), 30, 3)
                art = np.zeros_like(gray)
                cv2.circle(art, (220, 115), 3, 1, -1)
                gray[art > 0] = 20
                box = [172, 128, 268, 212]
                page = {"blocks": [{"id": "1", "box": box, "source": "これは文"}]}
                ensure_page_overlays(page, gray=gray)
                self.assertEqual(page["blocks"][0]["overlay"], "balloon")
                region = page["overlay_regions"][0]
                erased = np.zeros_like(gray)
                cv2.fillPoly(erased, [np.asarray(p, np.int32) for p in region["erase_points"]], 1)
                contour = np.zeros_like(gray)
                cv2.fillPoly(contour, [np.asarray(region["points"], np.int32)], 1)
                self.assertFalse(np.any(erased & (1-contour)))
                self.assertEqual(int(erased[150, 185]), 1)
                self.assertEqual(int(erased[110, 220]), 0)
                self.assertLess(int(erased.sum()), int(contour.sum()))
                self.assertTrue(contour[art > 0].all())
                self.assertFalse(erased[art > 0].any())
                x1, y1, x2, y2 = map(int, region["text_box"])
                self.assertTrue(contour[y1:y2, x1:x2].all())
                cleaned = gray.copy()
                cleaned[erased > 0] = region["fill_color"][0]
                np.testing.assert_array_equal(cleaned[art > 0], gray[art > 0])
                self.assertEqual(int(cleaned[150, 185]), 248)
                self.assertEqual(region["fill_color"], [248, 248, 248])

    def test_background_free_interjection_is_skipped(self):
        page = {"blocks": [{"id": "1", "box": [10, 10, 30, 30], "source": "えっ"}]}
        ensure_page_overlays(page, gray=np.full((100, 100), 255, dtype=np.uint8))
        self.assertEqual(page["blocks"][0]["overlay"], "skip")


if __name__ == "__main__":
    unittest.main()
