# Links carry the target's path plus a display alias, and mirror aliases sync with titles

Links were stored as bare `[[id]]`: unreadable in `$EDITOR` (the id says nothing)
and unnavigable without editor-specific setup. We decided the *file format* must
carry both readability and navigability, with zero editor-specific machinery. A
link is now written `[[{id}.md|{Title}]]` — the target as a real relative path
(the store is flat, so the path is always just the filename, and `gf`-style
navigation works out of the box) plus an optional alias that makes the raw text
human-readable. The parser accepts and keeps accepting every older form
(`[[id]]`, `[[id|alias]]`, `[[id.md]]`); identity is still the filename stem, so
`id.md` normalizes to `id`. The id remains the contract; the alias is
display-only and the TUI always resolves the live title.

Aliases sync one-way with titles: when a reload observes a title change (the TUI
still holds the previous index, so it can diff), every alias that *equaled the
old title* is a **mirror** and is rewritten to the new one; aliases that differ
are **contextual** and are never rewritten. Editing an alias never renames a
task. Sync is idempotent (writes only on actual mismatch, so the watcher cannot
loop), touches only links whose target lives in the same store (cross-project
dangling links are exempt), and has no cascade cap: renaming a task linked from
N files rewrites those N files atomically. A cold scan never writes — with no
prior index there is no diff and tt never guesses.

## Considered Options

- **Standard markdown links `[Title](id.md)`** — rejected: navigable everywhere
  including GitHub, but a second link dialect and a larger parser/index change;
  the wikilink convention was already established and Obsidian-compatible.
- **Editor-specific config snippets/plugins** — rejected (user): the format
  itself must carry navigability; shipping per-editor config is a maintenance
  surface with no home in tt.
- **Bidirectional sync (editing an alias renames the target)** — rejected: from
  a static vault you cannot distinguish "title edited" from "alias edited", and
  it destroys contextual aliases (a deliberately different `[[id|the milk
  errand]]` would silently rename the task).
- **Auto-syncing contextual aliases too** — rejected: stomps hand-written
  display text; the alias is the user's text, tt only maintains mirrors.

## Consequences

- `L` writes the full `[[{id}.md|{Title}]]` form; titles containing `|` or `]`
  fall back to the bare path form.
- Vaults full of bare `[[id]]` keep working unchanged; there is no migration —
  aliases appear as links are created or as renames cascade.
- The alias shown in raw text can go stale when a title changes outside any
  running tt session (cold scans never write), and a later cold session does
  not repair it either: only a running session that already held the old
  title rewrites mirrors on its next reload. The TUI preview and `o` picker
  are always truthful because they resolve live.
- Title renames become multi-file writes; each is atomic and idempotent, so a
  completed cascade leaves the next reload nothing to write. A write that
  fails leaves that alias stale until it is edited by hand.
