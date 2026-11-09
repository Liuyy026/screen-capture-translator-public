#!/usr/bin/env python3
"""Exercise INT, TERM, and hard process-group cancellation of owned processes."""
import fcntl,json,os,pathlib,shutil,signal,subprocess,time
from PIL import Image
root=pathlib.Path(__file__).resolve().parents[2]; review=root/'.runtime/super-resolution/realcugan/review'
helper=root/'macos/.build/koma-sr-realcugan'; records=[]
with open(root.parent/'gpu-test.lock','a') as lock:
    fcntl.flock(lock,fcntl.LOCK_EX)
    for name,sig,group in [('direct-int',signal.SIGINT,False),('group-term',signal.SIGTERM,True),('group-kill',signal.SIGKILL,True)]:
        job=review/('cancel-job-'+name); job.mkdir(exist_ok=True)
        inp=review/'人工降采样 page5 1080.png'; im=Image.open(inp); output=job/'output.png'
        with open(review/(name+'.stderr'),'w') as log:
            p=subprocess.Popen([str(helper),'--input',str(inp),'--output',str(output),'--width',str(im.width*4),'--height',str(im.height*4),'--preset','standard'],stdout=subprocess.PIPE,stderr=log,start_new_session=True)
            child=None
            try:
                deadline=time.monotonic()+10
                while time.monotonic()<deadline:
                    ids=subprocess.run(['pgrep','-P',str(p.pid)],capture_output=True,text=True).stdout.split()
                    if ids: child=int(ids[0]); break
                    if p.poll() is not None: break
                    time.sleep(.02)
                assert child,'Engine child not observed'
                pgid=os.getpgid(p.pid); childgroup=os.getpgid(child)
                start=time.monotonic()
                if group: os.killpg(pgid,sig)
                else: os.kill(p.pid,sig)
                stdout=p.communicate(timeout=10)[0]
                state=subprocess.run(['ps','-o','stat=','-p',str(child)],capture_output=True,text=True).stdout.strip()
                assert not state or state.startswith('Z'),state
                assert not output.exists() and not stdout
                remaining=[x.name for x in job.iterdir()]
                if sig!=signal.SIGKILL: assert not remaining,remaining
                records.append(dict(name=name,helper_pgid=pgid,child_pgid=childgroup,returncode=p.returncode,elapsed_ms=1000*(time.monotonic()-start),child_state=state or 'reaped',leftover_before_parent_cleanup=remaining))
                assert pgid==childgroup
            finally:
                if p.poll() is None: os.killpg(p.pid,signal.SIGKILL); p.communicate()
                shutil.rmtree(job)
(review/'cancel-results.json').write_text(json.dumps(records,indent=2))
print(json.dumps(records))
