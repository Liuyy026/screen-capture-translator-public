#!/usr/bin/env python3
"""Exercise the real JSONL daemon, queue, cancellation and isolated caches.

Fake inference and loopback HTTP cover failure/lifecycle cases. --display also
runs all corpus pages with installed detector/LaMa weights and compares frozen
display replies. It never contacts a shared translation server.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time

from verify_translate_pipeline import Server


class Client:
    def __init__(self, root, directory, weights, worker, seed=None):
        directory.mkdir(parents=True, exist_ok=False)
        self.directory = directory
        command = [str(root/'core/target/release/koma-core'), 'serve',
                   '--cache-root', str(directory/'pages'), '--assets-root', str(directory/'assets'),
                   '--python', sys.executable, '--worker', str(worker), '--weights', str(weights)]
        if seed: command += ['--seed-root', str(seed)]
        self.log = (directory/'stderr.log').open('w')
        environment = dict(os.environ, KOMA_TRANSLATION_API_KEY='fake-secret')
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, text=True, env=environment)
        self.events = queue.Queue()
        self.backlog = []
        self.all_events = []
        self.serial = 0
        def read():
            try:
                for line in self.process.stdout:
                    event = json.loads(line)
                    self.all_events.append(event)
                    self.events.put(event)
            finally:
                self.events.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def send(self, op, **values):
        self.serial += 1
        identifier = values.pop('id', str(self.serial))
        self.process.stdin.write(json.dumps(dict(id=identifier, op=op, **values))+'\n')
        self.process.stdin.flush()
        return identifier

    def wait(self, identifier, events=('result', 'error', 'cancelled'), timeout=60):
        deadline = time.monotonic()+timeout
        def matches(event):
            return event['id'] == identifier and event['event'] in events
        for index, event in enumerate(self.backlog):
            if matches(event): return self.backlog.pop(index)
        while time.monotonic() < deadline:
            event = self.events.get(timeout=max(.01, deadline-time.monotonic()))
            assert event is not None, f'daemon exited: {self.directory}'
            if matches(event): return event
            self.backlog.append(event)
        raise AssertionError(f'timeout: {identifier}, {events}')

    def call(self, op, **values):
        event = self.wait(self.send(op, **values))
        assert event['event'] == 'result', event
        return event['result']

    def close(self, eof=False):
        if self.process.poll() is None:
            if not eof: self.call('shutdown')
            self.process.stdin.close()
        try:
            assert self.process.wait(timeout=15) == 0
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            self.reader.join(timeout=5)
            self.log.close()
        assert 'fake-secret' not in json.dumps(self.all_events)
        assert 'fake-secret' not in (self.directory/'stderr.log').read_text()


FAKE_WORKER = r'''
import argparse, json, os, time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--workspace');p.add_argument('--weights');a=p.parse_args()
weights=Path(a.weights)
assert 'KOMA_TRANSLATION_API_KEY' not in os.environ
with (weights/'processes.jsonl').open('a') as f:f.write(json.dumps({'pid':os.getpid(),'workspace':a.workspace})+'\n')
for line in __import__('sys').stdin:
    r=json.loads(line);op=r['op'];result={}
    with (weights/'calls.jsonl').open('a') as f:f.write(json.dumps({'pid':os.getpid(),'op':op})+'\n')
    if op=='health':result={'protocol':1}
    if op=='ocr':
        if (weights/'hang').exists():
            (weights/'started').write_text(str(os.getpid()))
            time.sleep(60)
        if (weights/'crash').exists():os._exit(23)
        result={'img_width':64,'img_height':64,'blocks':[]}
    print(json.dumps({'id':r['id'],'ok':True,'result':result}),flush=True)
    if op=='shutdown':break
'''


def await_file(path):
    deadline = time.monotonic()+10
    while not path.exists():
        assert time.monotonic() < deadline, path
        time.sleep(.02)


def dead(pid):
    try: os.kill(pid, 0)
    except ProcessLookupError: return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--display', action='store_true')
    parser.add_argument('--baseline-results', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    pages = json.loads(args.manifest.read_text())
    def hashes():
        for p in pages:
            for field, key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(p[field]).read_bytes()).hexdigest() == p[key]
    hashes()
    records = []
    def passed(name, **details):
        records.append(dict(test=name, **details))
        print(json.dumps(records[-1]), flush=True)
    seed = Path(pages[0]['cache']).parent.parent
    fake = output/'fake_worker.py'; fake.write_text(FAKE_WORKER)
    weights = output/'weights'; weights.mkdir()
    # Cache keys need identities even when there is no inference.
    for name in ['comictextdetector.pt','anime-manga-big-lama.pt']:
        (weights/name).write_bytes(b'test identity')
    client = Client(root, output/'runtime', weights, fake, seed)
    try:
        assert client.call('health')['active'] == 0
        loaded = [client.call('load_page', image=p['image'], mode='vision') for p in pages]
        for p, result in zip(pages, loaded):
            assert Path(result['cache']).read_bytes() == Path(p['cache']).read_bytes()
            assert Path(result['cache']).stat().st_mode & 0o777 == 0o600
        assert not (weights/'calls.jsonl').exists()
        passed('35 stable caches copied byte-for-byte, private and lazy', pages=len(loaded))
        job = client.send('process', images=[p['image'] for p in pages], operation='ocr', mode='vision', model='unused')
        reply = client.wait(job)['result']
        assert len(reply['pages']) == 35 and all(p['ok'] for p in reply['pages'])
        assert not (weights/'calls.jsonl').exists()
        passed('35-page Rust queue reuses OCR without inference')
        first = client.call('load_page', image=pages[0]['image'], mode='vision')
        original = first['page']
        block_id = original['blocks'][0]['id']
        updated = client.call('edit', cache=first['cache'], block_id=block_id,
                              values={'translation':'手动校正内容', 'edited':True}, revision=first['revision'])
        expected = json.loads(json.dumps(original))
        expected['blocks'][0].update(translation='手动校正内容', edited=True)
        assert updated['page'] == expected
        stale = client.wait(client.send('edit', cache=first['cache'], block_id=block_id,
                                       values={'translation':'不应写入'}, revision=first['revision']))
        assert stale['event'] == 'error'
        foreign = client.wait(client.send('edit', cache=pages[0]['cache'], block_id=block_id, values={'translation':'不应写入'}))
        assert foreign['event'] == 'error'
        invalid = client.wait(client.send('edit', cache=first['cache'], block_id=block_id, values={'box':[0,0,1,1]}))
        assert invalid['event'] == 'error'
        assert json.loads(Path(first['cache']).read_text()) == expected
        passed('edits preserve metadata, reject stale revisions and stable-cache writes')
        warning_page = client.call('load_page', image=pages[1]['image'], mode='vision')
        warning_path = Path(warning_page['cache'])
        before = warning_path.read_bytes()
        value = warning_page['page']
        value['blocks'][0].update(translation='待翻译', edited=False, error='')
        warning_path.write_text(json.dumps(value))
        stored = warning_path.read_bytes()
        shown = client.call('load_page', image=pages[1]['image'], mode='vision')['page']
        assert shown['blocks'][0]['error'] == '模型返回了占位文字，没有提供实际译文'
        assert warning_path.read_bytes() == stored
        warning_path.write_bytes(before)
        passed('placeholder display warning preserves stored OCR and translation')
        broken = client.call('process', images=[str(output/'missing.webp'), pages[1]['image']],
                             operation='ocr', mode='vision', model='unused')
        assert [p['ok'] for p in broken['pages']] == [False, True]
        passed('missing page does not abort remaining queue')
        # A new copied image has the same hash but no ordinary-mode OCR cache.
        image = pages[0]['image']
        (weights/'hang').touch()
        active = client.send('process', images=[image], operation='ocr', mode='text', model='unused')
        await_file(weights/'started')
        pid = int((weights/'started').read_text())
        queued = client.send('process', images=[pages[1]['image']], operation='ocr', mode='text', model='unused')
        assert client.call('cancel', target=queued)['cancelled']
        assert client.call('cancel', target=active)['cancelled']
        assert client.wait(active, timeout=15)['event'] == 'cancelled'
        assert client.wait(queued, timeout=15)['event'] == 'cancelled'
        assert dead(pid), 'cancelled model process was not reaped'
        (weights/'hang').unlink()
        assert client.call('load_page', image=image, mode='text')['page'] is None
        recovered = client.call('process', images=[image], operation='ocr', mode='text', model='unused')
        assert recovered['pages'][0]['ok']
        pids = [json.loads(s)['pid'] for s in (weights/'processes.jsonl').read_text().splitlines()]
        assert len(pids) == 2 and pids[0] != pids[1]
        passed('active and queued cancellation, child reaping and fresh-worker recovery')
        (weights/'crash').touch()
        failed = client.call('process', images=[pages[1]['image']], operation='ocr', mode='text', model='unused')
        assert not failed['pages'][0]['ok']
        (weights/'crash').unlink()
        retried = client.call('process', images=[pages[1]['image']], operation='ocr', mode='text', model='unused')
        assert retried['pages'][0]['ok']
        passed('model crash reported and next job recovers')
        # Loopback translation uses the existing protocol simulator, never oMLX.
        with Server() as server:
            result = client.call('load_page', image=image, mode='text')
            page = result['page']
            page['blocks'] = [dict(id='1',box=[10,10,50,50],source='こんにちは',translation='',error='',edited=False),
                              dict(id='2',box=[10,10,50,50],source='さようなら',translation='保留手改',error='',edited=True)]
            Path(result['cache']).write_text(json.dumps(page))
            options = dict(images=[image], operation='translate', mode='text', model='same-model',
                           provider='omlx', base_url=server.url, key='fake-secret')
            assert client.call('process', **options)['pages'][0]['ok']
            translated = client.call('load_page', image=image, mode='text')['page']
            assert translated['blocks'][0]['translation'] == '测试中文'
            assert translated['blocks'][1]['translation'] == '保留手改'
            before = len([c for c in server.calls if c['method']=='POST'])
            assert client.call('process', **options, retry=True, block_id='1')['pages'][0]['ok']
            assert len([c for c in server.calls if c['method']=='POST']) == before+1
            final = client.call('load_page', image=image, mode='text')['page']
            assert final['blocks'][1]['translation'] == '保留手改'
        passed('loopback translation and per-block retry preserve manual edits')
        client.process.stdin.write('{bad json}\n'); client.process.stdin.flush()
        assert client.wait(None)['event'] == 'error'
        assert client.call('health')['protocol'] == 1
        passed('malformed input does not break subsequent requests')
    finally:
        client.close()
    for row in (weights/'processes.jsonl').read_text().splitlines():
        value = json.loads(row)
        assert dead(value['pid']) and not Path(value['workspace']).exists()
    passed('shutdown leaves no owned model process or temporary workspace')
    eof = Client(root, output/'eof-runtime', weights, fake)
    (weights/'started').unlink(); (weights/'hang').touch()
    eof.send('process', images=[pages[0]['image']], operation='ocr', mode='text', model='unused')
    await_file(weights/'started'); pid = int((weights/'started').read_text())
    eof.close(eof=True)
    assert dead(pid)
    (weights/'hang').unlink()
    passed('stdin EOF cancels running inference and exits cleanly')
    if args.display:
        assert args.baseline_results
        import cv2
        import numpy as np
        real = Client(root, output/'real-display', root/'.models', root/'core/python/model_worker.py', seed)
        try:
            for p in pages:
                cache = real.call('load_page', image=p['image'], mode='vision')['cache']
                request = dict(image=p['image'], cache=cache)
                actual = real.wait(real.send('display', **request), timeout=180)['result']
                assert not actual['cache_hit']
                expected = json.loads((args.baseline_results/'regions'/f"{p['page']:05}.json").read_text())
                reply = actual['reply']
                (real.directory/f"{p['page']:05}.json").write_text(json.dumps(reply))
                a_png = [r.pop('background_png') for r in reply['regions']]
                b_png = [r.pop('background_png') for r in expected['regions']]
                assert reply == expected
                for a,b in zip(a_png,b_png):
                    decode = lambda s:cv2.imdecode(np.frombuffer(base64.b64decode(s),np.uint8),cv2.IMREAD_UNCHANGED)
                    assert np.array_equal(decode(a),decode(b))
                for _ in range(3):
                    warm = real.call('display', **request)
                    assert warm['cache_hit']
                    assert warm['reply'] == json.loads((real.directory/f"{p['page']:05}.json").read_text())
                passed('real daemon display cold/warm parity', page=p['page'], regions=len(a_png))
        finally:
            real.close()
    hashes()
    report = dict(passed=True, records=records, original_inputs_unchanged=True,
                  scope='Rust daemon lifecycle, queue, private edits and cache; real display only with --display')
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({k:v for k,v in report.items() if k!='records'}))


if __name__ == '__main__': main()
