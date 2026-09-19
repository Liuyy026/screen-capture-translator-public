"""Run A/B against fresh copies of identical OCR. Never modifies source caches."""
from __future__ import annotations
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from backend import worker, structure


def quality(page):
    rows = [b for b in page['blocks'] if b.get('overlay') != 'skip' and b.get('text_kind') != 'noise']
    eligible_ids = {str(b['id']) for b in rows}
    units = [d for d in page['dialogues'] if set(d['source_block_ids']) <= eligible_ids]
    return {'eligible_blocks': len(rows), 'mapped_blocks': sum(structure.valid_block(b) for b in rows),
            'error_blocks': sum(bool(b.get('error')) for b in rows),
            'eligible_units': len(units), 'complete_units': sum(d['translation_complete'] for d in units),
            'whole_only_units': sum(d.get('mapping_status') == 'whole_only' and d['translation_complete'] for d in units),
            'review_units': sum(d.get('needs_review', False) for d in units)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('cache_dir', type=Path)
    parser.add_argument('image_dir', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--model', default='qwen3:4b-q6k')
    parser.add_argument('--limit', type=int, default=0)
    args = parser.parse_args()
    # A new destination prevents stale translations or timing files from being
    # mistaken for inference in this experiment.
    if args.output.exists():
        raise ValueError('输出目录已存在，请使用新的目录以免复用实验结果')
    models = worker.api('/api/tags')['models']
    model = next(m for m in models if m['name'] == args.model)
    paths = sorted(args.cache_dir.glob('*.json'))
    if args.limit:
        paths = paths[:args.limit]
    if not paths:
        raise ValueError('测试集为空')
    image_by_stem = {p.stem: p for p in args.image_dir.iterdir() if p.is_file()}
    seeds, manifest = [], {'model': args.model, 'digest': model.get('digest'), 'pages': [],
                          'protocol': 4, 'ordering': 'alternating AB/BA by page',
                          'limitations': ['自动有效性不等于翻译正确率', '未标注人工连贯性、分组准确率及误伤率']}
    args.output.mkdir(parents=True)
    for path in paths:
        raw = json.loads(path.read_text(encoding='utf-8'))
        image = image_by_stem[path.stem]
        blocks = [{k: b[k] for k in ('id', 'source', 'box', 'vertical') if k in b} for b in raw['blocks']]
        for b in blocks:
            b.update(translation='', error='', edited=False)
        page = {'width': raw.get('width'), 'height': raw.get('height'), 'blocks': blocks, 'model': ''}
        worker._overlay.ensure_page_overlays(page, image=image)
        structure.ensure_dialogues(page, image=image)
        manifest['pages'].append({'page': path.name, 'image': str(image.resolve()),
            'cache_sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'image_sha256': hashlib.sha256(image.read_bytes()).hexdigest(),
            'ocr_signature': structure.ocr_signature(page), 'structure_signature': page['structure']['signature']})
        worker.atomic_json(args.output / 'ocr' / path.name, page)
        seeds.append((path.name, image, page))
    worker.atomic_json(args.output / 'manifest.json', manifest)
    os.environ['KOMA_BENCHMARK_METRICS'] = str(args.output / 'requests.jsonl')
    records = []
    for index, (name, image, seed) in enumerate(seeds):
        for mode in (('blocks', 'dialogues') if index % 2 == 0 else ('dialogues', 'blocks')):
            page = copy.deepcopy(seed)
            dest = args.output / mode / name
            started = time.perf_counter()
            worker.translate(page, args.model, lambda _: None, lambda value: worker.atomic_json(dest, value),
                             image=image, translation_unit=mode, batch_size=8)
            assert structure.ocr_signature(page) == structure.ocr_signature(seed)
            record = {'page': name, 'mode': mode, 'seconds': time.perf_counter()-started, **quality(page)}
            records.append(record)
            worker.atomic_json(args.output / 'records.json', records)
            print(json.dumps(record, ensure_ascii=False), flush=True)
    summary = {'model': args.model, 'digest': model.get('digest'), 'pages': len(seeds), 'modes': {}}
    for mode in ('blocks', 'dialogues'):
        rows = [r for r in records if r['mode'] == mode]
        totals = {k: sum(r[k] for r in rows) for k in ('seconds', 'eligible_blocks', 'mapped_blocks',
                  'error_blocks', 'eligible_units', 'complete_units', 'whole_only_units', 'review_units')}
        totals['unit_completion_rate'] = totals['complete_units']/totals['eligible_units'] if totals['eligible_units'] else None
        summary['modes'][mode] = totals
    worker.atomic_json(args.output / 'summary.json', summary)
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
