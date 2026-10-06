#!/usr/bin/env bash
# Exercise modal dismissal with real pointer and keyboard input on a private display.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
if [[ ${1:-} != --private-bus ]]; then
  exec dbus-run-session -- bash "$0" --private-bus
fi

command -v mutter >/dev/null || { echo "test-modals needs mutter" >&2; exit 1; }
display="agtk-modal-test-$$"
log=$(mktemp "${TMPDIR:-/tmp}/agtk-modal-test.XXXXXX")
export AGTK_TEST_DISPLAY="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is required}/$display"
export AGTK_TEST_BUS="${DBUS_SESSION_BUS_ADDRESS:?private session bus is required}"
export GDK_BACKEND=wayland GSK_RENDERER=cairo GTK_A11Y=none GIO_USE_VFS=local GTK_USE_PORTAL=0
mutter --headless --wayland --no-x11 --virtual-monitor 1000x700 \
  --wayland-display "$display" >"$log" 2>&1 &
compositor=$!
cleanup() {
  kill "$compositor" 2>/dev/null || true
  wait "$compositor" 2>/dev/null || true
}
trap cleanup EXIT
for _ in {1..50}; do
  [[ -S "$AGTK_TEST_DISPLAY" ]] && break
  if ! kill -0 "$compositor" 2>/dev/null; then
    cat "$log" >&2
    exit 1
  fi
  sleep 0.1
done
[[ -S "$AGTK_TEST_DISPLAY" ]] || { cat "$log" >&2; exit 1; }

cd "$repo"
cargo test --locked --lib real_pointer_input_dismisses -- --ignored --test-threads=1
