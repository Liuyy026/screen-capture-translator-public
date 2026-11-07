"""Service selection, secret handling, and asynchronous model discovery."""
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QObject, QProcess, QSettings, QTimer, Signal
from PySide6.QtTest import QTest
from PySide6.QtWidgets import QApplication, QLineEdit
from windows import windows_app


class FakeProcess(QObject):
    finished = Signal(int, object)
    errorOccurred = Signal(object)
    NotRunning, Starting, Running = QProcess.NotRunning, QProcess.Starting, QProcess.Running
    FailedToStart = QProcess.FailedToStart

    def __init__(self, parent=None):
        super().__init__(parent)
        self._state = self.NotRunning
        self.output = b""
        self.killed = False
        self.deleted = False

    def setProgram(self, value):
        self.program = value

    def setArguments(self, value):
        self.arguments = list(value)

    def setProcessEnvironment(self, value):
        self.environment = value

    def state(self):
        return self._state

    def start(self):
        self._state = self.Running

    def kill(self):
        self.killed = True
        self._state = self.NotRunning

    def deleteLater(self):
        self.deleted = True

    def readAllStandardOutput(self):
        return self.output

    def complete(self, result, code=0):
        self.output = (result if isinstance(result, str) else json.dumps(result)).encode("utf-8")
        self._state = self.NotRunning
        self.finished.emit(code, QProcess.NormalExit)


class ServiceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.settings_path = Path(self.temporary.name) / "settings.ini"
        self.settings = QSettings(str(self.settings_path), QSettings.IniFormat)
        self.reader = windows_app.Reader(settings=self.settings)
        self.reader.platform_root = Path(self.temporary.name)
        self.addCleanup(self.reader.close)

    def dialog(self, provider="omlx", model="chosen-model"):
        dialog = windows_app.ServiceSettingsDialog(self.reader)
        self.reader.service_dialog = dialog
        dialog.provider.setCurrentIndex(dialog.provider.findData(provider))
        dialog.model.setEditText(model)
        return dialog

    def test_service_defaults_and_credential_free_url_validation(self):
        self.assertEqual(self.reader.provider, "ollama")
        self.assertEqual(self.reader.model.currentText(), windows_app.DEFAULT_MODEL)
        self.assertEqual(self.reader.service_endpoints["omlx"], "http://127.0.0.1:8000/v1")
        self.assertTrue(windows_app.valid_service_url("https://example.org:8000/v1"))
        for url in ("", "file:///tmp/service", "http://host:bad", "http://u:secret@host/v1",
                    "http://host/v1?api_key=secret", "http://host/#secret", "http://host/v1?",
                    "http://host/v1#", "http://bad host", "http://host/pa th"):
            self.assertFalse(windows_app.valid_service_url(url), url)

    def test_changed_server_clears_key_before_discovery_and_normalizes_address(self):
        dialog = self.dialog()
        dialog.api_key.setText("old-server-key")
        dialog.endpoint.setText("http://NEW-HOST:8000/")
        self.assertEqual(dialog.api_key.text(), "")
        self.assertIn("已清空", dialog.message.text())
        self.assertEqual(dialog.config().base_url, "http://new-host:8000/v1")
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            self.assertFalse(dialog._probe.environment.contains("KOMA_TRANSLATION_API_KEY"))

    def test_equivalent_service_address_keeps_session_key(self):
        dialog = self.dialog()
        dialog.api_key.setText("same-server-key")
        dialog.endpoint.setText("http://127.0.0.1:8000/")
        self.assertEqual(dialog.api_key.text(), "same-server-key")

    def test_config_secret_is_only_in_child_environment(self):
        config = windows_app.ServiceConfig("omlx", "http://localhost:8000/v1", "session-secret")
        self.assertNotIn("session-secret", repr(config))
        self.assertNotIn("session-secret", config.arguments())
        self.assertEqual(config.environment().value("KOMA_TRANSLATION_API_KEY"), "session-secret")
        with patch.dict(os.environ, {"KOMA_TRANSLATION_API_KEY": "inherited-secret"}):
            self.assertFalse(windows_app.ServiceConfig("ollama", "http://localhost:11439")
                             .environment().contains("KOMA_TRANSLATION_API_KEY"))

    def test_worker_uses_frozen_arguments_environment_and_disables_service_changes(self):
        self.reader.provider = "omlx"
        self.reader._api_keys["omlx"] = "worker-secret"
        self.reader.service_models["omlx"] = "model-a"
        self.reader._update_model_choices()
        self.reader.current = Path(self.temporary.name) / "page.png"
        with patch.object(windows_app, "QProcess", FakeProcess):
            self.reader.run_worker("both")
            process = self.reader.process
            self.assertEqual(process.arguments[-4:], ["--provider", "omlx", "--base-url", "http://127.0.0.1:8000/v1"])
            self.assertEqual(process.arguments[process.arguments.index("--model") + 1], "model-a")
            self.assertEqual(process.environment.value("KOMA_TRANSLATION_API_KEY"), "worker-secret")
            self.assertNotIn("worker-secret", " ".join(process.arguments))
            self.assertFalse(self.reader.model.isEnabled())
            self.assertFalse(self.reader.service_button.isEnabled())
            self.reader._api_keys["omlx"] = "replacement-secret"
            self.reader.service_endpoints["omlx"] = "http://different:8000/v1"
            self.assertEqual(process.environment.value("KOMA_TRANSLATION_API_KEY"), "worker-secret")
            self.assertEqual(self.reader._worker_config.base_url, "http://127.0.0.1:8000/v1")
            self.reader.stop_worker()
            self.assertTrue(self.reader.model.isEnabled())
            self.assertTrue(process.killed)

    def test_late_worker_cannot_finish_replacement_worker(self):
        self.reader.current = Path(self.temporary.name) / "page.png"
        with patch.object(windows_app, "QProcess", FakeProcess):
            self.reader.run_worker("ocr")
            old = self.reader.process
            self.reader.run_worker("ocr")
            current = self.reader.process
            old.complete({"ok": False, "error": "obsolete"})
            self.assertIs(self.reader.process, current)
            self.assertTrue(self.reader.worker_active())
            self.assertFalse(self.reader.model.isEnabled())
            self.assertNotIn("obsolete", self.reader.status.text())
            current.errorOccurred.emit(QProcess.FailedToStart)
            self.assertFalse(self.reader.worker_active())
            self.assertTrue(self.reader.model.isEnabled())

    def test_nonsecret_settings_and_per_provider_models_survive_reopen(self):
        self.reader.model.setEditText("ollama-saved")
        dialog = self.dialog(model="omlx-saved")
        dialog.endpoint.setText("http://192.168.1.2:8000/v1/")
        dialog.api_key.setText("must-never-be-persisted")
        self.reader._apply_service_settings(dialog)
        self.settings.sync()
        contents = self.settings_path.read_text()
        self.assertNotIn("must-never-be-persisted", contents)
        self.assertFalse(any("key" in key.lower() or "token" in key.lower() for key in self.settings.allKeys()))
        reopened = windows_app.Reader(QSettings(str(self.settings_path), QSettings.IniFormat))
        self.addCleanup(reopened.close)
        self.assertEqual(reopened.provider, "omlx")
        self.assertEqual(reopened.service_endpoints["omlx"], "http://192.168.1.2:8000/v1")
        self.assertEqual(reopened.model.currentText(), "omlx-saved")
        self.assertEqual(reopened.service_models["ollama"], "ollama-saved")
        self.assertEqual(reopened._api_keys, {"ollama": "", "omlx": ""})

    def test_provider_switch_restores_separate_drafts(self):
        dialog = self.dialog(model="omlx-model")
        dialog.endpoint.setText("http://remote-host:8000/v1")
        dialog.api_key.setText("omlx-key")
        dialog.provider.setCurrentIndex(dialog.provider.findData("ollama"))
        self.assertEqual(dialog.endpoint.text(), "http://127.0.0.1:11439")
        self.assertEqual(dialog.api_key.text(), "")
        dialog.model.setEditText("ollama-model")
        dialog.provider.setCurrentIndex(dialog.provider.findData("omlx"))
        self.assertEqual(dialog.endpoint.text(), "http://remote-host:8000/v1")
        self.assertEqual(dialog.api_key.text(), "omlx-key")
        self.assertEqual(dialog.model.currentText(), "omlx-model")
        self.assertEqual(dialog.api_key.echoMode(), QLineEdit.Password)

    def test_discovery_contract_and_preserving_selected_model(self):
        dialog = self.dialog()
        dialog.api_key.setText("probe-secret")
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            process = dialog._probe
            self.assertEqual(process.program, sys.executable)
            self.assertEqual(process.arguments, ["-X", "utf8", str(windows_app.PROJECT_ROOT / "backend/worker.py"),
                                                 "--list-models", "--provider", "omlx", "--base-url", "http://127.0.0.1:8000/v1"])
            self.assertEqual(process.environment.value("KOMA_TRANSLATION_API_KEY"), "probe-secret")
            self.assertNotIn("probe-secret", " ".join(process.arguments))
            process.complete({"ok": True, "models": ["other", "chosen-model", "other"]})
            self.assertEqual(dialog.model.currentText(), "chosen-model")
            self.assertEqual(dialog.discovered_models, ["other", "chosen-model"])
            self.assertIn("连接成功", dialog.message.text())

    def test_discovery_retains_absent_model_and_explains_it(self):
        dialog = self.dialog()
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            dialog._probe.complete({"ok": True, "models": ["new-model"]})
        self.assertEqual(dialog.model.currentText(), "chosen-model")
        self.assertIn("列表未包含", dialog.message.text())

    def test_discovery_selects_first_model_when_none_selected(self):
        dialog = self.dialog(model="")
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            dialog._probe.complete({"ok": True, "models": ["first", "second"]})
        self.assertEqual(dialog.model.currentText(), "first")

    def test_discovery_error_redacts_key_and_keeps_selection(self):
        dialog = self.dialog()
        dialog.api_key.setText("probe-secret")
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            dialog._probe.complete({"ok": False, "error": "invalid key: probe-secret"}, code=1)
        self.assertNotIn("probe-secret", dialog.message.text())
        self.assertIn("连接失败", dialog.message.text())
        self.assertEqual(dialog.model.currentText(), "chosen-model")

    def test_invalid_response_does_not_replace_model_selection(self):
        dialog = self.dialog()
        with patch.object(windows_app, "QProcess", FakeProcess):
            for response in ("not JSON", {"ok": True, "models": [42]}, ["invalid shape"]):
                dialog.refresh_models()
                dialog._probe.complete(response)
                self.assertIn("格式无效", dialog.message.text())
                self.assertEqual(dialog.model.currentText(), "chosen-model")

    def test_changed_endpoint_ignores_stale_discovery(self):
        dialog = self.dialog()
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            obsolete = dialog._probe
            dialog.endpoint.setText("http://new-host:8000/v1")
            self.assertTrue(obsolete.killed)
            obsolete.complete({"ok": True, "models": ["obsolete-model"]})
        self.assertIsNone(dialog.discovered_models)
        self.assertEqual(dialog.model.currentText(), "chosen-model")
        self.assertIn("配置已更改", dialog.message.text())

    def test_late_discovery_cannot_replace_new_request(self):
        dialog = self.dialog()
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            obsolete = dialog._probe
            dialog.refresh_models()
            active = dialog._probe
            obsolete.complete({"ok": True, "models": ["obsolete-model"]})
            self.assertIs(dialog._probe, active)
            active.complete({"ok": True, "models": ["chosen-model", "current-model"]})
        self.assertEqual(dialog.discovered_models, ["chosen-model", "current-model"])

    def test_timeout_and_start_failure_leave_dialog_usable(self):
        dialog = self.dialog()
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            process = dialog._probe
            dialog._probe_timed_out()
            self.assertTrue(process.killed)
            self.assertIn("超时", dialog.message.text())
            self.assertIsNone(dialog._probe)
            dialog.refresh_models()
            dialog._probe.errorOccurred.emit(QProcess.FailedToStart)
        self.assertIsNone(dialog._probe)
        self.assertIn("无法启动", dialog.message.text())

    def test_empty_list_is_valid_and_manual_model_stays_available(self):
        dialog = self.dialog()
        with patch.object(windows_app, "QProcess", FakeProcess):
            dialog.refresh_models()
            dialog._probe.complete({"ok": True, "models": []})
        self.assertEqual(dialog.discovered_models, [])
        self.assertIn("尚未列出模型", dialog.message.text())
        self.assertEqual(dialog.model.currentText(), "chosen-model")

    def test_credentials_change_invalidates_cached_model_list(self):
        self.reader._service_model_lists[("omlx", "http://127.0.0.1:8000/v1")] = ["old-key-model"]
        dialog = self.dialog()
        dialog.api_key.setText("new-key")
        self.reader._apply_service_settings(dialog)
        self.assertEqual(self.reader.model.count(), 0)
        self.assertEqual(self.reader.model.currentText(), "chosen-model")

    def test_endpoint_change_removes_previous_servers_cached_models(self):
        original = "http://127.0.0.1:8000/v1"
        self.reader._service_model_lists[("omlx", original)] = ["old-server-model"]
        dialog = self.dialog()
        dialog.endpoint.setText("http://another-server:8000/v1")
        self.reader._apply_service_settings(dialog)
        self.assertNotIn(("omlx", original), self.reader._service_model_lists)
        self.assertEqual(self.reader.model.count(), 0)

    def test_actual_process_keeps_event_loop_responsive(self):
        # This subprocess only emits a fixture; it contacts no model service.
        root = Path(self.temporary.name)
        (root / "backend").mkdir()
        (root / "backend/worker.py").write_text(
            "import json,time\ntime.sleep(.2)\nprint(json.dumps({'ok':True,'models':['fixture-model']}))\n")
        self.reader.root = root
        dialog = self.dialog(model="")
        ticks = []
        QTimer.singleShot(20, lambda: ticks.append(True))
        dialog.refresh_models()
        deadline = time.monotonic() + 5
        while dialog._probe is not None and time.monotonic() < deadline:
            QTest.qWait(10)
        self.assertIsNone(dialog._probe, "fixture subprocess did not finish")
        self.assertTrue(ticks, "GUI timer was blocked during model discovery")
        self.assertEqual(dialog.model.currentText(), "fixture-model")


if __name__ == "__main__":
    unittest.main()
