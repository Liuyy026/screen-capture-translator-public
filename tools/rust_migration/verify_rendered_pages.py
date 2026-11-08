#!/usr/bin/env python3
"""Render native display replies with the unchanged Swift reader layout.

Compiles the existing noninteractive review helper into the private output
directory. Does not launch or replace the installed application.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import unicodedata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--native-results',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    import cv2
    import numpy as np
    pages = json.loads(args.manifest.read_text())
    native = args.native_results.resolve()
    # Fail early if the full processing run did not pass all pages.
    result = json.loads((native/'report.json').read_text())
    assert result['passed'] and result['pages'] == len(pages)
    renderer = output/'render-review'
    with (output/'build.log').open('w') as log:
        subprocess.run(['xcrun','swiftc','-O','-o',str(renderer),
            str(root/'macos/Sources/ReadingLayout.swift'),str(root/'macos/Sources/ReadingOverlayView.swift'),
            str(root/'macos/tools/RenderReview.swift')],stdout=log,stderr=log,check=True)
    records = []
    for item in pages:
        n = item['page']
        prefix = output/f'{n:05}'
        regions_path = native/'regions'/f'{n:05}.json'
        with (output/f'{n:05}.log').open('w') as log:
            subprocess.run([str(renderer),item['image'],item['cache'],str(regions_path),str(prefix)],stdout=log,stderr=log,check=True)
        audit = json.loads(prefix.with_suffix('.json').read_text())
        baseline_audit = json.loads(Path(item['baseline_render']).with_suffix('.json').read_text())
        actual = cv2.imread(str(prefix.with_suffix('.png')),cv2.IMREAD_UNCHANGED)
        expected = cv2.imread(item['baseline_render'],cv2.IMREAD_UNCHANGED)
        assert actual.shape == expected.shape
        page = json.loads(Path(item['cache']).read_text())
        payload = json.loads(regions_path.read_text())
        by_id = {r['id']:r for r in payload['regions']}
        crossings,text_errors = [],[]
        for block in audit['items']:
            polygon = np.asarray(by_id[block['id']]['points'],np.float32)
            if ''.join(g['text'] for g in block['glyphs']) != ''.join(line.strip() for line in block['text'].splitlines()):
                text_errors.append(block['id'])
            for glyph in block['glyphs']:
                a,b,c,d = glyph['bounds']
                outline = max(.5,block['font']*.055)
                if any(cv2.pointPolygonTest(polygon,(float(x),float(y)),False) < 0
                       for x in np.linspace(a-outline,c+outline,3) for y in np.linspace(b-outline,d+outline,3)):
                    crossings.append({'region':block['id'],'text':glyph['text']})
        shown = set(audit['rendered_ids'])
        normalize = lambda s: unicodedata.normalize('NFKC',s).strip()
        missing = [b['id'] for b in page['blocks'] if b['id'] not in shown and b['id'] not in payload.get('preserved',{})
                   and b['translation'].strip() and not b.get('error') and normalize(b['source']) != normalize(b['translation'])]
        delta = np.abs(actual.astype(np.int16)-expected.astype(np.int16))
        record = {'page':n,'displayed':len(shown),'blocks':len(page['blocks']),
                  'different_pixel_values':int(np.count_nonzero(delta)),'max_delta':int(delta.max()),
                  'layout_audit_equal':audit == baseline_audit,'glyph_crossings':crossings,'text_errors':text_errors,'missing_translations':missing}
        records.append(record)
        (output/'progress.json').write_text(json.dumps(records,ensure_ascii=False,indent=2))
        print(json.dumps(record,ensure_ascii=False),flush=True)
    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image','cache','original_cache'))
    passed = unchanged and all(r['different_pixel_values'] == 0 and r['layout_audit_equal'] and not r['glyph_crossings'] and not r['text_errors'] and not r['missing_translations'] for r in records)
    report = {'scope':'full-page Swift raster output, original translations, glyph layout and contour safety',
              'pages':len(pages),'displayed':sum(r['displayed'] for r in records),'blocks':sum(r['blocks'] for r in records),
              'original_inputs_unchanged':unchanged,'passed':passed,'records':records}
    (output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k != 'records'},ensure_ascii=False))
    if not passed:
        raise SystemExit('Full-page comparison failed; inspect report.json')


if __name__ == '__main__':
    main()
