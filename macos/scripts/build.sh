#!/bin/zsh
set -euo pipefail
cd "${0:A:h:h}"
mkdir -p .build 'dist/Koma 漫画翻译.app/Contents/MacOS'
xcrun swiftc -O -target arm64-apple-macosx14.0 -parse-as-library Sources/*.swift -o 'dist/Koma 漫画翻译.app/Contents/MacOS/KomaReader' -framework SwiftUI -framework AppKit -framework CryptoKit -framework Security
python3 - <<'PY'
from pathlib import Path
import plistlib
root = Path.cwd()
info = {'CFBundleName': 'Koma 漫画翻译', 'CFBundleDisplayName': 'Koma 漫画翻译',
        'CFBundleIdentifier': 'local.koma.reader', 'CFBundleVersion': '1',
        'CFBundleShortVersionString': '0.1.0', 'CFBundleExecutable': 'KomaReader',
        'CFBundlePackageType': 'APPL', 'LSMinimumSystemVersion': '14.0',
        'NSHighResolutionCapable': True, 'NSPrincipalClass': 'NSApplication',
        'NSDesktopFolderUsageDescription': '读取桌面上的项目运行环境、模型和您选择的漫画图片。',
        'ProjectRoot': str(root.parent)}
with open(root / 'dist/Koma 漫画翻译.app/Contents/Info.plist', 'wb') as f:
    plistlib.dump(info, f)
PY
codesign --force --deep --sign - 'dist/Koma 漫画翻译.app'
print '已构建：dist/Koma 漫画翻译.app'
