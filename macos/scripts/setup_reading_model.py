#!/usr/bin/env python3
"""Explicitly install the optional local manga background-restoration model."""
from pathlib import Path
import hashlib
import os
import urllib.request

URL = "https://github.com/Sanster/models/releases/download/AnimeMangaInpainting/anime-manga-big-lama.pt"
# Published by IOPaint's AnimeLaMa adapter, not calculated from an untrusted download.
MD5 = "29f284f36a0a510bcacf39ecf4c4d54f"
TARGET = Path(__file__).resolve().parents[2] / ".models/anime-manga-big-lama.pt"


def checksum(path):
    digest = hashlib.md5()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    TARGET.parent.mkdir(parents=True, exist_ok=True)
    if TARGET.exists():
        if checksum(TARGET) != MD5:
            raise SystemExit(f"Existing model checksum differs; kept unchanged: {TARGET}")
        print(f"Verified existing model: {TARGET}")
        return
    partial = TARGET.with_suffix(TARGET.suffix + ".part")
    start = partial.stat().st_size if partial.exists() else 0
    request = urllib.request.Request(URL, headers={"Range": f"bytes={start}-"} if start else {})
    print("Downloading optional local background model (about 197 MiB)…", flush=True)
    with urllib.request.urlopen(request, timeout=60) as response:
        mode = "ab" if start and response.status == 206 else "wb"
        with partial.open(mode) as stream:
            while chunk := response.read(1024 * 1024):
                stream.write(chunk)
    if checksum(partial) != MD5:
        raise SystemExit(f"Download checksum mismatch; not installed: {partial}")
    os.replace(partial, TARGET)
    print(f"Installed and verified: {TARGET}")


if __name__ == "__main__":
    main()
