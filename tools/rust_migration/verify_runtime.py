#!/usr/bin/env python3
"""Exercise the real JSONL daemon, queue, cancellation and isolated caches.

Synthetic pages, fake inference and loopback HTTP cover failure/lifecycle
cases. No private corpus, model weights or shared translation server is used.
"""
import argparse
from fixtures import make_pages, core_binary
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
        command = [str(core_binary(root)), 'serve',
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
        result={'img_width':300,'img_height':300,'blocks':[]}
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
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    pages = make_pages(output/'fixtures')
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
            imported = json.loads(Path(result['cache']).read_bytes())
            review_signature = imported.pop('ocr_review_signature')
            assert len(review_signature) == 64
            original = json.loads(Path(p['cache']).read_bytes())
            inputs = [[b['id'], b['box'], b['source']] for b in original['blocks']]
            receipt_inputs = [p['image_hash'], original['width'], original['height'],
                              original.get('layout_provenance'), inputs, review_signature]
            signature = hashlib.sha256(json.dumps(receipt_inputs, ensure_ascii=False,
                                                  sort_keys=True).encode()).hexdigest()
            assert imported.pop('ocr_review_cache') == {
                'canvas_version': 1, 'review_version': 1, 'signature': signature}
            assert imported == original
            assert Path(result['cache']).stat().st_mode & 0o777 == 0o600
        assert not (weights/'calls.jsonl').exists()
        passed('Synthetic imports preserve original data and add OCR review metadata without inference', pages=len(loaded))
        job = client.send('process', images=[p['image'] for p in pages], operation='ocr', mode='vision', model='unused')
        reply = client.wait(job)['result']
        assert len(reply['pages']) == len(pages) and all(p['ok'] for p in reply['pages'])
        assert not (weights/'calls.jsonl').exists()
        passed('Synthetic Rust queue reuses OCR without inference')
        first = client.call('load_page', image=pages[0]['image'], mode='vision')
        original = first['page']
        block_id = original['blocks'][0]['id']
        updated = client.call('edit', cache=first['cache'], block_id=block_id,
                              values={'translation':'手动校正内容', 'edited':True}, revision=first['revision'])
        expected = json.loads(json.dumps(original))
        expected['blocks'][0].update(translation='手动校正内容', edited=True, ocr_confirmed=True)
        expected['cache_retention'] = {'version': 1, 'kind': 'manual'}
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
        for block in value['blocks']:
            block['translation'] = '「' + block['source'] + '！」'
        warning_path.write_text(json.dumps(value))
        stored = warning_path.read_bytes()
        shown = client.call('load_page', image=pages[1]['image'], mode='vision')['page']['blocks']
        assert '原样返回' in shown[0]['error'] and not shown[1]['error']
        assert warning_path.read_bytes() == stored
        warning_path.write_bytes(before)
        passed('display flags placeholders and punctuation echoes while preserving manual text and stored data')
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
            page['blocks'] = [dict(id='1',box=[50,50,85,110],source='こんにちは',translation='',error='',edited=False),
                              dict(id='2',box=[190,50,225,110],source='さようなら',translation='保留手改',error='',edited=True)]
            Path(result['cache']).write_text(json.dumps(page))
            options = dict(images=[image], operation='translate', mode='text', model='same-model',
                           provider='omlx', base_url=server.url, key='fake-secret')
            assert client.call('process', **options)['pages'][0]['ok']
            translated = client.call('load_page', image=image, mode='text')['page']
            assert translated['blocks'][0]['translation'] == '测试中文'
            assert translated['blocks'][1]['translation'] == '保留手改'
            visible = client.call('load_page', image=image, mode='text')
            manual = client.call('edit', cache=visible['cache'], revision=visible['revision'], block_id='2',
                                 values={'translation': '另一窗口刚保存的定稿', 'edited': True})
            retry = dict(options, retry=True, block_id='2', cache=visible['cache'])
            cache = Path(visible['cache'])
            saved = cache.read_bytes()
            server.calls.clear()
            missing = client.call('process', **retry)['pages'][0]
            stale = client.call('process', **retry, revision=visible['revision'])['pages'][0]
            assert not missing['ok'] and not stale['ok'] and stale['code'] == 'page_revision_changed'
            # The file at this path originally had the same bytes as the visible page.
            alias = output/'replaced.png'
            alias.write_bytes(Path(image).read_bytes())
            original = client.call('load_page', image=str(alias), mode='text')
            alias.write_bytes(Path(pages[1]['image']).read_bytes())
            replacement = client.call('load_page', image=str(alias), mode='text')
            replacement_file = Path(replacement['cache'])
            replacement_bytes = replacement_file.read_bytes()
            changed_image = client.call('process', **dict(retry, images=[str(alias)]),
                                        revision=original['revision'])['pages'][0]
            assert not changed_image['ok'] and changed_image['code'] == 'page_revision_changed'
            assert cache.read_bytes() == saved and replacement_file.read_bytes() == replacement_bytes
            assert not server.calls

            server.mode = 'http-error'
            failed = client.call('process', **retry, revision=manual['revision'])['pages'][0]
            assert not failed['ok'] and cache.read_bytes() == saved
            server.mode = 'good'
            assert client.call('process', **retry, revision=manual['revision'])['pages'][0]['ok']
            final = client.call('load_page', image=image, mode='text')['page']
            assert final['blocks'][1]['translation'] == '测试中文' and not final['blocks'][1]['edited']
        passed('single-block retry rejects obsolete or missing snapshots and commits only a successful explicit replacement')

        with Server() as server:
            for mode, source in [('text', image), ('vision', pages[1]['image'])]:
                options = dict(images=[source], operation='translate', mode=mode, model='same-model',
                               provider='omlx', base_url=server.url)
                server.translation = '原模型的译文'
                assert client.call('process', **options)['pages'][0]['result']['issues'] == 0
                original = client.call('load_page', image=source, mode=mode)['page']['blocks']
                prior = {b['id']: b['translation'] for b in original}
                next_model = dict(options, model='other-model')
                server.translation = '新模型的译文'
                entered, release = threading.Event(), threading.Event()
                server.pause_completion = entered, release
                try:
                    job = client.send('process', **next_model)
                    assert entered.wait(15), 'No model request reached the loopback server'
                    assert client.call('cancel', target=job)['cancelled']
                    assert client.wait(job, timeout=15)['event'] == 'cancelled'
                finally:
                    release.set()
                    server.pause_completion = None
                def load_blocks():
                    return client.call('load_page', image=source, mode=mode)['page']['blocks']
                assert {b['id']: b['translation'] for b in load_blocks()} == prior
                server.mode = 'http-error'
                failed = client.call('process', **next_model)['pages'][0]
                blocks = load_blocks()
                assert failed['ok'] and failed['result']['issues'] == sum(not b['edited'] for b in original)
                assert {b['id']: b['translation'] for b in blocks} == prior
                assert all(not b['error'] and (b['edited'] or b.get('retranslation_error')) for b in blocks)
                server.mode = 'punctuation-echo'
                mixed = client.call('process', **next_model)['pages'][0]
                assert mixed['ok'] and mixed['result']['issues'] == 1
                partial = load_blocks()
                pending = [b['id'] for b in partial if b.get('retranslation_error')]
                assert len(pending) == 1
                for b in partial:
                    retained = b['id'] in pending or b['edited']
                    assert b['translation'] == (prior[b['id']] if retained else server.translation)
                    assert not b['error']
                server.mode = 'good'
                server.calls.clear()
                retry = client.call('process', **next_model)['pages'][0]
                assert retry['ok'] and retry['result']['issues'] == 0
                posts = [c['payload'] for c in server.calls if c['method'] == 'POST']
                assert len(posts) == 1
                schema = (posts[0]['structured_outputs']['json'] if mode == 'vision'
                          else posts[0]['response_format']['json_schema']['schema'])
                assert schema['properties']['translations']['required'] == pending
                for b in load_blocks():
                    assert not b.get('retranslation_error')
                    assert b['translation'] == (prior[b['id']] if b['edited'] else server.translation)
                server.calls.clear()
                assert client.call('process', **next_model)['pages'][0]['result']['issues'] == 0
                assert not server.calls
                if mode == 'vision':
                    shown = client.call('load_page', image=source, mode=mode)
                    single = client.call('process', **next_model, retry=True, block_id=pending[0],
                                         cache=shown['cache'], revision=shown['revision'])['pages'][0]
                    assert single['ok']
                    stored = client.call('load_page', image=source, mode=mode)['page']
                    assert stored['vision_runs'][-1]['image'] == source
        passed('model changes retain readable translations through cancellation and failure, then resume in both modes')
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
    hashes()
    report = dict(passed=True, records=records, original_inputs_unchanged=True,
                  scope='Rust daemon lifecycle, queue, private edits and cache; synthetic fixtures and simulated inference only')
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({k:v for k,v in report.items() if k!='records'}))


if __name__ == '__main__': main()
