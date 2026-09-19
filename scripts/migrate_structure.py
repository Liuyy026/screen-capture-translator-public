"""Add dialogue/diagnostic metadata to existing page caches without OCR."""
from __future__ import annotations
import argparse, json
from pathlib import Path
import sys
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from backend.structure import build_dialogues


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("cache_dir", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    files = sorted(args.cache_dir.glob("*.json"))
    changed = 0
    for path in files:
        try: page = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError): continue
        before = json.dumps((page.get("dialogues"), page.get("structure"), page.get("diagnostics")), ensure_ascii=False, sort_keys=True)
        build_dialogues(page)
        if before == json.dumps((page.get("dialogues"), page.get("structure"), page.get("diagnostics")), ensure_ascii=False, sort_keys=True): continue
        changed += 1
        if not args.dry_run: path.write_text(json.dumps(page, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"files": len(files), "changed": changed, "dry_run": args.dry_run}, ensure_ascii=False))


if __name__ == "__main__": main()
