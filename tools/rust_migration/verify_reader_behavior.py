#!/usr/bin/env python3
"""Run the existing reader behavior assertions against the native display CLI.

Only test transport uses Python; the old reading_regions function is replaced
by Rust calls, including the old CLI test. No model weights are loaded.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    baseline = args.baseline_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True,exist_ok=False)
    sys.path.insert(0,str(baseline))
    import cv2
    import numpy as np
    binary = root/'core/target/release/koma-core'
    spec = importlib.util.spec_from_file_location('native_reader_assertions',baseline/'macos/tests/test_reading_overlay.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    calls = 0
    def run(page,gray,**kwargs):
        nonlocal calls
        calls += 1
        prefix = output/f'{calls:03}'
        image,cache = prefix.with_suffix('.png'),prefix.with_suffix('.json')
        assert cv2.imwrite(str(image),kwargs.get('color',gray))
        cache.write_text(json.dumps(page,ensure_ascii=False))
        command = [str(binary),'display','--image',str(image),'--cache',str(cache)]
        if kwargs.get('source_mask') is not None:
            mask = prefix.with_suffix('.mask.png')
            assert cv2.imwrite(str(mask),kwargs['source_mask'])
            command += ['--text-mask',str(mask)]
        assert kwargs.get('restorer') is None, 'Model callbacks are verified in the model boundary tests'
        result = subprocess.run(command,capture_output=True)
        prefix.with_suffix('.stderr.log').write_bytes(result.stderr)
        if result.returncode:
            raise ValueError(result.stderr.decode(errors='replace'))
        prefix.with_suffix('.reply.json').write_bytes(result.stdout)
        return json.loads(result.stdout)
    module.reading.reading_regions = lambda page,gray,**kw: run(page,gray,**kw)['regions']
    module.reading.preserved_markers = lambda page: run(page,np.full((page['height'],page['width']),255,np.uint8))['preserved']
    adapter = output/'native_cli_adapter.py'
    adapter.write_text('import os,sys\ncore = '+repr(str(binary))+'\nos.execv(core,[core,"display",*sys.argv[1:]])\n')
    module.SCRIPT = adapter
    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    report = {'scope':'existing reading-overlay behavior assertions executed through native display CLI',
              'tests':result.testsRun,'native_calls':calls,'failures':len(result.failures),'errors':len(result.errors),
              'skipped':len(result.skipped),'passed':result.wasSuccessful()}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps(report))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
