import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from model_worker import Models


class ModelWorkerTests(unittest.TestCase):
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
