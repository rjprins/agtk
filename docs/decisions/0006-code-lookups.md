# Go to definition and find references

The file and diff viewer can go to the definition of a name and list its
references in Python, TypeScript and JavaScript.

Decisions:

1. agtk runs real language servers: pyright for Python, and TypeScript's own
   server for TypeScript and JavaScript. Monaco's own TypeScript support only
   sees the one file the viewer shows, and it has nothing for Python.
   Searching with ctags or `git grep` guesses: for `self.save()` it finds
   every `save` in the project.
2. The servers run in agtk, not in the viewer page. The page sends only a line
   and column. agtk knows which file and worktree the tab shows. So the page
   still cannot read files or reach the network, as ADR-0001 decided.
3. This changes one part of ADR-0001: the viewer now starts other programs.
   pyright only reads the code. For TypeScript agtk runs the TypeScript in the
   project's `node_modules`, and its plugins. These are your own worktrees, so
   agtk accepts that.
4. For TypeScript agtk prefers the project's own TypeScript, in the worktree
   or the main checkout. TypeScript 7 has its own server, `tsc --lsp`. Older
   versions need typescript-language-server. agtk turns off its syntax-only
   server: it answers while the project loads, and its definitions stop at
   the import. Without a project TypeScript, agtk uses a global TypeScript 7,
   then typescript-language-server with the TypeScript installed beside it,
   then vtsls.
5. agtk starts one server per worktree and language, on the first lookup. It
   stops the server after ten minutes without lookups, and when the worktree is
   removed. Each server can use a few hundred MB, and there are many
   worktrees.
6. agtk sends the server the text the viewer shows. In a diff this is the new
   version, which can be a commit or the index. So positions match what you
   see. Other files come from the worktree on disk, so a jump from an old
   commit lands in today's version of the other file.
7. agtk keeps one document open per server. Opening another one closes the
   previous one, so the server reads that file from disk again.
8. Python lookups use the worktree's `.venv`. Worktrees that agents create
   often have none, so agtk then uses the `.venv` of the main checkout.
9. A definition in the shown file moves the cursor. A definition in another
   file opens it in the worktree's file tab. agtk only opens files that a
   lookup returned.
10. Find references uses Monaco's peek view. Peek can only show files it has
    text for, so agtk sends the text of the other files with the answer, up to
    100 files and 8 MB.
11. `Alt+Left` and `Alt+Right` go back and forward through jumps, per worktree.
    agtk had no history of places before. Without it, a jump to another file
    leaves you with no way back to where you were.
12. Only lookups at the cursor show messages, such as a missing server. Holding
    Ctrl over a name also looks it up, and that should stay quiet.
