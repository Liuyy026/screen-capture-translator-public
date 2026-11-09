#!/usr/bin/env python3
"""Private synthetic/real SR protocol and daemon checks. GPU tests serialize across chats."""
import argparse, fcntl, hashlib, json, os, subprocess, sys, time
from pathlib import Path
from PIL import Image, ImageChops, ImageStat
sys.path.insert(0,str(Path(__file__).parent))
from verify_runtime import Client

def main():
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--manifest',type=Path,required=True);a=p.parse_args()
    root=Path(__file__).resolve().parents[2];out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
    helper=root/'macos/.build/koma-sr-metalfx';checks=[];runs=[]
    def call(*args,good=True):
        t=time.perf_counter();r=subprocess.run([str(helper),*map(str,args)],capture_output=True,text=True)
        assert (r.returncode==0)==good,(r.stdout,r.stderr)
        return (json.loads(r.stdout) if good else r.stderr),(time.perf_counter()-t)*1000
    cap,_=call('--capabilities');assert cap['available'] and cap['backend_version'];(out/'capabilities.json').write_text(json.dumps(cap,indent=2))
    lock=root.parent/'gpu-test.lock'
    with lock.open('a') as gpu:
        fcntl.flock(gpu,fcntl.LOCK_EX)
        # Distinct top/bottom colors catch texture flips and channel swaps.
        im=Image.new('RGB',(80,100),'red');im.paste((0,0,255),(0,50,80,100));im.save(out/'orientation.png')
        r,_=call('--input',out/'orientation.png','--output',out/'orientation-up.png','--width',160,'--height',200,'--preset','standard')
        result=Image.open(out/'orientation-up.png').convert('RGB');assert result.getpixel((20,20))[0]>240 and result.getpixel((20,180))[2]>240
        checks.append('orientation_and_rgb_channels')
        for preset in ['denoise','conservative']:
            call('--input',out/'orientation.png','--output',out/'bad.png','--width',160,'--height',200,'--preset',preset,good=False)
        call('--input',out/'orientation.png','--output',out/'orientation.png','--width',160,'--height',200,'--preset','standard',good=False)
        checks.append('unsupported_presets_and_input_overwrite_rejected')
        pages=json.loads(a.manifest.read_text())
        for entry in pages[:2]:
            source=Path(entry['image']);original=Image.open(source).convert('RGB');digest=hashlib.sha256(source.read_bytes()).hexdigest()
            for edge in [540,720,1080]:
                low=original.copy();low.thumbnail((edge,edge),Image.Resampling.LANCZOS)
                stem=f"page{entry['page']:02d}-artificial-{edge}";path=out/(stem+'.png');low.save(path)
                for target in [1440,2160]:
                    w,h=(round(n*target/max(low.size)) for n in low.size);dest=out/f'{stem}-metalfx-{target}.png'
                    reply,wall=call('--input',path,'--output',dest,'--width',w,'--height',h,'--preset','standard')
                    enhanced=Image.open(dest).convert('RGB');reference=original.resize((w,h),Image.Resampling.LANCZOS)
                    bilinear=low.resize((w,h),Image.Resampling.BILINEAR)
                    # This reports pixel error against the source, never a quality-improvement claim.
                    mae=lambda x:sum(ImageStat.Stat(ImageChops.difference(x,reference)).mean)/3
                    runs.append(dict(page=entry['page'],artificial_downsample=True,input=str(path),output=str(dest),reply=reply,wall_ms=wall,mae_reference=mae(enhanced),bilinear_mae_reference=mae(bilinear)))
                    if edge==720 and target==1440:
                        # crops use the same source coordinates for comparison
                        box=(w//4,h//4,3*w//4,h//2)
                        for name,img in [('metalfx',enhanced),('bilinear',bilinear),('reference',reference)]:img.crop(box).save(out/f'{stem}-{name}-crop.png')
            assert hashlib.sha256(source.read_bytes()).hexdigest()==digest
        checks.append('two_real_pages_artificial_540_720_1080_to_1440_2160')
        # Exercise actual daemon effective identity and isolated cache selection.
        client=Client(root,out/'daemon',root/'.models',root/'core/python/model_worker.py')
        try:
            image=str(out/'page01-artificial-720.png');settings=dict(engine='metalfx',target=1440,axis='long_edge',preset='standard')
            off=client.call('load_page',image=image);assert off['effective_image']==image
            on=client.call('load_page',image=image,super_resolution=settings)
            assert on['effective_image']!=image and on['cache']!=off['cache'] and on['page'] is None
            Path(off['cache']).write_text(json.dumps({'blocks':[{'id':'manual','source':'原文','translation':'手改','edited':True}]}))
            again=client.call('load_page',image=image,super_resolution=settings);assert again['cache']==on['cache'] and again['page'] is None
            # Same-size tampering and metadata tampering must regenerate safely.
            effective=Path(on['effective_image']);good_hash=hashlib.sha256(effective.read_bytes()).hexdigest()
            Image.new('RGB',Image.open(effective).size,'green').save(effective)
            repaired=client.call('load_page',image=image,super_resolution=settings)
            assert hashlib.sha256(Path(repaired['effective_image']).read_bytes()).hexdigest()==good_hash
            metadata=effective.parent/'identity.json';metadata.write_text('{}')
            client.call('load_page',image=image,super_resolution=settings)
            assert json.loads(metadata.read_text())['output_sha256']==good_hash
            # Concurrent same-key load calls converge to one complete artifact.
            jobs=[client.send('load_page',image=image,super_resolution=settings) for _ in range(3)]
            assert all(client.wait(j)['result']['cache']==on['cache'] for j in jobs)
            # Enhanced text -> vision seeds only the enhanced ordinary OCR cache.
            enhanced_page={'version':2,'width':1038,'height':1440,'blocks':[{'id':'sr','source':'增强原文','translation':'旧自动译文','edited':False}]}
            Path(on['cache']).parent.mkdir(parents=True,exist_ok=True);Path(on['cache']).write_text(json.dumps(enhanced_page))
            vision=client.call('load_page',image=image,mode='vision',super_resolution=settings)
            assert vision['page']['blocks'][0]['source']=='增强原文' and vision['page']['blocks'][0]['translation']=='' 
            assert client.call('load_page',image=image)['page']['blocks'][0]['translation']=='手改'
            settings['target']=2160;larger=client.call('load_page',image=image,super_resolution=settings);assert larger['cache']!=on['cache']
            settings['engine']='realcugan';missing=client.wait(client.send('load_page',image=image,super_resolution=settings));assert missing['event']=='error'
            checks.extend(['daemon_effective_image_cache_identity_setting_switch_original_edits_preserved_missing_backend','same_size_output_and_metadata_corruption_rebuilt','concurrent_requests_converge','enhanced_text_to_vision_reuses_only_enhanced_ocr'])
            source=Path(pages[1]['image'])
            if max(Image.open(source).size)>=1440:
                settings=dict(engine='metalfx',target=1440,axis='long_edge',preset='standard')
                skipped=client.call('load_page',image=str(source),super_resolution=settings)
                assert skipped['effective_image']==str(source)
                checks.append('real_source_already_at_target_skips_unchanged')
        finally:client.close()
    report=dict(passed=True,checks=checks,runs=runs,notes='Artificial downsampling; helper elapsed includes decode/init/GPU/encode. Wall includes process startup. No claim of recovery of genuinely lost detail.')
    (out/'report.json').write_text(json.dumps(report,indent=2,ensure_ascii=False));print(json.dumps(dict(passed=True,checks=checks),ensure_ascii=False))
if __name__=='__main__':main()
