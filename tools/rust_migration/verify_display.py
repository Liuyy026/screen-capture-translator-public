#!/usr/bin/env python3
"""Compare complete Rust display replies with the frozen 35-page baseline.

Runs actual detector and restoration models through the model-only worker.
Region assembly, masks, cleanup and compositing all run in the Rust core.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--baseline-results',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    (output/'regions').mkdir()
    (output/'logs').mkdir()
    import cv2
    import numpy as np
    pages = json.loads(args.manifest.read_text())
    records = []
    for item in pages:
        number = item['page']
        for key in ('image','cache','original_cache'):
            assert hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
        command = [str(root/'core/target/release/koma-core'),'display','--image',item['image'],'--cache',item['cache'],
                   '--python',str(root/'macos/.venv/bin/python'),'--worker',str(root/'core/python/model_worker.py'),'--weights',str(root/'.models')]
        with (output/'logs'/f'{number:05}.log').open('w') as log:
            run = subprocess.run(command,stdout=subprocess.PIPE,stderr=log,check=True)
        (output/'regions'/f'{number:05}.json').write_bytes(run.stdout)
        actual = json.loads(run.stdout)
        expected = json.loads((args.baseline_results/'regions'/f'{number:05}.json').read_text())
        actual_pngs = [r.pop('background_png') for r in actual['regions']]
        expected_pngs = [r.pop('background_png') for r in expected['regions']]
        (output/f'{number:05}.actual-metadata.json').write_text(json.dumps(actual,ensure_ascii=False,indent=2))
        (output/f'{number:05}.expected-metadata.json').write_text(json.dumps(expected,ensure_ascii=False,indent=2))
        assert actual == expected, f'Region metadata differs on page {number}'
        differing = 0
        png_equal = 0
        outside = 0
        color = cv2.imread(item['image'])
        has_color = bool(np.max(np.ptp(color.astype(np.int16),axis=2)) > 8)
        original = color if has_color else cv2.cvtColor(cv2.cvtColor(color,cv2.COLOR_BGR2GRAY),cv2.COLOR_GRAY2BGR)
        for i,(a,b) in enumerate(zip(actual_pngs,expected_pngs)):
            a,b = base64.b64decode(a),base64.b64decode(b)
            ap = cv2.imdecode(np.frombuffer(a,np.uint8),cv2.IMREAD_UNCHANGED)
            ep = cv2.imdecode(np.frombuffer(b,np.uint8),cv2.IMREAD_UNCHANGED)
            assert ap.shape == ep.shape
            count = int(np.count_nonzero(ap != ep))
            differing += count
            png_equal += a == b
            x,y,r,d = actual['regions'][i]['background_box']
            invisible = ap[:,:,3] == 0
            outside += int(np.count_nonzero(ap[:,:,:3][invisible] != original[y:d,x:r][invisible]))
            if count:
                (output/f'{number:05}-{i}.actual.png').write_bytes(a)
                (output/f'{number:05}-{i}.expected.png').write_bytes(b)
        record = {'page':number,'regions':len(actual_pngs),'metadata_equal':True,
                  'different_pixel_values':differing,'outside_alpha_changes':outside,'identical_pngs':png_equal}
        records.append(record)
        (output/'progress.json').write_text(json.dumps(records,indent=2))
        assert differing == 0 and outside == 0, f'Repaired pixels differ on page {number}'
        print(f'Full display verified: page {number}, {len(actual_pngs)} regions',flush=True)
    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image','cache','original_cache'))
    report = {'scope':'full native display region metadata and repaired BGRA pixels; Swift text layout not run here',
              'pages':len(pages),'regions':sum(r['regions'] for r in records),'original_inputs_unchanged':unchanged,'passed':unchanged,'records':records}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k != 'records'}))
    if not unchanged:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
