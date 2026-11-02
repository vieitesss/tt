# ToTask (tt)

tt is a task manager for the projects you work in. A **project** is a directory
you register; its tasks live in an invisible **store** outside that directory,
one markdown file per task. The `tt` CLI is the public contract. The Neovim
plugin is a human client of the same CLI. Terms defined here are used exactly
in the code, `SKILL.md`, and `docs/adr/`.

## Architecture

- `cmd/tt`: entry point; calls `internal/cli`.
- `internal/cli`: Cobra commands, JSON and human output, exit codes, the
  interactive registration prompts.
- `internal/vault`: one store. Parses and renders task files, builds the tree,
  link, and backlink index, applies mutations, plans and applies a desired tree,
  moves subtrees between stores. Every write goes through this package.
- `internal/project`: the registry (config), project resolution, and git
  worktree mapping.
- `internal/fsutil`: atomic file writes and no-replace creation.
- `lua/tt`, `plugin/tt.lua`: the Neovim client. It renders and parses the
  buffer and calls `tt --json`; it never reads or writes task files itself.

The CLI is the public contract. `--json` shapes are stable and change only
additively. The plugin and agents depend on the same commands
(see [agent-first-cli](docs/adr/agent-first-cli.md)).

## Language

**Task**:
The unit of work. A Task has a title, a state (`open`, `done`, `cancelled`),
optional due date, priority, tags, a body, and an optional parent. Tasks nest
as a strict tree.
_Avoid_: Note, item, entry, todo, checklist item

**Sub-task**:
A Task whose parent is another Task. Sub-tasks are Tasks: they nest, link, and
carry tags.

**Project**:
A directory registered with `tt`. Its tasks live in its Store. Projects are the
unit of scoping: one Store, one task tree.
_Avoid_: vault, workspace, repo, folder

**Registry**:
The mapping of Project paths to Store slugs, plus per-project preferences
(`never_ask_nested`) and global settings (`capture_target`). It is the config
file, `$TT_CONFIG` or `$XDG_DATA_HOME/tt/config.toml` (else
`~/.local/share/tt/config.toml`). Paths under HOME are written as `~/...`.
An existing legacy `$XDG_CONFIG_HOME/tt/config.toml` is migrated on first use
unless `TT_CONFIG` is set. A deprecated `vault` key is warned about, ignored, and
kept on rewrite.
_Avoid_: index, database

**Store**:
The folder that holds one Project's task files: `$XDG_DATA_HOME/tt/<slug>/`
(else `~/.local/share/tt/<slug>/`). Users never organize it; tools write it.
_Avoid_: vault (the library's package name only), directory, data dir

**Identity**:
A Task is identified by the stem of its file, `<id>.md`. The `id` in the
frontmatter is a marker that must equal the stem. Parents and `[[id]]` links
resolve against the stem, so a title edit never re-keys a task. A mismatch is a
Store issue; writes preserve the declared id and never rename a file.
_Avoid_: uid, key, declared id

**Dangling**:
A reference whose target is not loaded. A dangling parent makes the Task a root
and is reported as a Store issue; the task reattaches when the parent appears.
A dangling `[[id]]` link is legitimate (it may point at another project).
_Avoid_: broken, orphan

**Store issue**:
A problem found while reading a Store: an `unreadable` or `malformed` file, an
`id mismatch`, a `dangling parent`, or a `cycle`. Each prints
`warning: <path> (<kind>): <detail>` on stderr; the command still succeeds. A
parent cycle removes its closing edge, and the task becomes a root.

**Link**:
An untyped, directed reference `[[id]]` in a Task body, meaning "related to".
Links and backlinks are computed from bodies; they are never stored.
_Avoid_: dependency, connection

**Alias**:
The display text of a Link. A **mirror** Alias equals the target's title and is
rewritten when the title changes. A **contextual** Alias differs on purpose and
is never rewritten. Identity is always the target id.

**Title sync**:
The one-way update of mirror Aliases in the same Store when a title changes
through tt: `tt edit --title`, `tt edit` in `$EDITOR`, or an Apply that changes
a title. Editing an alias never renames a task. A command that finds a title
changed outside tt (a cold scan) never writes; the alias stays stale until
edited.

**Tag**:
A hierarchical label written as a path (`work/admin` implies `work`). Tags are
labels: they have no state and are not Tasks.
_Avoid_: category, label

**Rank**:
A Task's optional place among its siblings: same parent, or roots together.
`tt list` shows ranked siblings first, lowest rank first, then unranked
siblings by lowercased title and id. Apply sets ranks from the order of the
desired tree. Rank is independent of Priority.
_Avoid_: sort, order, position

**Revision**:
A hash of the Store's identities and exact file bytes. `tt list --json` returns
it; `tt apply` requires the revision it was planned from. If the Store changed
since, Apply writes nothing.

**Apply**:
The command that takes the complete desired task tree and makes the Store match
it. The client sends the tree; Go computes the diff and performs the writes.
See [declarative-apply](docs/adr/declarative-apply.md).

## Project resolution

The starting directory is `--path`, else `$TT_PATH`, else the current
directory. Resolution, in order:

1. Walk up from the start directory, nearest first. The first registered
   Project wins.
2. In a linked git worktree (see [GLOSSARY](GLOSSARY.md): Worktree), a
   registration inside the worktree wins. Otherwise the start directory is
   mapped to the same relative path in the **main checkout** and walked up from
   there.
3. If nothing is registered, the path to register is the main checkout root
   (or the start directory outside git).

Non-git directories use only the ancestor walk. Explicit paths to `project add`
and `project remove` are mapped the same way: a worktree directory means the
same directory in the main checkout, and must exist there.

A directory nested inside a Project resolves to that Project. Interactive
`tt add` may offer to register it separately; `tt project add DIR` does so
explicitly. `never_ask_nested` silences that offer.

Non-interactive `tt add` (`--json`, piped, no TTY) registers an unregistered
directory silently. Read commands and `apply` fail with
`not a registered project; run tt add or tt project add`.

## Neovim plugin

`:TT` resolves the project from the current file's directory (or the working
directory) through `tt --json project current`, then opens the buffer
`tt://<slug>`. The buffer is the editable task tree: `:w` sends the whole tree
to `tt apply --dry-run`, shows the summary, and on confirmation runs
`tt apply`. The plugin stores the revision from `tt list`, so a change made
elsewhere stops the save. The binary is chosen by the `bin` setup option, then
`bin/tt` in the plugin directory, then `tt` on `PATH`.
