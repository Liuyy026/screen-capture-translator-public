"""Compare explicit model cache directories, rejecting incompatible A/B inputs."""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from backend.structure import ensure_dialogues, ocr_signature, valid_block


def collect(root: Path):
    if (root/'model-caches').is_dir():
        choices = [p for p in (root/'model-caches').iterdir() if p.is_dir()]
        if len(choices) != 1:
            raise ValueError('运行目录包含多个模型，请明确指定一个模型缓存目录')
        root = choices[0]
    paths = [root] if root.is_file() else sorted(root.glob('*.json'))
    if not paths:
        raise ValueError('缓存集为空')
    metrics = {key: 0 for key in ('pages', 'blocks', 'eligible_blocks', 'valid_blocks', 'error_blocks',
               'eligible_dialogues', 'complete_dialogues', 'whole_only_dialogues',
               'quality_warning_blocks', 'quality_warning_units', 'layout_failures')}
    inputs, models = {}, set()
    for path in paths:
        page = json.loads(path.read_text(encoding='utf-8'))
        if not isinstance(page, dict) or not isinstance(page.get('blocks'), list):
            raise ValueError(f'{path.name} 不是页面缓存')
        ensure_dialogues(page)
        rows = [b for b in page['blocks'] if b.get('overlay') != 'skip' and not b.get('edited') and b.get('text_kind') != 'noise']
        eligible = {str(b['id']) for b in rows}
        units = [d for d in page['dialogues'] if set(d['source_block_ids']) <= eligible]
        metrics['pages'] += 1
        metrics['blocks'] += len(page['blocks'])
        metrics['eligible_blocks'] += len(rows)
        metrics['valid_blocks'] += sum(valid_block(b) for b in rows)
        metrics['error_blocks'] += sum(bool(b.get('error')) for b in rows)
        metrics['eligible_dialogues'] += len(units)
        metrics['complete_dialogues'] += sum(d['translation_complete'] for d in units)
        metrics['whole_only_dialogues'] += sum(d['translation_complete'] and d.get('mapping_status') == 'whole_only' for d in units)
        metrics['quality_warning_blocks'] += sum(bool(b.get('quality_warnings')) for b in rows)
        metrics['quality_warning_units'] += sum(bool(d.get('quality_warnings')) for d in units)
        metrics['layout_failures'] += len(page.get('layout_failures', {}))
        models.add(page.get('model', ''))
        inputs[path.name] = {'ocr': ocr_signature(page), 'eligible': sorted(eligible),
                             'units': [d['block_ids'] for d in units]}
    metrics['mapped_block_rate'] = metrics['valid_blocks']/metrics['eligible_blocks'] if metrics['eligible_blocks'] else None
    metrics['unit_completion_rate'] = metrics['complete_dialogues']/metrics['eligible_dialogues'] if metrics['eligible_dialogues'] else None
    return {**metrics, 'inputs': inputs, 'models': sorted(models)}


def compare(left, right):
    a, b = collect(Path(left)), collect(Path(right))
    reasons = []
    if a['models'] != b['models'] or len(a['models']) != 1 or not a['models'][0]:
        reasons.append('模型不相同或缺少模型标识')
    if a['inputs'] != b['inputs']:
        reasons.append('页集合、OCR、可翻译成员或分组顺序不相同')
    return {'left': a, 'right': b, 'comparable': not reasons, 'mismatch_reasons': reasons,
            'limitations': ['完整译文状态是自动有效性指标，不是人工准确率', '逐块空值可能表示只有整句可用，不能等同于翻译失败']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('left', type=Path)
    parser.add_argument('right', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = compare(args.left, args.right)
    text = json.dumps(result, ensure_ascii=False, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding='utf-8')
    print(text)
    if not result['comparable']:
        raise SystemExit(2)

if __name__ == '__main__':
    main()
