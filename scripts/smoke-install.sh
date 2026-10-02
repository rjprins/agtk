#!/bin/sh
set -eu

task_repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_stage=$(mktemp -d)
trap 'find "$task_stage" -depth -delete' EXIT HUP INT TERM

"$task_repo/scripts/build-viewer.sh"
DESTDIR="$task_stage" PREFIX=/usr/local "$task_repo/scripts/install-local.sh"

task_bindir="$task_stage/usr/local/bin"
for task_binary in agtk agtk-session agtkctl agtk-mcp; do
    test -x "$task_bindir/$task_binary"
done

task_desktop="$task_stage/usr/local/share/applications/nl.rutger.Agtk.desktop"
test -f "$task_desktop"
grep -Fqx 'Exec=/usr/local/bin/agtk' "$task_desktop"
grep -Fqx 'TryExec=/usr/local/bin/agtk' "$task_desktop"
grep -Fqx 'Icon=nl.rutger.Agtk' "$task_desktop"

task_icon="$task_stage/usr/local/share/icons/hicolor/512x512/apps/nl.rutger.Agtk.png"
test -f "$task_icon"
cmp -s "$task_repo/galaxy.png" "$task_icon"
test ! -e "$task_stage/usr/local/share/icons/hicolor/scalable/apps/nl.rutger.Agtk.svg"

if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$task_desktop"
fi

AGTK_RUNTIME_ROOT="$task_stage/runtime" \
AGTK_STATE_ROOT="$task_stage/state" \
    "$task_bindir/agtk-mcp" --instance install-smoke </dev/null

install -Dm644 /dev/null "$task_stage/usr/local/share/agtk-smoke/sentinel"
install -Dm644 /dev/null "$task_stage/state/preserved"
DESTDIR="$task_stage" PREFIX=/usr/local "$task_repo/scripts/uninstall-local.sh"

for task_binary in agtk agtk-session agtkctl agtk-mcp; do
    test ! -e "$task_bindir/$task_binary"
done
test ! -e "$task_desktop"
test ! -e "$task_icon"
test -f "$task_stage/usr/local/share/agtk-smoke/sentinel"
test -f "$task_stage/state/preserved"

printf '%s\n' "Staged installation and uninstall smoke test passed"
