"""Add dialogue and diagnostic metadata to existing page caches without OCR."""
import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
STRUCTURE_FIELDS = ('dialogues', 'structure', 'diagnostics')


def built_core():
    for profile in ('release', 'debug'):
        path = ROOT / 'core/target' / profile / 'koma-core'
        if path.is_file():
            return path
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('cache_dir', type=Path)
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--core', type=Path, default=built_core(), help='Path to a built koma-core')
    args = parser.parse_args()
    if not args.core or not args.core.is_file():
        parser.error('Build koma-core with tools/rust_migration/cargo.py build or pass --core.')
    files = sorted(args.cache_dir.glob('*.json'))
    changed = 0
    for entry in files:
        path = entry.resolve()
        try:
            original = path.read_bytes()
            page = json.loads(original)
        except (OSError, UnicodeDecodeError, json.JSONDecodeError):
            continue
        if not isinstance(page, dict) or not isinstance(page.get('blocks'), list):
            continue
        migrated = json.loads(subprocess.check_output(
            [args.core.resolve(), 'structure-probe', '--cache', path, '--mode', 'build']))
        if all(page.get(field) == migrated.get(field) for field in STRUCTURE_FIELDS):
            continue
        changed += 1
        if args.dry_run:
            continue
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as temporary:
            pending = Path(temporary.name)
        try:
            pending.write_text(json.dumps(migrated, ensure_ascii=False, indent=2), encoding='utf-8')
            pending.chmod(stat.S_IMODE(path.stat().st_mode))
            if path.read_bytes() != original:
                raise SystemExit(f'Page changed during migration: {entry}. Run again when the page is idle.')
            os.replace(pending, path)
        finally:
            pending.unlink(missing_ok=True)
    print(json.dumps({'files': len(files), 'changed': changed, 'dry_run': args.dry_run}))


if __name__ == '__main__':
    main()
