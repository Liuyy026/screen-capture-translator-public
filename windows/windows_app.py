"""Minimal PySide6 reader for validating the shared OCR/translation backend on Windows."""
from __future__ import annotations

import json
import hashlib
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

from PySide6.QtCore import QLockFile, QProcess, QProcessEnvironment, QSettings, QStandardPaths, Qt, QTimer, QRectF, QPointF, Signal
from PySide6.QtGui import QImage, QPixmap, QFont, QPainter, QPainterPath, QPen, QColor, QWheelEvent, QMouseEvent, QFontMetrics, QFontDatabase
from PySide6.QtWidgets import (
    QApplication, QFileDialog, QLabel, QListWidget, QListWidgetItem,
    QMainWindow, QPushButton, QComboBox, QHBoxLayout, QVBoxLayout, QWidget,
    QPlainTextEdit, QDialog, QDialogButtonBox, QFormLayout, QLineEdit,
)

# Direct execution adds windows/ to sys.path; resolve the shared backend from
# the repository so launching from Explorer or another directory also works.
PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))

from backend.overlay import ensure_page_overlays
from backend.structure import ensure_dialogues
from backend.translation_service import DEFAULT_URLS, TranslationServiceError, normalize_base_url

IMAGE_EXTS = {".webp", ".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff"}
SERVICE_DEFAULTS = dict(DEFAULT_URLS)
SERVICE_NAMES = {"ollama": "Ollama", "omlx": "oMLX"}
DEFAULT_MODEL = "qwen3:4b-q6k"


@dataclass(frozen=True)
class ServiceConfig:
    provider: str
    base_url: str
    api_key: str = field(default="", repr=False)

    def arguments(self) -> list[str]:
        return ["--provider", self.provider, "--base-url", self.base_url]

    def environment(self) -> QProcessEnvironment:
        environment = QProcessEnvironment.systemEnvironment()
        environment.remove("KOMA_TRANSLATION_API_KEY")
        if self.api_key:
            environment.insert("KOMA_TRANSLATION_API_KEY", self.api_key)
        return environment


def valid_service_url(value: str, provider: str = "ollama") -> bool:
    """Keep credentials out of persisted URLs; the separate key is session only."""
    try:
        normalize_base_url(provider, value)
        return True
    except TranslationServiceError:
        return False


class ServiceSettingsDialog(QDialog):
    """Edit a service draft and discover models without blocking the reader."""

    def __init__(self, reader):
        super().__init__(reader)
        self.setWindowTitle("翻译服务设置")
        self.setMinimumWidth(560)
        self._reader = reader
        self._shown_provider = reader.provider
        self._drafts = {provider: {"base_url": reader.service_endpoints[provider],
                                   "api_key": reader._api_keys[provider],
                                   "model": reader.service_models[provider]}
                        for provider in SERVICE_DEFAULTS}
        self._generation = 0
        self._probe: QProcess | None = None
        self._processes: set[QProcess] = set()
        self.discovered_models: list[str] | None = None
        self.provider = QComboBox()
        for provider, name in SERVICE_NAMES.items():
            self.provider.addItem(name, provider)
        self.provider.setCurrentIndex(self.provider.findData(reader.provider))
        self.endpoint = QLineEdit()
        self.api_key = QLineEdit()
        self.api_key.setEchoMode(QLineEdit.Password)
        self.api_key.setPlaceholderText("无需认证时留空")
        self.model = QComboBox()
        self.model.setObjectName("translationModel")
        self.model.setEditable(True)
        self.model.setInsertPolicy(QComboBox.NoInsert)
        self.model.lineEdit().setPlaceholderText("获取模型后选择，也可输入模型 ID")
        self.refresh_button = QPushButton("测试连接并获取模型")
        self.message = QLabel("填写服务地址后，测试连接并选择模型。")
        self.message.setWordWrap(True)
        note = QLabel("API Key 仅在本次运行的内存中保留，关闭应用后需重新填写。\noMLX 模型由服务端管理，本应用不会启动或停止 oMLX 服务。")
        note.setWordWrap(True)
        note.setStyleSheet("color:#66747a; font-size:12px;")
        form = QFormLayout()
        form.addRow("服务", self.provider)
        form.addRow("服务地址", self.endpoint)
        form.addRow("API Key", self.api_key)
        form.addRow("模型", self.model)
        self.buttons = QDialogButtonBox(QDialogButtonBox.Save | QDialogButtonBox.Cancel)
        self.buttons.button(QDialogButtonBox.Save).setText("保存")
        self.buttons.button(QDialogButtonBox.Cancel).setText("取消")
        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(note)
        layout.addWidget(self.refresh_button)
        layout.addWidget(self.message)
        layout.addWidget(self.buttons)
        self._load_provider()
        self.provider.currentIndexChanged.connect(self._provider_changed)
        self.endpoint.textChanged.connect(self._endpoint_changed)
        self.api_key.textChanged.connect(self._connection_changed)
        self.refresh_button.clicked.connect(self.refresh_models)
        self.buttons.accepted.connect(self._save)
        self.buttons.rejected.connect(self.reject)
        self._timeout = QTimer(self)
        self._timeout.setSingleShot(True)
        self._timeout.timeout.connect(self._probe_timed_out)

    def _load_provider(self):
        draft = self._drafts[self._shown_provider]
        self.endpoint.setText(draft["base_url"])
        self.api_key.setText(draft["api_key"])
        self.model.clear()
        models = (self._reader._service_model_lists.get((self._shown_provider, draft["base_url"]))
                  if draft["api_key"] == self._reader._api_keys[self._shown_provider] else None)
        if models:
            self.model.addItems(models)
        self.model.setEditText(draft["model"])
        self.discovered_models = list(models) if models is not None else None
        self._key_endpoint = self.config().base_url

    def _provider_changed(self, _index):
        self._drafts[self._shown_provider] = {"base_url": self.endpoint.text().strip().rstrip("/"),
                                              "api_key": self.api_key.text(),
                                              "model": self.model.currentText().strip()}
        self._cancel_probe()
        self._shown_provider = self.provider.currentData()
        self.endpoint.blockSignals(True)
        self.api_key.blockSignals(True)
        self._load_provider()
        self.endpoint.blockSignals(False)
        self.api_key.blockSignals(False)
        self.message.setText("服务已切换，请测试连接并选择该服务的模型。")

    def _connection_changed(self, _text):
        self._cancel_probe()
        selected = self.model.currentText()
        self.model.clear()
        self.model.setEditText(selected)
        self.discovered_models = None
        self.message.setText("连接配置已更改，请重新测试并获取模型。")

    def _endpoint_changed(self, text):
        endpoint = self.config().base_url
        cleared = endpoint != self._key_endpoint and bool(self.api_key.text())
        if endpoint != self._key_endpoint:
            # A key supplied for one server must not travel to a new address.
            self.api_key.blockSignals(True)
            self.api_key.clear()
            self.api_key.blockSignals(False)
            self._key_endpoint = endpoint
        self._connection_changed(text)
        if cleared:
            self.message.setText("服务地址已更改，原 API Key 已清空。请为新地址填写密钥并重新测试。")

    def config(self) -> ServiceConfig:
        provider = self.provider.currentData()
        endpoint = self.endpoint.text().strip()
        try:
            endpoint = normalize_base_url(provider, endpoint)
        except TranslationServiceError:
            pass  # A draft may be incomplete while the user edits its address.
        return ServiceConfig(provider, endpoint, self.api_key.text())

    def _cancel_probe(self):
        self._generation += 1
        if hasattr(self, "_timeout"):
            self._timeout.stop()
        process, self._probe = self._probe, None
        if process is not None and process.state() != QProcess.NotRunning:
            process.kill()
        self.refresh_button.setText("测试连接并获取模型")

    def refresh_models(self):
        self._cancel_probe()
        config = self.config()
        if not valid_service_url(config.base_url):
            self.message.setText("请输入有效的 HTTP 或 HTTPS 服务地址；认证信息请填写在 API Key 中。")
            return
        generation = self._generation
        process = QProcess(self)
        self._processes.add(process)
        self._probe = process
        process.setProgram(sys.executable)
        process.setArguments(["-X", "utf8", str(self._reader.root / "backend" / "worker.py"),
                              "--list-models", *config.arguments()])
        process.setProcessEnvironment(config.environment())
        process.finished.connect(lambda code, status: self._probe_finished(process, generation, config, code, status))
        process.errorOccurred.connect(lambda error: self._probe_error(process, generation, error))
        self.message.setText("正在连接服务并获取模型…")
        self.refresh_button.setText("重新测试连接")
        self._timeout.start(30000)
        process.start()

    def _release_probe(self, process):
        self._processes.discard(process)
        process.deleteLater()

    def _probe_error(self, process, generation, error):
        if error != QProcess.FailedToStart:
            return
        if process is self._probe and generation == self._generation:
            self._timeout.stop()
            self._probe = None
            self.refresh_button.setText("测试连接并获取模型")
            self.message.setText("无法启动连接测试，请检查应用的 Python 环境。")
        self._release_probe(process)

    def _probe_timed_out(self):
        self._cancel_probe()
        self.message.setText("连接测试超时，请检查服务是否启动以及地址和认证信息。")

    def _probe_finished(self, process, generation, config, code, _status):
        output = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace").strip()
        current = process is self._probe and generation == self._generation and config == self.config()
        self._release_probe(process)
        if not current:
            return
        self._timeout.stop()
        self._probe = None
        self.refresh_button.setText("测试连接并获取模型")
        try:
            result = json.loads(output)
            if not isinstance(result, dict):
                raise ValueError
            if result.get("ok") is not True or code != 0:
                error = str(result.get("error") or f"测试进程退出（代码 {code}）")
                if config.api_key:
                    error = error.replace(config.api_key, "[已隐藏]")
                self.message.setText(f"连接失败：{error[:350]}")
                return
            models = result.get("models")
            if not isinstance(models, list) or any(not isinstance(model, str) or not model.strip() for model in models):
                raise ValueError
        except (json.JSONDecodeError, ValueError):
            self.message.setText("连接失败：服务返回的模型列表格式无效。")
            return
        selected = self.model.currentText().strip()
        models = list(dict.fromkeys(model.strip() for model in models))
        self.model.clear()
        self.model.addItems(models)
        self.model.setEditText(selected or (models[0] if models else ""))
        self.discovered_models = models
        if not models:
            self.message.setText("连接成功，服务尚未列出模型。请先在服务端准备模型，也可填写模型 ID。")
        elif selected and selected not in models:
            self.message.setText(f"连接成功，发现 {len(models)} 个模型。已保留当前模型 ID；列表未包含它，请核对或重新选择。")
        else:
            self.message.setText(f"连接成功，发现 {len(models)} 个模型。")

    def _save(self):
        if self._reader.worker_active():
            return
        if not valid_service_url(self.config().base_url):
            self.message.setText("请输入有效的 HTTP 或 HTTPS 服务地址；认证信息请填写在 API Key 中。")
            return
        self._cancel_probe()
        self.accept()

    def done(self, result):
        self._cancel_probe()
        super().done(result)


def natural_key(path: Path):
    return [int(part) if part.isdigit() else part.lower() for part in re.split(r"(\d+)", path.name)]


def usable_translation(source: str, translation: str) -> bool:
    """Return False for empty/original Japanese output shown in old caches."""
    source = (source or "").strip()
    translation = (translation or "").strip()
    if not translation:
        return False
    normalize = lambda value: re.sub(r"[\s\W_]", "", value, flags=re.UNICODE)
    if source and normalize(source) == normalize(translation):
        return False
    han = len(re.findall(r"[\u3400-\u9fff]", translation))
    kana = len(re.findall(r"[\u3040-\u30ff]", translation))
    return han > 0 or kana == 0


class ImageCanvas(QWidget):
    """Image viewer with real display modes and pointer-centered wheel zoom."""

    zoom_changed = Signal(int)
    _font_template: QFont | None = None
    _font_loaded = False

    def __init__(self):
        super().__init__()
        self.setMinimumSize(640, 500)
        self.setMouseTracking(True)
        self.setFocusPolicy(Qt.StrongFocus)
        self.pixmap = QPixmap()
        self.blocks: list[dict] = []
        self.mode = 0
        self.zoom = 1.0
        self.pan = QPointF(0, 0)
        self._drag_pos: QPointF | None = None
        self._gray = None
        self._overlay_regions: list[dict] = []
        self._overlay_signature: str | None = None
        self._overlay_content_signature: str | None = None
        self._overlay_metadata: list[dict] = []
        self._overlay_layouts: dict[str, dict] = {}
        self._layout_failures: dict[str, str] = {}
        self._layout_pending: dict[str, str] = {}
        self._panels: list[dict] = []
        self.setCursor(Qt.OpenHandCursor)
        self.setStyleSheet("background:#a7a9aa; border-radius:8px;")

    def set_image(self, pixmap: QPixmap):
        self._panels = []
        self.pixmap = pixmap
        self._gray = None
        self.blocks = []
        self._overlay_regions = []
        self._overlay_signature = None
        self._overlay_content_signature = None
        self._overlay_metadata = []
        self._overlay_layouts = {}
        if not pixmap.isNull():
            # Keep a small grayscale copy for balloon segmentation.  It is
            # computed once per page and avoids touching the image on every
            # repaint while zooming or panning.
            try:
                import numpy as np
                image = pixmap.toImage().convertToFormat(QImage.Format_Grayscale8)
                bits = image.constBits()
                self._gray = np.frombuffer(bits, dtype=np.uint8).reshape(
                    (image.height(), image.bytesPerLine()))[:, :image.width()].copy()
            except (ImportError, RuntimeError, ValueError):
                self._gray = None
        self.zoom = 1.0
        self.pan = QPointF(0, 0)
        self._drag_pos = None
        self.zoom_changed.emit(100)
        self.update()

    def set_blocks(self, blocks: list[dict], page: dict | None = None, *, _overlays_ready: bool = False):
        self.blocks = blocks
        signature = json.dumps({
            "blocks": [[block.get("id"), block.get("box"), block.get("source"),
                        block.get("vertical", False),
                        bool(str(block.get("translation") or "").strip()),
                        bool(block.get("error")), bool(block.get("edited"))] for block in blocks],
            "page_revision": page.get("overlay_revision") if page else None,
            "regions": page.get("overlay_regions") if page else None,
        }, ensure_ascii=False, sort_keys=True)
        content_signature = json.dumps([
            [block.get("id"), block.get("translation", ""), block.get("error", ""),
             block.get("edited", False), block.get("dialogue_translation"),
             block.get("dialogue_translation_complete"), block.get("dialogue_member_ids")] for block in blocks
        ], ensure_ascii=False, sort_keys=True)
        if signature != self._overlay_signature:
            page = page if page is not None else {"blocks": blocks}
            if not _overlays_ready:
                ensure_page_overlays(page, gray=self._gray)
            self._overlay_regions = page.get("overlay_regions", [])
            self._overlay_metadata = [
                {key: value for key, value in block.items() if key.startswith("overlay")}
                for block in blocks
            ]
            self._overlay_layouts = self._build_layouts()
            self._overlay_signature = signature
            self._overlay_content_signature = content_signature
        elif content_signature != self._overlay_content_signature:
            for block, metadata in zip(blocks, self._overlay_metadata):
                block.update(metadata)
            self._overlay_layouts = self._build_layouts()
            self._overlay_content_signature = content_signature
        self.update()

    def set_page(self, page: dict | None):
        """Install a cache page and compute/migrate overlay metadata once."""
        page = page or {"blocks": []}
        ensure_page_overlays(page, gray=self._gray)
        ensure_dialogues(page, gray=self._gray)
        self._panels = page.get('panels', [])
        self.set_blocks(page.get("blocks", []), page=page, _overlays_ready=True)

    @staticmethod
    def _group_translation(members: list[dict]) -> list[str] | None:
        """Return all usable group translations, or None for a partial group."""
        complete = {str(block.get("dialogue_translation")) for block in members
                    if block.get("dialogue_translation_complete") and block.get("dialogue_translation")}
        member_ids = {str(b.get("id")) for b in members}
        if (len(complete) == 1 and len({b.get("dialogue_id") for b in members}) == 1
                and all(b.get("dialogue_translation_complete") and not b.get("edited")
                        and not b.get("error") and b.get("overlay") != "skip"
                        and set(b.get("dialogue_member_ids", [])) == member_ids for b in members)):
            return [next(iter(complete))]
        values = []
        for block in members:
            if block.get("overlay") == "skip":
                return None
            text = (block.get("translation") or "").strip()
            edited = bool(block.get("edited"))
            if not text or (block.get("error") and not edited):
                return None
            if not edited and not usable_translation(block.get("source", ""), text):
                return None
            values.append(text)
        return values or None

    @staticmethod
    def _vertical_text(values: list[str]) -> tuple[str, bool]:
        """Choose direction without inserting breaks into English words."""
        text = "\n".join(values)
        cjk = len(re.findall(r"[\u2e80-\u9fff]", text))
        latin = len(re.findall(r"[A-Za-z0-9]", text))
        return text, cjk >= max(1, latin * 2)

    @staticmethod
    def _overlay_font() -> QFont:
        """Choose a Windows CJK font with a dependable fallback chain."""
        if ImageCanvas._font_template is not None:
            return QFont(ImageCanvas._font_template)
        registered_families = []
        # The offscreen Qt backend used by smoke tests does not always expose
        # the Windows font registry. Load the installed Noto face explicitly
        # in that case so Chinese glyphs never degrade to tofu boxes.
        if not ImageCanvas._font_loaded:
            font_paths = (
                Path("C:/Windows/Fonts/NotoSansSC-VF.ttf"),
                Path("C:/Windows/Fonts/NotoSerifSC-VF.ttf"),
                Path("C:/Windows/Fonts/msyh.ttc"),
                Path("C:/Windows/Fonts/Deng.ttf"),
                Path("C:/Windows/Fonts/simsun.ttc"),
                Path("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
            )
            for font_path in font_paths:
                if font_path.exists():
                    try:
                        font_id = QFontDatabase.addApplicationFont(str(font_path))
                        if font_id >= 0:
                            registered_families = QFontDatabase.applicationFontFamilies(font_id)
                        if registered_families:
                            break
                    except (OSError, RuntimeError):
                        pass
            ImageCanvas._font_loaded = True
        families = (
            "Microsoft YaHei UI", "Microsoft YaHei", "DengXian",
            "Noto Sans SC", "Noto Sans CJK SC", "SimSun", "sans-serif",
        )
        available = set(QFontDatabase.families()) | set(registered_families)
        family = next((name for name in families if name in available), families[-1])
        if ImageCanvas._font_template is None:
            ImageCanvas._font_template = QFont(family)
            ImageCanvas._font_template.setBold(True)
        return QFont(ImageCanvas._font_template)

    @staticmethod
    def _vertical_layout(text, font, rect):
        """Place upright glyph runs in compact, balanced right-to-left columns."""
        from PySide6.QtGui import QFontMetricsF
        import math

        metrics = QFontMetricsF(font)
        size = font.pixelSize()
        closing = "，。、；：！？!?….,;:）】》」』”’\u2014\u2015"
        tokens = re.findall(r"[A-Za-z0-9]+(?:['-][A-Za-z0-9]+)*|[^\s]", text)
        groups = []
        max_width = float(size)
        for token in tokens:
            display = token.translate(str.maketrans({"\u2014": "\ufe31", "\u2015": "\ufe31"}))
            ink = metrics.tightBoundingRect(display)
            max_width = max(max_width, ink.width())
            compact = all(char in closing for char in token)
            advance = max(ink.height() + size * .08, size * (.42 if compact else 1.05))
            item = (token, ink, advance, display)
            if compact and groups:
                groups[-1].append(item)
            else:
                groups.append([item])
        if not groups:
            return None
        heights = [sum(item[2] for item in group) for group in groups]
        total = sum(heights)
        gap = size * .22
        col_width = max_width + size * .08
        capacity = min(len(groups), int((rect.width() + gap) / (col_width + gap)))
        if capacity < 1 or max(heights) > rect.height():
            return None
        for count in range(max(1, math.ceil(total / rect.height())), capacity + 1):
            columns, cursor = [], 0
            remaining_height = total
            for index in range(count):
                remaining_columns = count - index
                target = remaining_height / remaining_columns
                start, used = cursor, 0.0
                while cursor < len(groups):
                    value = heights[cursor]
                    leave = len(groups) - cursor <= remaining_columns - 1
                    closer = used and abs(used - target) <= abs(used + value - target)
                    if used and (leave or used + value > rect.height() or (closer and remaining_columns > 1)):
                        break
                    used += value
                    cursor += 1
                columns.append((groups[start:cursor], used))
                remaining_height -= used
            if cursor != len(groups) or any(height > rect.height() for _, height in columns):
                continue
            used_width = count * col_width + (count - 1) * gap
            left = rect.left() + (rect.width() - used_width) / 2
            runs = []
            labels = []
            for index, (column, column_height) in enumerate(columns):
                center_x = left + (count - index - 1) * (col_width + gap) + col_width / 2
                y = rect.top() + (rect.height() - column_height) / 2
                labels.append("".join(item[0] for group in column for item in group))
                for group in column:
                    for token, ink, advance, display in group:
                        baseline_x = center_x - ink.center().x()
                        baseline_y = y + advance / 2 - ink.center().y()
                        runs.append({"text": display, "x": baseline_x, "y": baseline_y,
                                     "ink_box": (baseline_x + ink.left(), baseline_y + ink.top(),
                                                 ink.width(), ink.height())})
                        y += advance
            return {"runs": runs, "columns": labels}
        return None

    @staticmethod
    def _horizontal_layout(text, font_template, rect, maximum, minimum):
        from PySide6.QtGui import QFontMetricsF

        # Preserve whole English words while shrinking to a readable size.
        # Only an otherwise unrenderable long token enables character wrapping.
        for wrapping in (Qt.TextWordWrap, Qt.TextWrapAnywhere):
            flags = wrapping | Qt.AlignCenter
            for size in range(maximum, minimum - 1, -1):
                font = QFont(font_template)
                font.setPixelSize(size)
                bounds = QFontMetricsF(font).boundingRect(rect, flags, text)
                if bounds.width() <= rect.width() and bounds.height() <= rect.height():
                    return {"font_px": size, "font": font.toString(), "flags": flags,
                            "vertical": False, "columns": None, "runs": None}
        return None

    def _build_layouts(self) -> dict[str, dict]:
        self._layout_failures = {}
        self._layout_pending = {}
        if self.pixmap.isNull():
            return {}
        blocks_by_id = {str(block.get("id")): block for block in self.blocks}
        layouts: dict[str, dict] = {}
        font_template = self._overlay_font()
        for region in self._overlay_regions:
            if region.get("overlay") not in {"balloon", "rectangle"}:
                continue
            members = [blocks_by_id.get(str(block_id)) for block_id in region.get("block_ids", [])]
            if not members or any(block is None for block in members):
                self._layout_pending[str(region.get('id'))] = '区域成员不存在；保留原图，需检查分组'
                continue
            values = self._group_translation(members)
            if not values:
                self._layout_pending[str(region.get('id'))] = '成员译文不完整，或完整对白与区域成员不匹配；尚未尝试排版，保留原图'
                continue
            box = region.get("text_box") or []
            if len(box) != 4:
                self._layout_failures[str(region.get('id'))] = '排版区域坐标无效；保留原图'
                continue
            lx, ty, rx, by = [float(value) for value in box]
            width, height = rx - lx, by - ty
            if width <= 0 or height <= 0:
                self._layout_failures[str(region.get('id'))] = '排版区域尺寸无效；保留原图'
                continue
            rect = QRectF(lx, ty, width, height).adjusted(1, 1, -1, -1)
            text, cjk_vertical = self._vertical_text(values)
            preferred_vertical = (cjk_vertical and width < height * 1.2 and
                                  (bool(region.get("vertical")) or width < height * .7))
            minimum = max(8, min(12, round(self.pixmap.width() / 120)))
            # Short translations should retain page-sized dialogue type,
            # rather than expanding a single interjection to fill a balloon.
            maximum = min(64, max(24, round(self.pixmap.width() / 32)),
                          int(min(rect.width(), rect.height())))
            layout = None
            if preferred_vertical:
                for size in range(maximum, minimum - 1, -1):
                    font = QFont(font_template)
                    font.setPixelSize(size)
                    candidate = self._vertical_layout(text, font, rect)
                    if candidate:
                        layout = {**candidate, "font_px": size, "font": font.toString(),
                                  "vertical": True, "flags": Qt.TextSingleLine}
                        break
            horizontal = self._horizontal_layout(text, font_template, rect, maximum, minimum)
            if layout is None or (horizontal and horizontal["font_px"] > layout["font_px"] * 1.4):
                layout = horizontal
            if layout:
                layouts[str(region.get("id"))] = {
                    **layout, "text": text, "text_box": (lx, ty, rx, by),
                    "draw_box": (rect.x(), rect.y(), rect.width(), rect.height())}
            else:
                self._layout_failures[str(region.get('id'))] = '缩小字号及调整换行后仍无法容纳译文；保留原图'
        return layouts

    def set_mode(self, mode: int):
        self.mode = mode
        self.update()

    def wheelEvent(self, event: QWheelEvent):
        if self.pixmap.isNull():
            return
        steps = event.angleDelta().y() / 120.0
        if not steps:
            return
        old_rect = self._image_rect()
        if old_rect.isNull() or old_rect.width() <= 0:
            return
        cursor = event.position()
        image_x = (cursor.x() - old_rect.left()) / old_rect.width()
        image_y = (cursor.y() - old_rect.top()) / old_rect.height()
        self.zoom = max(0.25, min(4.0, self.zoom * (1.15 ** steps)))
        centered = self._centered_rect()
        # Preserve the image point below the cursor as the scale changes.
        self.pan = QPointF(
            cursor.x() - (centered.left() + image_x * centered.width()),
            cursor.y() - (centered.top() + image_y * centered.height()),
        )
        self.zoom_changed.emit(round(self.zoom * 100))
        self.update()
        event.accept()

    def _centered_rect(self) -> QRectF:
        if self.pixmap.isNull():
            return QRectF()
        size = self.pixmap.size()
        scale = min((self.width() - 24) / size.width(), (self.height() - 24) / size.height())
        scale *= self.zoom
        w, h = size.width() * scale, size.height() * scale
        return QRectF((self.width() - w) / 2, (self.height() - h) / 2, w, h)

    def _image_rect(self) -> QRectF:
        rect = self._centered_rect()
        if not rect.isNull():
            rect.translate(self.pan)
        return rect

    def mousePressEvent(self, event: QMouseEvent):
        if event.button() == Qt.LeftButton and not self.pixmap.isNull():
            self._drag_pos = event.position()
            self.setCursor(Qt.ClosedHandCursor)
            event.accept()
            return
        super().mousePressEvent(event)

    def mouseMoveEvent(self, event: QMouseEvent):
        if self._drag_pos is not None:
            current = event.position()
            self.pan += current - self._drag_pos
            self._drag_pos = current
            self.update()
            event.accept()
            return
        super().mouseMoveEvent(event)

    def mouseReleaseEvent(self, event: QMouseEvent):
        if event.button() == Qt.LeftButton and self._drag_pos is not None:
            self._drag_pos = None
            self.setCursor(Qt.ArrowCursor)
            event.accept()
            return
        super().mouseReleaseEvent(event)

    def paintEvent(self, _event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.SmoothPixmapTransform)
        painter.fillRect(self.rect(), QColor("#a7a9aa"))
        if self.pixmap.isNull():
            painter.setPen(QColor("#f4f7f8"))
            painter.drawText(self.rect(), Qt.AlignCenter, "打开漫画文件夹\n选择一页开始阅读")
            return
        rect = self._image_rect()
        painter.drawPixmap(rect.toRect(), self.pixmap)
        if self.mode == 0:
            return
        sx, sy = rect.width() / self.pixmap.width(), rect.height() / self.pixmap.height()
        for block in self.blocks:
            box = block.get("box") or []
            if len(box) != 4:
                continue
            x1, y1, x2, y2 = box
            target = QRectF(rect.left() + x1 * sx, rect.top() + y1 * sy,
                            max(2, (x2 - x1) * sx), max(2, (y2 - y1) * sy))
            if self.mode == 1:
                outline = "#e38b24" if block.get("layout_fallback") == "block" else "#00a7b8"
                painter.setPen(QPen(QColor(outline), max(2, int(2.2 * self.zoom))))
                painter.setBrush(QColor(255, 210, 74, 72))
                painter.drawRoundedRect(target, 5, 5)
                badge_w = min(34, max(24, target.width() * 0.24))
                badge_h = min(28, max(22, target.height() * 0.20))
                painter.setBrush(QColor("#007f91"))
                painter.setPen(Qt.NoPen)
                painter.drawRoundedRect(QRectF(target.left(), target.top(), badge_w, badge_h), 5, 5)
                badge_font = QFont(painter.font()); badge_font.setBold(True)
                badge_font.setPixelSize(max(11, int(badge_h * 0.58))); painter.setFont(badge_font)
                painter.setPen(QColor("#ffffff"))
                painter.drawText(QRectF(target.left(), target.top(), badge_w, badge_h), Qt.AlignCenter, str(block.get("id", "?")))
        if self.mode == 1:
            # Show detected bubble membership and direction on the inspection view.
            painter.setBrush(Qt.NoBrush)
            for region in self._overlay_regions:
                points = region.get("points") or []
                if len(points) < 3:
                    continue
                path = QPainterPath()
                path.moveTo(rect.left() + points[0][0] * sx, rect.top() + points[0][1] * sy)
                for px, py in points[1:]:
                    path.lineTo(rect.left() + px * sx, rect.top() + py * sy)
                path.closeSubpath()
                painter.setPen(QPen(QColor("#7b3fb5"), max(1, int(1.5 * self.zoom)), Qt.DashLine))
                painter.drawPath(path)
                ids = ",".join(str(x) for x in region.get("block_ids", []))
                label = f"气泡 {region.get('id', '?')} [{ids}] {'竖' if region.get('vertical') else '横'}"
                bounds = path.boundingRect()
                painter.setPen(QColor("#4a226d"))
                painter.drawText(QRectF(bounds.left(), max(rect.top(), bounds.top() - 18), max(70, bounds.width()), 18), label)
        if self.mode == 1:
            painter.setBrush(Qt.NoBrush)
            for panel in self._panels:
                x1, y1, x2, y2 = panel['box']
                bounds = QRectF(rect.left()+x1*sx, rect.top()+y1*sy, (x2-x1)*sx, (y2-y1)*sy)
                painter.setPen(QPen(QColor('#148046'), 2, Qt.DashLine))
                painter.drawRect(bounds.adjusted(1, 1, -1, -1))
                painter.drawText(bounds.adjusted(4, 2, -4, -2), Qt.AlignTop | Qt.AlignLeft,
                                 f"分镜候选 {panel['id']} · 顺序 {panel['reading_order']}")
        if self.mode == 2:
            # Fonts and wrapped lines are expressed in source-image units.
            # Scaling the painter preserves that layout at every zoom level.
            painter.save()
            painter.translate(rect.left(), rect.top())
            painter.scale(sx, sy)
            for region in self._overlay_regions:
                if region.get("overlay") not in {"balloon", "rectangle"}:
                    continue
                layout = self._overlay_layouts.get(str(region.get("id")))
                if not layout:
                    continue
                points = [QPointF(float(point[0]), float(point[1]))
                          for point in region.get("points", [])]
                if len(points) < 3:
                    continue
                bubble_path = QPainterPath(); bubble_path.moveTo(points[0])
                for point in points[1:]: bubble_path.lineTo(point)
                bubble_path.closeSubpath()
                color = region.get("fill_color", [255, 255, 255])
                fill = QColor(*(list(color)[:3]), 255)
                painter.save(); painter.setClipPath(bubble_path)
                text_rect = QRectF(*layout["draw_box"])
                font = QFont()
                font.fromString(layout["font"])
                if "erase_points" not in region:
                    painter.fillPath(bubble_path, fill)
                else:
                    for polygon in region.get("erase_points") or []:
                        if len(polygon) < 3:
                            continue
                        erase_path = QPainterPath()
                        erase_path.moveTo(QPointF(*polygon[0]))
                        for point in polygon[1:]:
                            erase_path.lineTo(QPointF(*point))
                        erase_path.closeSubpath()
                        painter.fillPath(erase_path, fill)
                painter.setClipRect(text_rect, Qt.IntersectClip)
                painter.setFont(font); painter.setPen(QColor("#111111"))
                if layout["runs"]:
                    for run in layout["runs"]:
                        painter.drawText(QPointF(run["x"], run["y"]), run["text"])
                else:
                    painter.drawText(text_rect, layout["flags"], layout["text"])
                painter.restore()
            # A partial dialogue is intentionally left untouched here. The
            # normal per-block fallback is selected by the caller only when a
            # trusted region exists; painting an OCR rectangle would risk
            # covering artwork and overflowing its bounds.
            painter.restore()


class Reader(QMainWindow):
    def __init__(self, settings: QSettings | None = None):
        super().__init__()
        self.setWindowTitle("Koma · 本地漫画翻译")
        self.resize(1440, 900)
        self.setMinimumSize(1100, 700)
        self.root = PROJECT_ROOT
        self.platform_root = Path(__file__).resolve().parent
        self.images: list[Path] = []
        self.process: QProcess | None = None
        self._worker_image: Path | None = None
        self._worker_generation = 0
        self._worker_config: ServiceConfig | None = None
        self.current: Path | None = None
        self.cache_path: Path | None = None
        self._display_initialized = False
        self.settings = settings if settings is not None else QSettings("KomaReader", "WindowsReader")
        saved_provider = self.settings.value("translation/provider", "ollama")
        self.provider = saved_provider if saved_provider in SERVICE_DEFAULTS else "ollama"
        self.service_endpoints = {}
        self.service_models = {}
        for provider, endpoint in SERVICE_DEFAULTS.items():
            saved_endpoint = str(self.settings.value(f"translation/{provider}/base_url", endpoint))
            self.service_endpoints[provider] = (normalize_base_url(provider, saved_endpoint)
                                                if valid_service_url(saved_endpoint, provider) else endpoint)
            self.service_models[provider] = str(self.settings.value(
                f"translation/{provider}/model", DEFAULT_MODEL if provider == "ollama" else ""))
        self._api_keys = {provider: "" for provider in SERVICE_DEFAULTS}
        self._service_model_lists: dict[tuple[str, str], list[str]] = {}
        self.service_dialog: ServiceSettingsDialog | None = None

        self.list = QListWidget()
        self.list.setMinimumWidth(240)
        self.list.setObjectName("pageList")
        self.list.currentItemChanged.connect(self.select_item)
        self.preview = ImageCanvas()
        self.status = QLabel("●  本地引擎就绪")
        self.status.setObjectName("status")
        self.results = QPlainTextEdit()
        self.results.setReadOnly(True)
        self.results.setPlaceholderText("识别结果和译文会显示在这里")
        self.results.setMinimumWidth(390)
        self.results.setObjectName("results")
        self.model = QComboBox()
        self.model.setObjectName("translationModel")
        self.model.setEditable(True)
        self.model.setInsertPolicy(QComboBox.NoInsert)
        self.model.setMinimumWidth(260)
        self.model.lineEdit().setPlaceholderText("在服务设置中获取模型，或输入模型 ID")
        self._update_model_choices()
        self.model.currentIndexChanged.connect(self._remember_model)
        self.model.lineEdit().editingFinished.connect(self._remember_model)
        self.service_button = QPushButton()
        self.service_button.clicked.connect(self.open_service_settings)
        self._update_service_button()
        open_btn = QPushButton("📁  打开文件夹")
        open_btn.setObjectName("primary")
        open_btn.clicked.connect(self.open_folder)
        run_btn = QPushButton("✦  识别并翻译")
        run_btn.setObjectName("primary")
        run_btn.clicked.connect(lambda: self.run_worker("both"))
        ocr_btn = QPushButton("只识别 OCR")
        ocr_btn.clicked.connect(lambda: self.run_worker("ocr"))
        stop_btn = QPushButton("停止")
        stop_btn.setObjectName("danger")
        stop_btn.clicked.connect(self.stop_worker)
        self.display = QComboBox()
        self.display.setObjectName("displayMode")
        self.display.addItems(["原图", "文字框", "中文"])
        self.display.setCurrentIndex(0)
        self.display.currentIndexChanged.connect(self.update_display)
        self.display.activated.connect(self._remember_display_choice)
        self.zoom = QLabel("100% · 滚轮缩放 · 拖动平移")
        self.zoom.setObjectName("zoomLabel")
        self.preview.zoom_changed.connect(lambda value: self.zoom.setText(f"{value}% · 滚轮缩放 · 拖动平移"))
        self.folder_label = QLabel("本地漫画")
        self.folder_label.setObjectName("folderTitle")
        bar = QHBoxLayout()
        bar.setContentsMargins(18, 14, 18, 10)
        bar.addWidget(self.folder_label)
        bar.addStretch(1)
        for widget in (open_btn, run_btn, ocr_btn, stop_btn):
            bar.addWidget(widget)
        bar.addWidget(QLabel("显示")); bar.addWidget(self.display); bar.addWidget(self.zoom)
        service_bar = QHBoxLayout()
        service_bar.setContentsMargins(18, 0, 18, 8)
        for widget in (QLabel("翻译服务"), self.service_button, QLabel("模型"), self.model):
            service_bar.addWidget(widget)
        service_bar.addStretch(1)
        main = QHBoxLayout()
        main.setContentsMargins(14, 0, 14, 10)
        main.setSpacing(12)
        main.addWidget(self.list)
        main.addWidget(self.preview, 1)
        main.addWidget(self.results)
        layout = QVBoxLayout()
        layout.addLayout(bar)
        layout.addLayout(service_bar)
        layout.addLayout(main, 1)
        layout.addWidget(self.status)
        layout.setContentsMargins(0, 0, 0, 6)
        host = QWidget()
        host.setLayout(layout)
        self.setCentralWidget(host)
        self.setStyleSheet("""
            QMainWindow, QWidget { background:#edf2f5; color:#263238; font-family:'Segoe UI'; font-size:14px; }
            QPushButton { background:#ffffff; border:1px solid #d7e0e5; border-radius:7px; padding:8px 12px; }
            QPushButton:hover { background:#e7f8fb; border-color:#18b9c9; }
            QPushButton#primary { background:#16b9c8; color:white; border:0; font-weight:600; }
            QPushButton#primary:hover { background:#0ea6b4; }
            QPushButton#danger { color:#c74343; }
            QComboBox { background:white; border:1px solid #d7e0e5; border-radius:7px; padding:7px 10px; min-width:90px; }
            QComboBox#translationModel { min-width:300px; }
            QLineEdit { background:white; border:1px solid #d7e0e5; border-radius:7px; padding:7px 10px; }
            QComboBox#displayMode { min-width:84px; color:#263238; selection-background-color:#16b9c8; }
            QComboBox#displayMode:hover { border-color:#16b9c8; }
            QListWidget#pageList { background:#dce6ec; border:1px solid #c8d6dd; border-radius:8px; padding:8px; }
            QListWidget#pageList::item { padding:10px 8px; border-radius:7px; }
            QListWidget#pageList::item:selected { background:#2f96f3; color:white; }
            QPlainTextEdit#results { background:#fbfdfd; border:1px solid #c8d6dd; border-radius:8px; padding:12px; }
            QLabel#folderTitle { font-size:20px; font-weight:700; color:#13b6c7; }
            QLabel#status { padding:0 18px; color:#22a879; font-size:13px; }
            QLabel#zoomLabel { color:#66747a; min-width:110px; }
        """)
        self.poll = QTimer(self)
        self.poll.timeout.connect(self.read_status)

    def service_config(self) -> ServiceConfig:
        return ServiceConfig(self.provider, self.service_endpoints[self.provider], self._api_keys[self.provider])

    def worker_active(self) -> bool:
        return self._worker_image is not None

    def _set_worker_busy(self, busy):
        self.model.setEnabled(not busy)
        self.service_button.setEnabled(not busy)
        if self.service_dialog is not None:
            if busy:
                self.service_dialog._cancel_probe()
            self.service_dialog.setEnabled(not busy)

    def _update_service_button(self):
        self.service_button.setText(f"{SERVICE_NAMES[self.provider]} · 服务设置…")
        self.service_button.setToolTip(self.service_endpoints[self.provider])

    def _update_model_choices(self):
        self.model.blockSignals(True)
        self.model.clear()
        models = self._service_model_lists.get((self.provider, self.service_endpoints[self.provider]), [])
        self.model.addItems(models)
        self.model.setEditText(self.service_models[self.provider])
        self.model.blockSignals(False)

    def _remember_model(self, *_args):
        if self.worker_active():
            return
        model = self.model.currentText().strip()
        self.service_models[self.provider] = model
        self.settings.setValue(f"translation/{self.provider}/model", model)

    def open_service_settings(self):
        if self.worker_active():
            return
        if self.service_dialog is not None and self.service_dialog.isVisible():
            self.service_dialog.raise_()
            self.service_dialog.activateWindow()
            return
        self._remember_model()
        if self.service_dialog is not None:
            self.service_dialog.deleteLater()
        dialog = ServiceSettingsDialog(self)
        self.service_dialog = dialog
        dialog.accepted.connect(lambda: self._apply_service_settings(dialog))
        # Window-modal keeps an unfinished service draft from racing a new
        # translation; QDialog.open() and its QProcess leave the event loop free.
        dialog.open()

    def _apply_service_settings(self, dialog):
        if self.worker_active():
            return
        config = dialog.config()
        self._remember_model()
        if (self._api_keys[config.provider] != config.api_key
                or self.service_endpoints[config.provider] != config.base_url):
            self._service_model_lists.pop((config.provider, self.service_endpoints[config.provider]), None)
            self._service_model_lists.pop((config.provider, config.base_url), None)
        self.provider = config.provider
        self.service_endpoints[config.provider] = config.base_url
        self._api_keys[config.provider] = config.api_key
        self.service_models[config.provider] = dialog.model.currentText().strip()
        self.settings.setValue("translation/provider", config.provider)
        self.settings.setValue(f"translation/{config.provider}/base_url", config.base_url)
        self.settings.setValue(f"translation/{config.provider}/model", self.service_models[config.provider])
        self.settings.sync()
        if dialog.discovered_models is not None:
            self._service_model_lists[(config.provider, config.base_url)] = list(dialog.discovered_models)
        self._update_model_choices()
        self._update_service_button()

    def open_folder(self):
        folder = QFileDialog.getExistingDirectory(self, "选择包含图片的文件夹")
        if not folder:
            return
        self.images = sorted((p for p in Path(folder).iterdir() if p.suffix.lower() in IMAGE_EXTS), key=natural_key)
        self.list.clear()
        for index, path in enumerate(self.images, start=1):
            item = QListWidgetItem(f"{index:02d}   {path.name}")
            item.setData(Qt.UserRole, str(path))
            self.list.addItem(item)
        if self.images:
            self.folder_label.setText(Path(folder).name or "本地漫画")
            self.list.setCurrentRow(0)
            self.status.setText(f"已加载 {len(self.images)} 张图片")
        else:
            self.preview.set_image(QPixmap())
            self.preview.set_blocks([])
            self.status.setText("文件夹内没有支持的图片")

    def select_item(self, item, _previous):
        if not item:
            return
        # A page switch invalidates any in-flight worker result.  The process
        # may still emit finished after being killed, so worker_finished also
        # checks the captured image before applying its cache.
        if self.process and self.process.state() != QProcess.NotRunning:
            self.stop_worker()
        self.current = Path(item.data(Qt.UserRole))
        pixmap = QPixmap(str(self.current))
        self.preview.set_image(pixmap)
        self.cache_path = self.cache_for_image(self.current)
        self.results.clear()
        self.preview.set_page({"blocks": []})
        self.load_cache()

    def update_display(self, index):
        self._display_initialized = True
        labels = ["原图", "文字框", "中文覆盖"]
        self.preview.set_mode(index)
        if self.current:
            self.status.setText(f"●  {labels[index]}")

    def _remember_display_choice(self, _index):
        # Activating the already-selected original mode is still explicit.
        self._display_initialized = True

    def cache_for_image(self, image: Path) -> Path:
        digest = hashlib.sha256(image.read_bytes()).hexdigest()
        if sys.platform == "win32":
            base = Path(__import__("os").environ.get("LOCALAPPDATA", Path.home() / "AppData/Local"))
            return base / "KomaReader" / "pages" / f"{digest}.json"
        if sys.platform == "darwin":
            return Path.home() / "Library/Application Support/KomaReader/pages" / f"{digest}.json"
        base = Path(__import__("os").environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))
        return base / "KomaReader/pages" / f"{digest}.json"

    def run_worker(self, operation):
        if not self.current:
            self.status.setText("请先打开文件夹并选择图片")
            return
        if operation != "ocr" and not self.model.currentText().strip():
            self.status.setText("请先在服务设置中获取模型，或输入模型 ID")
            return
        self.stop_worker()
        self._remember_model()
        config = self.service_config()
        status_path = self.platform_root / ".runtime" / "windows-status.json"
        status_path.parent.mkdir(exist_ok=True)
        process = QProcess(self)
        self.process = process
        # The running GUI already has a working interpreter. Reuse it for OCR.
        process.setProgram(sys.executable)
        args = ["-X", "utf8", str(self.root / "backend" / "worker.py"), "--image", str(self.current),
                "--operation", operation, "--model", self.model.currentText().strip(), "--status", str(status_path),
                *config.arguments()]
        process.setArguments(args)
        process.setProcessEnvironment(config.environment())
        self._worker_image = self.current
        self._worker_config = config
        generation = self._worker_generation
        process.finished.connect(lambda code, status: self.worker_finished(code, status, process, generation))
        process.errorOccurred.connect(lambda error: self._worker_error(process, generation, error))
        self._set_worker_busy(True)
        self.poll.start(250)
        self.status.setText(f"正在处理：{self.current.name}")
        process.start()

    def _worker_error(self, process, generation, error):
        if error == QProcess.FailedToStart and process is self.process and generation == self._worker_generation:
            self.poll.stop()
            self._worker_image = None
            self._worker_config = None
            self._set_worker_busy(False)
            self.status.setText("无法启动处理进程，请检查应用的 Python 环境。")

    def read_status(self):
        path = self.platform_root / ".runtime" / "windows-status.json"
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            if data.get("message"):
                self.status.setText(data["message"])
        except (OSError, json.JSONDecodeError):
            pass

    def worker_finished(self, code, _status, process=None, generation=None):
        process = process if process is not None else self.process
        if process is None or process is not self.process or (generation is not None and generation != self._worker_generation):
            return
        self.poll.stop()
        output = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace").strip()
        worker_image = self._worker_image
        config = self._worker_config
        self._worker_image = None
        self._worker_config = None
        self._set_worker_busy(False)
        if worker_image is None or self.current != worker_image:
            return
        self.select_item(self.list.currentItem(), None)
        try:
            result = json.loads(output)
            if result.get("ok"):
                self.cache_path = Path(result.get("cache", ""))
                self.load_cache()
                self.status.setText("完成")
            else:
                error = str(result.get("error", "未知错误"))
                if config and config.api_key:
                    error = error.replace(config.api_key, "[已隐藏]")
                self.status.setText(f"失败：{error}")
        except (json.JSONDecodeError, AttributeError):
            self.status.setText(f"进程退出（代码 {code}）")

    def load_cache(self):
        if not self.cache_path or not self.cache_path.exists():
            return
        try:
            page = json.loads(self.cache_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            self.results.setPlainText(f"读取缓存失败：{exc}")
            return
        ensure_dialogues(page)
        self.preview.set_page(page)
        # GUI migration stays in memory. The worker owns cache writes, so a
        # page opened while it is writing cannot be truncated by the reader.
        blocks = page.get("blocks", [])
        translated_count = sum(1 for block in blocks if block.get("translation") and not block.get("error"))
        dialogues = page.get("dialogues", [])
        diagnostics = page.get("diagnostics", {})
        grouping_diag = diagnostics.get("grouping", {})
        layout_diag = diagnostics.get("layout", {})
        failure_diag = diagnostics.get("failures", {})
        panel_order = page.get("structure", {}).get("panel_order", [])
        lines = ["对白对照", f"{len(blocks)} 处文字 · {len(dialogues)} 个对白单元 · {translated_count} 处已翻译", "",
                 f"图片：{self.current.name if self.current else ''}",
                 f"模型：{page.get('model') or '仅 OCR'}",
                 f"结构诊断：{grouping_diag.get('dialogues', len(dialogues))} 单元 / 未分组 {grouping_diag.get('unassigned', 0)}",
                 f"排版诊断：可信候选区域 {layout_diag.get('safe_regions', 0)} / 已排版 {len(self.preview._overlay_layouts)} / 前置条件未满足 {len(self.preview._layout_pending)}",
                 f"失败归因：OCR 数据异常 {failure_diag.get('ocr', 0)} 块 · 分组 {failure_diag.get('grouping', 0)} · 翻译 {failure_diag.get('translation', 0)} 单元 · 排版 {len(self.preview._layout_failures)} 区域",
                 f"尚待翻译或校验：{diagnostics.get('translation', {}).get('pending_dialogues', 0)} 单元",
                 f"人工复核候选：分组 {failure_diag.get('grouping_review', grouping_diag.get('review_candidates', 0))}",
                 f"分镜顺序：{', '.join(map(str, panel_order)) if panel_order else '未确认'}", ""]
        if dialogues:
            lines.append("对白单元（按阅读顺序）")
            for dialogue in dialogues:
                direction = "竖排" if dialogue.get("vertical") else "横排"
                source = str(dialogue.get("source", "")).replace("\n", " / ")
                translation = str(dialogue.get("translation", "")).replace("\n", " / ") or "暂无完整译文"
                lines.append(f"{dialogue.get('id', '?')} · 分镜 {dialogue.get('panel_id') or '未确认'} · {direction} · 块 {','.join(map(str, dialogue.get('block_ids', [])))}")
                lines.append(f"    {source} → {translation}")
                if dialogue.get('quality_warnings'):
                    lines.append(f"    语义提示：{'；'.join(dialogue['quality_warnings'])}")
                if dialogue.get('mapping_status') == 'whole_only':
                    lines.append('    回填：仅整句可用，尚无逐块拆分；成员完全匹配的安全区域可整体排版')
            lines.append("")
        for region_id, reason in self.preview._layout_failures.items():
            lines.append(f'区域 {region_id} 排版：{reason}')
        for region_id, reason in self.preview._layout_pending.items():
            lines.append(f'区域 {region_id} 待处理：{reason}')
        for block in blocks:
            source = block.get("source", "").strip()
            raw_translation = block.get("translation", "").strip()
            if block.get("overlay") == "skip":
                detail = block.get("overlay_reason") or "低影响文字"
                translation = (raw_translation + "  " if raw_translation else "") + "（原图保留：" + detail + "）"
            elif block.get("overlay") == "uncertain":
                detail = block.get("overlay_reason") or "边界不确定"
                translation = (raw_translation + "  " if raw_translation else "") + "（暂不覆盖：" + detail + "）"
            else:
                translation = raw_translation if usable_translation(source, raw_translation) else "（暂无有效中文译文）"
            error = f"  [{block['error']}]" if block.get("error") else ""
            warnings = f"  [语义提示：{'；'.join(block.get('quality_warnings', []))}]" if block.get('quality_warnings') else ""
            dialogue = block.get("dialogue_id", "?")
            order = block.get("dialogue_order", "?")
            kind = {"dialogue": "对白候选", "caption": "旁白候选", "noise": "空文本/人工排除",
                    "page_text_candidate": "页面文字候选", "sfx_candidate": "拟声/短语候选",
                    "utterance_candidate": "标点语气候选"}.get(block.get("text_kind"), "")
            lines.append(f"#{block.get('id', '?')}  [{dialogue}:{order}]  {source} 〈{kind}〉")
            lines.append(f"    → {translation}{error}{warnings}")
        if not blocks:
            lines.append("没有识别到文字")
        self.results.setPlainText("\n".join(lines))
        # Apply the initial translated view once; subsequent page loads and
        # worker completion respect the reader's selected display mode.
        if translated_count and not self._display_initialized:
            self._display_initialized = True
            self.display.setCurrentIndex(2)

    def stop_worker(self):
        self._worker_generation += 1
        self.poll.stop()
        self._worker_image = None
        self._worker_config = None
        self._set_worker_busy(False)
        process, self.process = self.process, None
        if process is not None:
            if process.state() != QProcess.NotRunning:
                # Let finished arrive asynchronously. Its captured generation
                # prevents it from applying a result to a replacement worker.
                process.finished.connect(process.deleteLater)
                process.kill()
            else:
                process.deleteLater()

    def closeEvent(self, event):
        if self.service_dialog is not None:
            self.service_dialog.reject()
        self.stop_worker()
        super().closeEvent(event)


if __name__ == "__main__":
    app = QApplication(sys.argv)
    # Keep double-clicks from opening competing readers over the same cache.
    # A named Windows mutex works across the CPU and CUDA Python environments.
    lock = None
    if sys.platform == "win32":
        import ctypes
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        lock = kernel32.CreateMutexW(None, False, "Local\\KomaReaderWindowsApp")
        if not lock or ctypes.get_last_error() == 183:  # ERROR_ALREADY_EXISTS
            if lock:
                kernel32.CloseHandle(lock)
            print("Koma Manga Translator is already running.", file=sys.stderr)
            sys.exit(0)
    else:
        lock_dir = Path(QStandardPaths.writableLocation(QStandardPaths.TempLocation))
        lock = QLockFile(str(lock_dir / "koma-reader-windows.lock"))
        if not lock.tryLock(100):
            print("Koma Manga Translator is already running.", file=sys.stderr)
            sys.exit(0)
    window = Reader()
    window.show()
    sys.exit(app.exec())
