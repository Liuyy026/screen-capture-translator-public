#!/usr/bin/env python3
"""Local private offline review; Pillow used for QA, not helper runtime."""
import argparse, fcntl, hashlib, json, os, pathlib, signal, subprocess, time
from PIL import Image, ImageOps, ImageDraw
parser=argparse.ArgumentParser(); parser.add_argument('--manifest',required=True); parser.add_argument('--output',required=True); parser.add_argument('--lock',required=True); a=parser.parse_args()
root=pathlib.Path(__file__).resolve().parents[2]; helper=root/'macos/.build/koma-sr-realcugan'
out=pathlib.Path(a.output).resolve(); out.mkdir(parents=True,exist_ok=True)
def digest(p): return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def pixels(p):
    im=ImageOps.exif_transpose(Image.open(p)).convert('RGBA'); return hashlib.sha256(im.tobytes()).hexdigest()
pages=json.load(open(a.manifest)); records=[]; original={}
# Only explicitly listed pages; no directory scan or network upload.
for index in [1,4]:
    p=pathlib.Path(pages[index]['image']); im=ImageOps.exif_transpose(Image.open(p)).convert('RGB')
    original[str(p)]={'sha256':digest(p),'pixel_sha256':pixels(p),'size':im.size}
    for low,target in [(540,1440),(720,2160),(1080,2160)]:
        size=(round(im.width*low/max(im.size)),round(im.height*low/max(im.size)))
        inp=out/f'人工降采样 page{index+1} {low}.png'; im.resize(size,Image.Resampling.LANCZOS).save(inp)
        targetsize=(round(im.width*target/max(im.size)),round(im.height*target/max(im.size)))
        reference=im.resize(targetsize,Image.Resampling.LANCZOS); reference.save(out/f'page{index+1}-reference-{target}.png')
        Image.open(inp).resize(targetsize,Image.Resampling.LANCZOS).save(out/f'page{index+1}-{low}-lanczos-{target}.png')
        for preset in ['conservative','standard','denoise']:
            dest=out/f'page{index+1}-{low}-{preset}-{target}.png'
            records.append(dict(page=index+1,input=str(inp),output=str(dest),preset=preset,target=targetsize,synthetic_downsample=True))
with open(a.lock,'a') as lock:
    fcntl.flock(lock,fcntl.LOCK_EX)
    for rec in records:
        dest=pathlib.Path(rec['output']); dest.unlink(missing_ok=True)
        cmd=[str(helper),'--input',rec['input'],'--output',str(dest),'--width',str(rec['target'][0]),'--height',str(rec['target'][1]),'--preset',rec['preset']]
        start=time.monotonic()
        with open(str(dest)+'.stderr','w') as log: p=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=log,text=True,timeout=300)
        rec.update(returncode=p.returncode,wall_ms=1000*(time.monotonic()-start),stdout=p.stdout)
        assert p.returncode==0,(rec,p.stdout)
        rec['result']=json.loads(p.stdout); assert Image.open(dest).size==tuple(rec['target'])
        rec['sha256']=digest(dest)
        # Pixel error describes this synthetic reconstruction only, not readability.
        ref=Image.open(out/f"page{rec['page']}-reference-{max(rec['target'])}.png").convert('RGB')
        import numpy as np
        error=np.asarray(Image.open(dest).convert('RGB'),dtype=float)-np.asarray(ref,dtype=float)
        rec['synthetic_psnr_db']=float(10*np.log10(255**2/np.mean(error**2)))
        print(json.dumps({k:rec[k] for k in ['page','preset','wall_ms','synthetic_psnr_db']}),flush=True)
        (out/'results.json').write_text(json.dumps(dict(originals=original,runs=records),ensure_ascii=False,indent=2))
    # TERM must return promptly, reap owned child, and leave no destination.
    inp=records[0]['input']; cancelout=out/'cancelled.png'; cancelout.unlink(missing_ok=True)
    with open(out/'cancel.stderr','w') as log:
        p=subprocess.Popen([str(helper),'--input',inp,'--output',str(cancelout),'--width',str(Image.open(inp).width*4),'--height',str(Image.open(inp).height*4),'--preset','standard'],stdout=subprocess.PIPE,stderr=log)
        child=[]
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            ids=subprocess.run(['pgrep','-P',str(p.pid)],capture_output=True,text=True).stdout.split()
            if ids: child=list(map(int,ids)); break
            if p.poll() is not None: break
            time.sleep(.03)
        start=time.monotonic(); p.terminate(); stdout=p.communicate(timeout=10)[0]
        alive=[]
        for pid in child:
            try: os.kill(pid,0); alive.append(pid)
            except ProcessLookupError: pass
        cancellation=dict(returncode=p.returncode,child_pids=child,remaining_children=alive,elapsed_ms=1000*(time.monotonic()-start),output_exists=cancelout.exists(),stdout=stdout.decode())
        assert child and not alive and not cancelout.exists(),cancellation
    # Invalid preset, overwrite, same input, invalid dimensions must fail without mutation.
    negative=[]
    for dest,preset,w in [(inp,'standard','1440'),(str(out/'bad.png'),'bogus','1440'),(str(out/'bad.png'),'standard','0'),(records[0]['output'],'standard','1440')]:
        q=subprocess.run([str(helper),'--input',inp,'--output',dest,'--width',w,'--height','2160','--preset',preset],capture_output=True,text=True)
        assert q.returncode!=0; negative.append(dict(returncode=q.returncode,stderr=q.stderr))
for p,ident in original.items(): assert digest(p)==ident['sha256'] and pixels(p)==ident['pixel_sha256']
summary=dict(originals=original,runs=records,cancellation=cancellation,negative_tests=negative,originals_unchanged=True)
(out/'results.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2))
# Comparable central crops: reference / Lanczos / three real inference presets.
for page in [2,5]:
    for low,target in [(540,1440),(720,2160),(1080,2160)]:
        paths=[out/f'page{page}-reference-{target}.png',out/f'page{page}-{low}-lanczos-{target}.png']+[out/f'page{page}-{low}-{p}-{target}.png' for p in ['conservative','standard','denoise']]
        canvas=Image.new('RGB',(5*360,600),'#dddddd'); draw=ImageDraw.Draw(canvas)
        for n,(path,label) in enumerate(zip(paths,['reference','Lanczos','conservative','standard','denoise'])):
            im=Image.open(path).convert('RGB'); x=int(im.width*.55); y=int(im.height*.25)
            canvas.paste(im.crop((x,y,x+350,y+560)),(n*360,30)); draw.text((n*360+4,5),label,fill='black')
        canvas.save(out/f'compare-page{page}-{low}-{target}.jpg',quality=95)
print('PASS: inference, dimensions, Unicode paths, cancellation, negatives, source invariance',flush=True)
