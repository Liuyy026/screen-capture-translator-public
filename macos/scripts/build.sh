#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
APP_BUNDLE='dist/Koma Rust 候选版.app'
python3 ../tools/rust_migration/cargo.py build --release
mkdir -p .build
xcrun swiftc -O -target arm64-apple-macosx14.0 tools/MetalFXUpscaler.swift -o .build/koma-sr-metalfx -framework Metal -framework MetalFX -framework CoreImage -framework ImageIO
bash scripts/build_realcugan.sh
mkdir -p "$APP_BUNDLE/Contents/"{MacOS,Resources}
xcrun swiftc -O -target arm64-apple-macosx14.0 -parse-as-library Sources/*.swift -o "$APP_BUNDLE/Contents/MacOS/KomaRustReader" -framework SwiftUI -framework AppKit -framework CryptoKit -framework Security
cp Resources/AppIcon.icns "$APP_BUNDLE/Contents/Resources/"
python3 - "$APP_BUNDLE/Contents/Info.plist" <<'PY'
from pathlib import Path
import plistlib
import os
import sys
root = Path.cwd()
info = {'CFBundleName': 'Koma Rust 候选版', 'CFBundleDisplayName': 'Koma Rust 候选版',
        'CFBundleIdentifier': os.environ.get('KOMA_APP_ID', 'local.koma.reader.rust'), 'CFBundleVersion': '1',
        'CFBundleShortVersionString': '0.2.0-dev', 'CFBundleExecutable': 'KomaRustReader',
        'CFBundleIconFile': 'AppIcon.icns',
        'CFBundlePackageType': 'APPL', 'LSMinimumSystemVersion': '14.0',
        'NSHighResolutionCapable': True, 'NSPrincipalClass': 'NSApplication',
        'NSDesktopFolderUsageDescription': '读取桌面上的项目运行环境、模型和您选择的漫画图片。',
        'ProjectRoot': str(root.parent), 'CacheNamespace': os.environ.get('KOMA_CACHE_NAMESPACE', 'KomaReaderRust')}
with open(sys.argv[1], 'wb') as f:
    plistlib.dump(info, f)
PY
codesign --force --deep --sign - "$APP_BUNDLE"
# Mark the bundle as updated so Finder can refresh its icon.
touch "$APP_BUNDLE"
print "已构建：$APP_BUNDLE"
