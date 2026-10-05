#!/bin/sh
set -eu

task_dev=0
case "$#:$*" in
    0:) ;;
    1:--dev) task_dev=1 ;;
    1:--help|1:-h)
        printf '%s\n' "Usage: scripts/install-local.sh [--dev]" \
            "  --dev  Link installed binaries to this checkout; rebuild and restart to use changes."
        exit 0 ;;
    *) printf '%s\n' "Usage: scripts/install-local.sh [--dev]" >&2; exit 2 ;;
esac

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
# A debug build with optimized dependencies; see the dev profile in Cargo.toml.
cargo build --manifest-path "$task_repo/Cargo.toml" --locked --bins

# Resolve a relative CARGO_TARGET_DIR before using it as a symlink target.
task_target=$(CDPATH= cd -- "$task_target" && pwd)
task_bindir="$task_destdir$task_prefix/bin"
mkdir -p "$task_bindir"
for task_binary in agtk agtk-session agtkctl agtk-mcp; do
    if [ "$task_dev" -eq 1 ]; then
        ln -sfn "$task_target/debug/$task_binary" "$task_bindir/$task_binary"
    else
        # Do not follow an earlier development link when installing a copy.
        if [ -L "$task_bindir/$task_binary" ]; then
            rm "$task_bindir/$task_binary"
        fi
        install -Dm755 "$task_target/debug/$task_binary" "$task_bindir/$task_binary"
    fi
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

# Agents in agtk sessions get the agtk tools without setup. Without
# --instance, agtk-mcp follows AGTK_INSTANCE, so each session reaches its own
# instance. Removing first replaces an entry from an earlier install.
if [ -z "$task_destdir" ]; then
    task_mcp="$task_prefix/bin/agtk-mcp"
    task_claude=${AGTK_CLAUDE_BIN:-claude}
    if command -v "$task_claude" >/dev/null 2>&1; then
        "$task_claude" mcp remove --scope user agtk >/dev/null 2>&1 || true
        if "$task_claude" mcp add --scope user agtk -- "$task_mcp" >/dev/null 2>&1; then
            printf '%s\n' "Added the agtk MCP server to Claude Code"
        else
            printf '%s\n' "Could not add the agtk MCP server to Claude Code" >&2
        fi
    fi
    task_codex=${AGTK_CODEX_BIN:-codex}
    if command -v "$task_codex" >/dev/null 2>&1; then
        "$task_codex" mcp remove agtk >/dev/null 2>&1 || true
        if "$task_codex" mcp add agtk -- "$task_mcp" >/dev/null 2>&1; then
            printf '%s\n' "Added the agtk MCP server to Codex"
        else
            printf '%s\n' "Could not add the agtk MCP server to Codex" >&2
        fi
    fi
fi

if [ -n "$task_destdir" ]; then
    printf '%s\n' "Staged agtk in $task_destdir$task_prefix"
else
    if [ "$task_dev" -eq 1 ]; then
        printf '%s\n' "Linked agtk in $task_prefix to $task_target/debug"
    else
        printf '%s\n' "Installed agtk in $task_prefix"
    fi
fi
