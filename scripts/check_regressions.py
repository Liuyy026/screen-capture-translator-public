"""Run core regressions with generated fixtures and isolated outputs."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
LOCAL_PYTHON_SUITES = ('tests', 'macos/tests')
NATIVE_TEST_FILES = (
    'tests/test_panels.py',
    'tests/test_structure.py',
    'tests/test_translation_service.py',
    'macos/tests/test_reading_overlay.py',
)
PYTHON_SUITE = '''
import sys
import unittest

directory = sys.argv[1]
suite = unittest.defaultTestLoader.discover(directory, pattern='test_*.py')
if not suite.countTestCases():
    sys.exit(f'No Python test cases found in {directory}/.')
sys.exit(0 if unittest.TextTestRunner().run(suite).wasSuccessful() else 1)
'''


def run(*command, env=None, label=None):
    print('Running:', label or ' '.join(map(str, command)), flush=True)
    result = subprocess.run(list(map(str, command)), cwd=ROOT, env=env, timeout=600)
    if result.returncode:
        raise SystemExit(result.returncode)


def test_files(directory, pattern):
    return sorted(path for path in (ROOT / directory).glob(pattern) if path.is_file())


def run_python_suite(directory, env=None):
    run(sys.executable, '-B', '-c', PYTHON_SUITE, directory,
        env=env, label=f'Python regressions in {directory}/')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--local', action='store_true', help='Require both local Python suites')
    parser.add_argument('--native', action='store_true', help='Also exercise a built Rust core; no models needed')
    parser.add_argument('--swift', action='store_true', help='Compile and run macOS Swift regressions')
    parser.add_argument('--windows', action='store_true', help='Also run Qt client tests; requires PySide6')
    args = parser.parse_args()

    missing_local = [directory for directory in LOCAL_PYTHON_SUITES
                     if not test_files(directory, 'test_*.py')]
    if args.local and missing_local:
        missing = ', '.join(f'{directory}/test_*.py' for directory in missing_local)
        parser.error(f'--local requires local Python tests: {missing}')
    if args.native:
        missing = [filename for filename in NATIVE_TEST_FILES if not (ROOT / filename).is_file()]
        if missing:
            parser.error(f'--native requires these local test files: {", ".join(missing)}')
        if not (ROOT / 'samples/01.webp').is_file():
            parser.error('--native requires samples/01.webp')
    if args.swift:
        if sys.platform != 'darwin':
            parser.error('--swift requires macOS')
        swift_tests = test_files('macos/tests', '*Tests.swift')
        if not swift_tests:
            parser.error('--swift requires local test files in macos/tests/*Tests.swift')
    if args.windows and not test_files('windows/tests', 'test_*.py'):
        parser.error('--windows requires local test files in windows/tests/test_*.py')
    if not test_files('core/python', 'test_*.py'):
        parser.error('Core Python test files are missing from core/python/')

    if missing_local:
        unavailable = ', '.join(f'{directory}/' for directory in missing_local)
        print(f'Skipping unavailable local Python suites: {unavailable}', flush=True)
    for directory in LOCAL_PYTHON_SUITES:
        if directory not in missing_local:
            run_python_suite(directory)
    run_python_suite('core/python')
    if args.windows:
        run_python_suite('windows/tests', env={**os.environ, 'QT_QPA_PLATFORM': 'offscreen'})
    if not (args.native or args.swift):
        return

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
            sources = sorted((ROOT/'macos/Sources').glob('*.swift'))
            for test in swift_tests:
                binary = output/test.stem
                run('xcrun', 'swiftc', '-parse-as-library', '-D', 'KOMA_REVIEW_HARNESS',
                    *sources, test, '-o', binary, '-framework', 'SwiftUI', '-framework',
                    'AppKit', '-framework', 'CryptoKit', '-framework', 'Security')
                arguments = [sys.executable, output/'transport'] if test.stem == 'NativeCoreTransportTests' else []
                run(binary, *arguments)


if __name__ == '__main__':
    main()
