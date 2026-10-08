# PR activity notifications for running agents

Azure DevOps PR polling also watches PRs linked to live Claude, Codex and Gemini
sessions, including the author's worktree and detached review checkouts. While
these agents are open, polling continues every two minutes without window focus.
The agtk application must remain running; session hosts do not poll on their own.

Implementation steps and acceptance criteria:

1. Normalize individual comment IDs and content update times from the existing
   thread request. Keep unavailable thread data distinct from an empty response.
   Verify with the fake Azure CLI and normalization tests.
2. Track the head commit and comments per session. The first successful snapshot
   is a baseline. Queue and coalesce subsequent changes, ignore comments by the
   signed-in user to avoid reply loops, and persist cursors and pending updates.
   Verify deduplication, independent sessions, failures and restart recovery.
3. Route updates through existing PR associations and agent terminal input. Only
   submit at a resting prompt, defer permission dialogs, busy agents and drafts,
   and keep focused terminals available for the user. Notifications identify the
   PR, changed head and comment threads; agents fetch the current details and
   update their checkout themselves. Verify routing and delivery in native UI tests.

Automatic reviews of newly published PRs remain a separate feature. Polling does
not launch agents, change checkouts, or post anything to Azure DevOps.

Comment identifiers and content update times follow the
[Azure DevOps PR threads schema](https://learn.microsoft.com/en-us/rest/api/azure/devops/git/pull-request-threads/list?view=azure-devops-rest-7.1).
Only the presence of a terminal draft is saved; its text stays in the agent.
