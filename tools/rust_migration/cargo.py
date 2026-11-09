#!/usr/bin/env python3
"""Run Cargo against the task-local native OpenCV build."""
import os
from pathlib import Path
import shutil
import subprocess
import sys


def main():
    root = Path(__file__).resolve().parents[2]
    native_root = Path(os.environ.get('KOMA_NATIVE_DEPS', root/'.runtime/native-deps')).resolve()
    native = native_root/'install'
    if not (native/'include/opencv4/opencv2/core.hpp').is_file():
        raise SystemExit('Build task-local OpenCV with build_opencv.py first')
    cargo = shutil.which('cargo') or str(Path.home()/'.cargo/bin/cargo')
    env = os.environ.copy()
    env['KOMA_NATIVE_DEPS'] = str(native_root)
    env['OPENCV_LINK_LIBS'] = 'opencv_core,opencv_imgproc,opencv_imgcodecs,opencv_photo'
    env['OPENCV_LINK_PATHS'] = str(native/'lib')
    env['OPENCV_INCLUDE_PATHS'] = str(native/'include/opencv4')
    if sys.platform == 'darwin':
        clang = Path('/Library/Developer/CommandLineTools/usr/lib')
        if (clang/'libclang.dylib').is_file():
            env.setdefault('LIBCLANG_PATH', str(clang))
    if sys.platform != 'win32':
        flags = ['-C', 'link-arg=-Wl,-rpath,'+str(native/'lib')]
        previous = env.get('CARGO_ENCODED_RUSTFLAGS', '')
        env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(([previous] if previous else [])+flags)
    arguments = sys.argv[1:] or ['build']
    command = [cargo, arguments[0], '--manifest-path', str(root/'core/Cargo.toml'), *arguments[1:]]
    raise SystemExit(subprocess.call(command, cwd=root, env=env))


if __name__ == '__main__':
    main()
