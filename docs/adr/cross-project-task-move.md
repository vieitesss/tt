# Move task subtrees between registered Projects without changing Identity

A Task may be reparented within a Project or moved between registered
Projects, but Identity is the stable link and parent reference contract. A move
carries the whole selected subtree with the same ids and bodies; descendants
keep their parent pointers. A cross-Project move gives each moved root the
chosen destination parent (or no parent) and clears its Rank; an in-Project
move uses `Vault::set_parent` to append the root among its new siblings and
close the old rank gap. Links are never rewritten: references left behind in
the source become legitimate Dangling links, and Title sync remains
Store-local.

In-Project reparenting is available from both the CLI and the TUI: `tt move`
uses `Vault::set_parent`, while the TUI `m` flow offers the interactive picker.
Passing the current Project as `--to-project` is treated as an in-Project move.
The TUI's cross-Project `m` flow chooses a registered Project, then a Task there
or its root, and switches the List to the destination on success.

## Decision

Write the complete subtree to the target Store first, using atomic create-without
replacement for each id. Only after all target writes succeed may the source
files be removed. A target id collision aborts before any source removal. If a
filesystem error interrupts either phase, report the number of target writes and
source removals; duplicates are preferable to losing a Task.

When several Tasks are marked, moving a marked ancestor carries its entire
subtree and subsumes any marked descendants, so each subtree moves once.

## Considered options

- **Rewrite `[[id]]` links to point across Projects** — rejected: identity is
  global and ids do not change; links remain meaningful as legitimate dangling
  references when the target is not loaded in the current Store.
- **Remove from the source before writing the destination** — rejected: a
  failed target write could lose the only copy.
- **Overwrite a target file with a matching id** — rejected: that would destroy
  a different Task. Existing ids, including ids in unreadable or malformed
  files, block the move.
- **Keep `tt move` cross-Project only** — rejected: the CLI should expose the
  same in-Project reparenting available in the TUI, with `Vault::set_parent`
  retaining the tree invariants and sibling-rank behavior.

## Consequences

- `tt move <ID> --parent ID` reparents within the current Project;
  `tt move <ID> --root` makes it a root. These forms require exactly one of
  `--parent` or `--root`. `--to-project DIR|SLUG` accepts only registered
  destinations; without a parent, the moved root is a destination root, and
  `--root` is equivalent to omitting `--parent`. A current-Store destination
  is handled as an in-Project move.
- JSON returns every Task in the moved subtree, ordered by Identity, plus the
  target Project reference for either move kind. Moved roots have a null Rank
  for cross-Project moves; in-Project moves use `set_parent` to append among
  destination siblings and close the old rank gap.
- An interrupted move can leave duplicate copies. The error reports commit
  progress, and retrying may require removing the extra target copies manually.
- A move is not a rename: task bodies and link aliases are not rewritten or
  synchronized as a side effect. Only a moved root's parent and Rank change.
