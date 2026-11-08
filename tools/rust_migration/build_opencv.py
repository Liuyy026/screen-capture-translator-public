#!/usr/bin/env python3
"""Build the baseline-matching OpenCV locally; never install into system paths."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import urllib.request

VERSION = '4.11.0'
URL = f'https://github.com/opencv/opencv/archive/refs/tags/{VERSION}.tar.gz'
ARCHIVE_SHA256 = '9a7c11f924eff5f8d8070e297b322ee68b9227e003fd600d4b8122198091665f'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--cmake', type=Path, required=True)
    parser.add_argument('--ninja', type=Path, required=True)
    parser.add_argument('--jobs', type=int, default=8)
    args = parser.parse_args()
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory/f'opencv-{VERSION}.tar.gz'
    if not archive.exists():
        temporary = archive.with_suffix('.download')
        urllib.request.urlretrieve(URL, temporary)
        temporary.replace(archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != ARCHIVE_SHA256:
        raise ValueError('OpenCV source archive checksum mismatch')
    source = directory/f'opencv-{VERSION}'
    if not source.exists():
        with tarfile.open(archive) as tar:
            for member in tar.getmembers():
                target = (directory/member.name).resolve()
                if directory not in target.parents or member.issym() or member.islnk():
                    raise ValueError('Unexpected archive member')
            tar.extractall(directory)
    prefix = directory/'install'
    build = directory/'build'
    subprocess.run([str(args.cmake.resolve()), '-S', str(source), '-B', str(build), '-G', 'Ninja',
        f'-DCMAKE_MAKE_PROGRAM={args.ninja.resolve()}', f'-DCMAKE_INSTALL_PREFIX={prefix}',
        '-DCMAKE_BUILD_TYPE=Release', '-DBUILD_LIST=core,imgproc,imgcodecs,photo',
        '-DBUILD_SHARED_LIBS=ON', '-DBUILD_TESTS=OFF', '-DBUILD_PERF_TESTS=OFF',
        '-DBUILD_EXAMPLES=OFF', '-DBUILD_opencv_apps=OFF', '-DBUILD_opencv_python3=OFF',
        '-DBUILD_opencv_python2=OFF', '-DBUILD_JAVA=OFF', '-DWITH_FFMPEG=OFF',
        '-DWITH_AVFOUNDATION=OFF', '-DWITH_OPENCL=OFF', '-DWITH_IPP=OFF',
        '-DWITH_OPENEXR=OFF', '-DWITH_TIFF=ON', '-DBUILD_TIFF=ON', '-DWITH_JASPER=OFF',
        '-DWITH_JPEG=ON', '-DBUILD_JPEG=ON', '-DWITH_PNG=ON', '-DBUILD_PNG=ON',
        '-DWITH_WEBP=ON', '-DBUILD_WEBP=ON', '-DOPENCV_GENERATE_PKGCONFIG=ON'], check=True)
    subprocess.run([str(args.cmake.resolve()), '--build', str(build), '--parallel', str(args.jobs)], check=True)
    subprocess.run([str(args.cmake.resolve()), '--install', str(build)], check=True)
    (directory/'manifest.json').write_text(json.dumps({'version': VERSION, 'source': URL,
        'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest(), 'install': str(prefix)}, indent=2))


if __name__ == '__main__':
    main()
