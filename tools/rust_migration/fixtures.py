"""Generated inputs for core regression tests; no private corpus or weights."""
import hashlib
import json
import os
from pathlib import Path

from PIL import Image, ImageDraw


def core_binary(root):
    path = Path(os.environ.get('KOMA_TEST_CORE', root/'core/target/debug/koma-core')).resolve()
    if not path.is_file():
        raise FileNotFoundError(f'Build the core first with tools/rust_migration/cargo.py build: {path}')
    return path


def make_pages(directory):
    directory.mkdir(parents=True, exist_ok=False)
    caches = directory/'seed/vision-v1'
    caches.mkdir(parents=True)
    pages = []
    for index in range(2):
        image = directory/f'page-{index}.png'
        pixels = Image.new('RGB', (300, 300), (40 + index, 40, 40))
        draw = ImageDraw.Draw(pixels)
        draw.ellipse((25, 25, 130, 160), fill='white', outline='black', width=3)
        draw.ellipse((165, 25, 280, 160), fill='white', outline='black', width=3)
        pixels.save(image)
        image_hash = hashlib.sha256(image.read_bytes()).hexdigest()
        page = {'version': 2, 'width': 300, 'height': 300, 'model': '', 'ocr': 'synthetic',
                'blocks': [
                    {'id': '1', 'box': [50, 50, 85, 110], 'source': 'こんにちは',
                     'translation': '你好', 'error': '', 'edited': False, 'vertical': True},
                    {'id': '2', 'box': [190, 50, 225, 110], 'source': 'さようなら',
                     'translation': '手改译文', 'error': '', 'edited': True, 'vertical': True}]}
        cache = caches/f'{image_hash}.json'
        cache.write_text(json.dumps(page, ensure_ascii=False), encoding='utf-8')
        pages.append({'page': index + 1, 'image': str(image), 'image_hash': image_hash,
                      'cache': str(cache), 'original_cache': str(cache),
                      'cache_hash': hashlib.sha256(cache.read_bytes()).hexdigest()})
    return pages
