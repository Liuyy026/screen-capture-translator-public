#!/usr/bin/env python3
"""Differential native translation jobs with controlled local HTTP responses.

Compares requests, page state, every checkpoint, progress and sanitized metrics.
Model output is deterministic test data; original pages/caches remain read-only.
"""
import argparse
from baseline import prepare_baseline
from fixtures import make_pages, core_binary
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch


class Server:
    def __init__(self):
        self.calls = []
        self.mode = 'good'
        self.pause_completion = None
        self.translation = '测试中文'
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                self.answer()
            def do_POST(self):
                self.answer()
            def answer(self):
                data = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                payload = json.loads(data) if data else None
                owner.calls.append({'method':self.command,'path':self.path,'payload':payload,
                                    'authorization':self.headers.get('Authorization')})
                if self.command == 'GET':
                    models = [] if owner.mode == 'missing-model' else ['same-model','other-model']
                    obj = {'models':[{'name':m} for m in models]} if self.path == '/api/tags' else {'data':[{'id':m} for m in models]}
                else:
                    if owner.pause_completion:
                        entered, release = owner.pause_completion
                        entered.set()
                        if not release.wait(timeout=20):
                            raise RuntimeError('Timed out waiting for the cancellation test')
                        return
                    visual = 'structured_outputs' in payload
                    schema = (payload['structured_outputs']['json'] if visual else
                              payload.get('format') or payload['response_format']['json_schema']['schema'])
                    ids = schema['properties']['translations']['required']
                    message = payload['messages'][1]['content']
                    context = json.loads(message[0]['text'] if visual else message.removesuffix('\n/no_think'))
                    sources = {b['id']:b['ja'] for b in context['page']}
                    translations = {key:owner.translation for key in ids}
                    if owner.mode == 'partial':
                        translations.pop(ids[0],None)
                    elif owner.mode == 'placeholder':
                        translations[ids[0]] = '待翻译'
                    elif owner.mode == 'echo':
                        translations[ids[0]] = sources[ids[0]]
                    elif owner.mode == 'punctuation-echo':
                        translations[ids[0]] = '「' + sources[ids[0]] + '！」'
                    elif owner.mode == 'long':
                        translations[ids[0]] = '超长'*1000
                    elif owner.mode == 'lines':
                        translations = {key:'第一句\n第二句' for key in ids}
                    body = {'translations':translations}
                    if visual:
                        body['source_suggestions'] = {key: '' for key in ids}
                    content = json.dumps(body, ensure_ascii=False)
                    if owner.mode == 'duplicate':
                        content = '{"translations":{' + json.dumps(ids[0])+':"第一条",'+json.dumps(ids[0])+':"第二条"}}'
                    elif owner.mode == 'duplicate-row':
                        content = json.dumps({'translations':[{'id':ids[0],'text':'第一条'},{'id':ids[0],'text':'第二条'}]})
                    elif owner.mode == 'invalid-json':
                        content = 'private source fake-secret'
                    reason = 'length' if owner.mode == 'truncated' else 'stop'
                    obj = {'message':{'content':content},'done_reason':reason} if self.path == '/api/chat' else {
                        'choices':[{'message':{'content':content},'finish_reason':reason}]}
                    obj.update(eval_count=30,load_duration=4.125,total_duration=123456,prompt_eval_duration=True,
                               private_field='private source fake-secret')
                    if owner.mode == 'bad-content':
                        obj = {'error':'private source fake-secret'}
                    if owner.mode == 'http-error':
                        obj = {'error':'private source fake-secret'}
                data = json.dumps(obj).encode()
                self.send_response(500 if payload and owner.mode=='http-error' else 200)
                self.send_header('Content-Length',str(len(data)))
                self.send_header('Content-Type','application/json')
                self.end_headers()
                self.wfile.write(data)
        self.server = ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.url = 'http://127.0.0.1:'+str(self.server.server_port)
        self.thread = threading.Thread(target=self.server.serve_forever,daemon=True)
    def __enter__(self):
        self.thread.start()
        return self
    def __exit__(self,*args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def copy_fields(target, source, fields):
    for field in fields:
        if field in source:
            target[field] = copy.deepcopy(source[field])
        else:
            target.pop(field, None)


def sync_retranslation_diagnostics(page, sync):
    sync(page)
    blocks = {block['id']: block for block in page['blocks']}
    failed = {key for key, block in blocks.items() if block.get('error') or block.get('retranslation_error')}
    dialogues = page.get('dialogues', [])
    issues = [d for d in dialogues if d.get('translation_error') or failed.intersection(d['block_ids'])]
    assigned = {key for d in dialogues for key in d['block_ids']}
    diagnostics = page['diagnostics']
    diagnostics['translation'].update(errors=len(failed), failed_dialogues=len(issues),
        pending_dialogues=sum(not d['translation_complete'] and d not in issues for d in dialogues))
    diagnostics['failures']['translation'] = len(issues) + len(failed - assigned)


def expect_retranslation(page, trace, *, sync, block_id, refresh_overlay=None, vision=False):
    """Apply the new commit rules to the baseline's independently generated responses."""
    events = trace['metrics']
    if not events:
        if trace['error'] is None and trace['page'].get('structure'):
            sync_retranslation_diagnostics(trace['page'], sync)
        return

    final = trace['page']
    wanted = final.get('translation_config')
    if vision and block_id is not None:
        selected = next(block for block in final['blocks'] if block['id'] == block_id)
        wanted = selected.get('translation_config_override', wanted)
    assert wanted is not None
    changing = page.get('translation_config') != wanted
    # The baseline emitted a destructive reset before the first response. Its
    # per-response snapshots and final ordinary save still describe the replies.
    batch_count = len(events)
    save_count = batch_count + (0 if vision else 1)
    assert len(trace['saves']) in (save_count, save_count + 1)
    trace['saves'] = trace['saves'][-save_count:]
    batches = trace['saves'][:batch_count]
    before = {b['id']: b for b in page['blocks']}
    prior_dialogues = {d['id']: d for d in page.get('dialogues', [])}
    translation_fields = ('translation', 'error', 'quality_warnings', 'vision_source',
                          'translation_config_override', 'retranslation_error')

    responses = {}
    for event, snapshot in zip(events, batches):
        ids = set(event['batch_ids'])
        if event.get('unit') == 'dialogues':
            ids = {key for d in snapshot['dialogues'] if d['id'] in ids for key in d['block_ids']}
        for block in snapshot['blocks']:
            if block['id'] in ids:
                responses[block['id']] = block.get('error') or ''
    finished = block_id is None and all(not reason for reason in responses.values())

    received = set()
    replaced_as_blocks = set()
    for index, snapshot in enumerate([*trace['saves'], final]):
        if index < batch_count:
            event = events[index]
            if event.get('unit') == 'dialogues':
                received.update(key for d in snapshot['dialogues'] if d['id'] in event['batch_ids']
                                for key in d['block_ids'])
            else:
                received.update(event['batch_ids'])
                replaced_as_blocks.update(key for key in event['batch_ids'] if not responses[key])
        promoted = finished and index >= (batch_count - 1 if vision else batch_count)
        if not promoted:
            copy_fields(snapshot, page, ('model', 'translation_config'))
        for block in snapshot['blocks']:
            key = block['id']
            prior = before[key]
            if key not in received:
                copy_fields(block, prior, translation_fields)
            elif reason := responses[key]:
                copy_fields(block, prior, translation_fields)
                field = 'retranslation_error' if prior.get('translation') and not prior.get('error') else 'error'
                block[field] = reason
            else:
                block.pop('retranslation_error', None)
                block.pop('translation_config_override', None)
                if changing and not promoted:
                    block['translation_config_override'] = wanted
        for dialogue in snapshot.get('dialogues', []):
            if replaced_as_blocks.intersection(dialogue['block_ids']):
                for field in ('full_translation', 'mapping', 'mapping_status', 'translation_error',
                              'translation_config', 'quality_warnings'):
                    dialogue.pop(field, None)
            elif dialogue.get('translation_error') and any(
                    before[key].get('translation') and not before[key].get('error')
                    for key in dialogue['block_ids'] if key in received):
                copy_fields(dialogue, prior_dialogues.get(dialogue['id'], {}), ('translation_error',))
        if refresh_overlay is not None:
            refresh_overlay(snapshot)
        sync_retranslation_diagnostics(snapshot, sync)


def assert_mixed_replacement(previous, result, replacement_text, reason):
    """Check a mixed reply against the original cached text and actual producer."""
    assert result['model'] == previous['model'] == 'same-model'
    assert result['translation_config'] == previous['translation_config']
    states = {'manual': 0, 'failed': 0, 'replaced': 0}
    for before, after in zip(previous['blocks'], result['blocks'], strict=True):
        assert before['id'] == after['id'] and before['source'] == after['source']
        if before.get('edited'):
            states['manual'] += 1
            assert after['translation'] == before['translation']
            assert after.get('translation_config_override') == before.get('translation_config_override')
            assert 'retranslation_error' not in after
        elif after.get('retranslation_error'):
            states['failed'] += 1
            assert after['retranslation_error'] == reason
            for field in ('translation', 'error', 'quality_warnings', 'vision_source', 'translation_config_override'):
                assert before.get(field) == after.get(field)
        else:
            states['replaced'] += 1
            assert after['translation'] == replacement_text != before['translation']
            assert after['error'] == ''
            assert after['translation_config_override'] == {**previous['translation_config'], 'model': 'other-model'}
    assert states == {'manual': 1, 'failed': 1, 'replaced': 1}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    output=args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    root=Path(__file__).resolve().parents[2]
    binary=core_binary(root)
    baseline = prepare_baseline(output / 'baseline')
    sys.path.insert(0,str(baseline))
    from backend import worker as old
    os.environ['KOMA_TRANSLATION_API_KEY']='fake-secret'
    manifest=make_pages(output/'fixtures')
    def hashes():
        for entry in manifest:
            for field,key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(entry[field]).read_bytes()).hexdigest()==entry[key]
    hashes()
    cases=[]
    posts=0
    with Server() as server,Server() as other:
        def run(page,mode='good',provider='omlx',unit='blocks',model='same-model',image=None,block_id=None,batch_size=8,endpoint=None):
            nonlocal posts
            endpoint=endpoint or server
            endpoint.mode=mode
            prefix=output/f'{len(cases)+1:04}'
            request={'page':page,'provider':provider,'base_url':endpoint.url,'model':model,'unit':unit,
                     'block_id':block_id,'batch_size':batch_size,'image':str(image) if image else None,'key':'fake-secret'}
            path=prefix.with_suffix('.request.json')
            path.write_text(json.dumps([request],ensure_ascii=False))
            reference=copy.deepcopy(page)
            expected={'page':reference,'saves':[],'progress':[],'metrics':[],'error':None}
            endpoint.calls=[]
            with patch.object(old,'record_benchmark',side_effect=lambda e:expected['metrics'].append(copy.deepcopy(e))):
                try:
                    old.translate(reference,model,expected['progress'].append,lambda p:expected['saves'].append(copy.deepcopy(p)),
                                  provider=provider,base_url=endpoint.url,translation_unit=unit,batch_size=batch_size,
                                  block_id=block_id,image=image)
                except ValueError as exc:
                    expected['error']=str(exc)
            expected_calls=copy.deepcopy(endpoint.calls)
            # A complete cache needs neither inference nor a model-list request.
            if expected['error'] is None and not expected['metrics'] and expected_calls:
                model_route = '/api/tags' if provider == 'ollama' else '/v1/models'
                assert len(expected_calls) == 1 and expected_calls[0]['method'] == 'GET'
                assert expected_calls[0]['path'] == model_route
                expected_calls = []
            if mode == 'missing-model':
                # Validation fails before publishing any changes to the page.
                expected['page'] = copy.deepcopy(page)
            refresh_overlay = None if image is None else lambda snapshot: old._overlay.ensure_page_overlays(snapshot, image=image)
            expect_retranslation(page, expected, sync=old._structure.sync_dialogue_translations,
                                 block_id=block_id, refresh_overlay=refresh_overlay)
            endpoint.calls=[]
            proc=subprocess.run([str(binary),'translation-probe','--requests',str(path)],capture_output=True,text=True,timeout=90)
            prefix.with_suffix('.stderr.log').write_text(proc.stderr)
            assert proc.returncode==0,proc.stderr
            actual=json.loads(proc.stdout)[0]
            actual_calls=copy.deepcopy(endpoint.calls)
            for data in [actual,expected]:
                for event in data['metrics']:
                    assert isinstance(event.pop('wall_seconds'),(int,float))
            # The only intentional error wording change: malformed JSON syntax
            # no longer contains parser-specific position/line diagnostics.
            if mode=='invalid-json':
                def normalize(value):
                    if isinstance(value,dict):return {k:normalize(v) for k,v in value.items()}
                    if isinstance(value,list):return [normalize(v) for v in value]
                    if isinstance(value,str) and value.startswith('翻译失败：Expecting value:'):
                        return '翻译失败：模型未返回有效 JSON'
                    return value
                expected=normalize(expected)
            prefix.with_suffix('.expected.json').write_text(json.dumps(expected,ensure_ascii=False,indent=2))
            prefix.with_suffix('.actual.json').write_text(json.dumps(actual,ensure_ascii=False,indent=2))
            prefix.with_suffix('.calls.json').write_text(json.dumps({'expected':expected_calls,'actual':actual_calls},ensure_ascii=False,indent=2))
            assert actual_calls==expected_calls,f'HTTP request difference at {prefix}'
            assert actual==expected,f'page/checkpoint difference at {prefix}'
            assert 'fake-secret' not in json.dumps(actual)
            assert 'private source' not in json.dumps(actual)
            for before,after in zip(page['blocks'],actual['page']['blocks']):
                assert before.get('source')==after.get('source')
                if before.get('edited'):assert before.get('translation')==after.get('translation')
            count=sum(call['method']=='POST' for call in actual_calls)
            posts+=count
            cases.append({'mode':mode,'provider':provider,'unit':unit,'image':str(image) if image else None,'posts':count,
                          'checkpoints':len(actual['saves'])})
            return actual['page']
        for entry in manifest:
            page=json.loads(Path(entry['cache']).read_text())
            for unit in ['blocks','dialogues']:
                current=run(page,image=Path(entry['image']),unit=unit)
                current=run(current,image=Path(entry['image']),unit=unit)
                # First resume can settle the structure created by rendering.
                # The next resume should exercise the complete cache path.
                run(current,image=Path(entry['image']),unit=unit)
        simple={'version':2,'width':300,'height':300,'model':'same-model','blocks':[
            {'id':'1','box':[10,10,60,60],'source':'危険もありません','translation':'旧译文','error':'','edited':False},
            {'id':'2','box':[100,10,150,60],'source':'こんにちは','translation':'手改','error':'','edited':True},
            {'id':'3','box':[200,10,250,60],'source':'私より小さい','translation':'旧三','error':'','edited':False},
            {'id':'4','box':[10,110,60,160],'source':'ん','translation':'','error':'','overlay':'skip'},
        ]}
        for provider in ['ollama','omlx']:
            for unit in ['blocks','dialogues']:
                for mode in ['good','partial','placeholder','echo','long','lines','duplicate','duplicate-row','invalid-json',
                             'truncated','bad-content','http-error','missing-model']:
                    run(simple,provider=provider,unit=unit,mode=mode,batch_size=1)
        current=run(simple,provider='ollama')
        current=run(current,provider='omlx')
        current=run(current,provider='omlx',endpoint=other)
        current=run(current,provider='omlx',endpoint=other,model='other-model')
        current=run(current,provider='omlx',endpoint=other,model='other-model',unit='dialogues')
        run(current,provider='omlx',endpoint=other,model='other-model',unit='blocks')
        failed=run(simple,mode='partial')
        failed_id = next(b['id'] for b in failed['blocks'] if b.get('retranslation_error'))
        run(failed,block_id=failed_id)
        run(failed,block_id='unknown')
        run(failed,block_id='2')
        previous = run({**simple, 'blocks': simple['blocks'][:3]})
        server.translation = '另一个模型的译文'
        mixed = run(previous, model='other-model', mode='placeholder')
        assert_mixed_replacement(previous, mixed, server.translation,
                                 '模型返回了占位文字，没有提供实际译文')
        server.translation = '测试中文'
        run({'blocks':[]})
        run({'blocks':[{'id':'1','source':'ん','translation':'','overlay':'skip'}]})
        run({'blocks':[{'id':'1','source':'私','translation':'我','edited':True}]})
        joined={'width':300,'height':300,'model':'same-model','blocks':[
            {'id':'1','box':[10,10,40,40],'source':'一','translation':'','dialogue_group':'joined'},
            {'id':'2','box':[10,41,40,70],'source':'二','translation':'','dialogue_group':'joined'}]}
        for mode in ['good','lines','long','partial']:
            run(joined,unit='dialogues',mode=mode)
        # Production CLI: progress/checkpoint files are actually persisted.
        cache=output/'cli-input.json';destination=output/'cli-result.json';status=output/'cli-status.json';metrics=output/'cli-metrics.jsonl'
        cache.write_text(json.dumps(simple,ensure_ascii=False))
        server.mode='good'
        proc=subprocess.run([str(binary),'translate','--cache',str(cache),'--destination',str(destination),'--status',str(status),
            '--metrics',str(metrics),'--provider','omlx','--base-url',server.url,'--model','same-model'],capture_output=True,text=True,timeout=90)
        assert proc.returncode==0,proc.stderr
        assert json.loads(proc.stdout)=={'ok':True,'cache':str(destination)}
        saved=json.loads(destination.read_text())
        assert saved['blocks'][1]['translation']=='手改'
        assert saved['blocks'][0]['translation']=='测试中文'
        assert json.loads(status.read_text())['message'].startswith('正在翻译')
        assert len(metrics.read_text().splitlines())==1
        assert destination.stat().st_mode&0o777==0o600
        assert cache.read_text()==json.dumps(simple,ensure_ascii=False)
        for name,page in [('empty',{'blocks':[]}),
                          ('edited',{'blocks':[{'id':'1','source':'日本語','translation':'手改','edited':True}]}),
                          ('skipped',{'blocks':[{'id':'1','source':'ん','translation':'','overlay':'skip'}]})]:
            source=output/f'cli-{name}-input.json';target=output/f'cli-{name}-result.json'
            source.write_text(json.dumps(page,ensure_ascii=False))
            original=source.read_bytes()
            proc=subprocess.run([str(binary),'translate','--cache',str(source),'--destination',str(target),
                '--provider','omlx','--base-url','http://127.0.0.1:1','--model','no-model'],
                capture_output=True,text=True,timeout=10)
            assert proc.returncode==0,proc.stderr
            assert json.loads(proc.stdout)=={'ok':True,'cache':str(target)}
            assert json.loads(target.read_text())==page
            assert target.stat().st_mode&0o777==0o600
            assert source.read_bytes()==original
    hashes()
    report={'pages':len(manifest),'scenarios':len(cases),'native_translation_posts':posts,'passed':True,
            'scope':'native normal translation orchestration, checkpoints and actual loopback HTTP; no model inference',
            'input_hashes_unchanged':True,'cases':cases}
    (output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k!='cases'}))


if __name__=='__main__':
    main()
