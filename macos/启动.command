#!/bin/zsh
set -euo pipefail
cd "${0:A:h}"
if ! /bin/bash scripts/build_and_run.sh --open; then
  print -u2 'Rust 候选版未能打开。首次使用请先按 macos/README.md 准备依赖和模型；上方保留具体错误。'
  if [[ -t 0 ]]; then
    read -r '?按回车关闭此窗口…'
  fi
  exit 1
fi
