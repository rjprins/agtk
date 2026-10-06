#!/usr/bin/env bash
# Render agtk in a private headless GNOME compositor and capture every surface.
# Nothing touches the live instance or the desktop.
#
# Usage: scripts/ui-preview.sh [--dark] [--keep] [--font-size N] [SURFACE...]
#   SURFACE: main status changes files launch appearance shortcuts history search worktrees agents
#            pull-requests review-settings claude-models close-session (default: all)
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
((${#surfaces[@]})) || surfaces=(main changes files launch appearance shortcuts history search worktrees agents pull-requests review-settings claude-models close-session)

command -v mutter >/dev/null || { echo "ui-preview needs mutter" >&2; exit 1; }
"$repo/scripts/build-viewer.sh"
cargo build --quiet --manifest-path "$repo/Cargo.toml" --bins
bin="$repo/target/debug"

# Unix sockets need a short path, so runtime files live under XDG_RUNTIME_DIR.
work=$(mktemp -d "${TMPDIR:-/tmp}/agtk-preview.XXXXXX")
runtime="${XDG_RUNTIME_DIR:-/tmp}/agtk-run-$$"
display="agtk-preview-$$"
out="$work/captures"
mkdir -p "$runtime" "$out" "$work/state" "$work/claude" "$work/codex"

pids=()
compositor=""
cleanup() {
  if ((keep)); then return; fi
  if [[ -S "$runtime/agtk/preview/control.sock" ]]; then
    for id in $(ctl state 2>/dev/null | grep -o '"id":"[^"]*"' | cut -d'"' -f4); do
      ctl session close "$id" >/dev/null 2>&1 || true
    done
  fi
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  # dbus-run-session does not pass the signal on, so stop its whole group.
  [[ -n $compositor ]] && kill -- "-$compositor" 2>/dev/null || true
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
  'repos pr list') cat "$AGTK_AZURE_PRS_FILE" ;;
  'repos pr work-item') printf '%s\n' '[{"id":2417,"fields":{"System.WorkItemType":"Product Backlog Item","System.Title":"Implement cursor pagination"}}]' ;;
  'devops invoke --org') cat "$AGTK_AZURE_THREADS_FILE" ;;
  *) echo 'unexpected az command' >&2; exit 2 ;;
esac
AZ
chmod 700 "$work/fake-az"

# A demo repository with an Azure remote and two extra worktrees.
project="$work/demo-service"
git init -q -b main "$project"
mkdir -p "$project/src"
cat > "$project/src/orders.py" <<'PY'
def fetch_orders(cursor=None):
    return query_orders(cursor=cursor)
PY
git -C "$project" add src/orders.py
git -C "$project" -c user.name=Demo -c user.email=demo@example.com commit -q -m "Add orders endpoint"
git -C "$project" remote add origin https://dev.azure.com/org/project/_git/demo-service
git -C "$project" worktree add -q -b pagination-cursor "$work/demo-service-pagination-cursor"
git -C "$project" worktree add -q -b retry-backoff "$work/demo-service-retry-backoff"
feature="$work/demo-service-pagination-cursor"
git -C "$feature" config user.name Demo
git -C "$feature" config user.email demo@example.com
cat > "$feature/src/orders.py" <<'PY'
def fetch_orders(cursor=None, page_size=50):
    return query_orders(cursor=cursor, limit=page_size)
PY
git -C "$feature" add src/orders.py
git -C "$feature" commit -q -m "Add cursor pagination"
cat >> "$feature/src/orders.py" <<'PY'

def next_cursor(rows):
    return rows[-1].id if rows else None
PY
git -C "$feature" add src/orders.py
git -C "$feature" commit -q -m "Return next page cursor"
cat > "$feature/src/staged.py" <<'PY'
def staged_example():
    return "ready"
PY
git -C "$feature" add src/staged.py
cat >> "$feature/src/orders.py" <<'PY'

# Local query instrumentation.
PY
cat > "$feature/notes.txt" <<'TXT'
Untracked preview file.
TXT
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
export AGTK_INSTANCE=preview AGTK_RUNTIME_ROOT="$runtime" AGTK_STATE_ROOT="$work/state"
export CLAUDE_CONFIG_DIR="$work/claude" CODEX_HOME="$work/codex"
export AGTK_CLAUDE_BIN="$work/claude-bin" AGTK_CODEX_BIN="$work/codex-bin"
export AGTK_AZURE_BIN="$work/fake-az" AGTK_AZURE_PRS_FILE="$work/azure-prs.json"
export AGTK_AZURE_THREADS_FILE="$work/azure-threads.json" AGTK_EMACSCLIENT=/bin/true
((dark)) && export ADW_DEBUG_COLOR_SCHEME=prefer-dark

# Usage preview fixtures: no real sign-ins and no outgoing usage requests.
mkdir -p "$work/bin"
cat > "$work/bin/curl" <<'CURL'
#!/bin/sh
cat >/dev/null
for url in "$@"; do :; done
reset=$(( $(date +%s) + 7200 ))
case "$url" in
  https://api.anthropic.com/api/oauth/usage)
    printf '{"five_hour":{"utilization":18,"resets_at":%s},"seven_day":{"utilization":39,"resets_at":%s}}' "$reset" "$reset" ;;
  https://chatgpt.com/backend-api/wham/usage)
    printf '{"rate_limit":{"primary_window":{"used_percent":28,"limit_window_seconds":18000,"reset_at":%s},"secondary_window":{"used_percent":54,"limit_window_seconds":604800,"reset_at":%s}}}' "$reset" "$reset" ;;
  *) exit 1 ;;
esac
CURL
chmod 700 "$work/bin/curl"
export PATH="$work/bin:$PATH"
printf '%s\n' '{"tokens":{"access_token":"preview-fixture"}}' > "$work/codex/auth.json"
printf '%s\n' '{"claudeAiOauth":{"accessToken":"preview-fixture"}}' > "$work/claude/.credentials.json"

ctl() { "$bin/agtkctl" --instance preview "$@"; }
id_of() { ctl state | grep -o "\"id\":\"[^\"]*\"[^}]*\"name\":\"$1\"" | head -1 | cut -d'"' -f4; }

setsid dbus-run-session -- mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 \
  --wayland-display "$display" >"$work/mutter.log" 2>&1 &
compositor=$!
for _ in $(seq 50); do [[ -S "${XDG_RUNTIME_DIR:-/tmp}/$display" ]] && break; sleep 0.2; done

start_ui() {
  "$bin/agtk" >>"$work/app.log" 2>&1 &
  ui_pid=$!
  for _ in $(seq 100); do ctl state >/dev/null 2>&1 && break; sleep 0.1; done
  sleep 1
}
start_ui

ctl session create --kind claude --cwd "$project" --project-root "$project" --worktree-path "$project" --name "demo-service" >/dev/null
ctl session create --kind codex --cwd "$feature" --project-root "$project" --worktree-path "$feature" --name "cursor pagination" >/dev/null
ctl session create --kind shell --cwd "$feature" --project-root "$project" --worktree-path "$feature" --name "test shell" >/dev/null
ctl session create --kind shell --cwd "$HOME" --name "home" >/dev/null
# A project whose last session closed, so the sidebar lists it under Inactive Projects.
mkdir -p "$work/billing-prototype"
ctl session create --kind shell --cwd "$work/billing-prototype" --project-root "$work/billing-prototype" --name "billing" >/dev/null
ctl session close "$(id_of billing)" >/dev/null
sleep 1
ctl session state "$(id_of 'cursor pagination')" waiting >/dev/null
main_id=$(id_of demo-service)
codex_id=$(id_of 'cursor pagination')
printf '%s\n' '{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"output_tokens":12000,"cached_input_tokens":88000},"last_token_usage":{"total_tokens":72000},"model_context_window":200000}}}' > "$work/codex/sessions/$day/rollout-status-preview.jsonl"
printf '%s\n' '{"session_id":"status-preview"}' | ctl session state "$codex_id" waiting --hook-input >/dev/null
ctl session input "$main_id" --text "Make the orders endpoint use cursor pagination" >/dev/null
ctl session select "$main_id" >/dev/null
[[ -n $font_size ]] && ctl appearance set --ui-font-size "$font_size" >/dev/null
# Open the launch dialog on Codex, the agent with the most options.
sqlite3 "$work/state/agtk/preview/agtk.db" \
  "insert or replace into preferences(key, data) values ('quickLaunch', '{\"kind\":\"codex\",\"cwd\":\"$project\",\"projectRoot\":\"$project\",\"worktreePath\":\"$project\"}')"
sleep 1

capture() {
  local name=$1 path
  path=$(ctl ui capture | grep -o '"path":"[^"]*"' | cut -d'"' -f4)
  cp "$path" "$out/$name.png"
  echo "$out/$name.png"
}

for surface in "${surfaces[@]}"; do
  if [[ $surface == status ]]; then
    ctl session select "$codex_id" >/dev/null
    sleep 2.5
    capture status
    continue
  fi
  if [[ $surface == main ]]; then
    capture main
    continue
  fi
  # Restart the UI so only one dialog is open; session hosts survive it.
  kill "$ui_pid"; wait "$ui_pid" 2>/dev/null || true
  start_ui
  ctl session select "$main_id" >/dev/null
  if [[ $surface == changes || $surface == files ]]; then
    ctl session select "$(id_of 'cursor pagination')" >/dev/null
  fi
  if [[ $surface == close-session ]]; then
    # The prompt only opens for the last session in a worktree.
    ctl session close "$(id_of 'test shell')" >/dev/null
    ctl session select "$(id_of 'cursor pagination')" >/dev/null
  fi
  # Prompt history only lives as long as one UI process.
  ctl session input "$main_id" --text "Make the orders endpoint use cursor pagination" >/dev/null
  shown=$(ctl ui show "$surface" | grep -o '"shown":[a-z]*' | cut -d: -f2)
  [[ $shown == true ]] || { echo "$surface: not available" >&2; continue; }
  if [[ $surface == changes ]]; then
    changes_session=$(id_of 'cursor pagination')
    ctl ui diff "$changes_session" --scope committed --path src/orders.py >/dev/null
    rendered=0
    for _ in $(seq 40); do
      if ctl ui inspect | grep -q '"label":"rendered:'; then
        rendered=1
        break
      fi
      sleep 0.1
    done
    ((rendered)) || { echo "Changes diff did not render" >&2; exit 1; }
  fi
  if [[ $surface == files ]]; then
    files_session=$(id_of 'cursor pagination')
    ctl ui file "$files_session" --path src/orders.py --line 2 --column 5 >/dev/null
    rendered=0
    for _ in $(seq 40); do
      if ctl ui inspect | grep -q '"label":"rendered:'; then
        rendered=1
        break
      fi
      sleep 0.1
    done
    ((rendered)) || { echo "File did not render" >&2; exit 1; }
  fi
  sleep 1.5
  capture "$surface"
done
pids+=("$ui_pid")

if ((keep)); then
  echo "Instance still running. Drive it with:"
  echo "  AGTK_RUNTIME_ROOT=$runtime $bin/agtkctl --instance preview state"
  echo "Stop it with: kill ${pids[*]}; kill -- -$compositor"
fi
