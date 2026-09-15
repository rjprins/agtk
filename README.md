# agmux native

agmux native is a personal, one-window workspace for terminal coding agents on Linux, Wayland, and GNOME. It keeps the useful shape of the original agmux, with grouped sessions in a persistent sidebar and one visible terminal, while replacing the browser terminal, WebSocket bridge, and tmux layer with GTK4 and VTE.

The application uses four cooperating binaries:

- `agmux-native` owns the GTK workspace and VTE terminals.
- `agmux-session` owns one PTY and child process independently of the UI.
- `agmuxctl` exposes the versioned local control socket as machine-readable commands.
- `agmux-mcp` maps MCP tools to that same control protocol.

Closing, crashing, or rebuilding the GTK process does not stop hosted sessions. Reopening it discovers and reattaches those hosts. Machine reboot survival and durable terminal scrollback are intentionally out of scope.

## Current features

- One compact native window with project and worktree grouped sessions in the sidebar
- Embedded VTE terminals with true color, selection-to-clipboard, normal copy and paste, search, and live prompt history navigation
- Shell, Codex, Claude, Gemini, and custom launches with exact working-directory and worktree context
- Independent PTY hosts with bounded detached replay and explicit process-group shutdown
- Persisted session metadata, ordering, project pins, collapsed groups, launch preferences, terminal themes, fonts, and keyboard overrides
- Ctrl-plus and Ctrl-minus font scaling for the TUI chrome and every embedded terminal, persisted with appearance settings
- Safe purpose-aware worktree creation and guarded reap with salvage and attic tags
- Recent Codex and Claude conversation discovery, preview, direct restore, and explicit busy, ready, and waiting callbacks
- Configurable Claude model and effort presets that send the native `/model` and `/effort` commands
- Emacs Magit and branch-review integration for the selected session
- Azure DevOps PR attention, exact acknowledgement, source-worktree matching, manual review launch, and per-project opt-in auto-review
- App-only PNG capture and structural UI inspection for isolated agent-driven testing
- Local CLI and MCP control without exposing a network listener

## Requirements

The current build targets recent GTK APIs. The tested Arch packages are `gtk4`, `libadwaita`, `vte4`, and `sqlite`, plus a Rust toolchain. The observed baseline is GTK 4.22, libadwaita 1.9, VTE 0.84, and Rust 1.98.

Azure PR support additionally needs the Azure CLI with the Azure DevOps extension and an existing local sign-in. Emacs actions need `emacsclient`. Codex and Claude need their respective command-line tools.

## Build and run

Build every cooperating binary before starting the workspace:

```sh
cargo build --bins
cargo run --bin agmux-native
```

The development UI finds `agmux-session` beside its own executable. Runtime sockets live below `$XDG_RUNTIME_DIR/agmux-native/default`. Durable metadata lives below `$XDG_STATE_HOME/agmux-native/default`, or `~/.local/state` when `XDG_STATE_HOME` is unset.

Useful executable overrides are `AGMUX_CODEX_BIN`, `AGMUX_CLAUDE_BIN`, `AGMUX_EMACSCLIENT`, and `AGMUX_AZURE_BIN`.

## Local installation

Install a release build, all helper binaries, and the GNOME desktop entry under `~/.local`:

```sh
./scripts/install-local.sh
```

Set an absolute `PREFIX` to choose another location. The staged smoke test builds the release artifacts, installs into a disposable root, validates the desktop entry when the validator is available, and starts the installed MCP adapter with isolated paths:

```sh
./scripts/smoke-install.sh
```

Rerun the installer to upgrade the four binaries, desktop entry, and app icon in place. Remove only those installed files with:

```sh
./scripts/uninstall-local.sh
```

Uninstalling preserves runtime and saved workspace data. After installation, launch “agmux native” from the GNOME overview or run `~/.local/bin/agmux-native`. Startup diagnostics written to stdout or stderr by a desktop launch are available in the user journal.

## Keyboard contract

These shortcuts work while VTE has focus and can be changed through `[keys]`:

| Action | Default |
| --- | --- |
| New shell | `Ctrl+Shift+Backquote` |
| Close selected session | `Ctrl+Shift+Q` |
| Toggle sidebar | `Ctrl+Shift+Backslash` |
| Next or previous session | `Ctrl+Shift+]` or `Ctrl+Shift+[` |
| Next ready session | `Ctrl+Shift+Space` |
| Reopen PR list | `Alt+Shift+P` |
| Switch Claude model preset | `Ctrl+Shift+M` |
| Copy or paste | `Ctrl+Shift+C` or `Ctrl+Shift+V` |
| Search terminal | `Ctrl+Shift+F` |
| Increase or decrease font size | `Ctrl++` or `Ctrl+-` |

Selecting terminal text also copies a whitespace-cleaned version to the regular clipboard. Prompt history is retained only while the current GTK process is running.

The launch surface follows the original agmux launch behavior. It keeps project and worktree fields editable, completes filesystem paths, remembers provider options per project, offers Claude, Codex, Gemini, and shell choices, and presents `+ New worktree`, `Current (...)`, and sorted Git worktree choices. New worktrees expose generated branch and base-branch fields before the session launches. Action controls use compact symbolic icons instead of bracket glyphs.

The Claude preset shortcut opens the chooser for a selected live Claude session. Repeating the shortcut cycles its focused preset, Enter applies it, and Escape cancels. Presets are editable as a validated JSON array in `[keys]`. Applying one assumes Claude is at an empty prompt.

## Local control

`agmuxctl` prints JSON to stdout. It accepts `--instance NAME` before the command, which keeps automated instances separate from the live `default` instance.

```sh
agmuxctl state
agmuxctl session create --kind codex --cwd /absolute/project
agmuxctl session text SESSION_ID --lines 200
agmuxctl ui inspect
agmuxctl ui capture
agmuxctl ui show pull-requests
agmuxctl pr list /absolute/project
agmuxctl pr review /absolute/project 1234
agmuxctl claude presets
agmuxctl claude apply SESSION_ID opus-high
```

Hosted agent processes receive `AGMUX_INSTANCE`, `AGMUX_SESSION_ID`, and `AGMUX_CONTROL_SOCKET`. A provider hook can report explicit readiness with:

```sh
agmuxctl session state "$AGMUX_SESSION_ID" ready
agmuxctl session state "$AGMUX_SESSION_ID" waiting
```

Submitted input marks agent sessions busy.

## MCP

An MCP client can start the adapter as a local stdio server:

```json
{
  "mcpServers": {
    "agmux-native": {
      "command": "/home/rutger/.local/bin/agmux-mcp",
      "args": ["--instance", "default"]
    }
  }
}
```

The tools cover sessions, terminal input and snapshots, worktrees, recent provider sessions, Claude presets, Emacs actions, PR workflows, readiness callbacks, UI structure, and app-only PNG capture. The adapter never captures the desktop.

## Verification

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The ignored `native_ui` suite requires a private Wayland compositor through `AGMUX_TEST_DISPLAY`. It creates isolated runtime and state roots and never attaches to the live instance.

## Adoption status

The native feature implementation and isolated verification are complete. The browser version remains untouched and available until a real daily-use trial confirms the native workspace is comfortable enough to replace it.
