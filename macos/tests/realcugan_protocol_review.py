#!/usr/bin/env python3
"""Additional real GPU protocol checks including native input and group cancellation."""
import fcntl,json,os,pathlib,signal,subprocess,time
from PIL import Image,ImageDraw
root=pathlib.Path(__file__).resolve().parents[2]; out=root/'.runtime/super-resolution/realcugan/review'; helper=root/'macos/.build/koma-sr-realcugan'
manifest=root/'.runtime/rust-migration/pages.json'
records=[]
def infer(inp,dest,w,h,preset):
    dest.unlink(missing_ok=True); start=time.monotonic()
    with open(str(dest)+'.stderr','w') as log: p=subprocess.run([str(helper),'--input',str(inp),'--output',str(dest),'--width',str(w),'--height',str(h),'--preset',preset],capture_output=False,stdout=subprocess.PIPE,stderr=log,text=True,timeout=120)
    assert p.returncode==0,p.stdout
    assert Image.open(dest).size==(w,h)
    records.append(dict(input=str(inp),output=str(dest),result=json.loads(p.stdout),wall_ms=1000*(time.monotonic()-start)))
with open(root.parent/'gpu-test.lock','a') as lock:
    fcntl.flock(lock,fcntl.LOCK_EX)
    native=pathlib.Path(json.load(open(manifest))[1]['image'])
    infer(native,out/'native-page2-standard-2160.png',1558,2160,'standard')
    inp=out/'人工降采样 page2 540.png'; im=Image.open(inp)
    infer(inp,out/'page2-x4-denoise.png',im.width*4,im.height*4,'denoise')
    # A marked EXIF JPEG has a physically horizontal raster, oriented portrait.
    im=Image.new('RGB',(160,96),'white'); d=ImageDraw.Draw(im); d.rectangle((0,0,60,40),fill='red'); d.rectangle((100,50,159,95),fill='blue')
    exif=Image.Exif(); exif[274]=6; jpg=out/'方向 test.jpg'; im.save(jpg,exif=exif)
    infer(jpg,out/'oriented.png',192,320,'conservative')
    oriented=Image.open(out/'oriented.png').convert('RGB')
    assert oriented.getpixel((170,20))[0]>200 and oriented.getpixel((20,300))[2]>200
    # A caller-created process group must include the inference child.
    inp=out/'人工降采样 page5 1080.png'; im=Image.open(inp); dest=out/'group-cancelled.png'; dest.unlink(missing_ok=True)
    with open(out/'group-cancel.stderr','w') as log:
        p=subprocess.Popen([str(helper),'--input',str(inp),'--output',str(dest),'--width',str(im.width*4),'--height',str(im.height*4),'--preset','denoise'],stdout=subprocess.PIPE,stderr=log,start_new_session=True)
        ids=[]; deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            ids=subprocess.run(['pgrep','-P',str(p.pid)],capture_output=True,text=True).stdout.split()
            if ids: break
            time.sleep(.02)
        assert ids
        child=int(ids[0]); pgids={'helper':os.getpgid(p.pid),'child':os.getpgid(child)}
        assert pgids['helper']==pgids['child'],pgids
        # TERM whole group; helper must reap its owned engine child.
        start=time.monotonic(); os.killpg(p.pid,signal.SIGTERM); p.communicate(timeout=10)
        try: os.kill(child,0); alive=True
        except ProcessLookupError: alive=False
        cancellation=dict(pgids=pgids,returncode=p.returncode,remaining_child=alive,elapsed_ms=1000*(time.monotonic()-start),output_exists=dest.exists())
        assert not alive and not dest.exists(),cancellation
(out/'protocol-results.json').write_text(json.dumps(dict(runs=records,group_cancellation=cancellation,orientation=True),ensure_ascii=False,indent=2))
print(json.dumps(cancellation))
