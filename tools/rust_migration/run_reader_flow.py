#!/usr/bin/env python3
"""Build and run the real Reader in an isolated, offline macOS review bundle."""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    pages = json.loads(args.manifest.read_text())
    def hashes():
        for page in pages:
            for field, key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(page[field]).read_bytes()).hexdigest() == page[key]
    hashes()
    bundle = output/'Koma Rust Flow Review.app'
    binary = bundle/'Contents/MacOS/KomaRustFlowReview'
    binary.parent.mkdir(parents=True)
    with (output/'build.log').open('w') as log:
        subprocess.run(['xcrun','swiftc','-O','-target','arm64-apple-macosx14.0','-parse-as-library',
            '-D','KOMA_REVIEW_HARNESS',*[str(p) for p in sorted((root/'macos/Sources').glob('*.swift'))],
            str(root/'macos/tools/ReaderFlowReview.swift'),'-o',str(binary),'-framework','SwiftUI',
            '-framework','AppKit','-framework','CryptoKit','-framework','Security'],stdout=log,stderr=log,check=True)
    with (bundle/'Contents/Info.plist').open('wb') as handle:
        plistlib.dump(dict(CFBundleIdentifier='local.koma.rust.flow-review', CFBundleName='Koma Rust Flow Review',
            CFBundleExecutable='KomaRustFlowReview',CFBundlePackageType='APPL',LSMinimumSystemVersion='14.0',
            NSPrincipalClass='NSApplication',NSHighResolutionCapable=True,
            NSDesktopFolderUsageDescription='读取您授权用于 Rust 重构验收的漫画图片、项目运行环境和模型；检查结果保存到独立测试目录。'),handle)
    subprocess.run(['codesign','--force','--deep','--sign','-',str(bundle)],check=True)
    subprocess.run(['open','-W','-n',str(bundle),'--args',str(root),str(Path(pages[0]['image']).parent),str(output)],check=True,timeout=300)
    hashes()
    result = json.loads((output/'flow.json').read_text())
    print(json.dumps(result,ensure_ascii=False,indent=2))
    assert result['passed']


if __name__ == '__main__': main()
