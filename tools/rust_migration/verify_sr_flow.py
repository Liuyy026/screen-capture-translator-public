#!/usr/bin/env python3
"""Real local OCR/inpainting on prepared images; deterministic loopback translations."""
import argparse,fcntl,hashlib,json,sys,time
from pathlib import Path
sys.path.insert(0,str(Path(__file__).parent))
from verify_runtime import Client
from verify_translate_pipeline import Server

def main():
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--image',type=Path,required=True);a=p.parse_args()
    root=Path(__file__).resolve().parents[2];out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
    original=hashlib.sha256(a.image.read_bytes()).hexdigest();records=[]
    with (root.parent/'gpu-test.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        client=Client(root,out/'daemon',root/'.models',root/'core/python/model_worker.py')
        # Client's interpreter must be the project venv (run this script with it).
        try:
            with Server() as server:
                for engine in ['off','metalfx']:
                    settings=dict(engine=engine,target=1440,axis='long_edge',preset='standard')
                    t=time.perf_counter()
                    prepared=client.call('load_page',image=str(a.image.resolve()),super_resolution=settings)
                    job=client.send('process',images=[str(a.image.resolve())],operation='both',model='same-model',provider='omlx',base_url=server.url+'/v1',super_resolution=settings)
                    reply=client.wait(job,timeout=180)['result'];assert reply['pages'][0]['ok'],reply
                    loaded=client.call('load_page',image=str(a.image.resolve()),super_resolution=settings)
                    page=loaded['page'];assert page['blocks'] and any(b['translation']=='测试中文' for b in page['blocks']),page
                    display=client.call('display',image=loaded['effective_image'],cache=loaded['cache'])
                    (out/f'{engine}-page.json').write_text(json.dumps(page,ensure_ascii=False,indent=2))
                    (out/f'{engine}-display.json').write_text(json.dumps(display,ensure_ascii=False))
                    records.append(dict(engine=engine,seconds=time.perf_counter()-t,effective=loaded['effective_image'],cache=loaded['cache'],blocks=len(page['blocks']),width=page['width'],height=page['height']))
                assert records[0]['cache']!=records[1]['cache']
        finally:client.close()
    assert hashlib.sha256(a.image.read_bytes()).hexdigest()==original
    (out/'report.json').write_text(json.dumps(dict(passed=True,records=records,note='Real detector/OCR/LaMa; mock Chinese translations prove transport and coordinates only, not translation quality. Timings include lazy initialization.'),indent=2));print(json.dumps(records))
if __name__=='__main__':main()
