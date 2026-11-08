"""Setup-only asset provenance. Does not access shared model directories."""
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
package = root / 'realcugan-ncnn-vulkan-20220728-macos'
files = {str(p.relative_to(package)): hashlib.sha256(p.read_bytes()).hexdigest()
         for p in sorted(package.rglob('*')) if p.is_file()}
manifest = dict(engine='realcugan-ncnn-vulkan', release='20220728', source=sys.argv[2],
    archive_sha256=hashlib.sha256((root/'release.zip').read_bytes()).hexdigest(), files=files,
    license_source='https://github.com/nihui/realcugan-ncnn-vulkan/blob/master/LICENSE',
    model_source='https://github.com/bilibili/ailab/tree/main/Real-CUGAN',
    license_note='Retain bundled LICENSE files and upstream model attribution; see handoff for redistribution limits.')
(root/'manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2)+'\n')
