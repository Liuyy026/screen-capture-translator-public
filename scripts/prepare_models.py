"""Download official OCR weights once; runtime uses only local files."""
import hashlib
import os
from pathlib import Path
import sys
import urllib.request

root = Path(__file__).resolve().parents[1]
external_models = os.environ.get('KOMA_MODEL_DIR')
models = Path(external_models) if external_models else root / '.models'
os.environ['HF_HUB_OFFLINE'] = '0'
os.environ.setdefault('HF_HOME', str(models / 'huggingface'))
sys.path.insert(0, str(root / 'backend'))
import worker
from huggingface_hub import snapshot_download

def verify_release_model(path, expected):
    if external_models:
        with path.open('rb') as stream:
            if hashlib.file_digest(stream, 'sha256').hexdigest() != expected:
                raise SystemExit(f'模型校验失败 · {path}')


dest = models / 'manga-ocr'
snapshot_download('kha-white/manga-ocr-base', local_dir=dest,
                  revision='aa6573bd10b0d446cbf622e29c3e084914df9741' if external_models else None,
                  allow_patterns=['*.json', '*.txt', 'pytorch_model.bin'])
verify_release_model(dest / 'pytorch_model.bin', 'c63e0bb5b3ff798c5991de18a8e0956c7ee6d1563aca6729029815eda6f5c2eb')
detector = models / 'comictextdetector.pt'
expected_detector = '1f90fa60aeeb1eb82e2ac1167a66bf139a8a61b8780acd351ead55268540cccb'
if not detector.exists():
    print('Downloading comic text detector…', flush=True)
    tmp = detector.with_suffix('.download')
    urllib.request.urlretrieve('https://github.com/zyddnys/manga-image-translator/releases/download/beta-0.2.1/comictextdetector.pt', tmp)
    verify_release_model(tmp, expected_detector)
    os.replace(tmp, detector)
else:
    verify_release_model(detector, expected_detector)
print('OCR 模型已下载，可离线运行。', flush=True)
