#!/usr/bin/env python3
"""Fresh OCR parity using installed weights, private outputs and immutable inputs.

Baseline and native runs are sequential, so their MPS models never overlap.
This checks model outputs and native page assembly, not OCR accuracy corrections.
"""
import argparse
import gc
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root',type=Path,required=True)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--pages',default='all')
    parser.add_argument('--phase',choices=['compare','baseline'],default='compare')
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    output=args.output.resolve()
    pages=json.loads(args.manifest.read_text())
    if args.pages!='all':pages=[p for p in pages if p['page'] in {int(n) for n in args.pages.split(',')}]
    if args.phase=='baseline':
        sys.path.insert(0,str(args.baseline_root.resolve()))
        from backend import worker
        for entry in pages:
            page=worker.recognize(entry['image'],lambda _:None)
            worker.atomic_json(output/'baseline'/f"{entry['page']:05}.json",page)
            print(json.dumps({'baseline_page':entry['page'],'blocks':len(page['blocks'])}),flush=True)
            del page
            gc.collect()
            if 'torch' in sys.modules and sys.modules['torch'].backends.mps.is_available():
                sys.modules['torch'].mps.empty_cache()
        return
    output.mkdir(parents=True,exist_ok=False)
    def hashes():
        for entry in pages:
            for field,key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(entry[field]).read_bytes()).hexdigest()==entry[key]
    hashes()
    command=[sys.executable,str(Path(__file__).resolve()),'--baseline-root',str(args.baseline_root.resolve()),
             '--manifest',str(args.manifest.resolve()),'--output',str(output),'--pages',args.pages,'--phase','baseline']
    with (output/'baseline.log').open('w') as log:
        subprocess.run(command,stderr=log,check=True)
    print('Baseline OCR finished; starting native persistent OCR',flush=True)
    with (output/'native.log').open('w') as log:
        result=subprocess.run([str(root/'core/target/release/koma-core'),'recognize',*[p['image'] for p in pages],
            '--output',str(output/'native'),'--python',str(root/'macos/.venv/bin/python'),
            '--worker',str(root/'core/python/model_worker.py'),'--weights',str(root/'.models')],
            stdout=subprocess.PIPE,stderr=log,check=True)
    (output/'native-replies.json').write_bytes(result.stdout)
    records=[]
    for entry,reply in zip(pages,json.loads(result.stdout)):
        expected=json.loads((output/'baseline'/f"{entry['page']:05}.json").read_text())
        actual=json.loads(Path(reply['cache']).read_text())
        equal=json.dumps(expected,sort_keys=True)==json.dumps(actual,sort_keys=True)
        records.append({'page':entry['page'],'blocks':len(actual['blocks']),'equal':equal})
    hashes()
    passed=len(records)==len(pages) and all(r['equal'] for r in records)
    report={'pages':len(pages),'blocks':sum(r['blocks'] for r in records),'passed':passed,
            'scope':'fresh model OCR plus native region/grouping/cache assembly; no translation or source-cache changes',
            'input_hashes_unchanged':True,'records':records}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps(report))
    if not passed:raise SystemExit('OCR/page assembly differs; inspect private baseline/native files')


if __name__=='__main__':main()
