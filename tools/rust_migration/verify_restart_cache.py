#!/usr/bin/env python3
"""Check all 35 persisted display replies in a new process with no model worker."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

from verify_runtime import Client


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--benchmark',type=Path,required=True)
    parser.add_argument('--reference',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    pages=json.loads(args.manifest.read_text())
    def hashes():
        for page in pages:
            for field,key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(page[field]).read_bytes()).hexdigest()==page[key]
    hashes()
    # No executable model script is provided: a false miss or hidden inference
    # would fail instead of quietly rebuilding an asset.
    client=Client(root,output/'runtime',root/'.models',output/'does-not-exist.py')
    shutil.copytree(args.benchmark/'runtime/assets',client.directory/'assets',dirs_exist_ok=True)
    shutil.copytree(args.benchmark/'runtime/pages',client.directory/'pages',dirs_exist_ok=True)
    (output/'regions').mkdir()
    records=[]
    try:
        client.call('health')
        for page in pages:
            cache=client.directory/'pages'/(page['image_hash']+'.json')
            reply=client.call('display',image=page['image'],cache=str(cache))
            assert reply['cache_hit']
            expected=json.loads((args.reference/'regions'/f"{page['page']:05}.json").read_text())
            assert reply['reply']==expected
            (output/'regions'/f"{page['page']:05}.json").write_text(json.dumps(reply['reply']))
            records.append({'page':page['page'],'cache_hit':True,'regions':len(reply['reply']['regions'])})
    finally:client.close()
    hashes()
    report={'passed':True,'pages':len(records),'records':records,'model_worker_unavailable':True,
            'input_hashes_unchanged':True,'scope':'new Rust daemon, copied persisted assets, no inference available'}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k!='records'}))


if __name__=='__main__':main()
