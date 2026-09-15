#!/bin/sh
set -eu

task_prefix=${PREFIX:-"${HOME:?HOME is required}/.local"}
task_destdir=${DESTDIR:-}

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

for task_binary in agmux-native agmux-session agmuxctl agmux-mcp; do
    rm -f -- "$task_destdir$task_prefix/bin/$task_binary"
done
rm -f -- "$task_destdir$task_prefix/share/applications/nl.rutger.AgmuxNative.desktop"
rm -f -- "$task_destdir$task_prefix/share/icons/hicolor/scalable/apps/nl.rutger.AgmuxNative.svg"

if [ -z "$task_destdir" ] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$task_prefix/share/applications" >/dev/null 2>&1 || true
fi
if [ -z "$task_destdir" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$task_prefix/share/icons/hicolor" >/dev/null 2>&1 || true
fi

if [ -n "$task_destdir" ]; then
    printf '%s\n' "Removed staged agmux native files from $task_destdir$task_prefix"
else
    printf '%s\n' "Uninstalled agmux native from $task_prefix"
fi
printf '%s\n' "Runtime and saved workspace data were preserved"
