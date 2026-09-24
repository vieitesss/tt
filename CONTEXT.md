# ToTask (tt)

A keyboard-driven terminal todo application where tasks form a navigable,
linkable graph. Work is organized into **projects**: a project is a directory
you already work in, its tasks live in an invisible central store, and the TUI
is a list.

## Language

**Task**:
The fundamental unit of the app. Every node in the system is a Task. A Task has
a title, a state, and may have sub-tasks, links, and tags. Deleting a Task
deletes its whole subtree: descendants are never reparented or kept alive, so
keeping them means moving them out first with `m`; `[[id]]` links to the
deleted Task are left dangling.
_Avoid_: Note, item, entry, todo — a "group" is a Task with sub-tasks, and a
Project is something else entirely.

**Project**:
A directory the user works in, registered with `tt`. Paths under HOME are recorded with `~/` notation and interpreted per machine; absolute paths outside HOME are allowed but nonportable. Running `tt` anywhere inside a project resolves to it, and its tasks live in that project's Store.
Projects are the unit of scoping: one store, one task tree, one TUI session.
_Avoid_: vault, workspace, repo, folder

**Registry**:
The shared mapping of Project paths to Store slugs, alongside project preferences. It lives alongside Stores in the tt data root (`$XDG_DATA_HOME/tt/config.toml` or `~/.local/share/tt/config.toml`); `TT_CONFIG` can override its location.
_Avoid_: index, database

**Store**:
The invisible central folder that holds one project's markdown files:
`$XDG_DATA_HOME/tt/<slug>/`, else `~/.local/share/tt/<slug>/`. One file per
Task, named for its Identity and normalized; users never browse or organize the
store.
_Avoid_: vault (the library still calls a folder of task files a vault, but the
user-facing term is store), directory, data dir

**Identity**:
A Task is identified by the stem of its store file; the frontmatter `id` is a
marker that must equal the stem. Parent pointers and `[[id]]` links resolve
against the stem, so editing content can never re-key a task. When the two
differ, the task still loads under the stem, an `id mismatch` issue names both
values, and every write preserves the declared id — tt never renames or
rewrites a file to repair the mismatch.
_Avoid_: uid, key, declared id (the frontmatter value is only a marker)

**Dangling**:
A reference whose target id is not loaded. A dangling parent makes the Task
render as a root (its subtree and rollups stay intact) and is reported as an
issue; if that id appears later, the task reattaches on the next scan because
nothing rewrote the file. Parent cycles are broken deterministically at the
first repeated node and reported the same way. Dangling `[[id]]` links are
legitimate (they may point at another project): they are not badge issues, and
the `o` picker marks them `(missing)`.
_Avoid_: broken, orphan (the file is intact; only the reference is unresolved)

**Store issue**:
A problem the most recent scan found in the Store: a file that will not parse
or read, a declared `id` that disagrees with its filename, a parent reference
that resolves to nothing, or a parent cycle. Issues are sticky state, never
transient: they surface as a badge in the Header and as an overlay listing each
issue's file, kind, and detail.
_Avoid_: vault issue, error, warning, problem

**Resolver**:
The rule that turns a working directory into a Project: start from `--path`,
then `$TT_PATH`, then the current directory; walk ancestors nearest-first; the
first registered project wins. An unregistered directory can become a project
(`tt project add`, an interactive prompt, or a silent register from a
non-interactive `tt add`); the session can instead bind to a different
already-registered project and leave the directory unregistered. A directory
nested inside a project resolves to that project unless the user asks for a
separate one.
_Avoid_: lookup, matcher

**Sub-task**:
A Task contained within another Task. Sub-tasks are first-class Tasks: they can
themselves nest, link, and be tagged, recursively. Containment forms a strict
tree — a Task has at most one parent.
_Avoid_: checklist item

**Link**:
An untyped reference from one Task to another, meaning "related to." Stored
directed, shown bidirectionally. Its written form carries the target file's
relative path and an optional display Alias, so the raw text stays readable and
navigable in any editor. Links stay in the model, the parser, and `--json`, but
they are demoted in the UI: `L` appends one from the selected
Task, the Preview shows only their metadata counts, and the `o` picker jumps
to them; there is no link graph view.
_Avoid_: dependency, connection

**Alias**:
The optional display text of a Link. A **mirror** Alias equals the target's
title and is kept in sync by Title sync; a **contextual** Alias differs from
the title on purpose and is never rewritten. The Alias is display-only: identity
is the target's id, and the UI always shows the live title.
_Avoid_: label, display name

**Title sync**:
The one-way propagation from a renamed Task to the mirror Aliases pointing at
it. It fires when a reload observes a title change, rewrites only mirrors, never
moves in the other direction (editing an Alias never renames), touches only the
owning store, and is idempotent — it writes only on actual mismatch.
_Avoid_: bidirectional sync, rename propagation

**Tag**:
A hierarchical label attached to a Task, written as a path (`#work/admin`
implies `#work`). Tags are pure labels: they have no state, no description, and
are not Tasks.
_Avoid_: category, label

**Filter**:
A session-only lens over the List that keeps Tasks matching one state, priority,
or Tag. Matches appear as independent depth-zero rows regardless of their
parent relationships; clearing the Filter restores the default tree.
_Avoid_: sort, search

**Rank**:
A Task's optional manual place among its siblings: Tasks with the same parent,
or root Tasks together. Rank is independent of Priority and has no meaning
across sibling groups.
_Avoid_: Sort, order, position, priority

**List**:
The only main view: by default, the current project's task tree as an indented
list in pre-order, parents above children; a Filter temporarily flattens its
matching Tasks. A Task whose parent is missing or part of a
cycle renders as a root (see **Dangling**). Each row is a state glyph (`○` open, `●`
done, `—` cancelled), the title, and quiet right-aligned metadata only when
present (relative due, priority, `done/total` rollup); parents also carry a
fold marker. Selection is linear (`j`/`k`, `gg`/`G`); `J`/`K` change the
cursor Task's Rank among its siblings (never across sibling groups, and
disabled while a Filter is active), and `Tab` adds
rows to a multi-selection; `/` searches titles, `p` switches projects, and
`o` jumps through the selected task's links. While anything is marked, `m`
moves the marked tasks under another task (or the root) and `d` deletes them
and all their descendants after a confirmation; `r` renames the cursor task
from a prefilled prompt and cascades mirror aliases in the same action.
Subtrees can be collapsed in memory (`h`/`l`); folds are visual only, never
persisted, and never hide a
search or link target. No
spatial layout, camera, or tab strip.
_Avoid_: map, canvas, graph view, outliner

**Preview**:
The persistent right-hand pane showing the selected Task; it is the larger pane
beside the List, separated from it by a narrow gap. Its first line is quiet,
dim metadata (due, priority, tags — the state is the List glyph), then the
title in a bold accent, then the parsed markdown body — headings, lists,
checkboxes, code, quotes, rules, `[text](url)`, and `[[id]]` wikilinks resolved
to the target's title when it is in this store — or a dim `[empty body]`
placeholder when the body is blank. Tasks that link to the selection follow as
dim `↩ ` lines showing each linker's live title, alphabetical by title; the
pane does not scroll, so an overlong list clips at its edge. It always follows
the List selection; links and backlinks are navigated with `o`. Editing happens
in `$EDITOR` (`e` or `Enter` suspends the TUI), not in the preview.
_Avoid_: detail pane, inspector, editor pane

**Header**:
The one-row bar above the List: the shortened project path (`path_display`
config chooses `home`, `short`, or `tail N`) on the left and the issue badge on
the right.

**Footer**:
The three rows below the List and Preview are a blank spacer, one hint row with
styled key/label pairs for the current mode (or the live input prompt), and a
context row with the current project slug and task count, active Filter, and
sticky `[external change pending]` flag. Transient feedback never lives here;
it is a Toast.
_Avoid_: status line, status bar

**Keymap**:
The complete, on-demand list of every binding, opened with `?`; the store-issues
overlay is opened with `g?`. It is the single reference for how the TUI is
driven, grouped by area; the Footer only hints at the most-used keys, so a
binding missing from the Footer is still findable here.
_Avoid_: help, cheat sheet, shortcuts window

**Toast**:
A small non-modal popup above the Footer for transient action feedback (`added
…`, `done …`, `switched to …`, errors). It never takes focus or blocks a key,
auto-dismisses after a few seconds, and a new message replaces the old one. The
issue badge and `[external change pending]` are sticky state, not toasts.
_Avoid_: notification, status message
