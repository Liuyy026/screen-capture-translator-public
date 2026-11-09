#!/usr/bin/env python3
"""Exercise both installed helpers with identical, private, derived fixtures.

This is an integration/measurement harness, not an image-quality score. Artificial
downsampling does not establish performance on naturally degraded downloads.
Python/Pillow are test dependencies only; neither is required by the helpers.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time

from PIL import Image, ImageOps


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def invoke(command, timeout=180):
    started = time.monotonic()
    process = subprocess.Popen(command, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
        raise RuntimeError(f"Helper timed out after {timeout}s")
    return process.returncode, stdout, stderr, time.monotonic() - started


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--lock', type=Path, required=True)
    args = parser.parse_args()
    root, source, output = args.root.resolve(), args.source.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source_hash = digest(source)
    original = ImageOps.exif_transpose(Image.open(source)).convert('RGB')
    fixtures = {}
    for height in (540, 720, 1080):
        width = round(original.width * height / original.height)
        path = output / f'输入 页面 高{height}.png'
        original.resize((width, height), Image.Resampling.LANCZOS).save(path)
        fixtures[height] = path
    report = {'source': str(source), 'source_sha256': source_hash,
              'fixture_origin': 'artificial Lanczos downsampling of one existing page',
              'quality_claim': 'none; visual/OCR review must be assessed separately',
              'records': [], 'guards': []}
    args.lock.parent.mkdir(parents=True, exist_ok=True)
    with args.lock.open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        for engine in ('metalfx', 'realcugan'):
            helper = root / f'macos/.build/koma-sr-{engine}'
            code, stdout, stderr, _ = invoke([str(helper), '--capabilities'])
            assert code == 0, stderr.decode(errors='replace')
            capabilities = json.loads(stdout)
            assert capabilities['engine'] == engine, capabilities
            assert capabilities['available'], capabilities
            preset = 'standard' if engine == 'metalfx' else 'conservative'
            assert preset in capabilities['presets'], capabilities
            for height, target in ((540, 1440), (540, 2160), (720, 2160), (1080, 2160)):
                fixture = fixtures[height]
                before = digest(fixture)
                with Image.open(fixture) as image:
                    target_width = round(image.width * target / image.height)
                result = output / f'{engine}-{height}-to-{target}.png'
                command = [str(helper), '--input', str(fixture), '--output', str(result),
                           '--width', str(target_width), '--height', str(target),
                           '--preset', preset]
                code, stdout, stderr, elapsed = invoke(command)
                (output / f'{result.stem}.stderr.log').write_bytes(stderr)
                assert code == 0, stderr.decode(errors='replace')
                metadata = json.loads(stdout)
                with Image.open(result) as image:
                    image.load()
                    assert image.size == (target_width, target), image.size
                assert metadata['width'] == target_width and metadata['height'] == target, metadata
                assert digest(fixture) == before, 'Input was modified'
                report['records'].append({'engine': engine, 'input_height': height,
                                          'target_height': target, 'output': str(result),
                                          'wall_seconds': elapsed, 'helper_result': metadata,
                                          'capabilities': capabilities})
                (output / 'report.partial.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
            # A helper must reject writing to its own source, even under a new name.
            guard = output / f'{engine}-protected-input.png'
            guard.write_bytes(fixtures[540].read_bytes())
            before = digest(guard)
            alias = output / f'{engine}-source-alias.png'
            alias.symlink_to(guard)
            for destination in (guard, alias):
                code, _, stderr, _ = invoke([str(helper), '--input', str(guard),
                    '--output', str(destination), '--width', '720', '--height', '1080',
                    '--preset', preset])
                assert code != 0, 'Input/output alias was accepted'
                assert digest(guard) == before, 'Protected source was modified'
                report['guards'].append({'engine': engine, 'case': destination.name,
                                         'rejected': True, 'source_unchanged': True})
    assert digest(source) == source_hash, 'Original manga was modified'
    report['passed'] = True
    (output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps({'passed': True, 'cases': len(report['records']),
                      'guards': len(report['guards']), 'report': str(output/'report.json')}))


if __name__ == '__main__':
    main()
