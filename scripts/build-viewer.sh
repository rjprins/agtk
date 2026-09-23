#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
viewer_dir="$repo_root/viewer"

if [[ ! -d "$viewer_dir/node_modules" ]]; then
  npm --prefix "$viewer_dir" ci
fi

npm --prefix "$viewer_dir" run build
: > "$viewer_dir/dist/.agmux-viewer-build"
