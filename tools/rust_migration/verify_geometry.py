#!/usr/bin/env python3
"""Compare native balloon geometry with the frozen accepted implementation.

Checks contour vertices, filled-mask hashes, assignment, internal rectangles
and erasure polygons. Does not claim background or whole-render equivalence.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--baseline-results', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--synthetic-only', action='store_true', help='Run boundary fixtures after an already verified chapter run')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    baseline = args.baseline_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(baseline))
    import cv2
    import numpy as np
    from backend.overlay import _box, _inner_rectangle, _erase_polygons
    from macos.backend import reading_overlay as old
    from macos.backend.reading_background import complete_vertical_strokes
    records = []

    def compare(page, image, mask_path=None, name=None):
        prefix = output/f'{len(records)+1:04}'
        before = copy.deepcopy(page)
        gray = cv2.cvtColor(cv2.imread(str(image)), cv2.COLOR_BGR2GRAY)
        mask = cv2.imread(str(mask_path), cv2.IMREAD_GRAYSCALE) if mask_path else None
        preserved = old.preserved_markers(page)
        boxes = [None if str(b['id']) in preserved else _box(b, gray.shape[1], gray.shape[0]) for b in page['blocks']]
        boxes, _ = complete_vertical_strokes(gray, boxes, page['blocks'], mask)
        _, labels, stats, _ = cv2.connectedComponentsWithStats(cv2.inRange(gray, 210, 255), 8)
        chosen = set()
        for box in boxes:
            if box is None:
                continue
            x1, y1, x2, y2 = box
            counts = np.bincount(labels[y1:y2, x1:x2].ravel())
            counts[0] = 0
            label = int(counts.argmax())
            if label and counts[label] >= (x2-x1)*(y2-y1)*.40:
                chosen.add(label)
        bubbles = []
        for label in sorted(chosen):
            candidate = old._bounded_bubble(gray, labels, stats, label, boxes)
            if candidate:
                bubbles.extend(old._split_lobes(candidate, boxes))
        bubbles.extend(old._outlined_bubbles(gray, boxes, bubbles))
        narrations = old._narration_groups(gray, page['blocks'], boxes, bubbles)
        def snapshot(candidate):
            result = {key: candidate[key] for key in ('origin', 'area', 'points', 'members')}
            result.update(fill_color=candidate.get('fill_color', [255]*3), score=candidate.get('score', 0.0),
                          mask_shape=list(candidate['mask'].shape),
                          mask_sha256=hashlib.sha256(candidate['mask'].tobytes()).hexdigest(),
                          inner_rectangle=_inner_rectangle(candidate['mask']),
                          erase_points=_erase_polygons(candidate, [boxes[i] for i in candidate['members']]))
            for key in ('solidity', 'textured', 'text_box'):
                if key in candidate:
                    result[key] = bool(candidate[key]) if key == 'textured' else candidate[key]
            return result
        cached = []
        for region in page.get('overlay_regions', []):
            c = old._cached_shape(region, gray.shape[1], gray.shape[0])
            cached.append(None if c is None else {'origin': c['origin'], 'area': c['area'],
                          'mask_shape': list(c['mask'].shape), 'mask_sha256': hashlib.sha256(c['mask'].tobytes()).hexdigest()})
        expected = json.loads(json.dumps({'bubbles': [snapshot(c) for c in bubbles],
                    'narrations': [snapshot(c) for c in narrations], 'cached': cached}))
        cache = prefix.with_suffix('.input.json')
        cache.write_text(json.dumps(page, ensure_ascii=False))
        command = [str(root/'core/target/debug/koma-core'), 'geometry-probe', '--image', str(image), '--cache', str(cache)]
        if mask_path:
            command += ['--mask', str(mask_path)]
        actual = json.loads(subprocess.check_output(command))
        prefix.with_suffix('.actual.json').write_text(json.dumps(actual, ensure_ascii=False, indent=2))
        prefix.with_suffix('.expected.json').write_text(json.dumps(expected, ensure_ascii=False, indent=2))
        assert actual == expected, f'Geometry mismatch: {prefix}'
        assert page == before
        records.append({'case': name or len(records)+1, 'bubbles': len(bubbles), 'narrations': len(narrations), 'equal': True})
        print(f'Geometry verified: {name or len(records)}', flush=True)
        return actual

    pages = json.loads(args.manifest.read_text())
    stat = (baseline/'.models/comictextdetector.pt').stat()
    for item in ([] if args.synthetic_only else pages):
        image = Path(item['image'])
        key = hashlib.sha256(image.read_bytes() + f'ctd-mask-v2:{stat.st_size}:{stat.st_mtime_ns}'.encode()).hexdigest()
        mask_path = args.baseline_results/'assets/masks'/f'{key}.png'
        page = json.loads(Path(item['cache']).read_text())
        if not mask_path.exists():
            assert not any(old._ready(b) for b in page['blocks'])
            mask_path = None
        compare(page, image, mask_path, name=f"page-{item['page']}")

    # Controlled shapes cover cases that a fixed chapter might not contain.
    for kind in ('oval', 'two-columns', 'joined', 'open', 'gray', 'rectangle', 'bottom-cut'):
        gray = np.full((1000, 700), 80, np.uint8)
        bounds = [[340, 180, 360, 360]]
        tone = 185 if kind == 'gray' else 255
        if kind == 'rectangle':
            cv2.rectangle(gray, (260, 130), (440, 410), tone, -1)
            cv2.rectangle(gray, (260, 130), (440, 410), 20, 3)
        elif kind == 'joined':
            for center in ((285, 270), (415, 270)):
                cv2.ellipse(gray, center, (75, 140), 0, 0, 360, 20, -1)
            for center in ((285, 270), (415, 270)):
                cv2.ellipse(gray, center, (71, 136), 0, 0, 360, tone, -1)
            bounds = [[270, 190, 295, 350], [405, 190, 430, 350]]
        else:
            center = (350, 920) if kind == 'bottom-cut' else (350, 270)
            cv2.ellipse(gray, center, (90, 140), 0, 0, 360, tone, -1)
            cv2.ellipse(gray, center, (90, 140), 0, 0, 360, 20, 3)
            if kind == 'bottom-cut':
                bounds = [[340, 850, 360, 970]]
            if kind == 'open':
                gray[250:295, 430:450] = 255
            if kind == 'two-columns':
                bounds = [[310, 180, 330, 360], [365, 180, 385, 360]]
        blocks = []
        for i, (a,b,c,d) in enumerate(bounds):
            for y in range(b+3, d-8, 16):
                cv2.rectangle(gray, (a+3,y), (c-4,y+5), 20, -1)
            blocks.append({'id': str(i), 'box': [a,b,c,d], 'source': '縦書き', 'translation': '中文翻译', 'vertical': True})
        image = output/f'{kind}.png'
        assert cv2.imwrite(str(image), gray)
        result = compare({'width': 700, 'height': 1000, 'blocks': blocks}, image, name=kind)
        if kind in ('oval', 'two-columns', 'gray', 'bottom-cut'):
            assert len(result['bubbles']) == 1, kind
        if kind == 'two-columns':
            assert result['bubbles'][0]['members'] == [0, 1]

    gray = np.full((1400, 1000), 80, np.uint8)
    cv2.ellipse(gray, (420, 220), (80, 150), 0, 0, 360, 255, -1)
    cv2.ellipse(gray, (280, 360), (80, 140), 0, 0, 360, 255, -1)
    blocks = [{'id': str(i), 'box': box, 'source': '縦書き', 'translation': '中文翻译', 'vertical': True}
              for i, box in enumerate(([395,150,425,300], [265,300,295,420]))]
    image = output/'narrow-neck.png'
    assert cv2.imwrite(str(image), gray)
    result = compare({'width': 1000, 'height': 1400, 'blocks': blocks}, image, name='narrow-neck')
    assert sorted(c['members'] for c in result['bubbles']) == [[0], [1]]

    for kind in ('narration', 'divider', 'gap-art', 'dark-narration'):
        gray = np.full((1000,700), 35 if kind == 'dark-narration' else 205, np.uint8)
        if kind != 'dark-narration':
            for yy in range(0,1000,5):
                for xx in range((yy//5 % 2)*2,700,5):
                    gray[yy,xx] = 45
        blocks = []
        for i,x in enumerate([100,215,330]):
            for yy in range(309,550,26):
                cv2.rectangle(gray,(x+12,yy-3),(x+28,yy+14),255,-1)
                cv2.rectangle(gray,(x+16,yy),(x+23,yy+10),10,-1)
            blocks.append({'id': str(i), 'box': [x,300,x+80,560], 'source': 'テキスト', 'translation': '中文旁白', 'vertical': True})
        if kind == 'divider':
            cv2.line(gray,(196,270),(196,590),0,3)
        if kind == 'gap-art':
            cv2.rectangle(gray,(181,320),(213,545),20,-1)
        image = output/f'{kind}.png'
        assert cv2.imwrite(str(image),gray)
        result = compare({'width': 700, 'height': 1000, 'blocks': blocks}, image, name=kind)
        if kind in ('narration','dark-narration'):
            assert [c['members'] for c in result['narrations']] == [[0,1,2]], kind
        elif kind == 'divider':
            assert all(not (0 in c['members'] and any(i in c['members'] for i in [1,2])) for c in result['narrations'])
        else:
            assert not result['narrations']

    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image', 'cache', 'original_cache'))
    report = {'scope': 'balloon and narration geometry, cached boundaries, filled masks, inner rectangles and source erasure polygons',
              'pages': 0 if args.synthetic_only else len(pages), 'comparisons': len(records), 'original_inputs_unchanged': unchanged,
              'passed': unchanged, 'records': records}
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({k: v for k, v in report.items() if k != 'records'}))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
