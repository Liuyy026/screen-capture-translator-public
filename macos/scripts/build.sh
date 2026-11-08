#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
python3 ../tools/rust_migration/cargo.py build --release
mkdir -p .build
xcrun swiftc -O -target arm64-apple-macosx14.0 tools/MetalFXUpscaler.swift -o .build/koma-sr-metalfx -framework Metal -framework MetalFX -framework CoreImage -framework ImageIO
mkdir -p 'dist/Koma Rust 候选版.app/Contents/MacOS'
xcrun swiftc -O -target arm64-apple-macosx14.0 -parse-as-library Sources/*.swift -o 'dist/Koma Rust 候选版.app/Contents/MacOS/KomaRustReader' -framework SwiftUI -framework AppKit -framework CryptoKit -framework Security
python3 - <<'PY'
from pathlib import Path
import plistlib
import os
root = Path.cwd()
info = {'CFBundleName': 'Koma Rust 候选版', 'CFBundleDisplayName': 'Koma Rust 候选版',
        'CFBundleIdentifier': os.environ.get('KOMA_APP_ID', 'local.koma.reader.rust'), 'CFBundleVersion': '1',
        'CFBundleShortVersionString': '0.2.0-dev', 'CFBundleExecutable': 'KomaRustReader',
        'CFBundlePackageType': 'APPL', 'LSMinimumSystemVersion': '14.0',
        'NSHighResolutionCapable': True, 'NSPrincipalClass': 'NSApplication',
        'NSDesktopFolderUsageDescription': '读取桌面上的项目运行环境、模型和您选择的漫画图片。',
        'ProjectRoot': str(root.parent), 'CacheNamespace': os.environ.get('KOMA_CACHE_NAMESPACE', 'KomaReaderRust')}
with open(root / 'dist/Koma Rust 候选版.app/Contents/Info.plist', 'wb') as f:
    plistlib.dump(info, f)
PY
codesign --force --deep --sign - 'dist/Koma Rust 候选版.app'
print '已构建：dist/Koma Rust 候选版.app'
