#!/usr/bin/env python3
"""Compare real model arrays before/after the Rust-managed worker boundary.

Does not translate, modify input caches, or claim full application parity.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def main():
    import cv2
    import numpy as np
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--baseline-results', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--pages', default='13,14,20,23')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    baseline = args.baseline_root.resolve()
    sys.path.insert(0, str(baseline))
    from macos.backend.reading_assets import LocalRestorer, text_mask
    pages = json.loads(args.manifest.read_text())
    selected = {int(n) for n in args.pages.split(',')}
    jobs, cases = [], []

    def array(name, value):
        value = np.ascontiguousarray(value, dtype=np.uint8)
        (output/name).write_bytes(value.tobytes())
        return {'path': name, 'shape': list(value.shape)}

    for page in pages:
        if page['page'] not in selected:
            continue
        n = page['page']
        image_bytes = Path(page['image']).read_bytes()
        image = cv2.imdecode(np.frombuffer(image_bytes, np.uint8), cv2.IMREAD_COLOR)
        original_hash = hashlib.sha256(image_bytes).hexdigest()
        assert original_hash == page['image_hash']
        jobs.append({'op': 'detect', 'image': array(f'{n}-image.bin', image),
                     'raw_output': f'{n}-raw.bin', 'refined_output': f'{n}-refined.bin'})
        rows = json.loads((args.baseline_results/'regions'/f'{n:05}.json').read_text())['regions']
        # Use the largest real source-repair crop on each review page.
        region = max(rows, key=lambda r: (r['background_box'][2]-r['background_box'][0])*(r['background_box'][3]-r['background_box'][1]))
        a, b, c, d = region['background_box']
        patch = cv2.imdecode(np.frombuffer(base64.b64decode(region['background_png']), np.uint8), cv2.IMREAD_UNCHANGED)
        crop = cv2.cvtColor(image, cv2.COLOR_BGR2GRAY)[b:d, a:c]
        mask = patch[:, :, 3].copy()
        jobs.append({'op': 'restore', 'image': array(f'{n}-crop.bin', crop),
                     'mask': array(f'{n}-mask.bin', mask), 'output': f'{n}-restored.bin'})
        cases.append((page, image_bytes, image, crop, mask))
    (output/'requests.json').write_text(json.dumps(jobs))
    with (output/'worker.log').open('w') as log:
        run = subprocess.run([str(root/'core/target/debug/koma-core'), 'model-run',
            '--python', str(root/'macos/.venv/bin/python'), '--worker', str(root/'core/python/model_worker.py'),
            '--weights', str(root/'.models'), '--workspace', str(output), '--requests', str(output/'requests.json')],
            capture_output=False, stdout=subprocess.PIPE, stderr=log, check=True)
    replies = json.loads(run.stdout)
    (output/'replies.json').write_bytes(run.stdout)
    restorer = LocalRestorer(baseline/'.models/anime-manga-big-lama.pt')
    results = []

    def read(value):
        return np.fromfile(output/value['path'], np.uint8).reshape(value['shape'])

    for index, (page, image_bytes, image, crop, mask) in enumerate(cases):
        detector_reply, restoration_reply = replies[index*2:index*2+2]
        actual_mask = ((read(detector_reply['raw']) > 160) | (read(detector_reply['refined']) > 0)).astype(np.uint8)*255
        expected_mask = text_mask(image, image_bytes, baseline/'.models/comictextdetector.pt', args.baseline_results/'assets')
        actual_restore = read(restoration_reply['image'])
        expected_restore = restorer(crop, mask)
        delta = np.abs(actual_restore.astype(int)-expected_restore.astype(int))
        result = {'page': page['page'], 'mask_different_pixels': int(np.count_nonzero(actual_mask != expected_mask)),
                  'restoration_different_pixels': int(np.count_nonzero(delta)), 'restoration_max_delta': int(delta.max()),
                  'image_hash_unchanged': hashlib.sha256(Path(page['image']).read_bytes()).hexdigest() == page['image_hash']}
        results.append(result)
    passed = all(r['mask_different_pixels'] == 0 and r['restoration_different_pixels'] == 0 and r['image_hash_unchanged'] for r in results)
    report = {'scope': 'real detector and restoration arrays across Rust-owned persistent model worker',
              'passed': passed, 'results': results}
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report))
    if not passed:
        raise SystemExit('Model bridge changed numeric results')


if __name__ == '__main__':
    main()
