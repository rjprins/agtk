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
   task that comes back the next morning stays hidden, read or done. A pending
   comment is kept longer: see 13.
4. Hiding an item moves it to a collapsed Hidden section at the bottom, like
   Inactive Projects in the sidebar. agtk never changes the task, mail or PR.
   Hidden was called removed before; saved briefs with `removed` still load.
5. Read, done and comments are written in the panel only, so the assistant
   cannot mark things as seen or done, or speak for the user. The assistant can
   unhide items with `unhideIds` when they need attention again. It cannot hide
   them. It can only read and clear a comment.
6. Done is a request to the assistant. agtk records when it was set, and the
   assistant completes the Todoist task on its next run. Done also marks the
   item read and hides it, so it moves to Hidden, struck through. Clicking its
   check mark there clears done and brings the item back.
7. Clicking a row's text opens its comment box (see 12). The row's buttons keep
   their own clicks. This used to toggle read, which went unused, so the panel
   no longer shows read: no dot, bold text, Mark all read or unread count. The link and session buttons and done still set `read`, and
   `read` and `unreadCount` stay in the saved brief and in `get_brief`, because
   the assistants' phone page uses them.
8. An item can link to an agent session with `session`: the agtk session id,
   and the provider and conversation id. agtk session ids end with the session,
   but the conversation does not. So the session button first looks for the
   session id, then for a session on the same conversation, and then offers a
   restore, like Resume Session. The restore needs the provider, because agtk
   reads Claude and Codex logs in different places.
9. The links at the top of the page, such as the calendar and Todoist, come
   from the assistant with `links`, not from agtk. A post without `links`
   keeps them, so the assistant sets them once.
10. No keyboard shortcut for now.
11. Each section heading collapses its items, like Hidden. A collapsed heading
    shows how many items it holds, with a dot when a pending comment is out of
    sight. Hidden does the same. agtk keeps
    the collapsed section names in the `briefCollapsedSections` preference.
    Assistants name sections freely, so a section that leaves the brief and
    comes back stays collapsed. Collapsing only changes the view: it sets no
    read, hidden or done marks, and `get_brief` does not report it.
12. A comment tells the assistant what to do with an item, such as "doing the
    review" or "won't do", which done cannot say. Each item has one comment and
    no thread: longer discussions happen in chat. It shows under the item until
    an assistant clears it, so the user can see it is pending. Clicking it edits
    it, and saving it empty removes it. Done stays separate and means finished.
    No wake-up: assistants see comments when they next read `get_brief`, about
    every 15 minutes. An open box outlives re-posts, which rebuild the rows.
13. A pending comment is never lost. A re-post keeps it. A retired item keeps it
    on its retired entry, in `get_brief`, and a re-post restores it. It does not
    expire after 30 days or fall to the limit on remembered items, because only
    an assistant ends it. `clearComments` works on retired items too. Each clear
    names the `commentedAt` the assistant read, and does nothing if that has
    changed. Otherwise an edit made while the assistant works on the old words
    would be wiped without anyone reading it.
14. The tab title shows the time since the last post, such as "Brief (2m)". It
    counts every post, also one that changes nothing, so it tells whether the
    assistants are keeping the brief up. `lastPostedAt` holds it. It is not the
    age of the newest item: that would look stale on a quiet day.
