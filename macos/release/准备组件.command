#!/bin/zsh
set -euo pipefail
umask 077

if [[ "$(/usr/sbin/sysctl -n hw.optional.arm64 2>/dev/null || true)" != 1 ]]; then
  echo '此下载包适用于搭载 Apple 芯片的 Mac。' >&2
  exit 1
fi

mode="${1:-all}"
case "$mode" in
  all|ocr|realcugan) ;;
  *) echo "用法 $0 [all|ocr|realcugan]" >&2; exit 2 ;;
esac

bundle="${0:A:h}/Koma.app"
resource="$bundle/Contents/Resources/KomaRuntime"
if [[ ! -f "$resource/macos/requirements.lock" ]]; then
  echo '请将准备命令与解压后的 Koma.app 放在同一目录。' >&2
  exit 1
fi
namespace="$(/usr/libexec/PlistBuddy -c 'Print :CacheNamespace' "$bundle/Contents/Info.plist")"
[[ -n "$namespace" && "$namespace" != *[^a-zA-Z0-9_-]* ]] || exit 1

runtime="$HOME/Library/Application Support/$namespace/runtime"
mkdir -p "$runtime/bin" "$runtime/macos"
lock="$runtime/.setup-lock"
if ! mkdir "$lock" 2>/dev/null; then
  echo "组件准备正在运行。若上次异常中断，确认进程已退出后可移除空目录 $lock" >&2
  exit 1
fi
temp=""
cleanup() {
  if [[ -n "$temp" ]]; then
    rm -f "$temp/uv.tar.gz" "$temp/uv"
    rmdir "$temp"
  fi
  rmdir "$lock"
}
trap cleanup EXIT
temp="$(mktemp -d "$runtime/.setup.XXXXXXXX")"

uv_version=0.12.17
uv="$runtime/bin/uv-$uv_version"
if [[ ! -x "$uv" ]]; then
  echo '正在下载本地环境管理工具…'
  curl -fsSL --retry 3 --connect-timeout 15 \
    "https://github.com/astral-sh/uv/releases/download/$uv_version/uv-aarch64-apple-darwin.tar.gz" \
    -o "$temp/uv.tar.gz"
  digest="$(shasum -a 256 "$temp/uv.tar.gz" | cut -d ' ' -f 1)"
  [[ "$digest" == 85f00cbdc6dd3e97eba4c31b4d014375a9fdfe8f570023b84e5102fc3456896b ]] \
    || { echo '环境工具下载校验失败。' >&2; exit 1; }
  tar -xOf "$temp/uv.tar.gz" uv-aarch64-apple-darwin/uv > "$temp/uv"
  install -m 755 "$temp/uv" "$uv"
fi

export UV_PYTHON_INSTALL_DIR="$runtime/python"
export UV_CACHE_DIR="$runtime/uv-cache"
export KOMA_MODEL_DIR="$runtime/.models"
export HF_HOME="$KOMA_MODEL_DIR/huggingface"
export PYTHONDONTWRITEBYTECODE=1

venv="$runtime/macos/.venv"
python="$venv/bin/python"
if [[ ! -x "$python" ]]; then
  echo '正在准备应用专用 Python 3.12…'
  "$uv" --no-config venv --managed-python --python 3.12 --allow-existing "$venv"
fi
"$python" -c 'import platform, sys; assert sys.version_info[:2] == (3, 12) and platform.machine() == "arm64"'

if [[ "$mode" != realcugan ]]; then
  echo '正在安装漫画识别和背景修补组件…'
  "$uv" --no-config pip install --python "$python" -r "$resource/macos/requirements.lock"
  "$python" "$resource/scripts/prepare_models.py"
  "$python" "$resource/macos/scripts/setup_reading_model.py"
fi
if [[ "$mode" != ocr ]]; then
  echo '正在准备 Real-CUGAN…'
  KOMA_COMPONENT_ROOT="$runtime" KOMA_SETUP_PYTHON="$python" \
    /bin/bash "$resource/macos/scripts/prepare_realcugan.sh"
fi
echo '组件准备完成。现在可以打开 Koma.app。'
