#!/bin/bash
set -euo pipefail

MODE="${1:-run}"
case "$MODE" in
  run|--open|--verify|--logs|--debug) ;;
  *) echo "Usage: $0 [--open|--verify|--logs|--debug]" >&2; exit 2 ;;
esac

PLATFORM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="$PLATFORM_DIR/dist/Koma Rust 候选版.app"
APP_BINARY="$APP_BUNDLE/Contents/MacOS/KomaRustReader"

# Daily launch reuses the built application, without restarting an active reader.
# A moved checkout must be rebuilt because the bundle records its project root.
if [[ "$MODE" == --open ]]; then
  PROJECT_DIR="$(cd "$PLATFORM_DIR/.." && pwd)"
  BUILT_ROOT=$(/usr/libexec/PlistBuddy -c 'Print :ProjectRoot' "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || true)
  if [[ -x "$APP_BINARY" && -x "$PROJECT_DIR/core/target/release/koma-core" && "$BUILT_ROOT" == "$PROJECT_DIR" ]]; then
    /usr/bin/open "$APP_BUNDLE"
    exit 0
  fi
  echo "首次启动或项目位置已变化，正在构建 Koma Rust 候选版…"
  MODE=--verify
fi

# Restart only the copy built from this checkout.
for pid in $(pgrep -x KomaRustReader || true); do
  if [[ "$(ps -p "$pid" -o comm=)" == "$APP_BINARY" ]]; then
    /usr/bin/osascript - "$APP_BUNDLE" <<'APPLESCRIPT'
on run argv
  tell application (item 1 of argv) to quit
end run
APPLESCRIPT
    for _ in {1..40}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$pid" 2>/dev/null; then
      echo "请先退出本项目正在运行的 Koma，再重新构建。" >&2
      exit 1
    fi
  fi
done

zsh "$PLATFORM_DIR/scripts/build.sh"
if [[ "$MODE" == --debug ]]; then
  exec lldb -- "$APP_BINARY"
fi
/usr/bin/open -n "$APP_BUNDLE"

if [[ "$MODE" == --verify ]]; then
  for _ in {1..30}; do
    for pid in $(pgrep -x KomaRustReader || true); do
      if [[ "$(ps -p "$pid" -o comm=)" == "$APP_BINARY" ]]; then
        # Finding a process once can falsely pass an immediate Swift startup
        # crash. Keep checking the same PID through initial scene creation.
        for _ in {1..40}; do
          sleep 0.25
          if ! kill -0 "$pid" 2>/dev/null; then
            echo "候选应用在启动后退出，请检查 DiagnosticReports 中的 KomaRustReader 崩溃报告。" >&2
            exit 1
          fi
        done
        echo "Koma Rust 候选版已启动并持续运行 10 秒（PID ${pid}）"
        exit 0
      fi
    done
    sleep 1
  done
  echo "应用未能启动。" >&2
  exit 1
elif [[ "$MODE" == --logs ]]; then
  exec /usr/bin/log stream --info --style compact --predicate 'process == "KomaRustReader"'
fi
