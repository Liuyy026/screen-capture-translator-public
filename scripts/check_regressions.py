"""Run Koma's macOS regressions with generated fixtures and isolated outputs."""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/rust_migration'))
from baseline import prepare_baseline

LOCAL_PYTHON_SUITES = ('tests', 'macos/tests')
PYTHON_SUITE = '''
import sys
import unittest

directory = sys.argv[1]
suite = unittest.defaultTestLoader.discover(directory, pattern='test_*.py')
if not suite.countTestCases():
    sys.exit(f'No Python test cases found in {directory}/.')
sys.exit(0 if unittest.TextTestRunner().run(suite).wasSuccessful() else 1)
'''


def run(*command, cwd=ROOT, label=None):
    print('Running:', label or ' '.join(map(str, command)), flush=True)
    result = subprocess.run(list(map(str, command)), cwd=cwd, timeout=600)
    if result.returncode:
        raise SystemExit(result.returncode)


def test_files(directory, pattern):
    return sorted(path for path in (ROOT / directory).glob(pattern) if path.is_file())


def run_python_suite(directory, cwd=ROOT):
    run(sys.executable, '-B', '-c', PYTHON_SUITE, directory, cwd=cwd,
        label=f'Python regressions in {directory}/')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--local', action='store_true', help='Require both private historical Python suites')
    parser.add_argument('--native', action='store_true', help='Compare a built Mac core with pinned Git history')
    parser.add_argument('--swift', action='store_true', help='Compile and run local macOS Swift regressions')
    args = parser.parse_args()

    local = [name for name in LOCAL_PYTHON_SUITES if test_files(name, 'test_*.py')]
    if args.local and len(local) != len(LOCAL_PYTHON_SUITES):
        missing = ', '.join(f'{name}/test_*.py' for name in LOCAL_PYTHON_SUITES if name not in local)
        parser.error(f'--local requires local Python tests: {missing}')
    if args.native and not (ROOT / 'samples/01.webp').is_file():
        parser.error('--native requires samples/01.webp')
    if args.swift:
        if sys.platform != 'darwin':
            parser.error('--swift requires macOS')
        swift_tests = test_files('macos/tests', '*Tests.swift')
        if not swift_tests:
            parser.error('--swift requires local test files in macos/tests/*Tests.swift')
    if not test_files('core/python', 'test_*.py'):
        parser.error('Core Python test files are missing from core/python/')

    run_python_suite('core/python')
    if not (local or args.native or args.swift):
        return

    runtime = ROOT/'.runtime'
    runtime.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='core-tests-', dir=runtime) as temporary:
        output = Path(temporary)
        if local:
            baseline = prepare_baseline(output / 'local-baseline', native_assertions=False)
            for name in local:
                # Old tests use __file__ to find sibling workers and private fixtures.
                shutil.copytree(ROOT / name, baseline / name,
                                ignore=shutil.ignore_patterns('__pycache__', '*.pyc', '*.swift'))
                run_python_suite(name, cwd=baseline)
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
