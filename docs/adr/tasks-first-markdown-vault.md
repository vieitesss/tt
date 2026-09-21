# Tasks-first data model stored as a plain markdown vault

The app is tasks-first: every node is a Task, tasks nest recursively (strict tree, one parent max), tasks connect via untyped `[[id]]` links shown bidirectionally, and nested tags (`#work/admin`) are pure labels. "Projects"/"groups" are not separate types — a group is just a Task with sub-tasks. All data lives in a user-owned folder of markdown files (one file per Task, YAML frontmatter for id/state/parent/tags, free markdown body holding links). The vault is the single source of truth; any index is a disposable cache.

## Considered Options

- **Notes-first (Obsidian model)** — rejected: per-task nodes on the map are a core requirement, and tasks-as-checkbox-lines makes tasks second-class.
- **Lightweight checklist sub-tasks** — rejected: two kinds of things; inevitably grows toward first-class sub-tasks anyway.
- **DAG containment (multiple parents)** — rejected: cross-cutting membership is served by tags and links; tree keeps rollups and the outliner model unambiguous.
- **Typed `blocks` dependency links** — deferred: adds cycle detection and "ready" computation; untyped links don't preclude adding it later.
- **SQLite as source of truth** — rejected: opaque data, loses git/grep/editor interop that defines the Obsidian aesthetic. May return as a *rebuildable cache* only.

## Consequences

- Links must use stable IDs, not titles, so renames never break the graph; the TUI renders them as titles.
- The tree lives in frontmatter (`parent:`), not in folder structure, so re-parenting never moves files.
- Every rename/re-parent operation is a text edit to a file the user can also edit externally — external edits must be tolerated (file watching, graceful parse failures).

## v2 amendment

Three parts of the wording above no longer describe the product (see
[project-centric-list-first](project-centric-list-first.md)):

- **"Project" means a registered directory**, not a task group. A group of
tasks is a Task with sub-tasks; a Project owns one Store.
- **The map is deleted.** The List is the only main view; the notes-first
rejection no longer rests on per-task nodes on a map.
- **Links stay but are demoted.** Links and backlinks remain in the model, the
parser, and `--json`; the UI surfaces them only as counts in the Preview plus
the `o` jump picker.

## v2.2 amendment

Identity is the filename stem: the frontmatter `id` is a marker that must equal
it, links and parents resolve against the stem, and a mismatch is reported as
an issue — never a rename or a rewrite.
