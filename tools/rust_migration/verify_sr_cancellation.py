#!/usr/bin/env python3
"""Controlled helper process tree; no real GPU work or shared backend mutation."""
import argparse,json,os,sys,time
from pathlib import Path
sys.path.insert(0,str(Path(__file__).parent))
from verify_runtime import Client,await_file,dead

def main():
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--image',type=Path,required=True);a=p.parse_args()
    root=Path(__file__).resolve().parents[2];out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
    fixture=out/'fixture';helper=fixture/'macos/.build/koma-sr-metalfx';helper.parent.mkdir(parents=True)
    worker=fixture/'core/python/model_worker.py';worker.parent.mkdir(parents=True);worker.write_text('')
    helper.write_text('''#!/usr/bin/env python3
import json,subprocess,sys,time,os
from pathlib import Path
if sys.argv[1:] == ['--capabilities']:
 print(json.dumps(dict(engine='metalfx',available=True,presets=['standard'],reason='',backend_version='controlled-cancellation-test')));sys.exit(0)
p=Path(__file__).parents[2]
child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)'])
(p/'pids.json').write_text(json.dumps([os.getpid(),child.pid]))
time.sleep(120)
''');helper.chmod(0o755)
    client=Client(root,out/'daemon',root/'.models',worker)
    try:
        settings=dict(engine='metalfx',target=1440,axis='long_edge',preset='standard')
        job=client.send('load_page',image=str(a.image.resolve()),super_resolution=settings)
        await_file(fixture/'pids.json');pids=json.loads((fixture/'pids.json').read_text())
        assert client.call('cancel',target=job)['cancelled'];assert client.wait(job)['event']=='cancelled'
        deadline=time.monotonic()+10
        while not all(dead(pid) for pid in pids) and time.monotonic()<deadline:time.sleep(.03)
        assert all(dead(pid) for pid in pids),pids
        off=client.call('load_page',image=str(a.image.resolve()));assert off['effective_image']==str(a.image.resolve())
        assert not list((out/'daemon/super-resolution-v1').rglob('effective.png'))
        assert not list((out/'daemon/super-resolution-v1').rglob('inference-*'))
        assert not any(e.get('id')==job and e.get('event')=='result' for e in client.all_events)
        (fixture/'pids.json').unlink()
        processing=client.send('process',images=[str(a.image.resolve())],operation='ocr',model='unused',super_resolution=settings)
        await_file(fixture/'pids.json');processing_pids=json.loads((fixture/'pids.json').read_text())
        assert client.call('cancel',target=processing)['cancelled'];assert client.wait(processing)['event']=='cancelled'
        deadline=time.monotonic()+10
        while not all(dead(pid) for pid in processing_pids) and time.monotonic()<deadline:time.sleep(.03)
        assert all(dead(pid) for pid in processing_pids)
        assert not any(e.get('id')==processing and e.get('event')=='page_finished' for e in client.all_events)
        pids+=processing_pids
    finally:client.close()
    (out/'report.json').write_text(json.dumps(dict(passed=True,checks=['cancelled helper and descendant reaped','no late result or published output','temporary inference directory removed','off request recovers immediately','processing-stage enhancement cancellation has no late page result'],pids=pids),indent=2));print('passed')
if __name__=='__main__':main()
