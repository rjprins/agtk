#!/usr/bin/env bash
# Render agmux in a private headless GNOME compositor and capture every surface.
# Nothing touches the live instance or the desktop.
#
# Usage: scripts/ui-preview.sh [--dark] [--keep] [--font-size N] [SURFACE...]
#   SURFACE: main launch appearance shortcuts history search worktrees agents
#            pull-requests claude-models (default: all)
#   --keep   leave the instance running and print how to drive it
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
dark=0 keep=0 font_size=""
surfaces=()
while (($#)); do
  case "$1" in
    --dark) dark=1 ;;
    --keep) keep=1 ;;
    --font-size) font_size=$2; shift ;;
    -h|--help) sed -n 2,9p "$0"; exit 0 ;;
    *) surfaces+=("$1") ;;
  esac
  shift
done
((${#surfaces[@]})) || surfaces=(main launch appearance shortcuts history search worktrees agents pull-requests claude-models)

command -v mutter >/dev/null || { echo "ui-preview needs mutter" >&2; exit 1; }
"$repo/scripts/build-viewer.sh"
cargo build --quiet --manifest-path "$repo/Cargo.toml" --bins
bin="$repo/target/debug"

# Unix sockets need a short path, so runtime files live under XDG_RUNTIME_DIR.
work=$(mktemp -d "${TMPDIR:-/tmp}/agmux-preview.XXXXXX")
runtime="${XDG_RUNTIME_DIR:-/tmp}/agmux-run-$$"
display="agmux-preview-$$"
out="$work/captures"
mkdir -p "$runtime" "$out" "$work/state" "$work/claude" "$work/codex"

pids=()
cleanup() {
  if ((keep)); then return; fi
  if [[ -S "$runtime/agmux-native/preview/control.sock" ]]; then
    for id in $(ctl state 2>/dev/null | grep -o '"id":"[^"]*"' | cut -d'"' -f4); do
      ctl session close "$id" >/dev/null 2>&1 || true
    done
  fi
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$runtime"
}
trap cleanup EXIT

# Fake providers so no real agent starts.
cat > "$work/fake-agent" <<'AGENT'
#!/bin/sh
printf '\033[1m%s\033[0m ready in %s\n\n> ' "$(basename "$0")" "$PWD"
exec cat
AGENT
chmod 700 "$work/fake-agent"
ln -s "$work/fake-agent" "$work/claude-bin"
ln -s "$work/fake-agent" "$work/codex-bin"
cat > "$work/fake-az" <<'AZ'
#!/bin/sh
case "$1 $2 $3" in
  'account show --query') echo 'reviewer@example.com' ;;
  'repos pr list') cat "$AGMUX_AZURE_PRS_FILE" ;;
  'devops invoke --org') cat "$AGMUX_AZURE_THREADS_FILE" ;;
  *) echo 'unexpected az command' >&2; exit 2 ;;
esac
AZ
chmod 700 "$work/fake-az"

# A demo repository with an Azure remote and two extra worktrees.
project="$work/demo-service"
git init -q -b main "$project"
git -C "$project" -c user.name=Demo -c user.email=demo@example.com commit -q --allow-empty -m "initial commit"
git -C "$project" remote add origin https://dev.azure.com/org/project/_git/demo-service
git -C "$project" worktree add -q -b pagination-cursor "$work/demo-service-pagination-cursor"
git -C "$project" worktree add -q -b retry-backoff "$work/demo-service-retry-backoff"
feature="$work/demo-service-pagination-cursor"
cat > "$work/azure-prs.json" <<JSON
[{"pullRequestId":4217,"title":"Use cursor pagination for the orders endpoint","sourceRefName":"refs/heads/pagination-cursor","targetRefName":"refs/heads/main","creationDate":"2026-09-22T10:00:00Z","isDraft":false,"createdBy":{"displayName":"A Colleague","uniqueName":"colleague@example.com"},"lastMergeSourceCommit":{"commitId":"0123456789abcdef"},"mergeStatus":"succeeded","reviewers":[]},
 {"pullRequestId":4230,"title":"Retry transient queue failures with backoff","sourceRefName":"refs/heads/retry-backoff","targetRefName":"refs/heads/main","creationDate":"2026-09-23T08:00:00Z","isDraft":true,"createdBy":{"displayName":"Another Colleague","uniqueName":"other@example.com"},"lastMergeSourceCommit":{"commitId":"fedcba9876543210"},"mergeStatus":"succeeded","reviewers":[]}]
JSON
echo '{"value":[]}' > "$work/azure-threads.json"

# Recent provider conversations for the restore dialog.
day=$(date +%Y/%m/%d)
mkdir -p "$work/codex/sessions/$day" "$work/claude/projects/demo"
printf '%s\n' \
  "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-preview-1\",\"cwd\":\"$project\"}}" \
  '{"type":"response_item","payload":{"role":"user","content":"Add an index for the orders lookup"}}' \
  '{"type":"response_item","payload":{"role":"assistant","content":"Added the index and a migration test."}}' \
  > "$work/codex/sessions/$day/rollout-preview.jsonl"
printf '%s\n' \
  "{\"type\":\"user\",\"sessionId\":\"claude-preview-1\",\"cwd\":\"$feature\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Explain why the cursor test is flaky\"}]}}" \
  > "$work/claude/projects/demo/claude-preview-1.jsonl"

export WAYLAND_DISPLAY="$display" GDK_BACKEND=wayland GSK_RENDERER=cairo
export AGMUX_INSTANCE=preview AGMUX_RUNTIME_ROOT="$runtime" AGMUX_STATE_ROOT="$work/state"
export CLAUDE_CONFIG_DIR="$work/claude" CODEX_HOME="$work/codex"
export AGMUX_CLAUDE_BIN="$work/claude-bin" AGMUX_CODEX_BIN="$work/codex-bin"
export AGMUX_AZURE_BIN="$work/fake-az" AGMUX_AZURE_PRS_FILE="$work/azure-prs.json"
export AGMUX_AZURE_THREADS_FILE="$work/azure-threads.json" AGMUX_EMACSCLIENT=/bin/true
((dark)) && export ADW_DEBUG_COLOR_SCHEME=prefer-dark

ctl() { "$bin/agmuxctl" --instance preview "$@"; }
id_of() { ctl state | grep -o "\"id\":\"[^\"]*\"[^}]*\"name\":\"$1\"" | head -1 | cut -d'"' -f4; }

dbus-run-session -- mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 \
  --wayland-display "$display" >"$work/mutter.log" 2>&1 &
pids+=($!)
for _ in $(seq 50); do [[ -S "${XDG_RUNTIME_DIR:-/tmp}/$display" ]] && break; sleep 0.2; done

start_ui() {
  "$bin/agmux-native" >>"$work/app.log" 2>&1 &
  ui_pid=$!
  for _ in $(seq 100); do ctl state >/dev/null 2>&1 && break; sleep 0.1; done
  sleep 1
}
start_ui

ctl session create --kind claude --cwd "$project" --project-root "$project" --worktree-path "$project" --name "demo-service" >/dev/null
ctl session create --kind codex --cwd "$feature" --project-root "$project" --worktree-path "$feature" --name "cursor pagination" >/dev/null
ctl session create --kind shell --cwd "$feature" --project-root "$project" --worktree-path "$feature" --name "test shell" >/dev/null
ctl session create --kind shell --cwd "$HOME" --name "home" >/dev/null
sleep 1
ctl session state "$(id_of 'cursor pagination')" waiting >/dev/null
main_id=$(id_of demo-service)
ctl session input "$main_id" --text "Make the orders endpoint use cursor pagination" >/dev/null
ctl session select "$main_id" >/dev/null
[[ -n $font_size ]] && ctl appearance set --ui-font-size "$font_size" >/dev/null
# Open the launch dialog on Codex, the agent with the most options.
sqlite3 "$work/state/agmux-native/preview/agmux.db" \
  "insert or replace into preferences(key, data) values ('quickLaunch', '{\"kind\":\"codex\",\"cwd\":\"$project\",\"projectRoot\":\"$project\",\"worktreePath\":\"$project\"}')"
sleep 1

capture() {
  local name=$1 path
  path=$(ctl ui capture | grep -o '"path":"[^"]*"' | cut -d'"' -f4)
  cp "$path" "$out/$name.png"
  echo "$out/$name.png"
}

for surface in "${surfaces[@]}"; do
  if [[ $surface == main ]]; then
    capture main
    continue
  fi
  # Restart the UI so only one dialog is open; session hosts survive it.
  kill "$ui_pid"; wait "$ui_pid" 2>/dev/null || true
  start_ui
  ctl session select "$main_id" >/dev/null
  # Prompt history only lives as long as one UI process.
  ctl session input "$main_id" --text "Make the orders endpoint use cursor pagination" >/dev/null
  shown=$(ctl ui show "$surface" | grep -o '"shown":[a-z]*' | cut -d: -f2)
  [[ $shown == true ]] || { echo "$surface: not available" >&2; continue; }
  sleep 1.5
  capture "$surface"
done
pids+=("$ui_pid")

if ((keep)); then
  echo "Instance still running. Drive it with:"
  echo "  AGMUX_RUNTIME_ROOT=$runtime $bin/agmuxctl --instance preview state"
  echo "Stop it with: kill ${pids[*]}"
fi
