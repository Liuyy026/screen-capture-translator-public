import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "macos/backend/reading_overlay.py"
spec = importlib.util.spec_from_file_location("macos_reading_overlay", SCRIPT)
reading = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reading)


def block(identifier="1", box=(340, 180, 360, 360), translation="中文翻译"):
    return {"id": identifier, "box": list(box), "source": "縦書き", "vertical": True,
            "translation": translation, "error": "", "edited": False}


def fixture(blocks=None):
    blocks = blocks or [block()]
    gray = np.full((1000, 700), 80, np.uint8)
    cv2.ellipse(gray, (350, 270), (90, 140), 0, 0, 360, 255, -1)
    cv2.ellipse(gray, (350, 270), (90, 140), 0, 0, 360, 20, 3)
    for entry in blocks:
        x, y, r, d = entry["box"]
        for top in range(y+3, d-8, 16):
            cv2.rectangle(gray, (x+3, top), (r-4, top+5), 20, -1)
    return {"width": 700, "height": 1000, "blocks": blocks, "overlay_regions": []}, gray


def rectangle(identifier, members, bounds):
    x, y, r, d = bounds
    return {"id": identifier, "block_ids": members, "overlay": "rectangle",
            "points": [[x, y], [r, y], [r, d], [x, d]], "text_box": list(bounds),
            "fill_color": [255, 255, 255], "vertical": True}


def polygon_mask(points, shape):
    mask = np.zeros(shape, np.uint8)
    cv2.fillPoly(mask, [np.asarray(points, np.int32)], 1)
    return mask


class ReadingOverlayTests(unittest.TestCase):
    def test_narrow_column_uses_confirmed_bubble_without_mutating_inputs(self):
        page, gray = fixture()
        before, original = copy.deepcopy(page), gray.copy()
        regions = reading.reading_regions(page, gray)
        self.assertEqual(len(regions), 1)
        region = regions[0]
        self.assertEqual(region["block_ids"], ["1"])
        self.assertGreater(region["text_box"][2]-region["text_box"][0], 4*20)
        self.assertEqual(page, before)
        np.testing.assert_array_equal(gray, original)

    def test_text_rectangle_and_eraser_are_inside_contour_and_eraser_is_local(self):
        page, gray = fixture()
        region = reading.reading_regions(page, gray)[0]
        bubble = polygon_mask(region["points"], gray.shape)
        x, y, r, d = region["text_box"]
        self.assertTrue(bubble[y:d, x:r].all())
        erased = np.zeros_like(gray)
        for polygon in region["erase_points"]:
            erased |= polygon_mask(polygon, gray.shape)
        self.assertFalse(np.any(erased & (1-bubble)))
        allowed = np.zeros_like(gray)
        allowed[173:367, 333:367] = 1
        self.assertFalse(np.any(erased & (1-allowed)))
        self.assertLess(erased.sum(), bubble.sum()*.2)

    def test_two_columns_in_same_oval_form_one_region(self):
        page, gray = fixture([block("a", (320, 185, 340, 350)), block("b", (360, 195, 380, 360))])
        regions = reading.reading_regions(page, gray)
        self.assertEqual([r["block_ids"] for r in regions], [["a", "b"]])
        self.assertEqual(len(regions[0]["erase_points"]), 2)

    def test_unready_member_suppresses_entire_group_and_stale_singleton(self):
        for change in ({"translation": ""}, {"error": "failed"}):
            with self.subTest(change=change):
                page, gray = fixture([block("a", (320, 185, 340, 350)), block("b", (360, 195, 380, 360))])
                page["blocks"][1].update(change)
                page["overlay_regions"] = [rectangle("old", ["a"], (317, 182, 343, 353))]
                self.assertEqual(reading.reading_regions(page, gray), [])

    def test_manual_flag_does_not_bypass_an_unresolved_error(self):
        page, gray = fixture()
        page["blocks"][0].update(edited=True, error="old failure")
        self.assertEqual(reading.reading_regions(page, gray), [])
        page["blocks"][0]["error"] = ""
        self.assertEqual(len(reading.reading_regions(page, gray)), 1)

    def test_page_background_and_bordered_rectangle_do_not_become_bubbles(self):
        page, _ = fixture()
        for gray in (np.full((1000, 700), 255, np.uint8), np.full((1000, 700), 80, np.uint8)):
            if gray[0, 0] == 80:
                cv2.rectangle(gray, (260, 130), (440, 410), 255, -1)
                cv2.rectangle(gray, (260, 130), (440, 410), 20, 3)
            self.assertEqual(reading.reading_regions(page, gray), [])

    def test_outline_gap_to_page_background_is_not_closed_artificially(self):
        page, gray = fixture()
        gray[:] = 255
        cv2.ellipse(gray, (350, 270), (90, 140), 0, 0, 360, 20, 3)
        gray[257:283, 430:450] = 255
        self.assertEqual(reading.reading_regions(page, gray), [])

    def test_bright_region_without_dark_boundary_is_rejected(self):
        page, gray = fixture()
        gray[:] = 235
        cv2.ellipse(gray, (350, 270), (90, 140), 0, 0, 360, 255, -1)
        self.assertEqual(reading.reading_regions(page, gray), [])

    def test_unrecognized_art_inside_bubble_is_not_painted_over(self):
        page, gray = fixture()
        cv2.circle(gray, (385, 275), 25, 10, -1)
        self.assertEqual(reading.reading_regions(page, gray), [])

    def test_large_enclosed_panel_is_rejected(self):
        page, gray = fixture()
        gray[:] = 80
        cv2.ellipse(gray, (350, 470), (290, 420), 0, 0, 360, 255, -1)
        self.assertEqual(reading.reading_regions(page, gray), [])

    def test_joined_lobes_are_split_without_merging_dialogue_units(self):
        entries = [block("a", (395, 150, 425, 300)), block("b", (265, 300, 295, 420))]
        page, gray = fixture(entries)
        gray = np.full((1400, 1000), 80, np.uint8)
        page.update(width=1000, height=1400)
        cv2.ellipse(gray, (420, 220), (80, 150), 0, 0, 360, 255, -1)
        cv2.ellipse(gray, (280, 360), (80, 140), 0, 0, 360, 255, -1)
        regions = reading.reading_regions(page, gray)
        self.assertEqual([r["block_ids"] for r in regions], [["a"], ["b"]])
        masks = [polygon_mask(region["points"], gray.shape) for region in regions]
        self.assertFalse(np.any(masks[0] & masks[1]))

    def test_old_disjoint_fallback_is_preserved_but_overlapping_fallback_is_rejected(self):
        page, gray = fixture()
        page["blocks"].append(block("outside", (70, 700, 95, 800)))
        safe = rectangle("old", ["outside"], (65, 695, 100, 805))
        page["overlay_regions"] = [safe]
        regions = reading.reading_regions(page, gray)
        self.assertEqual({key: regions[-1][key] for key in safe}, safe)
        self.assertIn("background_png", regions[-1])
        page["overlay_regions"] = [rectangle("bad", ["outside"], (300, 200, 390, 300))]
        self.assertEqual(len(reading.reading_regions(page, gray)), 1)

    def test_wrong_image_size_and_duplicate_identifiers_fail_closed(self):
        page, gray = fixture()
        with self.assertRaises(ValueError):
            reading.reading_regions(page, gray[:900])
        page["blocks"].append(copy.deepcopy(page["blocks"][0]))
        with self.assertRaises(ValueError):
            reading.reading_regions(page, gray)

    def test_cli_emits_json_and_does_not_modify_cache_or_image(self):
        page, gray = fixture()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cache, image = root / "page.json", root / "page.png"
            cache.write_text(json.dumps(page), encoding="utf-8")
            cv2.imwrite(str(image), gray)
            before = cache.read_bytes(), image.read_bytes()
            result = subprocess.run([sys.executable, str(SCRIPT), "--image", str(image), "--cache", str(cache)],
                                    check=True, text=True, capture_output=True)
            data = json.loads(result.stdout)
            self.assertEqual(data["version"], reading.VERSION)
            self.assertEqual(data["regions"][0]["block_ids"], ["1"])
            self.assertEqual(before, (cache.read_bytes(), image.read_bytes()))


if __name__ == "__main__":
    unittest.main()
