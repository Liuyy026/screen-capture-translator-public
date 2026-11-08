#!/usr/bin/env python3
"""Compare native translation protocols against the frozen Python workers.

Uses saved page copies and a loopback fake HTTP server only. No live inference,
real credentials, original cache writes, or model lifecycle requests.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    baseline = args.baseline_root.resolve()
    root = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = root / 'core/target/release/koma-core'
    sys.path.insert(0, str(baseline))
    from backend import worker as old
    from backend.translation_service import TranslationService, TranslationServiceError, normalize_base_url
    vision = load('baseline_vision_protocol', baseline / 'macos/backend/vision_worker.py')
    service_tests = load('baseline_service_tests', baseline / 'tests/test_translation_service.py')
    # Never inherit a user's real secret into test fixtures or native processes.
    os.environ['KOMA_TRANSLATION_API_KEY'] = ''
    manifest = json.loads(args.manifest.read_text())
    def hashes():
        for entry in manifest:
            for field, expected in [('image', 'image_hash'), ('cache', 'cache_hash'), ('original_cache', 'cache_hash')]:
                assert hashlib.sha256(Path(entry[field]).read_bytes()).hexdigest() == entry[expected], (entry['page'], field)
    hashes()
    calls = 0
    def native(command, requests):
        nonlocal calls
        calls += 1
        path = output / f'{calls:04}-{command}.json'
        path.write_text(json.dumps(requests, ensure_ascii=False))
        result = subprocess.run([str(binary), command, '--requests', str(path)], capture_output=True, text=True,
                                env={**os.environ, 'KOMA_TRANSLATION_API_KEY': ''}, timeout=90)
        path.with_suffix('.stderr.log').write_text(result.stderr)
        assert result.returncode == 0, result.stderr
        path.with_suffix('.reply.json').write_text(result.stdout)
        return json.loads(result.stdout)
    def expected(fn):
        try:
            return {'ok': True, 'value': fn()}
        except json.JSONDecodeError:
            # Native parser deliberately uses one safe, non-echoing syntax error.
            return {'ok': False, 'error': '模型未返回有效 JSON'}
        except ValueError as exc:
            return {'ok': False, 'error': str(exc)}
    requests, answers, labels = [], [], []
    def add(req, fn, label):
        requests.append(req)
        answers.append(expected(fn))
        labels.append(label)
    def quality(source, translation, region=False):
        req = {'op': 'quality', 'source': source, 'translation': translation, 'has_region': region}
        add(req, lambda: {'error': old.translation_error(translation), 'echo': old.same_as_source(source, translation),
                         'warnings': old.translation_quality_warnings(source, translation),
                         'low_impact': old.is_low_impact_utterance(source, region={'shape': 'bubble'} if region else None)},
            'quality')
    def parse(content, ids, sources=None):
        req = {'op': 'parse', 'content': content, 'expected': ids}
        if sources is not None:
            req['sources'] = sources
        def reference():
            found, errors = old.parse_translations(content, set(ids), sources)
            return {'found': found, 'errors': errors}
        add(req, reference, 'parse')
    pages = []
    for entry in manifest:
        page = json.loads(Path(entry['cache']).read_text())
        pages.append(page)
        for block in page['blocks']:
            quality(block['source'], block.get('translation', ''))
        add({'op': 'audit', 'page': page}, lambda page=page: audit(page), 'audit')
        sources = {str(b['id']): b['source'] for b in page['blocks']}
        parse(json.dumps({'translations': {str(b['id']): b.get('translation', '') for b in page['blocks']}}), list(sources), sources)
    for source in ['ん', 'えっ', 'まあ', 'ふふ', '......', '！？', '－－', '!? ' * 5, 'ありがとう', '高くない',
                   '私よりデカい', 'ありません', '', None, 42, 'İ', 'Ａ', '\x1c', 'あ\x1f']:
        for translation in ['没有比我更大', '翻译失败', 'ＴＲＡＮＳＬＡＴＩＯＮ', '...', None, 12, source]:
            quality(source, translation)
        quality(source, '中文', True)
    for placeholder in old.PLACEHOLDERS:
        quality('日本語', ' 【 ' + placeholder.upper() + ' 】 ')
    for content in ['{}', '[]', 'null', 'broken secret', '{"translations":{', '{"translations":null}',
                    '{"translations":{"1":"中文","1":"另一个"}}',
                    '{"translations":{"1":"中文"},"translations":{}}',
                    '{"translations":[{"id":"1","text":"好"},{"id":"1","text":"又一个"},{"id":"1","text":"第三个"}]}',
                    '{"translations":[null,7,{"id":1,"text":"中文"},{"id":"unknown","text":"忽略"}]}',
                    '{"translations":{"1":"おはよう","2":"中文"}}',
                    '{"translations":{"1":"翻译失败","2":"中文"}}',
                    '{"translations":{"1":true,"2":20}}']:
        parse(content, ['1', '2'], {'1': 'おはよう', '2': 'こんにちは'})
    for value in ['一整句', '第一句\n第二句', '一\r\n二', '一\x85二', '一\u2028二', ' 一\x1e二 ', '', '一\n\n二\n三']:
        for count in range(4):
            members = [{'id': str(i+1)} for i in range(count)]
            def reference(value=value, members=members):
                mapping, status = old.split_dialogue_translation(value, members)
                return {'mapping': mapping, 'status': status}
            add({'op': 'split', 'text': value, 'members': members}, reference, 'split')
    machine = {'blocks': [{'id': '1', 'source': 'おはよう', 'translation': 'おはよう', 'edited': False},
                          {'id': '2', 'source': '朝', 'translation': '翻译失败', 'edited': True},
                          {'id': '3', 'source': '夜', 'translation': '翻译失败', 'edited': False}]}
    add({'op': 'audit', 'page': machine}, lambda: audit(machine), 'manual-preservation')
    def compare(command, requests, answers, labels):
        actual = native(command, requests)
        mismatches = [{'index': i, 'label': labels[i], 'expected': want, 'actual': got}
                      for i, (want, got) in enumerate(zip(answers, actual)) if want != got]
        assert len(actual) == len(answers)
        (output / f'{command}-differences.json').write_text(json.dumps(mismatches, ensure_ascii=False, indent=2))
        assert not mismatches, f'{len(mismatches)} {command} differences; see private report'
        return len(actual)
    protocol_count = compare('protocol-probe', requests, answers, labels)
    requests, answers, labels = [], [], []
    for provider in ['ollama', 'omlx', 'unknown']:
        for url in [None, '', 'http://LOCALHOST:80', 'https://LOCALHOST:443/', 'http://[::1]:11439/',
                    'http://localhost:8000/prefix/', 'http://localhost:0', 'http://localhost:',
                    'http://u:secret@host', 'http://host?', 'http://host#', 'http://host:99999', 'http://ho st',
                    'ftp://localhost', 'localhost:8000']:
            add({'op': 'normalize', 'provider': provider, 'base_url': url},
                lambda p=provider, u=url: normalize_base_url(p, u), 'URL')
    for provider in ['ollama', 'omlx']:
        svc = TranslationService(provider)
        for response in [{}, [], None, {'error': 'private'}, {'choices': []}, {'message': {}},
                         {'message': {'content': None}}, {'message': {'content': '  '}},
                         {'message': {'content': '{}'}}, {'message': {'content': '{}'}, 'done_reason': 'length'},
                         {'choices': [{'message': {'content': '{}'}, 'finish_reason': 'length'}]},
                         {'choices': [{'message': {'content': '{}'}}]}]:
            add({'op': 'content', 'provider': provider, 'response': response},
                lambda r=response, s=svc: s.translation_content(r), 'content')
    for page in pages:
        svc = TranslationService('omlx')
        for start in range(0, len(page['blocks']), 8):
            batch = page['blocks'][start:start+8]
            add({'op': 'vision_payload', 'provider': 'omlx', 'page': page, 'batch': batch,
                 'image_url': 'data:image/jpeg;base64,fixed', 'model': 'same-model'},
                lambda p=page, b=batch: vision.make_payload(svc, 'same-model', p, b, 'data:image/jpeg;base64,fixed'),
                'vision-prompt')
    # Capture prompts from the actual old orchestrator; never reconstruct its
    # instructions in this verifier. Page copies and fake HTTP results only.
    for page in pages:
        for unit in ['blocks', 'dialogues']:
            page = copy.deepcopy(page)
            for block in page['blocks']:
                if not block.get('edited'):
                    block.update(translation='', error='')
            page.pop('translation_config', None)
            page['model'] = ''
            def fake(route, payload=None, timeout=300):
                if route == '/api/tags':
                    return {'models': [{'name': 'same-model'}]}
                data = json.loads(payload['messages'][1]['content'].removesuffix('\n/no_think'))
                add({'op': 'block_payload', 'provider': 'ollama', 'model': 'same-model', 'context': data['page'],
                     'ids': data['target_ids'], 'dialogue': unit == 'dialogues' and len(data['page']) == 1
                     and data['target_ids'][0].startswith('d')}, lambda: payload, 'block-or-dialogue-prompt')
                # Return no translations so no generated content can affect the next job.
                return {'message': {'content': '{"translations":{}}'}}
            with patch.object(old, 'api', side_effect=fake):
                old.translate(page, 'same-model', lambda _: None, lambda _: None, translation_unit=unit)
    payload_count = compare('service-probe', requests, answers, labels)

    def one(req):
        reply = native('service-probe', [req])[0]
        if not reply['ok']:
            raise TranslationServiceError(reply['error'])
        return reply['value']
    class NativeService:
        def __init__(self, provider='ollama', base_url=None):
            self.provider = provider
            self.key = os.environ.get('KOMA_TRANSLATION_API_KEY', '')
            self.identity = one({'op': 'identity', 'provider': provider, 'base_url': base_url, 'key': self.key})
            self.base_url = self.identity['base_url']
        def run(self, op, **kw):
            return one({'op': op, **self.identity, 'key': self.key, **kw})
        def list_models(self):
            return self.run('list_models')
        def translation_content(self, response):
            return self.run('content', response=response)
        def safe_error(self, exc):
            return self.run('safe_error', message=str(exc))
    service_tests.TranslationService = NativeService
    service_tests.normalize_base_url = lambda provider, base_url=None: one({'op': 'normalize', 'provider': provider, 'base_url': base_url})
    adapter = output / 'cli/backend/worker.py'
    adapter.parent.mkdir(parents=True)
    adapter.write_text('import os,sys\ncore='+repr(str(binary))+'\nos.execv(core,[core,"list-models",*[v for v in sys.argv[1:] if v!="--list-models"]])\n')
    service_tests.ROOT = adapter.parents[1]
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(service_tests.TranslationServiceTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    hashes()
    report = {'pages': len(pages), 'protocol_comparisons': protocol_count, 'service_payload_comparisons': payload_count,
              'http_behavior_tests': result.testsRun, 'http_failures': len(result.failures), 'http_errors': len(result.errors),
              'skipped': len(result.skipped), 'native_calls': calls, 'passed': result.wasSuccessful(),
              'scope': 'protocol, exact prompt strings and loopback HTTP only; orchestration/image encoding not yet migrated',
              'input_hashes_unchanged': True}
    (output / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(report))
    if not report['passed']:
        raise SystemExit(1)


def audit(page):
    from backend import worker
    page = copy.deepcopy(page)
    worker.audit_cache(page)
    return page


if __name__ == '__main__':
    main()
