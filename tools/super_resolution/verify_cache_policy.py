#!/usr/bin/env python3
"""Exercise real daemon eviction, advisory locks and GPU-cache regeneration.

All eviction fixtures are private. Sparse files simulate >5 GB logical cache
size without allocating that much disk space. No translation service is used.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--lock', type=Path, required=True)
    args = parser.parse_args()
    root, source, out = args.root.resolve(), args.input.resolve(), args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(root / 'tools/rust_migration'))
    from verify_runtime import Client

    def age(path, days):
        stamp = time.time() - days * 86400
        os.utime(path, (stamp, stamp))

    def fixture(path, size, days):
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open('wb') as file:
            file.truncate(size)
        age(path, days)
        return path

    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    records = []
    client = Client(root, out / 'runtime', root / '.models', root / 'core/python/model_worker.py')
    sr = client.directory / 'super-resolution-v1'
    display = client.directory / 'assets'
    try:
        assert client.call('health')['protocol'] == 1
        protected = client.directory / 'pages/enhanced-v1/user-edited.json'
        protected.parent.mkdir(parents=True, exist_ok=True)
        protected.write_text('{"translation":"人工修改必须保留"}')
        age(protected, 90)
        protected_bytes = protected.read_bytes()
        # External shared lock emulates another active candidate process. It
        # must prevent *both* roots being pruned by this daemon.
        with (sr / '.cache-maintenance.lock').open('a+') as lock:
            fcntl.flock(lock, fcntl.LOCK_SH)
            oldest = fixture(sr / f'{1:064x}' / 'effective.png', 3_000_000_000, 4)
            newest = fixture(display / 'pages' / f'{2:064x}.json', 2_400_000_000, 1)
            expired = fixture(display / 'masks' / f'{3:064x}.png', 100, 31)
            unknown = fixture(sr / f'{4:064x}' / 'effective.png', 10, 60)
            unknown.with_name('user-notes.txt').write_text('keep unknown content')
            client.call('cache_status')
            assert oldest.exists() and newest.exists() and expired.exists()
        status = client.call('cache_status')
        assert status['limit_bytes'] == 5_000_000_000 and status['unused_days'] == 30
        assert not oldest.exists() and not expired.exists() and newest.exists()
        assert unknown.exists() and protected.read_bytes() == protected_bytes
        assert status['last_sweep']['bytes_after'] <= 4_500_000_000
        records.append({'test': 'combined >5 GB quota, 30-day expiry, external activity lock and protected edits',
                        'status': status, 'sparse_test_files': True})
        newest.unlink()  # Only our own disposable sparse fixture.

        settings = {'engine': 'metalfx', 'target': 1440, 'axis': 'height', 'preset': 'standard'}
        with args.lock.open('a') as gpu_lock:
            fcntl.flock(gpu_lock, fcntl.LOCK_EX)
            loaded = client.call('load_page', image=str(source), mode='text', super_resolution=settings)
            effective, cache = Path(loaded['effective_image']), Path(loaded['cache'])
            image_hash = hashlib.sha256(effective.read_bytes()).hexdigest()
            cache.parent.mkdir(parents=True, exist_ok=True)
            page = {'blocks': [{'id': '1', 'source': '原文', 'translation': '我的人工译文', 'edited': True}]}
            cache.write_text(json.dumps(page, ensure_ascii=False))
            cache_bytes = cache.read_bytes()
            for name in ['effective.png', 'identity.json', '.last-used']:
                age(effective.parent / name, 32)
            pinned = client.call('cache_status')
            assert effective.exists() and pinned['last_sweep']['protected_entries'] == 1
            # Changing to the original releases the previous visible-image pin.
            client.call('load_page', image=str(source), mode='text', super_resolution={'engine': 'off'})
            client.call('cache_status')
            assert not effective.exists() and cache.read_bytes() == cache_bytes
            again = client.call('load_page', image=str(source), mode='text', super_resolution=settings)
            assert Path(again['effective_image']) == effective and Path(again['cache']) == cache
            assert hashlib.sha256(effective.read_bytes()).hexdigest() == image_hash
            assert again['page'] == page and cache.read_bytes() == cache_bytes
            records.append({'test': 'visible image pin, release, actual GPU regeneration and manual translation reuse',
                            'passed': True})

        assert source_hash == hashlib.sha256(source.read_bytes()).hexdigest()
        assert protected.read_bytes() == protected_bytes
        # Wait across a real maintenance tick while a request is only partly
        # written. The timer must neither require a user action nor discard the
        # JSONL parser's partial buffer.
        time.sleep(2)
        idle_stale = fixture(display / 'masks' / f'{6:064x}.png', 10, 31)
        client.process.stdin.write('{"id":"partial","op":"hea')
        client.process.stdin.flush()
        deadline = time.monotonic() + 75
        while idle_stale.exists() and time.monotonic() < deadline:
            time.sleep(0.25)
        assert not idle_stale.exists(), 'Idle periodic cleanup did not run'
        client.process.stdin.write('lth"}\n')
        client.process.stdin.flush()
        assert client.wait('partial')['result']['protocol'] == 1
        records.append({'test': 'idle 60-second maintenance and partial JSONL request preservation', 'passed': True})
    finally:
        client.close()

    # Restart the daemon on exactly the same private roots with expired assets.
    # Client intentionally requires a fresh directory, so launch directly here.
    import subprocess
    stale = fixture(display / 'pages' / f'{5:064x}.json', 10, 31)
    command = [str(root / 'core/target/release/koma-core'), 'serve',
               '--cache-root', str(client.directory / 'pages'), '--assets-root', str(display),
               '--python', sys.executable, '--worker', str(root / 'core/python/model_worker.py'),
               '--weights', str(root / '.models')]
    subprocess.run(command, input='{"id":"exit","op":"shutdown"}\n',
                   text=True, capture_output=True, timeout=30, check=True)
    assert not stale.exists() and protected.read_bytes() == protected_bytes
    records.append({'test': 'startup expiration without opening any page', 'passed': True})
    report = {'passed': True, 'records': records, 'source_unchanged': True, 'translation_service_used': False}
    (out / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(report, ensure_ascii=False))


if __name__ == '__main__':
    main()
