"""Read the pre-split Python reference implementation from pinned local Git objects."""
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
BACKEND_REVISION = 'cfb1f6d593e1dd074f92123b4e581c332a64258a'
# Last commit containing the public behavior assertions before tests became local.
TEST_REVISION = 'bd99ff63dfafbd77230b9bc32f5a0d08ccd207dc'
NATIVE_ASSERTIONS = (
    'tests/test_panels.py',
    'tests/test_structure.py',
    'tests/test_translation_service.py',
    'macos/tests/test_reading_overlay.py',
)


def git_output(repo, *arguments):
    result = subprocess.run(['git', '-C', str(repo), *arguments], capture_output=True,
                            env={**os.environ, 'GIT_NO_LAZY_FETCH': '1'})
    if result.returncode:
        raise SystemExit(
            f'Historical comparison needs local Git objects {BACKEND_REVISION[:7]} '
            f'and {TEST_REVISION[:7]}. Set KOMA_BASELINE_REPO to a full local clone.\n'
            + result.stderr.decode(errors='replace').strip())
    return result.stdout


def prepare_baseline(directory, *, native_assertions=True):
    """Recreate the original module layout without touching either working tree."""
    repo = Path(os.environ.get('KOMA_BASELINE_REPO', ROOT)).expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=False)
    files = git_output(repo, 'ls-tree', '-r', '--name-only', BACKEND_REVISION, '--',
                       'backend', 'macos/backend').decode().splitlines()
    sources = [(BACKEND_REVISION, name) for name in files]
    if native_assertions:
        sources.extend((TEST_REVISION, name) for name in NATIVE_ASSERTIONS)
    for revision, name in sources:
        contents = git_output(repo, 'cat-file', 'blob', f'{revision}:{name}')
        target = directory / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(contents)
    return directory
