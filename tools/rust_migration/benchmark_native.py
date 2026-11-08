#!/usr/bin/env python3
"""Time the production Rust daemon with fixed translations and private assets.

Includes request/response transport and client JSON decoding, excludes fixture
copying, compilation and validation. Startup is recorded separately. RSS samples
include the daemon and its owned inference child; they are not a GPU peak metric.
"""
import argparse
import hashlib
import json
import platform
from pathlib import Path
import statistics
import subprocess
import threading
import time

from verify_runtime import Client


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--reference',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    pages=json.loads(args.manifest.read_text())
    def hashes():
        for p in pages:
            for field,key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(p[field]).read_bytes()).hexdigest()==p[key]
    hashes()
    started=time.perf_counter()
    client=Client(root,output/'runtime',root/'.models',root/'core/python/model_worker.py')
    client.call('health')
    startup=time.perf_counter()-started
    cache_root=client.directory/'pages'
    for p in pages:(cache_root/(p['image_hash']+'.json')).write_bytes(Path(p['cache']).read_bytes())
    stop=threading.Event();memory=[]
    def monitor():
        while not stop.is_set():
            lines=subprocess.check_output(['ps','-axo','pid=,ppid=,rss='],text=True).splitlines()
            rows=[tuple(map(int,line.split())) for line in lines if len(line.split())==3]
            owned={client.process.pid}
            while True:
                expanded=owned|{pid for pid,ppid,_ in rows if ppid in owned}
                if expanded==owned:break
                owned=expanded
            selected=[(pid,rss) for pid,_,rss in rows if pid in owned]
            memory.append({'seconds':time.perf_counter()-started,'rss_bytes':sum(rss*1024 for _,rss in selected),
                           'processes':len(selected)})
            stop.wait(.25)
    monitor_thread=threading.Thread(target=monitor,daemon=True);monitor_thread.start()
    records=[];expected={}
    try:
        for iteration in range(4):
            for p in pages:
                start=time.perf_counter()
                result=client.call('display',image=p['image'],cache=str(cache_root/(p['image_hash']+'.json')))
                seconds=time.perf_counter()-start
                assert result['cache_hit']==(iteration!=0)
                reply=result['reply']
                normalized=hashlib.sha256(json.dumps(reply,sort_keys=True).encode()).hexdigest()
                if iteration==0:
                    expected[p['page']]=normalized
                    baseline=json.loads((args.reference/'regions'/f"{p['page']:05}.json").read_text())
                    # Native PNG encoding is already pixel-verified separately;
                    # for this input the full reply, including PNG, must match.
                    assert reply==baseline, f"page {p['page']} differs"
                else:assert expected[p['page']]==normalized
                item={'page':p['page'],'iteration':iteration,'phase':'fresh-assets' if iteration==0 else 'persisted-cache',
                      'seconds':seconds,'reply_bytes':len(json.dumps(reply).encode())}
                records.append(item);print(json.dumps(item),flush=True)
    finally:
        stop.set();monitor_thread.join();client.close()
    hashes()
    summary={phase:{'samples':len(values),'total_seconds':sum(values),'median_seconds':statistics.median(values),'max_seconds':max(values)}
             for phase in ['fresh-assets','persisted-cache']
             if (values:=[r['seconds'] for r in records if r['phase']==phase])}
    report={'passed':True,'backend':'Rust persistent service','records':records,'summary':summary,'startup_seconds':startup,
            'fresh_total_with_startup_seconds':startup+summary['fresh-assets']['total_seconds'],
            'environment':{'platform':platform.platform()},'system_file_cache_flushed':False,
            'scope':'fixed-translation display only, including native transport and client JSON decoding; no OCR/translation inference',
            'memory':{'sample_interval_seconds':.25,'max_sampled_rss_bytes':max(r['rss_bytes'] for r in memory),
                      'max_owned_processes':max(r['processes'] for r in memory),'samples':memory,
                      'caveat':'sum of sampled process RSS; not peak physical/GPU memory and not directly comparable to time -l peak RSS'},
            'input_hashes_unchanged':True}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps(summary))


if __name__=='__main__':main()
