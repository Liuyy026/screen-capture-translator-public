#!/usr/bin/env python3
"""Compare native cleanup masks with the exact masks used by the old renderer.

The Python reference runs with an identity model callback: this validates ink
selection and texture routing only, without rerunning LaMa or translation.
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
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    baseline = args.baseline_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(baseline))
    import cv2
    import numpy as np
    from macos.backend import reading_overlay as reading
    from macos.backend.reading_background import background_patch
    pages = json.loads(args.manifest.read_text())
    stat = (baseline/'.models/comictextdetector.pt').stat()
    records = []

    def run_case(name, image, page=None, source=None, direct=None):
        directory = output/str(name)
        directory.mkdir()
        color = cv2.imread(str(image))
        gray = cv2.cvtColor(color, cv2.COLOR_BGR2GRAY)
        requests, expected = [], []
        actual_source = None
        def observe(gray, boxes, **kwargs):
            nonlocal actual_source
            request = {'boxes': boxes, 'textured': bool(kwargs.get('textured', False))}
            if kwargs.get('clip') is not None:
                mask, origin = kwargs['clip']
                path = directory/f'clip-{len(requests)}.png'
                assert cv2.imwrite(str(path), mask)
                request.update(clip_path=str(path), clip_origin=list(origin))
            supplied = kwargs.get('source_mask')
            if supplied is not None:
                if actual_source is not None:
                    np.testing.assert_array_equal(actual_source, supplied)
                else:
                    actual_source = supplied.copy()
            captured = {}
            def profile(frame, event, arg):
                if event == 'return' and frame.f_code is background_patch.__code__:
                    local = frame.f_locals
                    captured.update(background_box=[local[k] for k in ('left','top','right','bottom')],
                                    textured=bool(local['textured']))
                    for key, variable in [('patch_sha256','patch'),('allowed_sha256','allowed'),('mask_sha256','byte_mask')]:
                        captured[key] = hashlib.sha256(local[variable].tobytes()).hexdigest()
            previous = sys.getprofile()
            sys.setprofile(profile)
            try:
                kwargs['restorer'] = lambda crop, mask: crop.copy()
                result = background_patch(gray, boxes, **kwargs)
            finally:
                sys.setprofile(previous)
            assert captured
            requests.append(request)
            expected.append(captured)
            return result
        original = reading.background_patch
        reading.background_patch = observe
        try:
            if direct is not None:
                for boxes, kwargs in direct:
                    observe(gray, boxes, source_mask=source, **kwargs)
            else:
                before = copy.deepcopy(page)
                reading.reading_regions(page, gray, source_mask=source)
                assert page == before
        finally:
            reading.background_patch = original
        request_file = directory/'requests.json'
        request_file.write_text(json.dumps(requests))
        command = [str(root/'core/target/debug/koma-core'), 'background-probe', '--image', str(image), '--requests', str(request_file)]
        if actual_source is not None:
            source_path = directory/'source.png'
            assert cv2.imwrite(str(source_path), actual_source)
            command += ['--mask', str(source_path)]
        actual = json.loads(subprocess.check_output(command))
        (directory/'expected.json').write_text(json.dumps(expected,indent=2))
        (directory/'actual.json').write_text(json.dumps(actual,indent=2))
        assert actual == expected, f'Background selection differs: {directory}'
        records.append({'case': name, 'regions': len(requests), 'equal': True})
        print(f'Background masks verified: {name} ({len(requests)} regions)', flush=True)

    for item in pages:
        image = Path(item['image'])
        key = hashlib.sha256(image.read_bytes()+f'ctd-mask-v2:{stat.st_size}:{stat.st_mtime_ns}'.encode()).hexdigest()
        path = args.baseline_results/'assets/masks'/f'{key}.png'
        page = json.loads(Path(item['cache']).read_text())
        source = cv2.imread(str(path),cv2.IMREAD_GRAYSCALE) if path.exists() else None
        if source is None:
            assert not any(reading._ready(b) for b in page['blocks'])
        run_case(f'page-{item["page"]}',image,page,source)

    # Exercise the non-neural path, light lettering and clipping, independently
    # of whether this chapter happens to trigger them.
    for kind in ('plain','halftone','dark','crossing-art'):
        gray = np.full((300,300),35 if kind == 'dark' else (205 if kind == 'halftone' else 255),np.uint8)
        if kind == 'halftone':
            gray[::5,::5] = 45
        for y in range(95,195,22):
            cv2.rectangle(gray,(138,y),(158,y+14),255,-1)
            cv2.rectangle(gray,(142,y+3),(151,y+11),10,-1)
        if kind == 'crossing-art':
            cv2.line(gray,(165,40),(165,270),0,3)
        image = output/f'{kind}.png'
        assert cv2.imwrite(str(image),gray)
        clip = np.zeros((220,180),np.uint8)
        cv2.ellipse(clip,(90,110),(85,105),0,0,360,1,-1)
        direct = [([[125,85,160,210]],{'textured': kind != 'plain'}),
                  ([[125,85,160,210]],{'textured': kind != 'plain','clip': (clip,(60,40))})]
        run_case(kind,image,direct=direct)
        source = np.zeros_like(gray)
        source[90:205,135:160] = 255
        run_case(kind+'-detector',image,source=source,direct=direct)

    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image','cache','original_cache'))
    report = {'scope':'native background selection only, not restored pixels', 'pages':len(pages),
              'cases':len(records),'regions':sum(r['regions'] for r in records),
              'original_inputs_unchanged':unchanged,'passed':unchanged,'records':records}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k != 'records'}))
    if not unchanged:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
