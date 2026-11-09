#!/bin/bash
# Downloads only into this checkout. Python is setup-only, never a helper dependency.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEST="${KOMA_COMPONENT_ROOT:-$ROOT}/.runtime/super-resolution/realcugan"
mkdir -p "$DEST"
URL=https://github.com/nihui/realcugan-ncnn-vulkan/releases/download/20220728/realcugan-ncnn-vulkan-20220728-macos.zip
if [[ ! -f "$DEST/release.zip" ]]; then
  curl --fail --location --retry 3 --continue-at - "$URL" -o "$DEST/release.zip.part"
  mv "$DEST/release.zip.part" "$DEST/release.zip"
fi
# Archive digest pinned from the downloaded upstream 20220728 macOS release.
EXPECTED=0df908cbb98b480f85897221b96d37b0bdb70f82d81b2c7037fe950dd5c0fa33
ACTUAL=$(shasum -a 256 "$DEST/release.zip" | cut -d ' ' -f 1)
[[ "$ACTUAL" == "$EXPECTED" ]] || { echo 'Release checksum mismatch' >&2; exit 1; }
unzip -oq "$DEST/release.zip" -d "$DEST"
file "$DEST/realcugan-ncnn-vulkan-20220728-macos/realcugan-ncnn-vulkan"
cp "$ROOT/docs/licenses/RealCUGAN-model-MIT.txt" "$DEST/LICENSE-model-MIT.txt"
"${KOMA_SETUP_PYTHON:-python3}" "$ROOT/macos/scripts/realcugan_manifest.py" "$DEST" "$URL"
