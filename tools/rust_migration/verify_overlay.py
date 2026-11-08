#!/usr/bin/env python3
"""Differential OCR-side region classification; immutable real-page inputs."""
import argparse
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    baseline, output = args.baseline_root.resolve(), args.output.resolve()
    root = Path(__file__).resolve().parents[2]
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(baseline))
    from backend import overlay
    import cv2
    import numpy as np
    binary = root/'core/target/release/koma-core'
    manifest = json.loads(args.manifest.read_text())
    def hashes():
        for entry in manifest:
            for field, expected in [('image', 'image_hash'), ('cache', 'cache_hash'), ('original_cache', 'cache_hash')]:
                assert hashlib.sha256(Path(entry[field]).read_bytes()).hexdigest() == entry[expected]
    hashes()
    comparisons = 0
    def run(page, image=None, mode='ensure', gray=None):
        nonlocal comparisons
        comparisons += 1
        prefix = output/f'{comparisons:04}'
        cache = prefix.with_suffix('.json')
        cache.write_text(json.dumps(page, ensure_ascii=False))
        if gray is not None:
            image = prefix.with_suffix('.png')
            assert cv2.imwrite(str(image), gray)
        command = [str(binary), 'overlay-probe', '--cache', str(cache), '--mode', mode]
        if image is not None:
            command += ['--image', str(image)]
        reply = subprocess.run(command, capture_output=True, text=True, timeout=60)
        prefix.with_suffix('.stderr.log').write_text(reply.stderr)
        assert reply.returncode == 0, reply.stderr
        prefix.with_suffix('.actual.json').write_text(reply.stdout)
        actual = json.loads(reply.stdout)
        reference = copy.deepcopy(page)
        if mode == 'ensure':
            changed = overlay.ensure_page_overlays(reference, gray=gray, image=image)
            expected = {'changed': changed, 'page': reference}
        else:
            expected = overlay._adjacent_text(gray, tuple(page['first']), tuple(page['second']), page['vertical'])
        prefix.with_suffix('.expected.json').write_text(json.dumps(expected, ensure_ascii=False, indent=2))
        # Number types affect downstream JSON signatures (e.g. 42 vs 42.0).
        assert json.dumps(actual,sort_keys=True) == json.dumps(expected,sort_keys=True), f'overlay difference at {prefix}'
        return actual
    for entry in manifest:
        page = json.loads(Path(entry['cache']).read_text())
        page.pop('overlay_signature', None)
        first = run(page, entry['image'])
        second = run(first['page'], entry['image'])
        assert first['changed'] and not second['changed']
        run(first['page'])
        ocr = copy.deepcopy(page)
        for block in ocr['blocks']:
            if not block.get('edited'):
                block.update(translation='', error='')
        run(ocr, entry['image'])
    # Old tests exercise small/large regions, joined fragments and panel barriers.
    spec = importlib.util.spec_from_file_location('native_overlay_tests', baseline/'tests/test_overlay.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    def ensure(page, gray=None, image=None):
        result = run(page, image=image, gray=gray)
        page.clear()
        page.update(result['page'])
        return result['changed']
    module.ensure_page_overlays = ensure
    module.overlay._adjacent_text = lambda gray, first, second, vertical: run(
        {'first': first, 'second': second, 'vertical': vertical}, gray=gray, mode='adjacent')
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(module.OverlayTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    # Preserve edited/failed state and tolerate malformed OCR boxes.
    for box in [None, [], [1,2,3], ['bad',2,30,40], [-12,-5,30,40], [30,40,1,2], [1.5,2.5,30.5,40.5]]:
        for edited, error in [(False,''), (False,'failed'), (True,'failed')]:
            run({'blocks':[{'id':'1','box':box,'source':'日本語','translation':'中文',
                            'edited':edited,'error':error}]}, gray=np.full((90,90),245,np.uint8))
    hashes()
    report = {'pages':len(manifest),'comparisons':comparisons,'behavior_tests':result.testsRun,
              'failures':len(result.failures),'errors':len(result.errors),
              'skipped':[reason for _,reason in result.skipped],
              'input_hashes_unchanged':True,'passed':result.wasSuccessful()}
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
