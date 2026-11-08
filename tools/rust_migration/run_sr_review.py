#!/usr/bin/env python3
"""Capture real ContentView using private outputs of verify_sr_flow, with no model service calls."""
import argparse,fcntl,json,plistlib,shutil,subprocess
from pathlib import Path

def main():
 p=argparse.ArgumentParser();p.add_argument('--flow',type=Path,required=True);p.add_argument('--image',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
 p.add_argument('--runtime-subdir',default='daemon');p.add_argument('--engines',default='metalfx,off');p.add_argument('--axis',choices=['long_edge','height'],default='long_edge');p.add_argument('--lock',type=Path);a=p.parse_args()
 root=Path(__file__).resolve().parents[2];out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 for old,new in [('pages','pages'),('super-resolution-v1','super-resolution-v1'),('assets','reading-native-v1')]:shutil.copytree(a.flow/a.runtime_subdir/old,out/new)
 bundle=out/'Koma SR Review.app';binary=bundle/'Contents/MacOS/KomaSRReview';binary.parent.mkdir(parents=True)
 with (out/'build.log').open('w') as log:
  subprocess.run(['xcrun','swiftc','-O','-target','arm64-apple-macosx14.0','-parse-as-library','-D','KOMA_REVIEW_HARNESS',*[str(f) for f in sorted((root/'macos/Sources').glob('*.swift'))],str(root/'macos/tools/SuperResolutionReview.swift'),'-o',str(binary),'-framework','SwiftUI','-framework','AppKit','-framework','CryptoKit','-framework','Security'],check=True,stdout=log,stderr=log)
 with (bundle/'Contents/Info.plist').open('wb') as f:plistlib.dump(dict(CFBundleIdentifier='local.koma.rust.sr-window-review',CFBundleName='Koma SR Review',CFBundleExecutable='KomaSRReview',CFBundlePackageType='APPL',LSMinimumSystemVersion='14.0',NSPrincipalClass='NSApplication',NSHighResolutionCapable=True),f)
 subprocess.run(['codesign','--force','--deep','--sign','-',str(bundle)],check=True)
 with (a.lock or root.parent/'gpu-test.lock').open('a') as lock:
  fcntl.flock(lock,fcntl.LOCK_EX)
  subprocess.run(['open','-W','-n',str(bundle),'--args',str(root),str(a.image.resolve()),str(out),a.engines,a.axis],check=True,timeout=180)
 report=json.loads((out/'report.json').read_text());print(json.dumps(report));assert report['passed']
if __name__=='__main__':main()
