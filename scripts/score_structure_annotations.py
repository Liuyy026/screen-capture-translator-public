"""Aggregate human structure annotation scores without inventing missing labels."""
from __future__ import annotations
import argparse, json, hashlib
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.annotate_structure import score, refresh_predictions


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('annotation_dir',type=Path);p.add_argument('--output',type=Path)
    p.add_argument('--cache-dir', type=Path, help='Score actual cached predictions against OCR-bound labels without rewriting annotations')
    a=p.parse_args(); rows=[]
    for f in sorted(a.annotation_dir.glob('*.json')):
        try:
            data=json.loads(f.read_text(encoding='utf8'))
            if a.cache_dir:
                cache = a.cache_dir / f.name
                raw = cache.read_bytes()
                data = refresh_predictions(data, json.loads(raw), cache.resolve(), hashlib.sha256(raw).hexdigest())
            rows.append({'page':f.name, **score(data)})
        except (OSError,ValueError,TypeError,AttributeError) as exc:
            rows.append({'page':f.name,'annotated':False,'excluded':True,'error':str(exc)})
    annotated=[r for r in rows if r.get('annotated')]
    def avg(key):
        values=[r[key] for r in annotated if isinstance(r.get(key), (int, float))]
        return sum(values)/len(values) if values else None
    report={'annotation_dir':str(a.annotation_dir),'templates':len(rows),'annotated_pages':len(annotated),
            'unannotated_pages':sum(not r.get('annotated') and not r.get('excluded') for r in rows),
            'excluded_pages':sum(bool(r.get('excluded')) for r in rows),
            'human_labeled_pages':sum(r.get('reviewer_type') == 'human' for r in annotated),
            'assistant_audited_pages':sum(r.get('reviewer_type') == 'assistant_visual_audit' for r in annotated),
            'dialogue_exact_mean':avg('dialogue_exact'),
            'panel_exact_mean':avg('panel_exact'),'dialogue_coverage_mean':avg('dialogue_coverage'),
            'panel_coverage_mean':avg('panel_coverage'),'block_attribute_exact_mean':avg('block_attribute_exact'),
            'page_reports':rows,
            'metric_pages':{key:sum(isinstance(r.get(key), (int, float)) for r in annotated)
                            for key in ('dialogue_exact', 'panel_exact', 'block_attribute_exact')},
            'limitations':['dialogue exact includes internal and external order; panel exact uses panel order and member sets',
                           'coverage is mean best Jaccard per expected unit, not one-to-one matching or accuracy',
                           'unannotated and invalid pages excluded from means; assistant audits are not human ground truth']}
    text=json.dumps(report,ensure_ascii=False,indent=2)
    if a.output:a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(text,encoding='utf8')
    print(text)
if __name__=='__main__':main()
