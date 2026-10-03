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

for task_binary in agtk agtk-session agtkctl agtk-mcp; do
    rm -f -- "$task_destdir$task_prefix/bin/$task_binary"
done
rm -f -- "$task_destdir$task_prefix/share/applications/nl.rutger.Agtk.desktop"
rm -f -- "$task_destdir$task_prefix/share/icons/hicolor/512x512/apps/nl.rutger.Agtk.png"
rm -f -- "$task_destdir$task_prefix/share/icons/hicolor/scalable/apps/nl.rutger.Agtk.svg"

if [ -z "$task_destdir" ] && command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$task_prefix/share/applications" >/dev/null 2>&1 || true
fi
if [ -z "$task_destdir" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$task_prefix/share/icons/hicolor" >/dev/null 2>&1 || true
fi

if [ -z "$task_destdir" ]; then
    task_claude=${AGTK_CLAUDE_BIN:-claude}
    if command -v "$task_claude" >/dev/null 2>&1; then
        "$task_claude" mcp remove --scope user agtk >/dev/null 2>&1 || true
    fi
    task_codex=${AGTK_CODEX_BIN:-codex}
    if command -v "$task_codex" >/dev/null 2>&1; then
        "$task_codex" mcp remove agtk >/dev/null 2>&1 || true
    fi
fi

if [ -n "$task_destdir" ]; then
    printf '%s\n' "Removed staged agtk files from $task_destdir$task_prefix"
else
    printf '%s\n' "Uninstalled agtk from $task_prefix"
fi
printf '%s\n' "Runtime and saved workspace data were preserved"
