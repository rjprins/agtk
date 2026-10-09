# ADR-0002: Browse and open any worktree file in agtk

## Status

Accepted

## Date

2026-09-30

## Context

The Changes sidebar only reaches files that changed. Reading the rest of the worktree, and opening a path an agent prints in the terminal, meant switching to Emacs. Later steps are code navigation: go to definition, find references and hover.

## Decision

The right sidebar gets two pages in an `AdwViewStack` with an `AdwInlineViewSwitcher`: Changes and Files. Files is a `GtkListView` over a `GtkTreeListModel`, so rows are recycled and folders build their children only when expanded. The tree comes from a filesystem walk per refresh, including untracked, hidden and ignored files such as build output. Git metadata (`.git`) stays out, and directory symlinks appear without traversing their targets. Change marks come from the Changes snapshot. A filter entry swaps the tree for a flat list of matching paths.

Files open read only in the same Monaco WebView as diffs. It now holds a diff editor and a plain editor and shows one of them. Each worktree has one reusable file buffer next to its diff buffer. A file on disk that changes while shown gets the same reload banner as a diff.

Image files use an image element in the same WebView, with fit-to-window and actual-size controls. The host reads at most 20 MiB and sends a base64 data URL through the existing bridge. The viewer still has no file or network access. SVG stays in an image element so it uses [SVG's secure image processing mode](https://www.w3.org/TR/SVG/conform.html#secure-animated-mode), with scripts and external references disabled.

Clicking a file path in the terminal opens it in this viewer at the printed line and column, in the worktree of the session that printed it. Emacs stays one click away from the viewer toolbar and the tree's context menu.

## Alternatives considered

### GtkSourceView for files

Rejected. It would add a second highlighting engine, and Monaco already has the navigation interface that later steps need.

### Recursive file monitors

Deferred. Agents change many files quickly, and monitoring a large tree costs more than relisting on the existing refresh.

## Consequences

- `ui.open_file`, `agtkctl ui file` and the MCP `open_file` tool open a file the way a terminal click does.
- Language servers will run in Rust per worktree and language. Monaco providers forward requests over the existing message bridge, so definition results in other files open in the same file buffer.
