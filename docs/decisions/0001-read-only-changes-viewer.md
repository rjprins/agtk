# ADR-0001: Embed a read-only worktree changes viewer

## Status

Accepted

## Date

2026-09-24

## Context

The workspace needs a quick way to inspect what each coding agent has changed. The left sidebar must continue to list individual agent sessions. Selecting a session supplies its worktree context to the center tabs and the optional right Changes sidebar. Agents remain responsible for Git operations.

The desired review surface includes source line numbers, character-level change highlights, and change markers along the scrollbar. The existing Emacs diff view does not provide the preferred reading experience. Embedding Emacs would also couple the viewer's lifecycle and presentation to the editor process.

## Decision

Use Git's read-only commands to gather worktree, index, branch, and commit comparisons. Render text comparisons in one lazily created WebKitGTK view with bundled Monaco assets. Keep native GTK widgets for navigation and status. Do not add Git mutation controls or edit operations.

The left sidebar remains session based. Center tabs represent sessions and file diffs. The right Changes sidebar follows the selected session's recorded worktree. Live diffs remain stable while being read and offer an explicit reload when their source changes.

## Alternatives considered

### Embed Emacs

Rejected because its diff view is not the desired visual experience and because it would make review depend on an external editor window or process.

### Build a native text diff renderer

Deferred because matching syntax highlighting, character-level changes, source line numbers, and overview markers would require a separate rendering project. Monaco already provides those reading features.

### Use a patch-only or Git-controls panel

Rejected because the goal is to browse changes over time and inspect files without adding Git actions that agents already perform.

## Consequences

- The application bundles Monaco and its workers into the native binary through GResource. No local server or network access is needed at runtime.
- A single WebKit view serves all diff tabs. Per-tab state stays lightweight, and old text models are disposed when switching files.
- Git and file reads run off the GTK thread with bounded output, time, and content size.
- Git history, staged content, unstaged content, and untracked files have distinct comparison scopes.
- This adds a WebKitGTK dependency and Node.js/npm build tooling for preparing viewer assets. Node.js is not needed to run an already built application.
- The first version is read only. Edit actions, Git controls, saved checkpoints, and editor keybinding emulation remain outside scope.
