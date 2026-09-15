#!/bin/sh
set -eu

task_repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_prefix=${PREFIX:-"${HOME:?HOME is required}/.local"}
task_destdir=${DESTDIR:-}
task_target=${CARGO_TARGET_DIR:-"$task_repo/target"}

case "$task_prefix" in
    /*) ;;
    *) printf '%s\n' "PREFIX must be an absolute path" >&2; exit 2 ;;
esac
case "$task_destdir" in
    ""|/*) ;;
    *) printf '%s\n' "DESTDIR must be empty or an absolute path" >&2; exit 2 ;;
esac
case "$task_prefix" in
    *[!A-Za-z0-9_./-]*) printf '%s\n' "PREFIX contains unsupported characters" >&2; exit 2 ;;
esac

cargo build --manifest-path "$task_repo/Cargo.toml" --release --locked --bins

task_bindir="$task_destdir$task_prefix/bin"
for task_binary in agmux-native agmux-session agmuxctl agmux-mcp; do
    install -Dm755 "$task_target/release/$task_binary" "$task_bindir/$task_binary"
done

task_desktop=$(mktemp)
trap 'rm -f -- "$task_desktop"' EXIT HUP INT TERM
sed "s|@BINDIR@|$task_prefix/bin|g" \
    "$task_repo/data/nl.rutger.AgmuxNative.desktop.in" > "$task_desktop"
install -Dm644 "$task_desktop" \
    "$task_destdir$task_prefix/share/applications/nl.rutger.AgmuxNative.desktop"

if [ -z "$task_destdir" ] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$task_prefix/share/applications" >/dev/null 2>&1 || true
fi

if [ -n "$task_destdir" ]; then
    printf '%s\n' "Staged agmux native in $task_destdir$task_prefix"
else
    printf '%s\n' "Installed agmux native in $task_prefix"
fi
