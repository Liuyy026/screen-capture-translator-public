"""Reproducible project-local installer; no Homebrew/system modifications."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import urllib.request
import time

root = Path(__file__).resolve().parents[1]
os.chdir(root)
env = dict(os.environ)
for kind, proxy in urllib.request.getproxies().items():
    if kind in ('http', 'https'):
        env.setdefault(kind.upper() + '_PROXY', proxy)
env['NO_PROXY'] = '127.0.0.1,localhost'
env['OLLAMA_HOST'] = '127.0.0.1:11439'
env['OLLAMA_MODELS'] = str(root / '.models/ollama')
env['OLLAMA_NO_CLOUD'] = '1'
env['OLLAMA_NUM_PARALLEL'] = '1'
os.environ.update(env)

def run(args):
    subprocess.run([str(x) for x in args], env=env, check=True)

uv = shutil.which('uv') or str(Path.home() / '.local/bin/uv')
if not Path(uv).is_file():
    raise SystemExit('需要 uv。请先从 https://docs.astral.sh/uv/ 安装 uv，然后重试。')
print('1/5 安装项目专用 Python 依赖…', flush=True)
if not (root / '.venv/bin/python').exists():
    run([uv, 'venv', '--python', '3.12', '.venv'])
requirements = 'requirements.lock' if (root / 'requirements.lock').exists() else 'requirements.txt'
run([uv, 'pip', 'install', '--python', '.venv/bin/python', '-r', requirements])
print('2/5 下载漫画 OCR 模型…', flush=True)
run(['.venv/bin/python', 'scripts/prepare_models.py'])
print('3/5 准备本地翻译引擎…', flush=True)
runtime = root / '.runtime/ollama'
runtime.mkdir(parents=True, exist_ok=True)
binary = runtime / 'ollama'
if not binary.exists():
    archive = root / '.runtime/ollama-darwin.tgz'
    urllib.request.urlretrieve('https://github.com/ollama/ollama/releases/download/v0.34.0/ollama-darwin.tgz', archive)
    with tarfile.open(archive) as f:
        f.extractall(runtime, filter='data')

private = urllib.request.build_opener(urllib.request.ProxyHandler({}))
def running():
    try:
        with private.open('http://127.0.0.1:11439/api/tags', timeout=2): return True
    except Exception: return False

server = None
try:
    if not running():
        server = subprocess.Popen([str(binary), 'serve'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(30):
            if running(): break
            time.sleep(1)
    print('4/5 下载 Qwen3 4B（约 2.5GB，可续传）…', flush=True)
    run([binary, 'pull', 'qwen3:4b'])
    print('5/5 构建 Mac 应用…', flush=True)
    run(['zsh', 'scripts/build.sh'])
finally:
    if server:
        server.terminate()
        server.wait(timeout=10)
print('安装完成。双击「启动.command」即可打开。')
