# Bottom status bar

Show account quota on the left, selected-session token usage in the middle,
and whole-machine CPU and RAM on the right. The session segment follows the
terminal's horizontal position when either sidebar changes width. Use one
compact row, with keyboard-accessible popovers for details.

Account quota means the provider's reported percentage remaining in each limit
window, not a token budget. Read Claude and Codex quota using their existing
local sign-ins. Credentials go only to the corresponding HTTPS usage endpoint;
never print, persist, or put them in process arguments. Refresh once a minute on
a background worker. An unavailable reading must not appear as zero usage.

Read selected-session tokens from local transcripts on a background worker.
Codex reports cumulative tokens and the most recent context window. Claude
transcripts report per-message usage; deduplicate streaming updates. Only show
a context percentage when the log supplies the capacity. Do not infer account
quota from cumulative token usage. Bound reads and retain incomplete lines.
Detect replacements, truncation and changed append boundaries immediately.
Reconcile changed transcripts from the beginning once a minute, in bounded
background chunks, to catch in-place edits outside those boundaries without
rereading all historical bytes on every two-second poll.

Sample `/proc/stat` and `/proc/meminfo` every two seconds; CPU is the change
between samples, and used memory excludes MemAvailable. No database changes or
new Rust dependencies are needed. Curl is optional for account quota.

Implementation lives in `src/usage/` and `src/ui/status_bar.rs`, following the
existing plain Rust structs and GTK widget composition. For example:

```rust
#[derive(Debug, Clone, Default)]
pub struct SessionUsage {
    pub input: u64,
    pub output: u64,
    pub context_capacity: Option<u64>,
}
```

Build: `./scripts/build-viewer.sh && cargo build --locked --bins`.
Check: `cargo fmt --all -- --check`, `cargo test --all-targets`, and
`cargo clippy --all-targets -- -D warnings`.

Verify provider parsing, incomplete and rewritten transcripts, invalid quota,
CPU deltas, and memory accounting with fixture tests. Capture the real GTK
window on a private compositor using `scripts/ui-preview.sh`. Check the full
window and a narrow layout; samples in a preview must be identified as samples.

Always keep I/O off GTK's main thread and preserve the user's agent settings.
Never generate a model request merely to obtain usage, refresh credentials, or
alter a live session. Authentication changes beyond reading existing sign-ins
and any persistence changes need a separate decision.

Order of work: system counters and bar layout; session log reader; account
quota; tests and native screenshots. Success is a working, responsive bar in
the agreed order, with the session segment aligned to the terminal and honest
missing-data states.
