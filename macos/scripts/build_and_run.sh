#!/bin/bash
set -euo pipefail

MODE="${1:-run}"
case "$MODE" in
  run|--open|--verify|--logs|--debug) ;;
  *) echo "Usage: $0 [--open|--verify|--logs|--debug]" >&2; exit 2 ;;
esac

PLATFORM_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
APP_BUNDLE="$PLATFORM_DIR/dist/Koma.app"
LEGACY_APP_BUNDLE="$PLATFORM_DIR/dist/Koma Rust 候选版.app"
APP_BINARY="$APP_BUNDLE/Contents/MacOS/KomaRustReader"

running_app_pids() {
  local pid
  local executable="$1/Contents/MacOS/KomaRustReader"
  for pid in $(pgrep -x KomaRustReader || true); do
    if [[ "$(ps -p "$pid" -o comm=)" == "$executable" ]]; then
      printf '%s\n' "$pid"
    fi
  done
}

request_app_action() {
  /usr/bin/osascript -l JavaScript - "$1" "$2" <<'JAVASCRIPT'
ObjC.import('AppKit');
function run(argv) {
  const app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(Number(argv[0]));
  if (app.isNil()) return;
  if (argv[1] === 'activate') {
    app.activateWithOptions(0);
  } else {
    app.terminate;
  }
}
JAVASCRIPT
}

# Daily launch reuses the built application, without restarting an active reader.
# A moved checkout must be rebuilt because the bundle records its project root.
if [[ "$MODE" == --open ]]; then
  # A pre-rename session may outlive its original bundle path.
  for bundle in "$APP_BUNDLE" "$LEGACY_APP_BUNDLE"; do
    for pid in $(running_app_pids "$bundle"); do
      request_app_action "$pid" activate >/dev/null
      if kill -0 "$pid" 2>/dev/null; then
        exit 0
      fi
    done
  done
  PROJECT_DIR="$(cd "$PLATFORM_DIR/.." && pwd)"
  BUILT_ROOT=$(/usr/libexec/PlistBuddy -c 'Print :ProjectRoot' "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || true)
  if [[ -x "$APP_BINARY" && -x "$PROJECT_DIR/core/target/release/koma-core" && "$BUILT_ROOT" == "$PROJECT_DIR" ]]; then
    exec /usr/bin/open "$APP_BUNDLE"
  fi
  echo "正在为当前项目构建 Koma…"
  MODE=--verify
fi

# Restart only the copy built from this checkout.
for bundle in "$APP_BUNDLE" "$LEGACY_APP_BUNDLE"; do
  for pid in $(running_app_pids "$bundle"); do
    request_app_action "$pid" quit >/dev/null
    for _ in {1..40}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$pid" 2>/dev/null; then
      echo "请先退出本项目正在运行的 Koma，再重新构建。" >&2
      exit 1
    fi
  done
done

zsh "$PLATFORM_DIR/scripts/build.sh"
if [[ "$MODE" == --debug ]]; then
  exec lldb -- "$APP_BINARY"
fi
/usr/bin/open -n "$APP_BUNDLE"

if [[ "$MODE" == --verify ]]; then
  for _ in {1..30}; do
    for pid in $(running_app_pids "$APP_BUNDLE"); do
      # Finding a process once can falsely pass an immediate Swift startup
      # crash. Keep checking the same PID through initial scene creation.
      for _ in {1..40}; do
        sleep 0.25
        if ! kill -0 "$pid" 2>/dev/null; then
          echo "Koma 在启动后退出，请检查 DiagnosticReports 中的 KomaRustReader 崩溃报告。" >&2
          exit 1
        fi
      done
      echo "Koma 已启动并持续运行 10 秒（PID ${pid}）"
      exit 0
    done
    sleep 1
  done
  echo "应用未能启动。" >&2
  exit 1
elif [[ "$MODE" == --logs ]]; then
  exec /usr/bin/log stream --info --style compact --predicate 'process == "KomaRustReader"'
fi
