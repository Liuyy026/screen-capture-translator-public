#!/usr/bin/env python3
"""Real SR and OCR through Rust; deterministic loopback translation only.

Checks effective image dimensions, isolated caches, manual edit retention and
the real display preparation path. Does not evaluate translation correctness.
"""
import argparse
import fcntl
import hashlib
import json
from pathlib import Path
import sys
import time
import unicodedata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--lock', type=Path, required=True)
    args = parser.parse_args()
    root, source, output = args.root.resolve(), args.input.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(root/'tools/rust_migration'))
    from verify_runtime import Client
    from verify_translate_pipeline import Server
    from PIL import Image
    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    records, identities = [], set()
    with args.lock.open('a') as lock, Server() as server:
        fcntl.flock(lock, fcntl.LOCK_EX)
        client = Client(root, output/'runtime', root/'.models', root/'core/python/model_worker.py')
        def call(op, **values):
            reply = client.wait(client.send(op, **values), timeout=240)
            assert reply['event'] == 'result', reply
            return reply['result']
        try:
            baseline_cache = None
            protected_bytes = None
            for engine in ('off', 'metalfx', 'realcugan'):
                settings = {'engine': engine, 'target': 1440, 'axis': 'height',
                            'preset': 'conservative' if engine == 'realcugan' else 'standard'}
                started = time.monotonic()
                prepared = call('load_page', image=str(source), mode='text', super_resolution=settings)
                effective = Path(prepared['effective_image'])
                expected = Image.open(effective).size
                assert (effective == source) == (engine == 'off')
                if engine != 'off':
                    assert expected[1] == 1440
                    assert prepared['page'] is None, 'Borrowed an unrelated OCR cache'
                reply = call('process', images=[str(source)], operation='both', mode='text',
                             super_resolution=settings, model='same-model', provider='omlx',
                             base_url=server.url, key='fake-secret')
                assert reply['pages'][0]['ok'], reply
                loaded = call('load_page', image=str(source), mode='text', super_resolution=settings)
                page = loaded['page']
                assert (page['width'], page['height']) == expected, 'OCR used the wrong image dimensions'
                assert len(page['blocks']) > 0, 'Real OCR detected no text in this fixture'
                assert loaded['cache'] not in identities, 'Backends shared OCR cache'
                identities.add(loaded['cache'])
                display = call('display', image=str(effective), cache=loaded['cache'])
                assert display['reply']['version'] == 3
                assert display['reply']['regions'], 'No translated display regions'
                displayed = {str(key) for region in display['reply']['regions'] for key in region['block_ids']}
                preserved = display['reply'].get('preserved', {})
                normalize = lambda text: unicodedata.normalize('NFKC', text).strip()
                missing = [b['id'] for b in page['blocks'] if b.get('translation', '').strip()
                           and not b.get('error') and normalize(b['translation']) != normalize(b['source'])
                           and str(b['id']) not in displayed and str(b['id']) not in preserved]
                assert not missing, f'Translations missing from display: {missing}'
                (output/f'{engine}-page.json').write_text(json.dumps(page, ensure_ascii=False, indent=2))
                (output/f'{engine}-display.json').write_text(json.dumps(display, ensure_ascii=False))
                records.append({'engine':engine, 'settings':settings, 'effective_image':str(effective),
                                'cache':loaded['cache'], 'width':page['width'], 'height':page['height'],
                                'detected_blocks':len(page['blocks']),
                                'display_regions':len(display['reply']['regions']),
                                'displayed_blocks':len(displayed),
                                'pipeline_seconds':time.monotonic()-started})
                if engine == 'off':
                    block = page['blocks'][0]
                    call('edit', cache=loaded['cache'], block_id=block['id'], revision=loaded['revision'],
                         values={'translation':'这是原图缓存的人工修改，增强测试必须保留。','edited':True})
                    baseline_cache = Path(loaded['cache'])
                    protected_bytes = baseline_cache.read_bytes()
                else:
                    assert baseline_cache.read_bytes() == protected_bytes, 'Original manual edit changed'
                    vision = call('load_page', image=str(source), mode='vision', super_resolution=settings)
                    assert vision['page'] is not None, 'Enhanced ordinary OCR was not reused in vision mode'
                    assert (vision['page']['width'], vision['page']['height']) == expected
                    assert [b['source'] for b in vision['page']['blocks']] == [b['source'] for b in page['blocks']]
                cached = call('load_page', image=str(source), mode='text', super_resolution=settings)
                assert cached['cache'] == loaded['cache'] and cached['effective_image'] == str(effective)
                (output/'report.partial.json').write_text(json.dumps(records, ensure_ascii=False, indent=2))
            assert baseline_cache.read_bytes() == protected_bytes
            assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
            report = {'passed':True,'real_super_resolution':True,'real_ocr':True,
                      'real_display_models':True,'translation':'deterministic loopback stub, not a language model',
                      'ocr_accuracy_claim':False,'records':records,'translation_requests':len(server.calls),
                      'original_input_unchanged':True,'original_manual_edit_unchanged':True}
            (output/'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2))
            print(json.dumps(report, ensure_ascii=False))
        finally:
            client.close()


if __name__ == '__main__':
    main()
