# PR comments for running agents

## Status

Revised on 2026-11-11. agtk first sent new PR comments to linked agents on its
own. It now shows the comments and lets the user send one at a time.

## Decision

The unresolved thread count in the PR bar above a session opens a popover with
the PR's comment threads. agtk fetches them from Azure DevOps when the popover
opens and does not store their text. Open threads come first; each shows its file
and line and its comments with author and age.

In an agent session each comment has a send button. It asks the agent to look into
that comment, quoting its text and naming the PR, thread, file, line and author.
The text is marked as untrusted input, and control characters are replaced so a
comment cannot inject terminal sequences.

The request is submitted once the agent rests at its prompt: it waits while the
agent works or shows a permission dialog. Text in the input box does not hold it
back; the request is added after it and sent with it. Unlike the old automatic
delivery, it does not wait for the user to leave a focused terminal, because the
user asked for it.

Every comment also has a reply button. It opens a text box under the comment;
Reply, or Ctrl+Enter, posts the text to that comment's thread in Azure DevOps as
the signed-in az account, with the comment as its parent. az takes a request body
only from a file, so the reply goes through a private temporary file that is
removed after the call. A failed post keeps the text in the box. After a post the
threads are fetched again, so the reply shows up in the list.

## Why not automatic delivery

Not every comment is for the agent in a session. Sending per comment lets the user
decide what reaches which agent, and the agent works from the text it was sent
instead of fetching all threads itself.

Because nothing is delivered in the background any more, PR polling runs only
while an agtk window has focus, as it did before automatic delivery.

Comment fields follow the
[Azure DevOps PR threads schema](https://learn.microsoft.com/en-us/rest/api/azure/devops/git/pull-request-threads/list?view=azure-devops-rest-7.1),
and replies the
[PR thread comments create API](https://learn.microsoft.com/en-us/rest/api/azure/devops/git/pull-request-thread-comments/create?view=azure-devops-rest-7.1).
