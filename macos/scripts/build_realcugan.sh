#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$ROOT/macos/.build"
xcrun swiftc -O -target arm64-apple-macosx14.0 "$ROOT/macos/tools/RealCUGANUpscaler.swift" -o "$ROOT/macos/.build/koma-sr-realcugan" -framework AppKit -framework CoreImage
"$ROOT/macos/.build/koma-sr-realcugan" --capabilities
