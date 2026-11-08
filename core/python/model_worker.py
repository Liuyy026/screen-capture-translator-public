#!/usr/bin/env python3
"""Private JSONL model service. Geometry, translation, caching and compositing live in Rust.

Arrays use task-owned raw uint8 files to avoid base64 copies. This worker
loads installed weights only and never contacts a translation endpoint.
"""
import argparse
import contextlib
import gc
import json
import math
from pathlib import Path
import sys

PROTOCOL = 1
MAX_ARRAY_BYTES = 128 * 1024 * 1024


class Models:
    def __init__(self, workspace, weights):
        self.workspace = Path(workspace).resolve(strict=True)
        self.weights = Path(weights).resolve(strict=True)
        self.loaded = {}

    def path(self, value):
        path = (self.workspace / value).resolve()
        if not path.is_relative_to(self.workspace):
            raise ValueError('Array path is outside the job workspace')
        return path

    def read_array(self, value):
        import numpy as np
        shape = value['shape']
        if (not isinstance(shape, list) or len(shape) not in (2, 3)
                or any(type(n) is not int or n <= 0 for n in shape)
                or (len(shape) == 3 and shape[2] != 3)
                or math.prod(shape) > MAX_ARRAY_BYTES):
            raise ValueError('Invalid uint8 array shape')
        path = self.path(value['path'])
        if path.stat().st_size != math.prod(shape):
            raise ValueError('Array byte count does not match shape')
        return np.fromfile(path, dtype=np.uint8).reshape(shape)

    def write_array(self, relative, array):
        import numpy as np
        array = np.ascontiguousarray(array, dtype=np.uint8)
        path = self.path(relative)
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open('xb') as handle:
            handle.write(array.tobytes())
        return {'path': str(path.relative_to(self.workspace)), 'shape': list(array.shape)}

    def detector(self):
        if 'detector' not in self.loaded:
            import torch
            from comic_text_detector.inference import TextDetector
            torch.set_num_threads(4)
            self.loaded['detector'] = TextDetector(model_path=str(self.weights/'comictextdetector.pt'),
                                                   input_size=1024, device='cpu', act='leaky')
        return self.loaded['detector']

    def ocr(self, request):
        if 'ocr' not in self.loaded:
            from mokuro.manga_page_ocr import MangaPageOcr
            from manga_ocr import MangaOcr
            # Same model preprocessing and defaults as the frozen Mokuro pipeline.
            engine = MangaPageOcr(disable_ocr=True)
            engine.disable_ocr = False
            engine.text_detector = self.detector()
            engine.mocr = MangaOcr(str(self.weights/'manga-ocr'))
            self.loaded['ocr'] = engine
        raw = self.loaded['ocr'](str(self.path(request['image_path'])))
        # Detector coordinates contain NumPy scalar types; normalize the wire
        # representation without changing recognition or grouping semantics.
        return {'version': raw['version'], 'img_width': int(raw['img_width']),
                'img_height': int(raw['img_height']), 'blocks': [
                    {'box': [float(n) for n in block['box']],
                     'vertical': bool(block['vertical']), 'font_size': float(block['font_size']),
                     'lines_coords': block['lines_coords'], 'lines': block['lines']}
                    for block in raw['blocks']]}

    def restore(self, request):
        import cv2
        import numpy as np
        import torch
        torch.set_num_threads(4)
        if 'lama' not in self.loaded:
            self.loaded['lama'] = torch.jit.load(str(self.weights/'anime-manga-big-lama.pt'), map_location='cpu').eval()
        image = self.read_array(request['image'])
        mask = self.read_array(request['mask'])
        if mask.ndim != 2 or image.shape[:2] != mask.shape:
            raise ValueError('Image and mask sizes differ')
        height, width = mask.shape
        rgb = cv2.cvtColor(image, cv2.COLOR_GRAY2RGB if image.ndim == 2 else cv2.COLOR_BGR2RGB)
        factor = min(1.0, 1024 / max(height, width))
        if factor < 1:
            size = (max(1, round(width*factor)), max(1, round(height*factor)))
            rgb = cv2.resize(rgb, size, interpolation=cv2.INTER_AREA)
            mask = cv2.resize(mask, size, interpolation=cv2.INTER_NEAREST)
        h, w = mask.shape
        rgb = cv2.copyMakeBorder(rgb, 0, (-h) % 8, 0, (-w) % 8, cv2.BORDER_REFLECT)
        mask = cv2.copyMakeBorder(mask, 0, (-h) % 8, 0, (-w) % 8, cv2.BORDER_CONSTANT)
        values = torch.from_numpy(rgb.transpose(2, 0, 1).copy()).unsqueeze(0).float()/255
        selection = torch.from_numpy((mask > 0).astype(np.float32)).unsqueeze(0).unsqueeze(0)
        with torch.inference_mode():
            result = self.loaded['lama'](values, selection)[0].permute(1, 2, 0).cpu().numpy()
        result = np.clip(result[:h, :w]*255, 0, 255).astype(np.uint8)
        if factor < 1:
            result = cv2.resize(result, (width, height), interpolation=cv2.INTER_CUBIC)
        result = cv2.cvtColor(result, cv2.COLOR_RGB2GRAY if image.ndim == 2 else cv2.COLOR_RGB2BGR)
        return {'image': self.write_array(request['output'], result)}

    def handle(self, request):
        operation = request['op']
        if operation in ('health', 'shutdown'):
            return {'protocol': PROTOCOL, 'loaded': sorted(self.loaded)}
        if operation == 'release':
            self.loaded.clear()
            gc.collect()
            if 'torch' in sys.modules and sys.modules['torch'].backends.mps.is_available():
                sys.modules['torch'].mps.empty_cache()
            return {'loaded': []}
        if operation == 'detect':
            image = self.read_array(request['image'])
            raw, refined, _ = self.detector()(image, refine_mode=0, keep_undetected_mask=True)
            return {'raw': self.write_array(request['raw_output'], raw),
                    'refined': self.write_array(request['refined_output'], refined)}
        if operation == 'ocr':
            return self.ocr(request)
        if operation == 'restore':
            return self.restore(request)
        raise ValueError('Unknown model operation')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', required=True)
    parser.add_argument('--weights', required=True)
    args = parser.parse_args()
    models = Models(args.workspace, args.weights)
    output = sys.stdout
    for line in sys.stdin:
        identifier = None
        operation = None
        try:
            if len(line) > 1024*1024:
                raise ValueError('Request exceeds protocol limit')
            request = json.loads(line)
            identifier = request['id']
            operation = request['op']
            if request.get('protocol') != PROTOCOL:
                raise ValueError('Unsupported model protocol')
            with contextlib.redirect_stdout(sys.stderr):
                result = models.handle(request)
            reply = {'id': identifier, 'ok': True, 'result': result}
        except Exception as exc:
            # No credentials are accepted by this protocol; diagnostics stay off stdout.
            print(f'{type(exc).__name__}: {exc}', file=sys.stderr, flush=True)
            reply = {'id': identifier, 'ok': False, 'error': type(exc).__name__}
        output.write(json.dumps(reply, ensure_ascii=False)+'\n')
        output.flush()
        if operation == 'shutdown' and reply['ok']:
            break


if __name__ == '__main__':
    main()
