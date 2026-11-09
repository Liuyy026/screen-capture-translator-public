#!/usr/bin/env python3
"""Run Reader controller tests without a window, using only loopback HTTP."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--assets',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2];output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    pages=json.loads(args.manifest.read_text())
    def hashes():
        for p in pages:
            for field,key in [('image','image_hash'),('cache','cache_hash'),('original_cache','cache_hash')]:
                assert hashlib.sha256(Path(p[field]).read_bytes()).hexdigest()==p[key]
    hashes()
    cache=output/'page-copies';(cache/'vision-v1').mkdir(parents=True)
    for p in pages:
        for directory in [cache,cache/'vision-v1']:
            (directory/(p['image_hash']+'.json')).write_bytes(Path(p['cache']).read_bytes())
    shutil.copytree(args.assets,output/'reading-native-v1')
    calls=[]
    class Handler(BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_GET(self):self.reply({'data':[{'id':'same-model'}]})
        def do_POST(self):
            payload=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            calls.append({'path':self.path,'authorized':self.headers.get('Authorization')=='Bearer fake-secret'})
            if (output/'slow').exists():
                (output/'post-started').touch()
                while (output/'slow').exists():time.sleep(.01)
            schema=payload['response_format']['json_schema']['schema']
            ids=schema['properties']['translations']['required']
            content=json.dumps({'translations':{key:'测试中文' for key in ids}})
            if (output/'fail').exists():self.reply({'error':'private fake-secret'},500)
            else:self.reply({'choices':[{'message':{'content':content},'finish_reason':'stop'}]})
        def reply(self,value,status=200):
            data=json.dumps(value).encode();self.send_response(status)
            self.send_header('Content-Length',str(len(data)));self.end_headers()
            try:self.wfile.write(data)
            except (BrokenPipeError,ConnectionResetError):pass
    server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    binary=output/'reader-state-tests'
    try:
        with (output/'build.log').open('w') as log:
            subprocess.run(['xcrun','swiftc','-O','-target','arm64-apple-macosx14.0','-parse-as-library',
                '-D','KOMA_REVIEW_HARNESS',*[str(p) for p in sorted((root/'macos/Sources').glob('*.swift'))],
                str(root/'macos/tests/NativeReaderStateTests.swift'),'-o',str(binary),'-framework','SwiftUI',
                '-framework','AppKit','-framework','CryptoKit','-framework','Security'],stdout=log,stderr=log,check=True)
        with (output/'test.log').open('w') as log:
            subprocess.run([str(binary),str(root),str(output),f'http://127.0.0.1:{server.server_port}/v1'],stdout=log,stderr=log,check=True,timeout=240)
    finally:
        (output/'slow').unlink(missing_ok=True)
        server.shutdown();server.server_close();thread.join()
        (output/'http-calls.json').write_text(json.dumps(calls,indent=2))
    hashes()
    result=json.loads((output/'report.json').read_text());assert result['passed']
    assert all(c['authorized'] for c in calls)
    assert 'fake-secret' not in (output/'test.log').read_text()
    print(json.dumps(result,ensure_ascii=False))


if __name__=='__main__':main()
