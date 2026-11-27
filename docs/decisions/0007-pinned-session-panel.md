# Pinned session panel

One session can be pinned to the right sidebar. Its terminal shows in a panel
under the sidebar's pages, next to whichever session is selected. It is meant
for an assistant session that the user wants in view all day.

Decisions:

1. The panel is part of the right sidebar, under the pages. It is not a fifth
   page: a page would hide the Brief while the assistant shows, and the two
   belong together, because the assistant posts the brief. The line between the
   pages and the panel can be dragged. agtk keeps the panel's share of the
   height in the `pinnedPanelShare` preference, so it fits any window size.
2. One session is pinned per agtk instance, kept in the `pinnedSessionId`
   preference. An instance has one window and one right sidebar. A pin per
   project would make the panel change with the selected session, which is not
   what an assistant needs.
3. The terminal moves from the main area to the panel, because a GTK widget has
   one parent. So a pinned session is never the selected session. Its row stays
   in the sidebar, cannot be selected, and leads to the panel. The session
   leaves the tabs of its worktree in the main area.
4. Pinning another session moves the first one back. Unpinning in the window
   selects the session in the main area. Unpinning over `agtkctl` or MCP does
   not take the selection, like other changes a script makes.
5. Restart Agent and a resume after a reboot start a new session with a new id.
   The new session takes over the panel. agtk passes this on together with the
   row's place, so the pin does not need a stable id.
6. At startup the pin does not open a closed sidebar. Pinning a session, and
   going to the pinned session, do open it.
7. A finished turn in the panel stays ready until the user clicks or types in
   the panel. The panel is always in view, so the 7 seconds that count for a
   selected session would mark every turn as viewed. A turn that finishes while
   the keyboard is in the panel is viewed at once.
8. Copy, paste and Close selected session follow the keyboard: they act on the
   pinned session while its panel has the focus. Search, prompt history and the
   Claude model preset stay with the selected session for now.
9. A file path clicked in the panel opens in the main area, with the tabs of
   the selected session. `agtkctl ui diff` refuses a pinned session, because
   its worktree's diff tabs are no longer in the main area.
10. In a narrow window libadwaita lays the right sidebar over the main area,
    which then cannot take the keyboard. So going from the panel to a session
    in the main area closes the sidebar there.
11. No keyboard shortcut for now. The row, a click in the panel,
    `select_session` and Next ready session all go to the panel.
12. The panel has a text size of its own, `pinnedFontSize` in the appearance
    settings. The panel is narrow, so its terminal often needs a smaller font
    than the main area. Ctrl+scroll over the panel changes only this size.
    It has no value until the user sets one: the panel then uses the size
    of the terminal font. The size belongs to the panel, not to the session,
    so an unpinned session gets the terminal size back. Ctrl++ and Ctrl+-
    change every size, and take this one along once it is set.
13. A nudge is one short message above the panel's header, in the accent
    color. It is for an assistant that wants to interrupt the user. It opens
    the right sidebar, because an interrupt that nobody sees is no use. Only
    one shows at a time, and it stays until the user closes it or the caller
    clears it. It belongs to the panel: it is refused when no session is
    pinned, and it goes away when the panel empties or agtk restarts. A caller
    that needs to know if the user saw it reads `nudge` from the state.
