# agmux native

An early native replacement for agmux's browser terminal layer. It targets
Linux, Wayland, and GNOME with a Rust, GTK4, libadwaita, and VTE stack.

The prototype currently provides:

- one application window with a session sidebar
- embedded VTE terminals
- shell processes that survive application restarts
- automatic copy on terminal selection
- per-session input history that scrolls to retained prompts
- explicit session shutdown

Prompt history and terminal scrollback are intentionally not persisted when the
application exits.

## Run

Build both the UI and session host before starting the application:

```sh
cargo build --bins
cargo run --bin agmux-native
```

The UI launches `agmux-session` from the same target directory. Session sockets
live under `$XDG_RUNTIME_DIR/agmux-native` and are rediscovered on startup.

## Verify

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The lifecycle integration test starts only disposable processes and verifies
attachment, detachment, replay, reattachment, and explicit shutdown.
