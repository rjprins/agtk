#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

# Build all binaries because agmux-native starts agmux-session beside itself.
"$ROOT/scripts/build-viewer.sh"
cargo build --bins

exec "$ROOT/target/debug/agmux-native" "$@"
