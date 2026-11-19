"""Download official OCR weights once; runtime uses only local files."""
import hashlib
import os
from pathlib import Path
import urllib.request


REPO = "kha-white/manga-ocr-base"
RELEASE_REVISION = "aa6573bd10b0d446cbf622e29c3e084914df9741"
RELEASE_FILES = {
    "config.json": "8c0e395de8fa699daaac21aee33a4ba9bd1309cfbff03147813d2a025f39f349",
    "preprocessor_config.json": "af4eb4d79cf61b47010fc0bc9352ee967579c417423b4917188d809b7e048948",
    "pytorch_model.bin": "c63e0bb5b3ff798c5991de18a8e0956c7ee6d1563aca6729029815eda6f5c2eb",
    "special_tokens_map.json": "303df45a03609e4ead04bc3dc1536d0ab19b5358db685b6f3da123d05ec200e3",
    "tokenizer_config.json": "d775ad1deac162dc56b84e9b8638f95ed8a1f263d0f56f4f40834e26e205e266",
    "vocab.txt": "344fbb6b8bf18c57839e924e2c9365434697e0227fac00b88bb4899b78aa594d",
}
DETECTOR_URL = "https://github.com/zyddnys/manga-image-translator/releases/download/beta-0.2.1/comictextdetector.pt"
DETECTOR_SHA256 = "1f90fa60aeeb1eb82e2ac1167a66bf139a8a61b8780acd351ead55268540cccb"


def verify_model(path, expected):
    with path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != expected:
            raise SystemExit(f"模型校验失败，文件已保留 · {path}")


def download_manga_ocr(dest, release):
    from huggingface_hub import snapshot_download

    if release:
        snapshot_download(REPO, local_dir=dest, revision=RELEASE_REVISION,
                          allow_patterns=list(RELEASE_FILES))
    else:
        snapshot_download(REPO, local_dir=dest,
                          allow_patterns=["*.json", "*.txt", "pytorch_model.bin"])


def main():
    external_models = os.environ.get("KOMA_MODEL_DIR")
    release = bool(external_models)
    models = Path(external_models) if release else Path(__file__).resolve().parents[1] / ".models"
    os.environ.setdefault("HF_HOME", str(models / "huggingface"))
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    os.environ.setdefault("HF_HUB_DISABLE_XET", "1")
    os.environ["HF_HUB_OFFLINE"] = "0"
    dest = models / "manga-ocr"
    if not release or not all((dest / name).is_file() for name in RELEASE_FILES):
        download_manga_ocr(dest, release)
    if release:
        for name, digest in RELEASE_FILES.items():
            verify_model(dest / name, digest)

    detector = models / "comictextdetector.pt"
    if not detector.exists():
        print("Downloading comic text detector…", flush=True)
        temporary = detector.with_suffix(".download")
        urllib.request.urlretrieve(DETECTOR_URL, temporary)
        if release:
            verify_model(temporary, DETECTOR_SHA256)
        os.replace(temporary, detector)
    elif release:
        verify_model(detector, DETECTOR_SHA256)
    print("OCR 模型已下载，可离线运行。", flush=True)


if __name__ == "__main__":
    main()
