"""Render fixed A/B outputs and measure confinement to approved region contours."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import sys
import time
os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import cv2
import numpy as np
from PySide6.QtGui import QImage, QPixmap
from PySide6.QtWidgets import QApplication
from PIL import Image, ImageDraw, ImageFont
from windows_app import ImageCanvas
from backend.worker import atomic_json


def pixels(canvas, mode):
    canvas.set_mode(mode)
    image = canvas.grab().toImage().convertToFormat(QImage.Format_RGBA8888)
    return np.frombuffer(image.constBits(), dtype=np.uint8).reshape(image.height(), image.bytesPerLine())[:, :image.width()*4].reshape(image.height(), image.width(), 4).copy()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    parser.add_argument('--output', type=Path, help='Save a new report without replacing historical evidence')
    parser.add_argument('--no-samples', action='store_true', help='Measure all pages without rewriting sample images')
    args = parser.parse_args()
    manifest = json.loads((args.run/'manifest.json').read_text(encoding='utf-8'))
    app = QApplication.instance() or QApplication([])
    canvas = ImageCanvas()
    records = []
    samples = {'00001.json', '00012.json', '00030.json', '00045.json'}
    target = args.run/'renders'
    target.mkdir(exist_ok=True)
    for entry in manifest['pages']:
        for mode in ('blocks', 'dialogues'):
            cache = args.run/mode/entry['page']
            if not cache.exists():
                continue
            page = json.loads(cache.read_text(encoding='utf-8'))
            pixmap = QPixmap(entry['image'])
            canvas.set_image(pixmap)
            canvas.setMinimumSize(1, 1)
            canvas.resize(pixmap.width()+24, pixmap.height()+24)
            start = time.perf_counter()
            canvas.set_page(page)
            original, rendered = pixels(canvas, 0), pixels(canvas, 2)
            elapsed = time.perf_counter()-start
            allowed = np.zeros(original.shape[:2], dtype=np.uint8)
            rect = canvas._image_rect()
            for region in canvas._overlay_regions:
                if region.get('overlay') not in {'balloon', 'rectangle'} or str(region['id']) not in canvas._overlay_layouts:
                    continue
                points = np.array([[round(rect.x()+x*rect.width()/pixmap.width()),
                                    round(rect.y()+y*rect.height()/pixmap.height())] for x,y in region.get('points', [])], dtype=np.int32)
                if len(points) >= 3:
                    cv2.fillPoly(allowed, [points], 1)
            # Two source pixels accommodate contour rasterization at the edge.
            allowed = cv2.dilate(allowed, np.ones((5,5), dtype=np.uint8))
            changed = np.any(original != rendered, axis=2)
            records.append({'page': entry['page'], 'mode': mode, 'layout_seconds': elapsed,
                            'safe_regions': sum(r.get('overlay') in {'balloon','rectangle'} for r in canvas._overlay_regions),
                            'laid_out_regions': len(canvas._overlay_layouts), 'layout_failures': canvas._layout_failures,
                            'layout_pending': canvas._layout_pending,
                            'changed_pixels': int(changed.sum()), 'outside_contour_pixels': int((changed & (allowed == 0)).sum())})
            if not args.no_samples and entry['page'] in samples:
                for name, data in [('original', original), (mode, rendered), ('structure', pixels(canvas, 1))]:
                    image = Image.fromarray(data)
                    image.thumbnail((1100, 1700))
                    image.save(target/f'{Path(entry["page"]).stem}-{name}.png')
    atomic_json(args.output or args.run/'layout-report.json', {'measure': 'changed pixels outside approved contours, 2px edge tolerance; not artwork false-positive rate',
        'records': records, 'outside_contour_pixels': sum(r['outside_contour_pixels'] for r in records)})
    print(json.dumps({mode: {'pages': sum(r['mode']==mode for r in records),
         'laid_out_regions': sum(r['laid_out_regions'] for r in records if r['mode']==mode),
         'safe_regions': sum(r['safe_regions'] for r in records if r['mode']==mode),
         'outside_contour_pixels': sum(r['outside_contour_pixels'] for r in records if r['mode']==mode)}
         for mode in ('blocks', 'dialogues')}, ensure_ascii=False))
    canvas.close()

if __name__ == '__main__':
    main()
