#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runtime_dir="$(mktemp -d "${TMPDIR:-/tmp}/agmux-diff-runtime.XXXXXX")"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/agmux-diff-preview.XXXXXX")"
display_name="agmux-diff-preview-$$"
capture_path="${1:-$work_dir/viewer.png}"
mkdir -m 700 -p "$runtime_dir"

cleanup() {
  rm -rf "$runtime_dir"
}
trap cleanup EXIT

"$repo_root/scripts/build-viewer.sh"

dbus-run-session -- bash -c '
set -euo pipefail
display_name=$1
runtime_dir=$2
repo_root=$3
capture_path=$4
log_path=$5
export XDG_RUNTIME_DIR="$runtime_dir"
mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 --wayland-display "$display_name" >"$log_path" 2>&1 &
mutter_pid=$!
trap "kill $mutter_pid 2>/dev/null || true; wait $mutter_pid 2>/dev/null || true" EXIT
for _ in $(seq 100); do
  [[ -S "$runtime_dir/$display_name" ]] && break
  sleep 0.1
done
[[ -S "$runtime_dir/$display_name" ]] || { cat "$log_path" >&2; exit 1; }
export WAYLAND_DISPLAY="$display_name" GDK_BACKEND=wayland GSK_RENDERER=cairo
export AGMUX_TEST_DISPLAY="$runtime_dir/$display_name"
export AGMUX_DIFF_CAPTURE="$capture_path"
cargo run --quiet --manifest-path "$repo_root/Cargo.toml" --example diff_viewer_preview
' _ "$display_name" "$runtime_dir" "$repo_root" "$capture_path" "$work_dir/mutter.log"

printf 'Preview capture: %s\n' "$capture_path"
