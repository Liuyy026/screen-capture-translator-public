#!/usr/bin/env python3
"""Exercise actual chapter scheduling and retries with synthetic OCR and loopback HTTP."""
import hashlib
import json
from pathlib import Path
import tempfile
from threading import Event

from PIL import Image, PngImagePlugin
from fixtures import make_pages
from verify_runtime import Client, FAKE_WORKER
from verify_translate_pipeline import Server


def inputs(tmp):
    directory = tmp / 'fixtures'
    fixtures = make_pages(directory)
    cover, first = fixtures
    cache = Path(cover['cache'])
    saved = json.loads(cache.read_bytes())
    saved['blocks'] = []
    cache.write_text(json.dumps(saved))
    images = [cover['image'], first['image']]
    for i in range(2):
        image = directory / f'new-{i}.png'
        metadata = PngImagePlugin.PngInfo()
        metadata.add_text('page', str(i))
        with Image.open(first['image']) as pixels:
            pixels.save(image, pnginfo=metadata)
        images.append(str(image))
    weights = tmp / 'weights'
    weights.mkdir()
    worker = tmp / 'worker.py'
    blocks = [dict(box=[50,50,85,110],vertical=True,lines=['こんにちは']),
              dict(box=[190,50,225,110],vertical=True,lines=['さようなら'])]
    worker.write_text(FAKE_WORKER.replace("'blocks':[]", "'blocks':" + repr(blocks)))
    return images, directory/'seed', weights, worker


def server_plan(server):
    handler = server.server.RequestHandlerClass
    original = handler.answer
    state = dict(list_status=[503,200], list_calls=0, chats=0, partial_second=True)
    def answer(self):
        if self.path == '/v1/models':
            state['list_calls'] += 1
            statuses = state['list_status']
            status = statuses.pop(0) if len(statuses)>1 else statuses[0]
            if status != 200:
                self.send_response(status)
                self.end_headers()
                return
        if self.path == '/v1/chat/completions':
            state['chats'] += 1
            hook = state.pop('on_chat', None)
            if hook:
                hook()
            payload=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            server.calls.append({'path':self.path,'payload':payload})
            server.resident_models.add(payload['model'])
            ids=payload['structured_outputs']['json']['properties']['translations']['required']
            translations=dict.fromkeys(ids,'测试中文')
            if state['partial_second'] and state['chats']==2:
                translations[ids[0]]='待翻译'
            content=json.dumps({'translations':translations,'source_suggestions':dict.fromkeys(ids,'')},ensure_ascii=False)
            body=json.dumps({'choices':[{'message':{'content':content},'finish_reason':'stop'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Length',str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass  # A cancellation fixture deliberately closes this request.
            return
        original(self)
    handler.answer = answer
    return state


def main():
    root=Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix='koma-chapter-schedule-') as temp, Server() as server:
        tmp=Path(temp)
        images,seed,weights,worker=inputs(tmp)
        control=server_plan(server)
        request=dict(images=images,chapter=True,operation='both',mode='vision',provider='omlx',
                     model='same-model',base_url=server.url,previous_images={images[i]:images[i-1] for i in range(2,len(images))})
        client=Client(root,tmp/'daemon',weights,worker,seed)
        try:
            job=client.send('process',**request)
            pages=client.wait(job)['result']['pages']
            events=[e for e in client.all_events if e['id']==job]
            started=[e['image'] for e in events if e['event']=='page_started']
            finished=[e for e in events if e['event']=='page_finished']
            assert started == images and [e['page']['image'] for e in finished] == images
            assert [e['done'] for e in finished] == [1,2,3,4]
            assert [p['image'] for p in pages if p.get('retried')] == images[1:3], pages
            assert all(p['ok'] and p['result']['issues']==0 for p in pages)
            assert [e['image'] for e in events if e['event']=='page_retrying'] == images[1:3]
            assert events.index(finished[1]) < next(i for i,e in enumerate(events) if e['event']=='chapter_phase')
            assert [(e['phase'], e['index']) for e in events if e['event']=='chapter_phase'][0] == ('ocr',2)
            chats=[c['payload'] for c in server.calls if c['path']=='/v1/chat/completions']
            targets=[p['structured_outputs']['json']['properties']['translations']['required'] for p in chats]
            assert targets[0]==['1'] and len(targets)==4 and set(targets[1])=={'1','2'},targets
            assert targets[2]==targets[1][:1] and targets[3]==targets[1],targets
            assert sum(c['type']=='image_url' for c in chats[2]['messages'][1]['content'])==2
            cached=json.loads(Path(pages[1]['result']['cache']).read_bytes())
            assert cached['blocks'][1]['translation']=='手改译文' and cached['blocks'][1]['edited']
            assert sum(c['op']=='ocr' for c in map(json.loads,(weights/'calls.jsonl').read_text().splitlines()))==2
            warm=client.send('process',**request)
            again=client.wait(warm)['result']
            assert all(not p.get('retried') for p in again['pages']) and control['chats']==len(chats)
            phase=None
            for event in (e for e in client.all_events if e['id']==warm):
                if event['event']=='chapter_phase': phase=event['phase']
                assert not (phase=='ocr' and event['event']=='checkpoint'),event
            # A manually started page needs full OCR even though it has a file.
            # Simulate an external cache removal while its preceding page is
            # translating; the eventual both pass must also recover this page.
            partial=Path(pages[2]['result']['cache'])
            data=json.loads(partial.read_bytes())
            manual=next(b for b in data['blocks'] if b['id']=='1')
            manual.update(edited=True, translation='人工保留')
            partial.write_text(json.dumps(dict(version=2,width=300,height=300,blocks=[manual],
                                               manual_ocr_partial_page=True,ocr='manual / manga-ocr')))
            last=Path(pages[3]['result']['cache'])
            control['on_chat']=last.unlink
            restored=client.call('process',**request)['pages']
            assert all(p['ok'] and p['result']['issues']==0 for p in restored)
            recovered=json.loads(partial.read_bytes())
            assert not recovered.get('manual_ocr_partial_page') and len(recovered['blocks'])==2
            assert any(b['translation']=='人工保留' and b['edited'] for b in recovered['blocks'])
            assert len(json.loads(last.read_bytes())['blocks'])==2
            assert sum(c['op']=='ocr' for c in map(json.loads,(weights/'calls.jsonl').read_text().splitlines()))==4
        finally:
            client.close()
        # Use a fresh daemon and namespace to force discovery again.
        control.update(list_status=[503],list_calls=0,partial_second=False)
        client=Client(root,tmp/'failure-daemon',weights,worker,seed)
        try:
            request.update(images=images[1:3],previous_images={})
            job=client.send('process',**request)
            event=client.wait(job,events=('page_retrying',))
            assert event['attempt']==2
            assert client.call('cancel',target=job)['cancelled'] and client.wait(job)['event']=='cancelled'
            assert control['list_calls']==1
            assert not any(e['id']==job and e['event']=='page_finished' for e in client.all_events)
            single=client.call('process',**dict(request,images=images[1:2]))['pages'][0]
            assert not single['ok'] and not single.get('retried') and control['list_calls']==2
            failed=client.call('process',**request)['pages']
            assert all(not p['ok'] and p['retried'] and '503' in p['error'] and '503' in p['first_error'] for p in failed)
            assert control['list_calls']==6
            control['list_status']=[403]
            rejected=client.call('process',**request)['pages']
            assert all(not p['ok'] and not p.get('retried') for p in rejected)
            assert control['list_calls']==8
        finally:
            client.close()
        control.update(list_status=[200],list_calls=0)
        client=Client(root,tmp/'resume-daemon',weights,worker,seed)
        entered,release=Event(),Event()
        try:
            def ocr_count():
                return sum(json.loads(line)['op']=='ocr' for line in (weights/'calls.jsonl').read_text().splitlines())
            before=ocr_count()
            # Hold the first post-OCR translation before its response. This
            # works even if memory pressure reduces a chapter group to one.
            def wait_for_cancel():
                entered.set()
                assert release.wait(20)
            control['on_chat']=lambda: control.update(on_chat=wait_for_cancel)
            request['images']=images
            job=client.send('process',**request)
            assert entered.wait(15)
            digest=hashlib.sha256(Path(images[2]).read_bytes()).hexdigest()
            checkpoint=client.directory/'pages'/'vision-v1'/(digest+'.json')
            assert sum(bool(b['translation']) for b in json.loads(checkpoint.read_bytes())['blocks'])==0
            assert client.call('cancel',target=job)['cancelled']
            assert client.wait(job)['event']=='cancelled'
            assert not any(e['id']==job and e['event']=='page_finished' and e['page']['image']==images[2]
                           for e in client.all_events)
            release.set()
            resumed=client.call('process',**request)['pages']
            assert all(p['ok'] and p['result']['issues']==0 for p in resumed)
            assert ocr_count()==before+2 and sum(bool(b['translation']) for b in json.loads(checkpoint.read_bytes())['blocks'])==2
        finally:
            release.set()
            client.close()
    print('PASS chapter phases, first readable page, transient model discovery, partial block recovery, manual edits, cache reuse, bounded retries, authentication, cancellation and OCR checkpoint resumption')

if __name__=='__main__':
    main()
