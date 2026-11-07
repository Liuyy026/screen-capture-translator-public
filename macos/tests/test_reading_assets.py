import tempfile
from pathlib import Path
import unittest

from macos.backend.reading_assets import asset_key, atomic_write


class ReadingAssetsTests(unittest.TestCase):
    def test_translation_image_and_model_changes_invalidate_only_derived_key(self):
        with tempfile.TemporaryDirectory() as directory:
            weights = Path(directory) / "model.pt"
            original = asset_key(b"original", b"translation", [weights])
            self.assertEqual(original, asset_key(b"original", b"translation", [weights]))
            self.assertNotEqual(original, asset_key(b"edited image", b"translation", [weights]))
            self.assertNotEqual(original, asset_key(b"original", b"edited translation", [weights]))
            weights.write_bytes(b"installed")
            self.assertNotEqual(original, asset_key(b"original", b"translation", [weights]))

    def test_atomic_assets_are_complete_and_leave_no_temporary_files(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "derived" / "page.json"
            atomic_write(path, b'{"version": 1}')
            atomic_write(path, b'{"version": 3}')
            self.assertEqual(path.read_bytes(), b'{"version": 3}')
            self.assertEqual(list(path.parent.iterdir()), [path])
