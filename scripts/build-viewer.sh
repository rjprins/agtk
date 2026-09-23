#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
viewer_dir="$repo_root/viewer"
build_stamp="$viewer_dir/dist/.agmux-viewer-build"
needs_build=0

if [[ ! -d "$viewer_dir/node_modules" ]]; then
  npm --prefix "$viewer_dir" ci
  needs_build=1
fi

if [[ ! -f "$build_stamp" || ! -f "$viewer_dir/dist/index.html" ]]; then
  needs_build=1
else
  for source in \
    "$viewer_dir/index.html" \
    "$viewer_dir/vite.config.js" \
    "$viewer_dir/package.json" \
    "$viewer_dir/package-lock.json"; do
    [[ "$source" -nt "$build_stamp" ]] && needs_build=1
  done
  if find "$viewer_dir/src" -type f -newer "$build_stamp" -print -quit | grep -q .; then
    needs_build=1
  fi
fi

if ((needs_build)); then
  npm --prefix "$viewer_dir" run build
  : > "$build_stamp"
else
  printf '%s\n' 'Changes viewer bundle is up to date'
fi
