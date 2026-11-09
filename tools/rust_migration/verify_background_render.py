#!/usr/bin/env python3
"""Compare actual local repaired pixels, alpha and contrast before/after Rust.

Uses requests captured by verify_background_plan.py. No source writes or
translation requests. Timing here is validation, never a performance claim.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root',type=Path,required=True)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--plan-results',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--with-model',action='store_true')
    parser.add_argument('--synthetic-only',action='store_true')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    baseline = args.baseline_root.resolve()
    plans = args.plan_results.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    sys.path.insert(0,str(baseline))
    import cv2
    import numpy as np
    from macos.backend.reading_background import background_patch
    from macos.backend.reading_assets import LocalRestorer
    restorer = LocalRestorer(baseline/'.models/anime-manga-big-lama.pt') if args.with_model else None
    pages = json.loads(args.manifest.read_text())
    records = []

    def run_case(name,image,request_dir,color_override=None):
        directory = output/name
        directory.mkdir()
        color = cv2.imread(str(image))
        gray = cv2.cvtColor(color,cv2.COLOR_BGR2GRAY)
        has_color = bool(np.max(np.ptp(color.astype(np.int16),axis=2)) > 8) if color_override is None else color_override
        source_file = request_dir/'source.png'
        source = cv2.imread(str(source_file),cv2.IMREAD_GRAYSCALE) if source_file.exists() else None
        requests = json.loads((request_dir/'requests.json').read_text())
        for request in requests:
            request['color'] = has_color
        request_file = directory/'requests.json'
        request_file.write_text(json.dumps(requests))
        command = [str(root/'core/target/release/koma-core'),'background-render','--image',str(image),'--requests',str(request_file)]
        if source is not None:
            command += ['--mask',str(source_file)]
        if args.with_model:
            command += ['--python',str(root/'macos/.venv/bin/python'),'--worker',str(root/'core/python/model_worker.py'),'--weights',str(root/'.models')]
        with (directory/'worker.log').open('w') as log:
            run = subprocess.run(command,stdout=subprocess.PIPE,stderr=log,check=True)
        actual = json.loads(run.stdout)
        assert len(actual) == len(requests)
        for i,(request,result) in enumerate(zip(requests,actual)):
            clip = None
            if 'clip_path' in request:
                clip = (cv2.imread(request['clip_path'],cv2.IMREAD_GRAYSCALE),request['clip_origin'])
            expected = background_patch(gray,request['boxes'],clip=clip,textured=request.get('textured',False),
                                        color=color if has_color else None,source_mask=source,restorer=restorer)
            expected_bytes = base64.b64decode(expected['background_png'])
            actual_bytes = base64.b64decode(result['background_png'])
            (directory/f'{i:03}.expected.png').write_bytes(expected_bytes)
            (directory/f'{i:03}.actual.png').write_bytes(actual_bytes)
            ep = cv2.imdecode(np.frombuffer(expected_bytes,np.uint8),cv2.IMREAD_UNCHANGED)
            ap = cv2.imdecode(np.frombuffer(actual_bytes,np.uint8),cv2.IMREAD_UNCHANGED)
            assert ep.shape == ap.shape
            delta = np.abs(ap.astype(np.int16)-ep.astype(np.int16))
            a,b,c,d = result['background_box']
            original = color[b:d,a:c] if has_color else cv2.cvtColor(gray[b:d,a:c],cv2.COLOR_GRAY2BGR)
            outside = ap[:,:,3] == 0
            record = {'case':name,'region':i,'max_delta':int(delta.max()),'different_values':int(np.count_nonzero(delta)),
                      'outside_alpha_changes':int(np.count_nonzero(ap[:,:,:3][outside] != original[outside])),
                      'contrast_equal':result['dark_background'] == expected['dark_background'],
                      'bounds_equal':result['background_box'] == expected['background_box'],
                      'png_bytes_equal':actual_bytes == expected_bytes,'color':has_color}
            records.append(record)
            (directory/'comparisons.json').write_text(json.dumps(records[-(i+1):],indent=2))
            assert record['max_delta'] == 0 and record['outside_alpha_changes'] == 0 and record['contrast_equal'] and record['bounds_equal'], f'Render mismatch: {name}/{i}'
        print(f'Repaired pixels verified: {name} ({len(requests)} regions)',flush=True)

    if not args.synthetic_only:
        for item in pages:
            run_case(f'page-{item["page"]}',Path(item['image']),plans/f'page-{item["page"]}')
    for kind in ('plain','halftone','dark','crossing-art'):
        for suffix in ('','-detector'):
            run_case(kind+suffix,plans/f'{kind}.png',plans/(kind+suffix))
    # Preserve original chroma outside alpha; compare both model and non-model
    # color reconstruction paths on deliberately saturated input.
    y,x = np.indices((300,300))
    color = np.stack(((x*3)%256,(y*2)%256,((x+y)*2)%256),axis=2).astype(np.uint8)
    gray = cv2.imread(str(plans/'plain.png'),cv2.IMREAD_GRAYSCALE)
    color[gray < 100] = 10
    color_file = output/'color.png'
    assert cv2.imwrite(str(color_file),color)
    run_case('color',color_file,plans/'halftone-detector',True)
    empty_dir = output/'empty-input'
    empty_dir.mkdir()
    (empty_dir/'requests.json').write_text(json.dumps([{'boxes':[[0,0,2,2]],'textured':False}]))
    # Empty source mask gives an unmodified patch but still exercises encoding.
    assert cv2.imwrite(str(empty_dir/'source.png'),np.zeros((300,300),np.uint8))
    run_case('empty-mask',color_file,empty_dir,True)
    unchanged = all(hashlib.sha256(Path(item[key]).read_bytes()).hexdigest() == item['cache_hash' if key == 'original_cache' else key+'_hash']
                    for item in pages for key in ('image','cache','original_cache'))
    report = {'scope':'local repaired BGRA pixels, contrast and bounds; not whole-page layout',
              'with_model':args.with_model,'pages':0 if args.synthetic_only else len(pages),
              'regions':len(records),'original_inputs_unchanged':unchanged,'passed':unchanged,'records':records}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k != 'records'}))
    if not unchanged:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
