import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from model_worker import Models


class ModelWorkerTests(unittest.TestCase):
    def test_lama_rgb_bands_bound_workspace_and_preserve_full_width_convolution(self):
        import torch
        from lama_inference import BoundedRGBConvolution
        from torch.utils._python_dispatch import TorchDispatchMode

        shapes = []

        class ObserveConvolution(TorchDispatchMode):
            def __torch_dispatch__(self, func, types, args=(), kwargs=None):
                if func in (torch.ops.aten.convolution.default, torch.ops.aten._convolution.default):
                    shapes.append(tuple(args[0].shape[-2:]))
                return func(*args, **(kwargs or {}))

        generator = torch.Generator().manual_seed(6)
        image = torch.randn((1, 64, 78, 94), generator=generator)
        kernel = torch.randn((3, 64, 7, 7), generator=generator)
        bias = torch.randn((3,), generator=generator)
        args = (image, kernel, bias, [1, 1], [0, 0], [1, 1], False, [0, 0], 1)
        for convolution, flags in [(torch.ops.aten.convolution.default, ()),
                                   (torch.ops.aten._convolution.default, (False, False, True, True))]:
            with self.subTest(operator=str(convolution)), torch.inference_mode():
                shapes.clear()
                expected = convolution(*args, *flags)
                with ObserveConvolution(), BoundedRGBConvolution():
                    actual = convolution(*args, *flags)
                self.assertEqual(shapes, [(38, 94), (38, 94), (14, 94)])
                self.assertTrue(torch.equal(actual.view(torch.int32), expected.view(torch.int32)))

    def test_crop_calls_recognizer_directly_and_releases_it(self):
        import numpy as np
        with tempfile.TemporaryDirectory() as directory:
            models = Models(directory, directory)
            seen = []
            def recognize(image):
                seen.append((image.size, image.getpixel((0, 0))))
                return 'ケーキも'
            models.loaded['recognizer'] = recognize
            crop = np.full((10, 4, 3), [22, 66, 77], dtype=np.uint8)
            encoded = models.write_array('job/crop.bin', crop)
            self.assertEqual(models.handle({'op': 'ocr_crop', 'image': encoded}), {'source': 'ケーキも'})
            self.assertEqual(seen, [((4, 10), (77, 66, 22))])
            self.assertEqual(set(models.loaded), {'recognizer'})
            self.assertEqual(models.handle({'op': 'release_ocr'}), {'loaded': []})

    def test_protocol_recovers_from_bad_request_and_shuts_down(self):
        with tempfile.TemporaryDirectory() as directory:
            requests = '\n'.join([
                '{bad json',
                json.dumps({'protocol': 99, 'id': 1, 'op': 'health'}),
                json.dumps({'protocol': 1, 'id': 2, 'op': 'health'}),
                json.dumps({'protocol': 1, 'id': 3, 'op': 'shutdown'}),
            ])+'\n'
            process = subprocess.run([sys.executable, str(Path(__file__).with_name('model_worker.py')),
                                      '--workspace', directory, '--weights', directory],
                                     input=requests, text=True, capture_output=True, timeout=10, check=True)
            replies = [json.loads(line) for line in process.stdout.splitlines()]
            self.assertEqual([r['ok'] for r in replies], [False, False, True, True])
            self.assertEqual(replies[2]['result'], {'protocol': 1, 'loaded': []})
            self.assertEqual(replies[3]['id'], 3)

    def test_array_roundtrip_and_path_boundary(self):
        import numpy as np
        with tempfile.TemporaryDirectory() as directory:
            models = Models(directory, directory)
            expected = np.arange(36, dtype=np.uint8).reshape(3, 4, 3)
            encoded = models.write_array('job/image.bin', expected)
            np.testing.assert_array_equal(models.read_array(encoded), expected)
            with self.assertRaises(FileExistsError):
                models.write_array('job/image.bin', expected)
            with self.assertRaises(ValueError):
                models.path('../escape.bin')
            with self.assertRaises(ValueError):
                models.read_array({'path': encoded['path'], 'shape': [1, 2, 3]})
            with self.assertRaises(ValueError):
                models.read_array({'path': encoded['path'], 'shape': [3, 4, 4]})


if __name__ == '__main__':
    unittest.main()
