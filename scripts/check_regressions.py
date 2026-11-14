"""Run core regressions with generated fixtures and isolated outputs."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(*command, env=None):
    print('Running:', ' '.join(map(str, command)), flush=True)
    subprocess.run(list(map(str, command)), cwd=ROOT, env=env, check=True, timeout=600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', action='store_true', help='Also exercise a built Rust core; no models needed')
    parser.add_argument('--swift', action='store_true', help='Compile and run macOS Swift regressions')
    parser.add_argument('--windows', action='store_true', help='Also run Qt client tests; requires PySide6')
    args = parser.parse_args()
    for directory in ('tests', 'macos/tests', 'core/python'):
        run(sys.executable, '-B', '-m', 'unittest', 'discover', '-s', directory, '-p', 'test_*.py')
    if args.windows:
        run(sys.executable, '-B', '-m', 'unittest', 'discover', '-s', 'windows/tests',
            '-p', 'test_*.py', env={**os.environ, 'QT_QPA_PLATFORM': 'offscreen'})
    runtime = ROOT/'.runtime'
    runtime.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='core-tests-', dir=runtime) as temporary:
        output = Path(temporary)
        if args.native:
            for name in ('structure', 'reader_behavior', 'translation_protocol',
                         'translate_pipeline', 'vision_pipeline', 'runtime', 'sr_cancellation'):
                command = [sys.executable, '-B', ROOT/f'tools/rust_migration/verify_{name}.py',
                           '--output', output/name]
                if name == 'sr_cancellation':
                    command += ['--image', ROOT/'samples/01.webp']
                run(*command)
        if args.swift:
            if sys.platform != 'darwin':
                parser.error('--swift requires macOS')
            sources = sorted((ROOT/'macos/Sources').glob('*.swift'))
            for test in sorted((ROOT/'macos/tests').glob('*Tests.swift')):
                binary = output/test.stem
                run('xcrun', 'swiftc', '-parse-as-library', '-D', 'KOMA_REVIEW_HARNESS',
                    *sources, test, '-o', binary, '-framework', 'SwiftUI', '-framework',
                    'AppKit', '-framework', 'CryptoKit', '-framework', 'Security')
                arguments = [sys.executable, output/'transport'] if test.stem == 'NativeCoreTransportTests' else []
                run(binary, *arguments)
    print('All requested core regressions passed.', flush=True)


if __name__ == '__main__':
    main()
