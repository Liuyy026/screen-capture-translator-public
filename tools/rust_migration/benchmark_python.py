#!/usr/bin/env python3
"""Measure the frozen Python display pipeline with private fixture copies.

No translation endpoints, source-image edits, or original-cache writes.
Run once per fresh output directory; warm passes reuse its private assets.
"""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import re
import statistics
import subprocess
import sys
import time


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--warm-passes', type=int, default=3)
    args = parser.parse_args()
    root = args.baseline_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output/'regions').mkdir()
    (output/'logs').mkdir()
    pages = json.loads(args.manifest.read_text())
    for page in pages:
        for name in ('image', 'cache'):
            assert digest(page[name]) == page[name+'_hash'], (page['page'], name)
    records = []
    report = {'backend': 'python', 'baseline_commit': subprocess.check_output(
        ['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip(),
        'environment': {'platform': platform.platform(), 'python': sys.version},
        'scope': 'display generation with fixed existing translations; no translation inference',
        'system_file_cache_flushed': False, 'records': records}
    expected = {}
    for iteration in range(args.warm_passes+1):
        phase = 'fresh-assets' if iteration == 0 else 'persisted-cache'
        for page in pages:
            command = [str(root/'macos/.venv/bin/python'), str(root/'macos/backend/reading_overlay.py'),
                       '--image', page['image'], '--cache', page['cache'], '--assets-dir', str(output/'assets')]
            if sys.platform == 'darwin':
                command = ['/usr/bin/time', '-l', *command]
            start = time.perf_counter()
            run = subprocess.run(command, capture_output=True)
            elapsed = time.perf_counter()-start
            stderr = run.stderr.decode(errors='replace')
            (output/'logs'/f"{iteration}-{page['page']:05}.log").write_text(stderr)
            if run.returncode:
                raise RuntimeError(f"Page {page['page']} failed; inspect private logs")
            value = json.loads(run.stdout)
            assert value['version'] == 3 and isinstance(value['regions'], list)
            normalized = hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()
            if iteration == 0:
                expected[page['page']] = normalized
                (output/'regions'/f"{page['page']:05}.json").write_bytes(run.stdout)
            else:
                assert expected[page['page']] == normalized, 'Warm output changed'
            memory = re.search(r'(\d+)\s+maximum resident set size', stderr)
            item = {'page': page['page'], 'phase': phase, 'iteration': iteration,
                    'seconds': elapsed, 'reply_bytes': len(run.stdout),
                    'peak_rss_bytes': int(memory.group(1)) if memory else None}
            records.append(item)
            (output/'report.json').write_text(json.dumps(report, indent=2))
            print(json.dumps(item), flush=True)
    for page in pages:
        for name in ('image', 'cache'):
            assert digest(page[name]) == page[name+'_hash']
        if page.get('original_cache'):
            assert digest(page['original_cache']) == page['cache_hash']
    report['summary'] = {phase: {'samples': len(values), 'total_seconds': sum(values),
        'median_seconds': statistics.median(values), 'max_seconds': max(values)}
        for phase in ('fresh-assets', 'persisted-cache')
        if (values := [r['seconds'] for r in records if r['phase'] == phase])}
    report['input_hashes_unchanged'] = True
    report['passed'] = True
    (output/'report.json').write_text(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
