#!/usr/bin/env python3
"""Compare complete native visual translation using fixed loopback responses."""
import argparse
from fixtures import make_pages, core_binary
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from unittest.mock import patch


class Server:
    def __init__(self):
        self.mode='good';self.calls=[]
        owner=self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):self.answer()
            def do_POST(self):self.answer()
            def answer(self):
                raw=self.rfile.read(int(self.headers.get('Content-Length',0)))
                payload=json.loads(raw) if raw else None
                owner.calls.append({'method':self.command,'path':self.path,'payload':payload,'auth':self.headers.get('Authorization')})
                status=200
                if payload is None:
                    obj={'data':[] if owner.mode=='missing-model' else [{'id':'same-model'},{'id':'other-model'}]}
                else:
                    data=json.loads(payload['messages'][1]['content'][0]['text'])
                    ids=data['target_ids'];sources={b['id']:b['ja'] for b in data['page']}
                    rows={key:'测试中文' for key in ids};suggestions={key:'' for key in ids}
                    if owner.mode=='partial':rows.pop(ids[0])
                    if owner.mode=='echo':rows={key:sources[key] for key in ids}
                    if owner.mode=='placeholder':rows[ids[0]]='待翻译'
                    if owner.mode=='suggestion':suggestions[ids[0]]='  こんばんは  '
                    if owner.mode=='same-suggestion':suggestions={key:sources[key] for key in ids}
                    if owner.mode=='long-suggestion':suggestions[ids[0]]='𠮷'*2001
                    if owner.mode=='invalid-suggestion':suggestions=[]
                    if owner.mode=='invalid-suggestion-value':suggestions[ids[0]]=42
                    content=json.dumps({'translations':rows,'source_suggestions':suggestions},ensure_ascii=False)
                    if owner.mode=='duplicate':content='{"translations":{'+json.dumps(ids[0])+':"甲",'+json.dumps(ids[0])+':"乙"}}'
                    if owner.mode=='duplicate-row':content=json.dumps({'translations':[{'id':ids[0],'text':'甲'},{'id':ids[0],'text':'乙'}]})
                    if owner.mode=='invalid-json':content='private source fake-secret'
                    reason='length' if owner.mode=='truncated' else 'stop'
                    obj={'choices':[{'message':{'content':content},'finish_reason':reason}],
                         'usage':{'prompt_tokens':123,'completion_tokens':10,'flag':True,'secret':'private source fake-secret'}}
                    if owner.mode=='usage-list':obj['usage']=[]
                    if owner.mode=='no-usage':obj.pop('usage')
                    if owner.mode=='bad-content':obj={'error':'private source fake-secret'}
                    if owner.mode=='http-error':status=500;obj={'error':'private source fake-secret'}
                data=json.dumps(obj).encode()
                self.send_response(status);self.send_header('Content-Length',str(len(data)))
                self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(data)
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.url='http://127.0.0.1:'+str(self.server.server_port)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True)
    def __enter__(self):self.thread.start();return self
    def __exit__(self,*args):self.server.shutdown();self.server.server_close();self.thread.join()


def comparable(value):
    if isinstance(value,dict):return {k:comparable(v) for k,v in value.items() if k!='wall_seconds'}
    if isinstance(value,list):return [comparable(v) for v in value]
    if isinstance(value,str) and value.startswith('视觉翻译失败：Expecting value:'):
        return '视觉翻译失败：模型未返回有效 JSON'
    return value


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2];baseline=Path(__file__).resolve().parents[2]
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    sys.path.insert(0,str(baseline))
    spec=importlib.util.spec_from_file_location('baseline_visual_worker',baseline/'macos/backend/vision_worker.py')
    old=importlib.util.module_from_spec(spec);spec.loader.exec_module(old)
    from backend.translation_service import TranslationService
    from PIL import Image
    binary=core_binary(root)
    os.environ['KOMA_TRANSLATION_API_KEY']='fake-secret'
    manifest=make_pages(output/'fixtures')
    def hashes():
        for entry in manifest:
            for field,key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(entry[field]).read_bytes()).hexdigest()==entry[key]
    hashes();cases=[];calls=0
    def native(request):
        nonlocal calls
        calls+=1;prefix=output/f'{calls:04}'
        request_file=prefix.with_suffix('.request.json');request_file.write_text(json.dumps([request],ensure_ascii=False))
        result=subprocess.run([str(binary),'vision-probe','--requests',str(request_file)],capture_output=True,text=True,timeout=90)
        prefix.with_suffix('.stderr.log').write_text(result.stderr)
        assert result.returncode==0,result.stderr
        return json.loads(result.stdout)[0],prefix
    with Server() as server,Server() as other:
        def run(page,image,mode='good',model='same-model',block_id=None,endpoint=None,provider='omlx'):
            endpoint=endpoint or server;endpoint.mode=mode;endpoint.calls=[]
            reference=copy.deepcopy(page)
            expected={'page':reference,'saves':[],'progress':[],'metrics':[],'error':None}
            with patch.object(old.shared,'record_benchmark',side_effect=lambda e:expected['metrics'].append(copy.deepcopy(e))):
                try:
                    old.translate(reference,image,model,TranslationService(provider,endpoint.url),expected['progress'].append,
                                  lambda p:expected['saves'].append(copy.deepcopy(p)),block_id)
                except ValueError as exc:expected['error']=str(exc)
            expected_calls=copy.deepcopy(endpoint.calls);endpoint.calls=[]
            request={'page':page,'image':str(image),'model':model,'provider':provider,'base_url':endpoint.url,'block_id':block_id,'key':'fake-secret'}
            actual,prefix=native(request)
            result_page=copy.deepcopy(actual['page'])
            actual_calls=copy.deepcopy(endpoint.calls)
            actual,expected=comparable(actual),comparable(expected)
            prefix.with_suffix('.expected.json').write_text(json.dumps(expected,ensure_ascii=False,indent=2))
            prefix.with_suffix('.actual.json').write_text(json.dumps(actual,ensure_ascii=False,indent=2))
            # Keep full failed requests locally, but avoid duplicating successful
            # base64 images in hundreds of test artifacts.
            if expected_calls!=actual_calls:
                prefix.with_suffix('.calls.json').write_text(json.dumps({'expected':expected_calls,'actual':actual_calls},ensure_ascii=False))
            assert expected_calls==actual_calls,f'visual request difference at {prefix}'
            assert expected==actual,f'visual page/checkpoint difference at {prefix}'
            assert 'fake-secret' not in json.dumps(actual)
            assert 'private source' not in json.dumps(actual)
            for before,after in zip(page['blocks'],result_page['blocks']):
                assert before['source']==after['source']
                if before.get('edited'):assert before.get('translation')==after.get('translation')
            cases.append({'mode':mode,'image':str(image),'posts':sum(c['method']=='POST' for c in actual_calls),
                          'checkpoints':len(actual['saves'])})
            return result_page
        for entry in manifest:
            page=json.loads(Path(entry['cache']).read_text())
            current=run(page,Path(entry['image']))
            run(current,Path(entry['image']))
        image=output/'fixture.png';Image.new('RGB',(2400,3000),'white').save(image)
        page={'version':2,'width':2400,'height':3000,'model':'','blocks':[
            {'id':str(i),'box':[20*i,30,20*i+15,90],'source':'こんにちは','translation':'','error':'',
             'edited':False,'vertical':True} for i in range(1,21)]}
        page['blocks'][0].update(source='当然',translation='原手改',edited=True)
        page['blocks'][1]['source']='当然'
        page['blocks'][2]['source']='本日はありません'
        page['blocks'][3]['source']='ん';page['blocks'][3]['overlay']='skip'
        page['blocks'][4]['source']='';page['blocks'][4]['text_kind_override']='noise'
        for mode in ['good','partial','echo','placeholder','suggestion','same-suggestion','long-suggestion','invalid-suggestion',
                     'invalid-suggestion-value','duplicate','duplicate-row','invalid-json','truncated','usage-list','no-usage','bad-content','http-error','missing-model']:
            run(page,image,mode=mode)
        current=run(page,image)
        run(current,image)
        current=run(current,image,model='other-model')
        current=run(current,image,model='other-model',endpoint=other)
        current['blocks'][5]['source']='こんばんは'
        current=run(current,image,model='other-model',endpoint=other)
        failed=run(page,image,mode='partial')
        run(failed,image,block_id='2')
        run(failed,image,block_id='unknown')
        run(failed,image,block_id='1')
        run(page,image,provider='ollama')
        run({'blocks':[]},image)
        history=copy.deepcopy(page);history['vision_runs']=[{'old':i,'wall_seconds':0.5} for i in range(60)]
        capped=run(history,image);assert len(capped['vision_runs'])==50
        # Seed isolated visual caches from ordinary OCR caches, preserving edits.
        ordinary=output/'ordinary';destination=output/'vision-v1'/'page.json'
        with patch.object(old.shared,'CACHE',ordinary):
            original=old.shared.cache_path(image)
            old.shared.atomic_json(original,page)
            original_bytes=original.read_bytes()
            for existing in [False,True]:
                expected=old.prepare_page(image,destination)
                actual,_=native({'op':'prepare','image':str(image),'destination':str(destination),'ordinary_root':str(ordinary)})
                assert actual==expected
                assert original.read_bytes()==original_bytes
                assert actual['blocks'][0]['translation']=='原手改'
                if not existing:old.shared.atomic_json(destination,expected)
            empty,_=native({'op':'prepare','image':str(image),'destination':str(output/'absent.json'),'ordinary_root':str(output/'missing')})
            assert empty is None
        # Native CLI persists the separate destination and exposes suggestions.
        source=output/'cli-input.json';target=output/'cli-output.json';status=output/'cli-status.json'
        source.write_text(json.dumps(page,ensure_ascii=False));server.mode='suggestion'
        process=subprocess.run([str(binary),'translate','--vision','--cache',str(source),'--destination',str(target),
            '--image',str(image),'--status',str(status),'--model','same-model','--provider','omlx','--base-url',server.url],
            capture_output=True,text=True,timeout=90)
        assert process.returncode==0,process.stderr
        assert json.loads(process.stdout)=={'ok':True,'cache':str(target)}
        result=json.loads(target.read_text())
        assert result['blocks'][0]['translation']=='原手改'
        assert any(b.get('vision_source')=='こんばんは' for b in result['blocks'])
        assert source.read_text()==json.dumps(page,ensure_ascii=False)
        assert target.stat().st_mode&0o777==0o600
    hashes()
    report={'pages':len(manifest),'scenarios':len(cases),'native_calls':calls,'posts':sum(c['posts'] for c in cases),
            'passed':True,'input_hashes_unchanged':True,'scope':'native visual JPEG/prompt/HTTP/checkpoint/cache behavior with simulated model output','cases':cases}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({k:v for k,v in report.items() if k!='cases'}))


if __name__=='__main__':main()
