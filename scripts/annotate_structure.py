"""Snapshot cached predictions and score independently recorded structure labels."""
from __future__ import annotations
import argparse
import copy
import hashlib
import json
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from backend.structure import ocr_signature

SNAPSHOT_KEYS = ('width', 'height', 'blocks', 'predicted_dialogues', 'predicted_panel_order')


def digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True).encode()).hexdigest()


def template(page, cache_path=None, cache_sha256=None):
    # Capture actual predictions without silently rerunning a newer algorithm.
    snapshot = {
        'width': page.get('width'), 'height': page.get('height'),
        'blocks': [{'id': str(b.get('id')), 'source': b.get('source', ''), 'box': b.get('box'),
                    'vertical': bool(b.get('vertical')), 'text_kind': b.get('text_kind'),
                    'predicted_dialogue': b.get('dialogue_id'), 'predicted_panel': b.get('panel_id')}
                   for b in page.get('blocks', [])],
        'predicted_dialogues': [d.get('block_ids', []) for d in page.get('dialogues', [])],
        'predicted_panel_order': [p.get('block_ids', []) for p in sorted(
            page.get('panels', []), key=lambda p: p.get('reading_order', 0))],
    }
    return copy.deepcopy({
        'version': 2, **snapshot,
        'instructions': 'Label independently from the image. Groups must partition all block IDs. '
                        'Dialogue lists include internal reading order; panel lists encode membership only. '
                        'Leave unknown fields null; identify reviewer and evidence in label_provenance.',
        'prediction_provenance': {
            'cache_path': str(cache_path) if cache_path else None, 'cache_sha256': cache_sha256,
            'ocr_signature': ocr_signature(page), 'prediction_sha256': digest(snapshot),
            'structure_version': page.get('structure', {}).get('version'),
            'structure_signature': page.get('structure', {}).get('signature'),
        },
        'label_provenance': None,
        'expected_dialogues': None, 'expected_panel_order': None, 'expected_block_attributes': None,
    })


def refresh_predictions(annotation, page, cache_path=None, cache_sha256=None):
    fresh = template(page, cache_path, cache_sha256)
    previous = (annotation.get('prediction_provenance') or {}).get('ocr_signature')
    if previous is None:
        raise ValueError('Legacy annotation is not OCR-bound; independently review and migrate labels first')
    if previous != fresh['prediction_provenance']['ocr_signature']:
        raise ValueError('OCR fingerprint mismatch; labels cannot be reused on changed OCR')
    result = copy.deepcopy(annotation)
    for key in ('version', *SNAPSHOT_KEYS, 'prediction_provenance'):
        result[key] = fresh[key]
    return result


def score(annotation):
    result = {'annotated': False, 'label_warnings': []}
    warnings = result['label_warnings']
    blocks = annotation.get('blocks', [])
    known_ids = {str(b.get('id')) for b in blocks if isinstance(b, dict)}
    if not any(annotation.get(k) is not None for k in (
            'expected_dialogues', 'expected_panel_order', 'expected_block_attributes')):
        return {'annotated': False}
    provenance = annotation.get('prediction_provenance') or {}
    if provenance.get('prediction_sha256') != digest({k: annotation.get(k) for k in SNAPSHOT_KEYS}):
        warnings.append('Predictions are unbound or modified; refresh from the actual cache')
    if provenance.get('ocr_signature') != ocr_signature(annotation):
        warnings.append('OCR fingerprint is missing or mismatched')
    labels = annotation.get('label_provenance')
    if not isinstance(labels, dict) or labels.get('reviewer_type') not in ('human', 'assistant_visual_audit'):
        warnings.append('Label provenance must identify reviewer_type (human or assistant_visual_audit)')
    if len(known_ids) != len(blocks):
        warnings.append('OCR block IDs are duplicated or malformed')

    def inspect_groups(groups, label):
        if not isinstance(groups, list):
            warnings.append(f'{label} must be a list of block-ID lists'); return
        seen = set()
        for index, group in enumerate(groups, 1):
            if not isinstance(group, list):
                warnings.append(f'{label}[{index}] must be a list'); continue
            if any(not isinstance(x, str) for x in group):
                warnings.append(f'{label}[{index}] block IDs must be strings'); continue
            ids = set(group)
            if not ids:
                warnings.append(f'{label}[{index}] is empty')
            if len(ids) != len(group) or ids & seen:
                warnings.append(f'{label}[{index}] repeats block ids')
            if ids - known_ids:
                warnings.append(f'{label}[{index}] unknown block ids: {sorted(ids - known_ids)}')
            seen.update(ids)
        if label.startswith('expected_') and seen != known_ids:
            warnings.append(f'{label} does not partition all OCR block IDs')

    for suffix in ('dialogues', 'panel_order'):
        if annotation.get('expected_' + suffix) is not None:
            inspect_groups(annotation.get('expected_' + suffix), 'expected_' + suffix)
            inspect_groups(annotation.get('predicted_' + suffix), 'predicted_' + suffix)
    attrs = annotation.get('expected_block_attributes')
    if attrs is not None:
        if not isinstance(attrs, dict):
            warnings.append('expected_block_attributes must be an object')
        else:
            for bid, expected in attrs.items():
                if bid not in known_ids or not isinstance(expected, dict):
                    warnings.append(f'Invalid expected_block_attributes for {bid}'); continue
                for key, value in expected.items():
                    if (key not in ('vertical', 'text_kind') or
                            (key == 'vertical' and not isinstance(value, bool)) or
                            (key == 'text_kind' and (not isinstance(value, str) or not value))):
                        warnings.append(f'Invalid block attribute {bid}.{key}')
    # Exclude invalid labels/provenance entirely, never score them as a 0 or 1.
    if warnings:
        result['excluded'] = True
        return result
    result.pop('label_warnings')
    result['reviewer_type'] = labels['reviewer_type']
    for suffix, name in (('dialogues', 'dialogue'), ('panel_order', 'panel')):
        expected = annotation.get('expected_' + suffix)
        if expected is None:
            continue
        predicted = annotation['predicted_' + suffix]
        pred_sets, exp_sets = list(map(set, predicted)), list(map(set, expected))
        a, b = (predicted, expected) if name == 'dialogue' else (pred_sets, exp_sets)
        result[name + '_exact'] = (sum(x == y for x, y in zip(a, b)) / len(b) if b else 1.0) if len(a) == len(b) else 0.0
        result[name + '_coverage'] = _coverage(pred_sets, exp_sets)
        result[name + 's_predicted'] = len(predicted)
        result[name + 's_expected'] = len(expected)
        result['annotated'] = True
    if attrs is not None:
        predicted = {b['id']: b for b in blocks}
        checks = [predicted[bid].get(key) == value for bid, expected in attrs.items() for key, value in expected.items()]
        result['block_attributes_checked'] = len(checks)
        result['block_attribute_exact'] = sum(checks) / len(checks) if checks else None
        result['annotated'] |= bool(checks)
    return result


def _coverage(predicted, expected):
    """Mean best Jaccard per expected unit; not one-to-one matching or accuracy."""
    if not expected:
        return 1.0 if not predicted else 0.0
    return sum(max((len(target & candidate) / len(target | candidate)
                    for candidate in predicted), default=0.0) for target in expected) / len(expected)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('cache', type=Path); p.add_argument('annotation', type=Path)
    p.add_argument('--init', action='store_true'); p.add_argument('--force', action='store_true')
    p.add_argument('--refresh', action='store_true', help='Refresh predictions preserving labels bound to identical OCR')
    p.add_argument('--score', action='store_true')
    a = p.parse_args()
    raw = a.cache.read_bytes(); page = json.loads(raw)
    kwargs = {'cache_path': a.cache.resolve(), 'cache_sha256': hashlib.sha256(raw).hexdigest()}
    if a.init:
        if a.annotation.exists() and not a.force:
            p.error(f'annotation already exists: {a.annotation}; use --force to replace it')
        annotation = template(page, **kwargs)
    else:
        annotation = json.loads(a.annotation.read_text(encoding='utf8'))
        if a.score or a.refresh:
            try:
                annotation = refresh_predictions(annotation, page, **kwargs)
            except ValueError as exc:
                p.error(str(exc))
    if a.init or a.refresh:
        a.annotation.parent.mkdir(parents=True, exist_ok=True)
        a.annotation.write_text(json.dumps(annotation, ensure_ascii=False, indent=2), encoding='utf8')
    print(json.dumps(score(annotation) if a.score else annotation, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
