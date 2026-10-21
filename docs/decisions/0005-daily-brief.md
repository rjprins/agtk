# Daily brief

An assistant (a Claude session) posts a daily brief to agtk over MCP. The Brief
page in the right sidebar shows it, next to Changes and Files.

Decisions:

1. The brief is a list of items, not one block of text. Each item has a stable
   id from its source, so a re-post updates the item instead of adding a copy.
2. agtk stores the brief as the `brief` preference in its own database, like PR
   attention. MCP and `agtkctl` are the only ways in.
3. The assistant decides what leaves the brief: `replaceSections` and
   `retireIds`. Nothing expires per day. When an item leaves, agtk keeps its
   read, removed and done marks for 30 days, only if one of them is set. So a
   task that comes back the next morning stays hidden, read or done.
4. Removing an item only hides it. agtk never changes the task, mail or PR.
5. Read, removed and done are set in the panel only. There is no MCP tool for
   them, so the assistant cannot mark things as seen or done for the user.
6. Done is a request to the assistant. agtk records when it was set, and the
   assistant completes the Todoist task on its next run. Clicking the check mark
   again clears it. Done also marks the item read.
7. Clicking a row marks it read. The link has its own button, so reading an
   item does not open a browser.
8. No keyboard shortcut for now.
