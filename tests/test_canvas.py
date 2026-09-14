import copy
import json
import os
import unittest
from unittest.mock import patch
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import numpy as np
from PySide6.QtCore import QPointF, Qt, QRectF
from PySide6.QtGui import QColor, QImage, QPixmap, QFont, QFontMetricsF, QFontDatabase, QRawFont, QPainter
from PySide6.QtWidgets import QApplication, QListWidgetItem

from windows_app import ImageCanvas, Reader


class CanvasTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])

    def setUp(self):
        self.canvas = ImageCanvas()
        self.canvas.setMinimumSize(1, 1)
        self.canvas.resize(264, 204)
        image = QPixmap(240, 180)
        image.fill(QColor(80, 80, 80))
        self.canvas.set_image(image)

    def tearDown(self):
        self.canvas.close()

    @staticmethod
    def page(text="Hello world", edited=False, error=""):
        return {
            "version": 2, "overlay_revision": 1,
            "blocks": [{"id": "1", "box": [80, 60, 160, 120],
                        "source": "source", "translation": text,
                        "edited": edited, "error": error, "overlay": "balloon"}],
            "overlay_regions": [{"id": "1", "block_ids": ["1"],
                                 "overlay": "balloon", "overlay_score": .95,
                                 "points": [[60, 90], [120, 35], [180, 90], [120, 145]],
                                 "text_box": [95, 68, 145, 112],
                                 "fill_color": [248, 248, 248], "vertical": False}],
        }

    def install(self, page):
        with patch("windows_app.ensure_page_overlays", return_value=False):
            self.canvas.set_page(page)

    def pixels(self, mode):
        self.canvas.set_mode(mode)
        image = self.canvas.grab().toImage().convertToFormat(QImage.Format_RGBA8888)
        return np.frombuffer(image.constBits(), dtype=np.uint8).reshape(
            image.height(), image.bytesPerLine())[:, :image.width() * 4].reshape(
                image.height(), image.width(), 4).copy()

    def test_translated_pixels_stay_inside_contour(self):
        self.install(self.page())
        before, after = self.pixels(0), self.pixels(2)
        changed = np.any(before != after, axis=2)
        self.assertTrue(changed.any())
        y, x = np.where(changed)
        # Source coordinates are offset by the canvas's 12-pixel margin.
        self.assertTrue(np.all(np.abs(x - 132) / 60 + np.abs(y - 102) / 55 <= 1.04))

    def test_partial_group_never_fills(self):
        page = self.page()
        page["blocks"].append({"id": "2", "box": [120, 60, 140, 110],
                               "source": "other", "translation": "", "overlay": "balloon"})
        page["overlay_regions"][0]["block_ids"].append("2")
        self.install(page)
        np.testing.assert_array_equal(self.pixels(0), self.pixels(2))

    def test_erase_polygons_preserve_nontext_art_inside_bubble(self):
        page = self.page()
        page["overlay_regions"][0]["erase_points"] = [
            [[95, 68], [145, 68], [145, 112], [95, 112]]]
        image = QPixmap(240, 180)
        image.fill(QColor(248, 248, 248))
        painter = QPainter(image)
        painter.fillRect(114, 48, 12, 4, QColor(0, 0, 0))
        painter.fillRect(100, 74, 3, 30, QColor(0, 0, 0))
        painter.end()
        self.canvas.set_image(image)
        self.install(page)
        before, after = self.pixels(0), self.pixels(2)
        # Decorative ink above the OCR rectangle is inside the balloon.
        np.testing.assert_array_equal(before[60:64, 126:138], after[60:64, 126:138])
        self.assertTrue(np.any(before[60:64, 126:138, :3] == 0))
        self.assertGreater(int(after[87, 113, 0]), 200)
        changed_y, changed_x = np.where(np.any(before != after, axis=2))
        self.assertGreater(len(changed_x), 0)
        self.assertTrue(np.all((changed_x >= 107) & (changed_x <= 157)))
        self.assertTrue(np.all((changed_y >= 80) & (changed_y <= 124)))

    def test_empty_erase_polygons_do_not_fall_back_to_full_fill(self):
        page = self.page()
        page["overlay_regions"][0]["erase_points"] = []
        self.install(page)
        before, after = self.pixels(0), self.pixels(2)
        np.testing.assert_array_equal(before[62:67, 127:137], after[62:67, 127:137])
        changed_y, changed_x = np.where(np.any(before != after, axis=2))
        self.assertGreater(len(changed_x), 0)
        self.assertTrue(np.all((changed_x >= 107) & (changed_x <= 157)))
        self.assertTrue(np.all((changed_y >= 80) & (changed_y <= 124)))

    def test_erasure_is_clipped_to_balloon_contour(self):
        page = self.page()
        page["overlay_regions"][0]["erase_points"] = [
            [[0, 0], [239, 0], [239, 179], [0, 179]]]
        self.install(page)
        before, after = self.pixels(0), self.pixels(2)
        y, x = np.where(np.any(before != after, axis=2))
        self.assertGreater(len(x), 0)
        self.assertTrue(np.all(np.abs(x - 132) / 60 + np.abs(y - 102) / 55 <= 1.04))

    def test_legacy_region_without_erase_points_keeps_fill(self):
        self.install(self.page())
        before, after = self.pixels(0), self.pixels(2)
        self.assertEqual(int(before[62, 132, 0]), 80)
        self.assertEqual(int(after[62, 132, 0]), 248)

    def test_erase_polygons_are_not_drawn_for_unusable_group(self):
        page = self.page(text="")
        page["overlay_regions"][0]["erase_points"] = [
            [[95, 68], [145, 68], [145, 112], [95, 112]]]
        self.install(page)
        np.testing.assert_array_equal(self.pixels(0), self.pixels(2))

    def test_manual_source_equal_and_stale_error_are_rendered(self):
        self.install(self.page("source", edited=True, error="old model error"))
        self.assertTrue(self.canvas._overlay_layouts)
        self.assertTrue(np.any(self.pixels(0) != self.pixels(2)))

    def test_machine_error_is_not_rendered(self):
        self.install(self.page(error="model error"))
        np.testing.assert_array_equal(self.pixels(0), self.pixels(2))

    def test_modes_pan_zoom_do_not_recompute(self):
        page = self.page()
        self.install(page)
        layout = copy.deepcopy(self.canvas._overlay_layouts)
        with patch("windows_app.ensure_page_overlays", side_effect=AssertionError("detector called")), \
                patch.object(self.canvas, "_build_layouts", side_effect=AssertionError("layout called")), \
                patch.object(self.canvas, "_overlay_font", side_effect=AssertionError("font discovery called")):
            self.pixels(0)
            self.pixels(1)
            self.canvas.zoom = 2.25
            self.canvas.pan = QPointF(-40, 32)
            self.pixels(2)
        self.assertEqual(layout, self.canvas._overlay_layouts)

    def test_changed_region_and_translation_replace_cached_metadata(self):
        page = self.page()
        self.install(page)
        page = copy.deepcopy(page)
        page["overlay_revision"] = 2
        page["overlay_regions"][0]["text_box"] = [92, 64, 148, 116]
        page["blocks"][0]["translation"] = "Updated English translation"
        self.install(page)
        layout = self.canvas._overlay_layouts["1"]
        self.assertEqual(layout["text"], "Updated English translation")
        self.assertEqual(layout["text_box"], (92, 64, 148, 116))

    def test_direct_set_blocks_rechecks_geometry_when_translation_becomes_ready(self):
        page = self.page(text="")
        blocks = page["blocks"]

        def classify(value, **_kwargs):
            value["overlay_regions"] = (copy.deepcopy(page["overlay_regions"])
                                        if value["blocks"][0]["translation"].strip() else [])

        with patch("windows_app.ensure_page_overlays", side_effect=classify) as detector:
            self.canvas.set_blocks(blocks)
            self.assertFalse(self.canvas._overlay_regions)
            blocks[0]["translation"] = "Hello world"
            self.canvas.set_blocks(blocks)
            self.assertEqual(detector.call_count, 2)
            self.assertTrue(self.canvas._overlay_layouts)
            blocks[0]["translation"] = "Revised translation"
            self.canvas.set_blocks(blocks)
            self.assertEqual(detector.call_count, 2)
            blocks[0]["error"] = "model error"
            self.canvas.set_blocks(blocks)
            self.assertEqual(detector.call_count, 3)
            self.assertFalse(self.canvas._overlay_layouts)
            blocks[0]["edited"] = True
            self.canvas.set_blocks(blocks)
            self.assertEqual(detector.call_count, 4)
            self.assertTrue(self.canvas._overlay_layouts)

    def test_narrow_english_stays_horizontal_and_wraps_long_word(self):
        page = self.page("extraordinarilylongword stays readable")
        page["overlay_regions"][0]["text_box"] = [103, 45, 137, 135]
        page["overlay_regions"][0]["vertical"] = True
        self.install(page)
        layout = self.canvas._overlay_layouts["1"]
        self.assertFalse(layout["vertical"])
        self.assertEqual(layout["text"], page["blocks"][0]["translation"])
        self.assertGreaterEqual(layout["font_px"], 8)
        self.assertEqual(layout["flags"], Qt.TextWrapAnywhere | Qt.AlignCenter)

    def test_unfit_translation_preserves_original(self):
        page = self.page("too much text " * 200)
        page["overlay_regions"][0]["text_box"] = [118, 85, 122, 95]
        self.install(page)
        self.assertFalse(self.canvas._overlay_layouts)
        np.testing.assert_array_equal(self.pixels(0), self.pixels(2))

    def test_chinese_glyphs_render_as_dark_pixels(self):
        page = self.page("欢迎回来，主人", edited=True)
        self.install(page)
        before, after = self.pixels(0), self.pixels(2)
        # The text is explicitly black and must produce dark pixels distinct
        # from both the light fill and the dark source image.
        dark = (after[:, :, 0] < 45) & (after[:, :, 1] < 45) & (after[:, :, 2] < 45)
        self.assertGreater(int(dark.sum()), 8)
        self.assertTrue(np.any(before != after))

    def test_vertical_dialogue_uses_dynamic_right_to_left_columns(self):
        page = self.page("欢迎主人回家这是今天的问候", edited=True)
        page["overlay_regions"][0]["vertical"] = True
        page["overlay_regions"][0]["text_box"] = [70, 20, 180, 160]
        self.install(page)
        layout = self.canvas._overlay_layouts["1"]
        self.assertTrue(layout["vertical"])
        self.assertGreaterEqual(len(layout["columns"]), 2)
        self.assertGreaterEqual(layout["font_px"], 8)
        self.assertEqual("".join(layout["columns"]), page["blocks"][0]["translation"])
        previous_x, previous_y = None, None
        for run in layout["runs"]:
            x, y, width, height = run["ink_box"]
            self.assertTrue(QRectF(*layout["draw_box"]).contains(QRectF(x, y, width, height)))
            if previous_y is not None and run["y"] < previous_y:
                self.assertLess(run["x"], previous_x)
            previous_x, previous_y = run["x"], run["y"]
        self.assertTrue(np.any(self.pixels(0) != self.pixels(2)))

    def test_vertical_column_count_adapts_to_available_width(self):
        text = "欢迎主人回家这是今天的问候"
        font = self.canvas._overlay_font()
        font.setPixelSize(20)
        tall = self.canvas._vertical_layout(text, font, QRectF(0, 0, 30, 320))
        wide = self.canvas._vertical_layout(text, font, QRectF(0, 0, 160, 100))
        self.assertEqual(len(tall["columns"]), 1)
        self.assertGreater(len(wide["columns"]), 2)

    def test_vertical_dash_uses_a_tall_glyph_without_changing_translation(self):
        text = "等等\u2014\u2014"
        font = self.canvas._overlay_font()
        font.setPixelSize(24)
        layout = self.canvas._vertical_layout(text, font, QRectF(0, 0, 100, 220))
        self.assertEqual("".join(layout["columns"]), text)
        dashes = [run for run in layout["runs"] if run["text"] == "\ufe31"]
        self.assertEqual(len(dashes), 2)
        self.assertTrue(all(run["ink_box"][3] > run["ink_box"][2] for run in dashes))

    def test_compact_punctuation_stays_with_previous_character(self):
        font = self.canvas._overlay_font()
        font.setPixelSize(20)
        layout = self.canvas._vertical_layout("主人，欢迎回来。今天也请关照！", font, QRectF(0, 0, 160, 100))
        for column in layout["columns"]:
            self.assertNotIn(column[0], "，。！")
        positions = layout["runs"]
        comma_index = next(i for i, run in enumerate(positions) if run["text"] == "，")
        previous = positions[comma_index - 1]["ink_box"]
        comma = positions[comma_index]["ink_box"]
        previous_center_y = previous[1] + previous[3] / 2
        comma_center_y = comma[1] + comma[3] / 2
        self.assertLess(comma_center_y - previous_center_y, 20)

    def test_existing_font_registration_provides_real_chinese_glyphs(self):
        with patch.object(ImageCanvas, "_font_template", None), \
                patch.object(ImageCanvas, "_font_loaded", False), \
                patch.object(QFontDatabase, "families", return_value=[]), \
                patch.object(QFontDatabase, "addApplicationFont", wraps=QFontDatabase.addApplicationFont) as register:
            font = ImageCanvas._overlay_font()
            font.setPixelSize(24)
            raw = QRawFont.fromFont(font)
            self.assertTrue(register.called)
            glyphs = raw.glyphIndexesForString("中文欢迎")
            self.assertEqual(len(glyphs), 4)
            self.assertTrue(all(index > 0 for index in glyphs))
            self.assertEqual(len(set(glyphs)), 4)
            self.assertTrue(all(not raw.pathForGlyph(index).isEmpty() for index in glyphs))

    def test_english_layout_metrics_match_cached_paint_flags(self):
        page = self.page("maid Cosplay? Welcome home", edited=True)
        page["overlay_regions"][0]["text_box"] = [68, 76, 172, 112]
        self.install(page)
        layout = self.canvas._overlay_layouts["1"]
        font = QFont()
        font.fromString(layout["font"])
        rect = QRectF(*layout["draw_box"])
        bounds = QFontMetricsF(font).boundingRect(rect, layout["flags"], layout["text"])
        self.assertLessEqual(bounds.width(), rect.width())
        self.assertLessEqual(bounds.height(), rect.height())
        self.assertEqual(layout["flags"], Qt.TextWordWrap | Qt.AlignCenter)
        self.assertNotIn("\ufffd", layout["text"])

    def test_mixed_cosplay_keeps_whole_word(self):
        page = self.page("maid 的 Cosplay？", edited=True)
        page["overlay_regions"][0]["text_box"] = [80, 30, 155, 140]
        page["overlay_regions"][0]["vertical"] = True
        self.install(page)
        layout = self.canvas._overlay_layouts["1"]
        font = QFont()
        font.fromString(layout["font"])
        self.assertFalse(layout["vertical"])
        self.assertEqual(layout["text"], "maid 的 Cosplay？")
        self.assertEqual(layout["flags"], Qt.TextWordWrap | Qt.AlignCenter)
        self.assertLessEqual(QFontMetricsF(font).horizontalAdvance("Cosplay"), layout["draw_box"][2])

    def test_wide_short_safe_area_uses_horizontal_chinese(self):
        page = self.page("欢迎回家", edited=True)
        page["overlay_regions"][0]["text_box"] = [40, 50, 180, 75]
        page["overlay_regions"][0]["vertical"] = True
        self.install(page)
        self.assertFalse(self.canvas._overlay_layouts["1"]["vertical"])

    def test_missing_group_member_never_erases_source(self):
        page = self.page()
        page["overlay_regions"][0]["block_ids"].append("missing")
        self.install(page)
        np.testing.assert_array_equal(self.pixels(0), self.pixels(2))

    def navigate_reader(self, reader, filename, translation="中文"):
        page = {"blocks": [{"id": "1", "source": "日文", "translation": translation, "error": ""}]}
        item = QListWidgetItem(filename)
        item.setData(Qt.UserRole, filename)
        with patch.object(reader, "cache_for_image", return_value=Path("test-cache.json")), \
                patch.object(Path, "exists", return_value=True), \
                patch.object(Path, "read_text", return_value=json.dumps(page)), \
                patch.object(reader.preview, "set_page"):
            reader.select_item(item, None)

    def test_first_translated_page_defaults_chinese_and_later_pages_keep_original(self):
        reader = Reader()
        self.addCleanup(reader.close)
        self.navigate_reader(reader, "00001.webp", translation="")
        self.assertEqual(reader.display.currentIndex(), 0)
        self.navigate_reader(reader, "00002.webp")
        self.assertEqual(reader.display.currentIndex(), 2)
        reader.display.setCurrentIndex(0)
        self.navigate_reader(reader, "00003.webp")
        self.assertEqual(reader.display.currentIndex(), 0)
        self.assertEqual(reader.preview.mode, 0)
        reader.display.setCurrentIndex(1)
        self.navigate_reader(reader, "00004.webp")
        self.assertEqual(reader.display.currentIndex(), 1)
        self.assertEqual(reader.preview.mode, 1)

    def test_explicit_original_before_first_page_is_respected(self):
        reader = Reader()
        self.addCleanup(reader.close)
        reader.display.activated.emit(0)
        self.navigate_reader(reader, "00001.webp")
        self.assertEqual(reader.display.currentIndex(), 0)
        self.assertEqual(reader.preview.mode, 0)


if __name__ == "__main__":
    unittest.main()
