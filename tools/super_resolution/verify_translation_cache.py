#!/usr/bin/env python3
"""Validate the real daemon's independent translation budget in private roots.

Hard-linked test records simulate >500 MB logical cache using only a few MB of
physical storage. All links point to this test's own fixture, never user data.
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
    args = parser.parse_args()
    root, source, out = args.root.resolve(), args.input.resolve(), args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(root / 'tools/rust_migration'))
    from verify_runtime import Client

    def age(path, days):
        stamp = time.time() - days * 86400
        os.utime(path, (stamp, stamp))

    def save(path, page, days):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(page, ensure_ascii=False))
        age(path, days)
        return path

    automatic = {'version': 2, 'width': 389, 'height': 540,
                 'blocks': [{'id': '1', 'source': '原文', 'translation': '自动译文', 'edited': False}],
                 'cache_retention': {'version': 1, 'kind': 'automatic'}}
    records = []
    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    client = Client(root, out / 'runtime', root / '.models', root / 'core/python/model_worker.py')
    pages = client.directory / 'pages'
    try:
        client.call('health')
        # Filling the translation bucket must leave a recent image cache alone.
        image = client.directory / 'assets/masks' / f'{1:064x}.png'
        image.parent.mkdir(parents=True, exist_ok=True)
        image.write_bytes(b'private cache fixture')
        with (pages / '.cache-maintenance.lock').open('a+') as lock:
            fcntl.flock(lock, fcntl.LOCK_SH)
            template = save(out / 'large-record.json', dict(automatic, test_padding='x' * 4_000_000), 2)
            large = []
            for index in range(130):
                path = pages / f'{index:064x}.json'
                os.link(template, path)
                large.append(path)
            key = f'{200:064x}'
            expired = [save(pages / name, automatic, 31) for name in [f'{key}.json',
                       f'vision-v1/{key}.json', f'enhanced-v1/{key}/{key}.json',
                       f'enhanced-v1/{key}/vision-v1/{key}.json']]
            manual = save(pages / f'{201:064x}.json', dict(automatic, cache_retention={'version': 1, 'kind': 'manual'}), 90)
            legacy_data = {k: v for k, v in automatic.items() if k != 'cache_retention'}
            legacy = save(pages / f'{202:064x}.json', legacy_data, 90)
            protected = {p: p.read_bytes() for p in [manual, legacy]}
            client.call('cache_status')
            assert all(p.exists() for p in expired + large)
        status = client.call('cache_status')
        policy = status['translations']
        assert status['limit_bytes'] == 5_000_000_000 and policy['limit_bytes'] == 500_000_000
        assert policy['unused_days'] == 30 and policy['last_sweep']['bytes_after'] <= 450_000_000
        assert all(not p.exists() for p in expired)
        remaining = [p for p in large if p.exists()]
        assert 0 < len(remaining) < 130 and large[-1].exists() and not large[0].exists()
        assert image.read_bytes() == b'private cache fixture'
        assert all(p.read_bytes() == data for p, data in protected.items())
        records.append({'test': '500 MB independent quota, 450 MB low water, all namespaces, legacy/manual protection',
                        'logical_bytes_before': policy['last_sweep']['bytes_before'],
                        'logical_bytes_after': policy['last_sweep']['bytes_after'],
                        'hardlinked_private_fixtures': True})
        for path in remaining:
            path.unlink()
        template.unlink()

        # Renew last use without rewriting the page, then protect its stable
        # sidecar across a manual edit's atomic file replacement.
        cached = save(pages / f'{source_hash}.json', automatic, 29)
        cached_bytes = cached.read_bytes()
        loaded = client.call('load_page', image=str(source), mode='text')
        assert cached.read_bytes() == cached_bytes and time.time() - cached.stat().st_mtime < 10
        age(cached, 32)
        client.call('cache_status')
        assert cached.exists(), 'Visible translation was evicted'
        edited = client.call('edit', cache=str(cached), block_id='1', revision=loaded['revision'],
                             values={'source': '人工修正的日文', 'translation': '', 'edited': False})
        assert edited['page']['cache_retention']['kind'] == 'manual'
        manual_bytes = cached.read_bytes()
        age(cached, 90)
        # A distinct source hash unpins the former visible page.
        alternate = out / 'alternate.png'
        alternate.write_bytes(source.read_bytes() + b'\n')
        client.call('load_page', image=str(alternate), mode='text')
        client.call('cache_status')
        assert cached.read_bytes() == manual_bytes
        records.append({'test': 'load renews use without changing revision; current page and source-only edits protected', 'passed': True})

        external = save(pages / f'{203:064x}.json', automatic, 31)
        with external.with_suffix('.json.pin').open('a+') as pin:
            fcntl.flock(pin, fcntl.LOCK_SH)
            client.call('cache_status')
            assert external.exists()
        client.call('cache_status')
        assert not external.exists()
        records.append({'test': 'external viewer pin survives cleanup; releasing it allows expiry', 'passed': True})

        # Actual idle timer, not a cache_status-triggered pass.
        time.sleep(2)
        idle = save(pages / f'{204:064x}.json', automatic, 31)
        deadline = time.monotonic() + 75
        while idle.exists() and time.monotonic() < deadline:
            time.sleep(.25)
        assert not idle.exists(), 'Idle translation expiry did not run'
        assert all(p.read_bytes() == data for p, data in protected.items())
        assert cached.read_bytes() == manual_bytes
        records.append({'test': 'idle 60-second sweep expires automatic translations and retains manual/legacy records', 'passed': True})
    finally:
        client.close()
    assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
    report = {'passed': True, 'records': records, 'source_unchanged': True,
              'translation_service_used': False}
    (out / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps(report, ensure_ascii=False))


if __name__ == '__main__':
    main()
