# Daily brief

An assistant (a Claude session) posts a daily brief to agtk over MCP. The Brief
page in the right sidebar shows it, next to Changes and Files.

Decisions:

1. The brief is a list of items, not one block of text. Each item has a stable
   id from its source, so a re-post updates the item instead of adding a copy.
2. agtk stores the brief as the `brief` preference in its own database, like PR
   attention. MCP and `agtkctl` are the only ways in.
3. The assistant decides what leaves the brief: `replaceSections` and
   `retireIds`. Nothing expires per day. A section with no items leaves the
   brief. When an item leaves, agtk keeps its
   read, hidden and done marks for 30 days, only if one of them is set. So a
   task that comes back the next morning stays hidden, read or done.
4. Hiding an item moves it to a collapsed Hidden section at the bottom, like
   Inactive Projects in the sidebar. agtk never changes the task, mail or PR.
   Hidden was called removed before; saved briefs with `removed` still load.
5. Read and done are set in the panel only, so the assistant cannot mark things
   as seen or done for the user. The assistant can unhide items with
   `unhideIds` when they need attention again. It cannot hide them.
6. Done is a request to the assistant. agtk records when it was set, and the
   assistant completes the Todoist task on its next run. Done also marks the
   item read and hides it, so it moves to Hidden, struck through. Clicking its
   check mark there clears done and brings the item back.
7. Clicking a row marks it read, and clicking it again marks it unread. The
   link has its own button, so reading an item does not open a browser.
8. An item can link to an agent session with `session`: the agtk session id,
   and the provider and conversation id. agtk session ids end with the session,
   but the conversation does not. So the session button first looks for the
   session id, then for a session on the same conversation, and then offers a
   restore, like Resume Session. The restore needs the provider, because agtk
   reads Claude and Codex logs in different places.
9. No keyboard shortcut for now.
