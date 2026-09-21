---
name: tt
description: Operate a tt (ToTask) project from the CLI. Use when adding, listing, showing, completing, cancelling, reopening, or renaming tasks with `tt`; when registering projects with `tt project`; when scripting or bulk-editing a project with `tt --json`; or when reading and hand-editing the one-file-per-task markdown stored in a project store.
---

`tt` is a keyboard-driven terminal todo app organized into **projects**: a
project is a directory registered with `tt`, and its tasks live in an invisible
central **store** of plain markdown files, one file per task. The CLI covers
the full task lifecycle; the TUI (what `tt` opens with no subcommand) and the
CLI are clients of the same library and write identical files. Design rationale
lives in `CONTEXT.md` and `docs/adr/`.

## Invocation

```
tt [--path PATH] [--json] <command>
```

Project resolution, first match wins:

1. `--vault PATH` / `$TT_VAULT` — hidden escape hatch: open that folder
   directly and skip the registry.
2. `--path PATH`, else `$TT_PATH`, else the current working directory.

From the starting directory, `tt` walks ancestors nearest-first; the first
registered project wins. Consequences:

- A directory inside a registered project resolves to that project.
- An unregistered directory: `tt add` registers it silently when
  non-interactive (`--json`, piped, or not a TTY) and prompts otherwise; read
  commands fail with `not a registered project; run tt add or tt project add`.
- A directory nested inside a project must be registered explicitly
  (`tt project add`) to become its own project.

Paths:

- Config/registry: `$TT_CONFIG`, else `$XDG_CONFIG_HOME/tt/config.toml`, else
  `~/.config/tt/config.toml`. A missing config is fine; an invalid one is an error.
- Store: `$XDG_DATA_HOME/tt/<slug>/`, else `~/.local/share/tt/<slug>/`.

```toml
capture_target = "abc1234567" # default parent for `tt add` without --parent

[path_display]
style = "short"               # "home" | "short" | "tail"
tail = 1

[[project]]
path = "/home/me/work/app"
slug = "app"
never_ask_nested = false
```

A legacy `vault = "..."` key is deprecated: `tt` warns on stderr, ignores it,
and preserves it when the config is rewritten.

## Output contract

- Pass `--json` and parse stdout; the shapes below are stable. Human output is not.
- Success: exit `0` with the JSON object on stdout.
- Runtime failure: exit `1`. With `--json` the error is
  `{"error":"<message>"}` on stdout; human mode prints `error: <message>` to stderr.
- Usage errors (unknown flag, bad enum value): exit `2` with usage on stderr, never JSON.
- Store problems print `warning: <file> (<kind>): <detail>` on stderr and never
  fail the command.

### Task object

Every task-producing command returns this shape:

```json
{"id":"bzfca8bm5j","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null}
```

- `id` — the task's identity: the stem of its store file (10 characters from
  `[0-9a-z]` when `tt` creates it). Use it in `[[id]]` links; the frontmatter
  `id` must equal it. A mismatch is warned about as `id mismatch` and the task
  still keys by the filename, so `tt` never renames or rewrites a file to
  "fix" a user edit.
- `state` — `open` | `done` | `cancelled`.
- `parent` — parent task id or `null` (strict tree, one parent max).
- `tags` — without a leading `#`; a `work` filter also matches `work/admin`.
- `due` — `YYYY-MM-DD`, date only. `priority` — `high` | `med` | `low`.

## Commands

Examples run in a fresh project; ids and the project path are illustrative.

### `tt add`

```
tt add TITLE | --title TITLE [--parent ID] [--tag TAG]... [--due YYYY-MM-DD]
       [--priority high|med|low] [--body TEXT|-]
```

Creates a task and returns it. Pass the title positionally or via `--title`, not
both. `--body -` reads the body from stdin; bodies may contain `[[id]]` links.
Without `--parent`, a configured `capture_target` is used, else the project root.
On an unregistered directory a non-interactive `tt add` registers it first.

```console
$ mkdir -p /tmp/tt-demo/project && cd /tmp/tt-demo/project
$ export TT_CONFIG=/tmp/tt-demo/config.toml XDG_DATA_HOME=/tmp/tt-demo/data
$ tt --json project add
{"path":"/tmp/tt-demo/project","slug":"project"}
$ tt --json add --title "Write the plan"
{"id":"8d7jfebh28","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null}
$ tt --json add "Ship it" --parent 8d7jfebh28 --tag work/admin --due 2026-09-17 --priority high --body "See [[8d7jfebh28]]."
{"id":"3wcss7oxg4","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high"}
$ printf 'body from stdin\n' | tt --json add "Stdin task" --body -
{"id":"yubpl8hb0y","title":"Stdin task","state":"open","parent":null,"tags":[],"due":null,"priority":null}
```

### `tt list`

```
tt list [--flat] [--tag TAG] [--state open|done|cancelled] [--due-today]
```

Tree by default: `{"tasks":[{...task,"children":[...]}]}`. A matching task whose
parent does not match is promoted to a root; ancestors of matches are excluded.
`--flat` returns `{"tasks":[{...task}]}` sorted by id, without `children`.
`--due-today` includes overdue tasks; filters combine with AND.

```console
$ tt --json list --flat --tag work
{"tasks":[{"id":"3wcss7oxg4","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high"}]}
$ tt --json list
{"tasks":[{"id":"yubpl8hb0y","title":"Stdin task","state":"open","parent":null,"tags":[],"due":null,"priority":null,"children":[]},{"id":"8d7jfebh28","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null,"children":[{"id":"3wcss7oxg4","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high","children":[]}]}]}
```

### `tt show <ID>`

Task object plus `body`, `links`, `backlinks`, and `children` (arrays of ids).
Dangling link targets are allowed and reported as-is.

```console
$ tt --json show 3wcss7oxg4
{"id":"3wcss7oxg4","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high","body":"See [[8d7jfebh28]].","links":["8d7jfebh28"],"backlinks":[],"children":[]}
$ tt --json show 8d7jfebh28
{"id":"8d7jfebh28","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null,"body":"","links":[],"backlinks":["3wcss7oxg4"],"children":["3wcss7oxg4"]}
```

### `tt done <ID>` / `tt cancel <ID>` / `tt reopen <ID>`

Set the state (`reopen` sets `open`) and return the task object.

```console
$ tt --json done 3wcss7oxg4
{"id":"3wcss7oxg4","title":"Ship it","state":"done","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high"}
$ tt --json reopen 3wcss7oxg4
{"id":"3wcss7oxg4","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high"}
```

### `tt edit <ID> [--title TITLE]`

With `--title`, renames the task; the id and file never change. Without
`--title`, opens the task file in `$EDITOR` (fallback `vi`) and returns the
task object after the editor exits. Use `--title` in scripts; the interactive
form is for humans.

```console
$ tt --json edit 3wcss7oxg4 --title "Ship it v2"
{"id":"3wcss7oxg4","title":"Ship it v2","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high"}
```

### `tt project list` / `tt project add [DIR]` / `tt project remove DIR|SLUG`

Register, list, and unregister directories. `add` defaults to the resolved
starting directory and creates the project's store folder. `remove` only
unregisters; store files are kept.

```console
$ tt --json project list
{"projects":[{"path":"/tmp/tt-demo/project","slug":"project","never_ask_nested":false}]}
$ tt --json project add /tmp/tt-demo/other
{"path":"/tmp/tt-demo/other","slug":"other"}
$ tt --json project remove other
{"removed":{"path":"/tmp/tt-demo/other","slug":"other","never_ask_nested":false}}
```

### Unregistered reads

```console
$ mkdir -p /tmp/tt-demo/unregistered && cd /tmp/tt-demo/unregistered
$ tt --json list
{"error":"not a registered project; run tt add or tt project add"}
$ echo $?
1
```

## TUI

Bare `tt` opens the list for the resolved project: one indented row per task
in pre-order, with tree guides, a fold marker on parents, a state glyph, and
quiet right-aligned metadata only when present (relative due, priority, and the
`done/total` rollup). The header shows the project path (never the invisible
store) and the issue badge. The footer is three rows: the first two show key
hints for the current mode (or the live input prompt on the first), and the
last the project slug and task count. Short-lived confirmations and errors
appear as a toast popup above the footer that dismisses itself after about
three seconds and never takes focus.
The list is the smaller pane; the larger right-hand preview follows the
selection: a dim metadata line (due, priority, tags — the list glyph already
shows the state), the title in bold accent, then the parsed markdown body
(headings, lists, code, quotes, rules, and `[text](url)` links plus `[[...]]`
wikilinks in every accepted form, shown as the target's live title when it is
in this store; a dangling target shows its id, and an alias is never
displayed). An empty body shows a dim `[empty body]` placeholder. Tasks that
link to the selection are listed after the body as dim `↩ Title` lines,
alphabetical by live title; a list taller than the pane clips at its edge,
because the pane does not scroll.
Link and backlink navigation stays on `o`.

| Key | Action |
| --- | --- |
| `j`/`k`, `↓`/`↑` | Move the selection through the flat list |
| `gg`/`G` | Jump to the first/last row |
| `h`/`←` | Collapse the selected parent; if already collapsed or childless, jump to its parent |
| `l`/`→` | Expand the selected collapsed parent (no-op otherwise) |
| `a`/`A` | Add a child under / a sibling of the selection |
| `N` | Quick capture to the configured `capture_target`, else the project root |
| `x` | Cycle the selection (or every marked task) open ↔ done; cancelled tasks are left unchanged |
| `Tab` | Toggle the current row in the multi-selection; `Esc` clears the selection first |
| `m` | Move the selection (or every marked task) under another task or `⌂ root` |
| `d` | Delete the selection (or every marked task) and its descendants after confirming |
| `L` | Append a `[[{id}.md|{Title}]]` link from the selected task to another task |
| `r` | Rename the selected task from a prompt prefilled with its current title; committing rewrites the title and immediately cascades mirror aliases in other tasks |
| `e`/`Enter` | Open the selected task in `$EDITOR` |
| `/` | Search titles; `Enter` selects, `Esc` restores the previous selection |
| `p` | Switch to another registered project |
| `P` | Register a new project by path (expands `~`, creates the directory, and switches to it) |
| `o` | Jump through the selection's links and backlinks; unresolvable outlinks are marked `(missing)` and `Enter` toasts `not found` instead of jumping |
| `?` | Open the store-issues overlay |
| `q` | Quit |

`Tab` builds a multi-selection; marked rows show a `▪` marker in the reserved
left gutter over a yellow background, and the footer switches to selection
hints. While anything is marked, `m`, `d`, and `x` act on every marked task;
with nothing marked they act on the selection alone.
`m` opens a `move under…` picker offering `⌂ root` plus every task outside the
moving set and its descendants; committing unfolds the new parent and selects
the first moved task. `d` opens a confirmation: one task asks
`Delete "<title>"?` and several ask `Delete N tasks?`, adding
`and its K descendants` or `and their K descendants` when the subtrees hold
more than the requested tasks. `[ Delete ] [ Cancel ]` are highlighted with
Cancel default, so a bare `Enter` never deletes; `y` (or `d`) confirms and
`Esc` (or `n`) cancels. Deleting removes a task and all its descendants —
nothing is reparented, so move children up first with `m` if you want to keep
them — and `[[id]]` links to deleted tasks are left dangling. `L` appends a
`[[{id}.md|{Title}]]` link from the selected task through a `link to…` picker,
ignoring the multi-selection; a title containing `|` or `]` cannot carry an
alias, so the bare `[[{id}.md]]` form is written instead. `r` opens a prompt
prefilled with the cursor task's title (marking is ignored, and marks survive);
`Enter` rewrites the title and immediately cascades mirror aliases in the same
action, because the TUI already knows the old and new title, while `Esc`
cancels with no writes. Renaming a task by any route while the TUI is running
cascades to mirror aliases (see Title sync); there is no unlink command yet.

Folds are in-memory only: they are never persisted, and a new session starts
fully expanded. Rollups keep counting folded children, and search, link jumps,
and any other programmatic selection unfold the target's ancestors, so a folded
task can never become unreachable.

While the issues overlay is open: `j`/`k` move between issues, `e`/`Enter`
opens the highlighted file in `$EDITOR` (the vault reloads on return, and once
the issues are fixed the overlay closes and confirms with a toast), `Esc`/`?`
closes it, and `q` quits. If an editor round-trip raises the issue count, the
TUI immediately toasts `⚠ N issues — press ?` so a content edit that broke a
file cannot go unnoticed.

On launch, an unregistered directory offers registration and a nested
directory offers to become its own project. Both questions show highlighted
buttons: `←`/`→` (or `h`/`l`/Tab) move the highlight, `Enter` confirms, and
`Esc` (or the Cancel button) quits without opening anything. The `y`/`n` keys
still answer directly, and `!` on the nested question keeps the parent and
stops asking there. The register question defaults to Yes; the nested question
defaults to Register parent, so `n` (or Register parent) still opens the parent
project.

## On-disk format

One file per task at `<store>/<id>.md`, where `<store>` is
`$XDG_DATA_HOME/tt/<slug>/` (else `~/.local/share/tt/<slug>/`):

```markdown
---
id: 3wcss7oxg4
title: Ship it
state: open
parent: 8d7jfebh28
tags:
- work/admin
due: 2026-09-17
priority: high
---
See [[8d7jfebh28]].
```

- The filename stem is the task's identity. The frontmatter `id` must match
  it; a mismatch is reported as `id mismatch` and preserved verbatim on
  rewrite. Files are never renamed automatically, and `tt` never rewrites a
  file to repair a mismatch.
- A `parent` id that is missing (or forms a cycle) makes the task render as a
  root and is reported as `dangling parent` or `cycle`; if the missing parent
  later appears, the task reattaches on the next scan because nothing rewrote
  the file. `[[id]]` links may dangle (for example cross-project); they are
  not warnings.
- Rewrites preserve unknown frontmatter keys and are atomic (temp file + rename).
- `[[...]]` links are extracted from the body; backlinks are computed, never
  stored. Link by id (the filename stem), not by title or alias.
- A `.md` file is a task only if its frontmatter parses and declares an `id`.
  Files without a frontmatter block, or with frontmatter but no `id` (notes,
  skills, editor documents), are ignored silently. A file that claims an `id`
  but fails to parse is skipped and warned about; it is never rewritten.
- Users never browse the store; prefer `tt --json` for changes so the index
  stays consistent.

### Links

A link is written `[[{id}.md|{Title}]]`: the target's real store filename (the
store is flat, so `gf`-style navigation works in any editor) plus a display
alias. The parser accepts the older and shorter forms too — `[[id]]`,
`[[id|alias]]`, `[[id.md]]`, `[[id.md|alias]]`, and `.md`-suffixed paths — and
normalizes them all to the target's filename stem, which is and remains the
identity. Aliases and paths never affect resolution, and `--json`
`links`/`backlinks` stay id arrays. `L` writes the full form; when the title
contains `|` or `]` it falls back to the bare `[[{id}.md]]`.

### Title sync

While the TUI is running, a reload that observes a title change rewrites every
alias that equaled the target's old title (a *mirror*) to the new one, through
the same atomic write path as any other edit. Sync is one-way (editing an alias
never renames a task), mirror-only (aliases that deliberately differ are never
touched), in-store only (dangling and cross-project targets have no old title
and are exempt), and idempotent (it writes only on a real mismatch, so a
cascade's next reload is silent). A cold scan — any CLI command, or a fresh TUI
start — has no previous index and never writes, so a rename made while no TUI
was running leaves aliases stale until they are edited by hand.

## Troubleshooting

- Wrong project? Resolution is `--path` > `$TT_PATH` > cwd, nearest registered
  ancestor first; run `tt project list` to see the registry, or pass `--path`
  explicitly in scripts.
- Task missing from `tt list`? Its file must be named `<id>.md` and its
  frontmatter must declare the matching `id`, `title`, and `state`; notes and
  broken files are ignored or warned about instead.
- `warning:` lines on stderr name skipped or repairable files (malformed,
  id mismatch, dangling parent, cycle); the command still exits `0`.
- Hand-editing or an editor round-trip looks wrong? `?` in the TUI opens the
  store-issue overlay: `j`/`k` move between issues, `e`/`Enter` opens the
  highlighted file, and the `⚠ N issues` header badge counts the same warnings.
  An edit from inside the TUI that raises the issue count toasts
  `⚠ N issues — press ?` right away.
