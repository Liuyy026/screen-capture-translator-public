#!/usr/bin/env python3
"""Compare native visual input JPEG bytes to the frozen Pillow encoder."""
import argparse
import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-root',type=Path,required=True)
    parser.add_argument('--manifest',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    baseline=args.baseline_root.resolve();sys.path.insert(0,str(baseline))
    spec=importlib.util.spec_from_file_location('baseline_vision',baseline/'macos/backend/vision_worker.py')
    old=importlib.util.module_from_spec(spec);spec.loader.exec_module(old)
    import numpy as np
    from PIL import Image
    images=[]
    manifest=json.loads(args.manifest.read_text())
    for entry in manifest:
        path=Path(entry['image']);assert hashlib.sha256(path.read_bytes()).hexdigest()==entry['image_hash']
        images.append(path)
    fixture=output/'fixtures';fixture.mkdir()
    def pattern(w,h):
        y,x=np.ogrid[:h,:w]
        array=np.empty((h,w,3),np.uint8)
        array[:,:,0]=(x*3+y*5)%256
        array[:,:,1]=(x//7+y//3)%256
        array[:,:,2]=np.where((x//13+y//11)%2,10,245)
        return Image.fromarray(array)
    for w,h in [(2400,3000),(1776,1777),(3551,7103),(7101,6001),(103,1801),(13,6001),(6001,13),(1,1776)]:
        path=fixture/f'rgb-{w}-{h}.png';pattern(w,h).save(path);images.append(path)
    picture=pattern(213,157)
    for format in ['PNG','JPEG','BMP','TIFF','WEBP']:
        path=fixture/f'color.{format.lower()}';picture.save(path);images.append(path)
    for orientation in range(1,9):
        exif=Image.Exif();exif[274]=orientation
        for fmt in ['JPEG','PNG','TIFF','WEBP']:
            path=fixture/f'orientation-{orientation}.{fmt.lower()}'
            picture.save(path,exif=exif);images.append(path)
    for mode in ['1','L','P','RGBA','CMYK']:
        converted=picture.convert(mode)
        fmt='JPEG' if mode=='CMYK' else 'PNG'
        if mode=='RGBA':
            converted.putalpha(Image.fromarray(np.tile(np.arange(213,dtype=np.uint8),(157,1))))
        path=fixture/f'mode-{mode}.{fmt.lower()}';converted.save(path);images.append(path)
    gray16=Image.fromarray(np.arange(213*157,dtype=np.uint16).reshape(157,213))
    for fmt in ['PNG','TIFF']:
        path=fixture/f'gray16.{fmt.lower()}';gray16.save(path);images.append(path)
    import cv2
    color16=np.arange(213*157*3,dtype=np.uint16).reshape(157,213,3)
    path=fixture/'color16.png';assert cv2.imwrite(str(path),color16);images.append(path)
    rng=np.random.default_rng(7345)
    cmyk=Image.frombytes('CMYK',(213,157),rng.integers(0,256,size=(157,213,4),dtype=np.uint8).tobytes())
    path=fixture/'cmyk-ink.jpeg';cmyk.save(path,quality=92);images.append(path)
    for dtype in [np.int32,np.float32]:
        values=np.linspace(-100,500,213*157,dtype=dtype).reshape(157,213)
        path=fixture/f'gray-{dtype.__name__}.tiff';Image.fromarray(values).save(path);images.append(path)
    records=[]
    for index,path in enumerate(images):
        url,size=old.encode_image(path)
        result=subprocess.run([str(root/'core/target/release/koma-core'),'vision-image','--image',str(path)],capture_output=True,timeout=90)
        if result.returncode:
            records.append({'image':str(path),'size':size,'native_size':None,'same_jpeg':False,'error':result.stderr.decode()})
            continue
        native=json.loads(result.stdout)
        expected=base64.b64decode(url.split(',',1)[1]);actual=base64.b64decode(native['url'].split(',',1)[1])
        identical=expected==actual
        error=None
        if not identical:
            (output/f'{index:03}-expected.jpg').write_bytes(expected)
            (output/f'{index:03}-actual.jpg').write_bytes(actual)
            a=np.asarray(Image.open(io.BytesIO(expected))).astype(int)
            b=np.asarray(Image.open(io.BytesIO(actual))).astype(int)
            error={'expected_shape':a.shape,'actual_shape':b.shape}
            if a.shape==b.shape:error.update(max_delta=int(np.abs(a-b).max()),different_pixels=int(np.any(a!=b,axis=2).sum()))
        records.append({'image':str(path),'size':size,'native_size':[native['width'],native['height']],
                        'same_jpeg':identical,'expected_sha256':hashlib.sha256(expected).hexdigest(),
                        'actual_sha256':hashlib.sha256(actual).hexdigest(),'error':error})
    passed=all(r['same_jpeg'] and list(r['size'])==r['native_size'] for r in records)
    report={'pages':len(manifest),'fixtures':len(images)-len(manifest),'passed':passed,'records':records}
    (output/'report.json').write_text(json.dumps(report,indent=2))
    print(json.dumps({'pages':len(manifest),'fixtures':len(images)-len(manifest),'passed':passed,
                      'differences':[{k:v for k,v in r.items() if k in ['image','error']} for r in records if not r['same_jpeg']]}))
    if not passed:raise SystemExit(1)


if __name__=='__main__':main()
