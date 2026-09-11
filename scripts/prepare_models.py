"""Download official OCR weights once; runtime uses only local files."""
import os
from pathlib import Path
import sys
import urllib.request

root = Path(__file__).resolve().parents[1]
os.environ['HF_HUB_OFFLINE'] = '0'
sys.path.insert(0, str(root / 'backend'))
import worker
from huggingface_hub import snapshot_download

dest = root / '.models/manga-ocr'
snapshot_download('kha-white/manga-ocr-base', local_dir=dest,
                  allow_patterns=['*.json', '*.txt', 'pytorch_model.bin'])
detector = root / '.models/comictextdetector.pt'
if not detector.exists():
    print('Downloading comic text detector…', flush=True)
    tmp = detector.with_suffix('.download')
    urllib.request.urlretrieve('https://github.com/zyddnys/manga-image-translator/releases/download/beta-0.2.1/comictextdetector.pt', tmp)
    os.replace(tmp, detector)
print('OCR 模型已下载，可离线运行。', flush=True)
