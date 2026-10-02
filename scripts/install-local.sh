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

"$task_repo/scripts/build-viewer.sh"
cargo build --manifest-path "$task_repo/Cargo.toml" --release --locked --bins

task_bindir="$task_destdir$task_prefix/bin"
for task_binary in agtk agtk-session agtkctl agtk-mcp; do
    install -Dm755 "$task_target/release/$task_binary" "$task_bindir/$task_binary"
done

task_desktop=$(mktemp)
trap 'rm -f -- "$task_desktop"' EXIT HUP INT TERM
sed "s|@BINDIR@|$task_prefix/bin|g" \
    "$task_repo/data/nl.rutger.Agtk.desktop.in" > "$task_desktop"
install -Dm644 "$task_desktop" \
    "$task_destdir$task_prefix/share/applications/nl.rutger.Agtk.desktop"
install -Dm644 "$task_repo/galaxy.png" \
    "$task_destdir$task_prefix/share/icons/hicolor/512x512/apps/nl.rutger.Agtk.png"
# Earlier installs shipped a scalable SVG, which the icon theme would prefer.
rm -f -- "$task_destdir$task_prefix/share/icons/hicolor/scalable/apps/nl.rutger.Agtk.svg"

if [ -z "$task_destdir" ] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$task_prefix/share/applications" >/dev/null 2>&1 || true
fi
if [ -z "$task_destdir" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$task_prefix/share/icons/hicolor" >/dev/null 2>&1 || true
fi

if [ -n "$task_destdir" ]; then
    printf '%s\n' "Staged agtk in $task_destdir$task_prefix"
else
    printf '%s\n' "Installed agtk in $task_prefix"
fi
