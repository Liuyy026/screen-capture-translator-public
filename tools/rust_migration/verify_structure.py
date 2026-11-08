#!/usr/bin/env python3
"""Differential checks against frozen Python, never touching original caches.

Runs all 35 pages through cold reconstruction, cached reuse and no-image reuse;
then executes baseline behavior tests through the native implementation. Python
is the test oracle only, not part of the native processing path.
"""
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
    root = Path(__file__).resolve().parents[2]
    baseline = args.baseline_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(baseline))
    import cv2
    from backend import panels, structure
    records = []
    serial = 0

    def compare(page, mode='ensure', image=None, gray=None, panel_only=False):
        nonlocal serial
        serial += 1
        prefix = output/f'{serial:04}'
        expected = copy.deepcopy(page)
        if gray is not None:
            image = prefix.with_suffix('.png')
            assert cv2.imwrite(str(image), gray)
        kwargs = {'image': image} if image is not None else {}
        if panel_only:
            result = panels.ensure_panels(expected, **kwargs)
        else:
            fn = {'ensure': structure.ensure_dialogues, 'build': structure.build_dialogues,
                  'sync': structure.sync_dialogue_translations}[mode]
            result = fn(expected, **kwargs)
        cache = prefix.with_suffix('.input.json')
        cache.write_text(json.dumps(page, ensure_ascii=False))
        command = [str(root/'core/target/debug/koma-core'),
                   'panel-probe' if panel_only else 'structure-probe', '--cache', str(cache)]
        if not panel_only:
            command += ['--mode', mode]
        if image is not None:
            command += ['--image', str(image)]
        actual = json.loads(subprocess.check_output(command))
        prefix.with_suffix('.expected.json').write_text(json.dumps(expected, ensure_ascii=False, indent=2))
        prefix.with_suffix('.actual.json').write_text(json.dumps(actual, ensure_ascii=False, indent=2))
        if actual != expected:
            raise AssertionError(f'Native {mode} differs; inspect {prefix.name}.* in {output}')
        page.clear()
        page.update(actual)
        records.append({'case': serial, 'mode': 'panels' if panel_only else mode, 'equal': True})
        if panel_only:
            return result
        if mode != 'sync':
            return page.get('dialogues', [])

    pages = json.loads(args.manifest.read_text())
    for item in pages:
        page = json.loads(Path(item['cache']).read_text())
        # Force both implementations to calculate, not trust an old signature.
        page.pop('panel_signature', None)
        page.pop('structure', None)
        compare(page, image=Path(item['image']))
        compare(page, image=Path(item['image']))
        compare(page)

    suite = unittest.TestSuite()
    skipped = []
    for name in ('test_panels', 'test_structure'):
        spec = importlib.util.spec_from_file_location(name, baseline/'tests'/f'{name}.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.ensure_panels = lambda page, **kw: compare(page, panel_only=True, **kw)
        module.ensure_dialogues = lambda page, **kw: compare(page, **kw)
        module.build_dialogues = lambda page: compare(page, mode='build')
        module.sync_dialogue_translations = lambda page: compare(page, mode='sync')
        def classify(source):
            page = {'blocks': [{'id': '1', 'box': [0, 0, 20, 20], 'source': source}]}
            compare(page, mode='build')
            return page['blocks'][0]['candidate_kind']
        module.classify_text = classify
        for test_group in unittest.defaultTestLoader.loadTestsFromModule(module):
            for test in test_group:
                # Python import-failure behavior has no counterpart in native
                # OpenCV. The missing-image behavior is tested separately.
                if test._testMethodName == 'test_cached_geometry_needs_no_opencv_import':
                    skipped.append({'test': test.id(), 'reason': 'Python import availability is not native behavior'})
                else:
                    suite.addTest(test)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image', 'cache', 'original_cache'))
    report = {'scope': 'native panels, grouping, signatures, ordering and translation synchronization',
              'pages': len(pages), 'comparisons': len(records), 'behavior_tests': result.testsRun,
              'tests_passed': result.wasSuccessful(), 'skipped': skipped + [{'test': t.id(), 'reason': reason} for t, reason in result.skipped],
              'original_inputs_unchanged': unchanged, 'passed': result.wasSuccessful() and unchanged,
              'records': records}
    (output/'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps({k: v for k, v in report.items() if k != 'records'}, ensure_ascii=False))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
