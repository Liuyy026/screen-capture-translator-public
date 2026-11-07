import base64
import copy
import unittest

import cv2
import numpy as np

from macos.backend.reading_background import background_patch
from macos.backend.reading_overlay import reading_regions


def decode(region):
    return cv2.imdecode(np.frombuffer(base64.b64decode(region["background_png"]), np.uint8), cv2.IMREAD_UNCHANGED)


def tone(width, height):
    gray = np.full((height, width), 205, np.uint8)
    for y in range(0, height, 5):
        for x in range((y//5 % 2)*2, width, 5):
            gray[y:y+1, x:x+1] = 45
    return gray


def glyphs(gray, box):
    x, y, r, d = box
    for yy in range(y+9, d-10, 26):
        cv2.rectangle(gray, (x+12, yy-3), (x+28, yy+14), 255, -1)
        cv2.rectangle(gray, (x+16, yy), (x+23, yy+10), 10, -1)


def narration_fixture():
    gray = tone(700, 1000)
    blocks = []
    for identifier, x in enumerate([100, 215, 330], 1):
        box = [x, 300, x+80, 560]
        blocks.append({"id": str(identifier), "box": box, "source": "テキスト",
                       "translation": "中文旁白", "vertical": True, "reading_order": 4-identifier})
        glyphs(gray, box)
    return {"width": 700, "height": 1000, "blocks": blocks, "overlay_regions": []}, gray


class ReadingBackgroundTests(unittest.TestCase):
    def test_narration_groups_in_reading_order_without_changing_page_or_image(self):
        page, gray = narration_fixture()
        before, original = copy.deepcopy(page), gray.copy()
        regions = reading_regions(page, gray)
        self.assertEqual(len(regions), 1)
        self.assertEqual(regions[0]["overlay"], "narration")
        self.assertEqual(regions[0]["block_ids"], ["3", "2", "1"])
        self.assertFalse(regions[0]["vertical"])
        self.assertEqual(page, before)
        np.testing.assert_array_equal(gray, original)

    def test_patterned_patch_has_local_alpha_and_never_a_white_rectangle(self):
        page, gray = narration_fixture()
        region = reading_regions(page, gray)[0]
        patch = decode(region)
        x, y, r, d = region["background_box"]
        self.assertEqual(patch.shape, (d-y, r-x, 4))
        alpha = patch[:, :, 3] > 0
        self.assertTrue(alpha.any())
        self.assertLess(alpha.mean(), .45)
        restored = patch[:, :, 0][alpha]
        self.assertGreater(np.std(restored), 10)
        self.assertLess(np.mean(restored), 225)
        np.testing.assert_array_equal(patch[:, :, 0][~alpha], gray[y:d, x:r][~alpha])

    def test_aligned_text_across_panel_divider_is_not_grouped(self):
        page, gray = narration_fixture()
        cv2.line(gray, (196, 270), (196, 590), 0, 3)
        self.assertFalse(any(r["overlay"] == "narration" for r in reading_regions(page, gray)))

    def test_substantial_artwork_in_narration_gap_is_not_a_blank_canvas(self):
        page, gray = narration_fixture()
        cv2.rectangle(gray, (181, 320), (213, 545), 20, -1)
        self.assertFalse(any(r["overlay"] == "narration" for r in reading_regions(page, gray)))

    def test_incomplete_narration_suppresses_old_partial_white_fallback(self):
        page, gray = narration_fixture()
        page["blocks"][1]["translation"] = ""
        page["overlay_regions"] = [{"id": "old", "overlay": "rectangle", "block_ids": ["1"],
                                    "points": [[99, 299], [181, 299], [181, 561], [99, 561]],
                                    "text_box": [100, 300, 180, 560]}]
        self.assertEqual(reading_regions(page, gray), [])

    def test_gray_closed_balloon_is_restored_with_original_tone(self):
        gray = np.full((1000, 700), 230, np.uint8)
        mask = np.zeros_like(gray)
        cv2.ellipse(mask, (350, 270), (90, 140), 0, 0, 360, 1, -1)
        texture = tone(700, 1000)
        gray[mask > 0] = texture[mask > 0]
        cv2.ellipse(gray, (350, 270), (90, 140), 0, 0, 360, 15, 3)
        box = [320, 180, 380, 360]
        glyphs(gray, box)
        page = {"width": 700, "height": 1000, "blocks": [{"id": "gray", "box": box,
                 "source": "日本語", "translation": "中文", "vertical": True}], "overlay_regions": []}
        regions = reading_regions(page, gray)
        self.assertEqual([r["block_ids"] for r in regions], [["gray"]])
        patch = decode(regions[0])
        alpha = patch[:, :, 3] > 0
        self.assertLess(patch[:, :, 0][alpha].mean(), 230)
        x, y, r, d = regions[0]["background_box"]
        outline = cv2.dilate(mask, np.ones((3, 3), np.uint8))-cv2.erode(mask, np.ones((5, 5), np.uint8))
        self.assertFalse(np.any(alpha & (outline[y:d, x:r] > 0)))

    def test_nearby_source_residue_removed_while_boundary_and_far_art_stay(self):
        gray = np.full((400, 400), 255, np.uint8)
        mask = np.zeros_like(gray)
        cv2.ellipse(mask, (200, 200), (100, 140), 0, 0, 360, 1, -1)
        cv2.ellipse(gray, (200, 200), (100, 140), 0, 0, 360, 0, 3)
        cv2.rectangle(gray, (203, 145), (210, 155), 0, -1)
        cv2.rectangle(gray, (226, 180), (231, 190), 0, -1)  # just beyond OCR
        cv2.line(gray, (280, 150), (280, 240), 0, 3)  # unrelated and distant
        region = background_patch(gray, [[190, 140, 222, 240]], clip=(mask, (0, 0)))
        patch = decode(region)
        x, y, r, d = region["background_box"]
        self.assertGreater(patch[185-y, 228-x, 0], 240)
        self.assertEqual(patch[185-y, 228-x, 3], 255)
        if x <= 280 < r:
            self.assertEqual(patch[185-y, 280-x, 3], 0)


if __name__ == "__main__":
    unittest.main()
