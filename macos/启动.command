#!/bin/zsh
set -euo pipefail
cd "${0:A:h}"
if [[ ! -x 'dist/Koma 漫画翻译.app/Contents/MacOS/KomaReader' ]]; then
  zsh scripts/build.sh
fi
open 'dist/Koma 漫画翻译.app'
