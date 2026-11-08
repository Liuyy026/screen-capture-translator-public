#!/usr/bin/env python3
"""Compare display bounds and every completed mask pixel against Python."""
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
    parser.add_argument('--baseline-results', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    baseline = args.baseline_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(baseline))
    import cv2
    import numpy as np
    from backend.overlay import _box
    from macos.backend.reading_overlay import preserved_markers
    from macos.backend.reading_background import complete_vertical_strokes
    records = []

    def compare(page, image, mask_path=None):
        prefix = output/f'{len(records)+1:04}'
        gray = cv2.cvtColor(cv2.imread(str(image)), cv2.COLOR_BGR2GRAY)
        mask = cv2.imread(str(mask_path), cv2.IMREAD_GRAYSCALE) if mask_path else None
        before = copy.deepcopy(page)
        preserved = preserved_markers(page)
        boxes = [None if str(b['id']) in preserved else _box(b, gray.shape[1], gray.shape[0]) for b in page['blocks']]
        expanded, supplemented = complete_vertical_strokes(gray, boxes, page['blocks'], mask)
        expected = {'preserved': preserved, 'boxes': expanded,
                    'mask_sha256': hashlib.sha256(supplemented.tobytes()).hexdigest() if supplemented is not None else None}
        expected = json.loads(json.dumps(expected))
        cache = prefix.with_suffix('.input.json')
        cache.write_text(json.dumps(page, ensure_ascii=False))
        command = [str(root/'core/target/debug/koma-core'), 'ink-probe', '--image', str(image), '--cache', str(cache)]
        if mask_path:
            command += ['--mask', str(mask_path)]
        actual = json.loads(subprocess.check_output(command))
        prefix.with_suffix('.actual.json').write_text(json.dumps(actual, ensure_ascii=False, indent=2))
        prefix.with_suffix('.expected.json').write_text(json.dumps(expected, ensure_ascii=False, indent=2))
        assert actual == expected, f'Ink mismatch: {prefix}'
        assert page == before
        records.append({'case': len(records)+1, 'equal': True, 'has_mask': mask is not None,
                        'extended_boxes': sum(list(a) != b for a, b in zip(boxes, actual['boxes']) if a is not None)})
        return actual, supplemented

    pages = json.loads(args.manifest.read_text())
    stat = (baseline/'.models/comictextdetector.pt').stat()
    for item in pages:
        image = Path(item['image'])
        key = hashlib.sha256(image.read_bytes() + f'ctd-mask-v2:{stat.st_size}:{stat.st_mtime_ns}'.encode()).hexdigest()
        mask_path = args.baseline_results/'assets/masks'/f'{key}.png'
        page = json.loads(Path(item['cache']).read_text())
        # An absent mask is allowed only for a page without ready translations.
        if not mask_path.exists():
            assert not any(isinstance(b.get('translation'), str) and b['translation'].strip() and not b.get('error') for b in page['blocks'])
            mask_path = None
        compare(page, image, mask_path)

    spec = importlib.util.spec_from_file_location('baseline_background_tests', baseline/'macos/tests/test_reading_background.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    def strokes(gray, boxes, blocks, source):
        n = len(records)+1
        image, mask_path = output/f'{n:04}.png', output/f'{n:04}.mask.png'
        assert cv2.imwrite(str(image), gray)
        if source is not None:
            assert cv2.imwrite(str(mask_path), source)
        else:
            mask_path = None
        page = {'width': gray.shape[1], 'height': gray.shape[0], 'blocks': [
            {**b, 'id': str(i), 'box': box, 'edited': True} for i, (b, box) in enumerate(zip(blocks, boxes))]}
        actual, supplemented = compare(page, image, mask_path)
        return actual['boxes'], supplemented
    module.complete_vertical_strokes = strokes
    names = ['test_partly_detected_leading_dash_extends_display_only',
             'test_missing_trailing_dash_requires_adjacent_detected_text',
             'test_undetected_crossing_scene_line_does_not_extend_source_bounds']
    result = unittest.TextTestRunner(verbosity=2).run(unittest.TestSuite(module.ReadingBackgroundTests(name) for name in names))
    # Exercise no-detector behavior and footer/manual overrides too.
    gray = np.full((1000, 700), 255, np.uint8)
    footer_image = output/'footer.png'
    assert cv2.imwrite(str(footer_image), gray)
    page = {'width': 700, 'height': 1000, 'blocks': [{'id': 'footer', 'box': [70, 950, 90, 970], 'source': 'ッ', 'translation': '啊'}]}
    compare(page, footer_image)
    page['blocks'][0]['edited'] = True
    compare(page, footer_image)
    page['blocks'][0].update(edited=False, box=[70, 600, 90, 620])
    compare(page, footer_image)
    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image', 'cache', 'original_cache'))
    report = {'scope': 'display-only boxes, preserved footers and full completed-mask pixel hashes',
              'pages': len(pages), 'comparisons': len(records), 'behavior_tests': result.testsRun,
              'original_inputs_unchanged': unchanged, 'passed': result.wasSuccessful() and unchanged, 'records': records}
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({k: v for k, v in report.items() if k != 'records'}))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
