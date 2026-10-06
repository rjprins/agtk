#!/usr/bin/env bash
# Verify image rendering and file-opening on a private display.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
if [[ ${1:-} != --private-bus ]]; then
  "$repo/scripts/build-viewer.sh"
  exec dbus-run-session -- bash "$0" --private-bus
fi

command -v mutter >/dev/null || { echo "test-image-viewer needs mutter" >&2; exit 1; }
runtime=$(mktemp -d "${TMPDIR:-/tmp}/agtk-image-test.XXXXXX")
export XDG_RUNTIME_DIR="$runtime"
export AGTK_TEST_DISPLAY="$runtime/wayland-test" WAYLAND_DISPLAY="$runtime/wayland-test"
export GDK_BACKEND=wayland GSK_RENDERER=cairo GTK_A11Y=none GIO_USE_VFS=local GTK_USE_PORTAL=0
mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 \
  --wayland-display wayland-test >"$runtime/mutter.log" 2>&1 &
compositor=$!
cleanup() {
  kill "$compositor" 2>/dev/null || true
  wait "$compositor" 2>/dev/null || true
  rm -rf "$runtime"
}
trap cleanup EXIT
for _ in {1..50}; do
  [[ -S "$AGTK_TEST_DISPLAY" ]] && break
  sleep 0.1
done
[[ -S "$AGTK_TEST_DISPLAY" ]] || { cat "$runtime/mutter.log" >&2; exit 1; }

cd "$repo"
cargo test --locked --test image_viewer -- --ignored
cargo test --locked --test native_ui files_page_lists -- --ignored --test-threads=1
