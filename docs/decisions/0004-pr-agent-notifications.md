# PR activity notifications for running agents

Azure DevOps PR polling also watches PRs linked to live Claude, Codex and Gemini
sessions, including the author's worktree and detached review checkouts. While
these agents are open, polling continues every two minutes without window focus.
The agtk application must remain running; session hosts do not poll on their own.

Implementation steps and acceptance criteria:

1. Normalize individual comment IDs and content update times from the existing
   thread request. Keep unavailable thread data distinct from an empty response.
   Verify with the fake Azure CLI and normalization tests.
2. Track comments per session. The first successful snapshot is a baseline. Queue
   and coalesce subsequent changes, ignore comments by the signed-in user to avoid
   reply loops, and persist cursors and pending updates. Verify deduplication,
   independent sessions, failures and restart recovery. Pushes and votes do not
   notify: a push is usually the agent's own, and an agent needs to act on what
   people write, not on PR state.
3. Route updates through existing PR associations and agent terminal input. Only
   submit at a resting prompt, defer permission dialogs and busy agents, and keep
   focused terminals available for the user. Text in the input box does not defer
   an update, which is added after it and sent with it: most of that text is a
   suggested prompt, and on screen a suggestion reads the same as a draft.
   Notifications identify the PR and comment threads; agents fetch the current
   details themselves. Verify routing and delivery in native UI tests.
4. Let the user turn this off per session with the bell in the PR bar above the
   agent. A muted session drops its queued update and keeps its baseline current,
   so turning it back on only sends comments made after that. The choice is saved
   and outlives the agent disconnecting.

Automatic reviews of newly published PRs remain a separate feature. Polling does
not launch agents, change checkouts, or post anything to Azure DevOps.

Comment identifiers and content update times follow the
[Azure DevOps PR threads schema](https://learn.microsoft.com/en-us/rest/api/azure/devops/git/pull-request-threads/list?view=azure-devops-rest-7.1).
