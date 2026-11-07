"""Regression checks for the client/shared-backend directory boundary."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QSettings
from PySide6.QtWidgets import QApplication
from windows import windows_app


class ProjectPathTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])

    def test_direct_client_import_outside_repository_finds_shared_backend(self):
        client = Path(windows_app.__file__).resolve()
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, "-I", "-c",
                 "import runpy,sys; m=runpy.run_path(sys.argv[1]); "
                 "print(m['PROJECT_ROOT'])", str(client)],
                cwd=directory, capture_output=True, text=True, check=True,
            )
        self.assertEqual(Path(result.stdout.strip()), client.parents[1])

    def test_worker_uses_shared_script_and_platform_status_file(self):
        with tempfile.TemporaryDirectory() as directory:
            reader = windows_app.Reader(QSettings(str(Path(directory) / "settings.ini"), QSettings.IniFormat))
            self.addCleanup(reader.close)
            reader.platform_root = Path(directory)
            reader.current = windows_app.PROJECT_ROOT / "samples/01.webp"
            with patch.object(windows_app, "QProcess") as process_type:
                process = process_type.return_value
                reader.run_worker("ocr")
                process.setProgram.assert_called_once_with(sys.executable)
                args = process.setArguments.call_args.args[0]
                self.assertEqual(Path(args[2]), windows_app.PROJECT_ROOT / "backend/worker.py")
                status = Path(args[args.index("--status") + 1])
                self.assertEqual(status, Path(directory) / ".runtime/windows-status.json")
                self.assertTrue(status.parent.is_dir())
                status.write_text(json.dumps({"message": "test progress"}))
                reader.read_status()
                self.assertEqual(reader.status.text(), "test progress")
                process.start.assert_called_once()
                reader.process = None
                reader.poll.stop()
