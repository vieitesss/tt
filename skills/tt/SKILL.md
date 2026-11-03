---
name: tt
description: "Operate a tt (ToTask) project from the CLI. Use when the user asks about tasks: adding, listing, showing, completing, cancelling, reopening, or renaming; when registering projects with `tt project`; when scripting or bulk-editing a project with `tt --json`, including `tt apply`; or when reading and hand-editing the one-file-per-task markdown stored in a project store."
---

`tt` keeps a task list for each **project**: a directory registered with `tt`.
Its tasks live in an invisible **store** of plain markdown files, one file per
task. The `tt` CLI is the interface for agents and scripts. The Neovim plugin
is a human client of the same commands. Design rationale lives in `CONTEXT.md`
and `docs/adr/`.

## Invocation

```
tt [--path PATH] [--json] <command>
```

- `--json` prints JSON on stdout. Parse it: the shapes below are stable. Human
  output is not.
- `--path PATH` sets the starting directory for project resolution.
- `-V`, `--version` prints the version. Bare `tt` prints help.
- `--vault PATH` (hidden) opens a store folder directly and skips the registry.
  `$TT_VAULT` does the same.
- `--projects` and `-p` do not exist; `tt` rejects them as unknown flags (exit 2).

## Project resolution

The starting directory is `--path`, else `$TT_PATH`, else the current
directory. Then:

1. Walk up from the starting directory, nearest first. The first registered
   project wins.
2. In a linked git worktree, a registration inside the worktree wins. Otherwise
   the same relative path in the **main checkout** is walked up.
3. If nothing is registered, the path to register is the main checkout root
   (or the starting directory outside git).

All worktrees of one repository therefore use one project and one store. New
tasks record no worktree information.

Consequences:

- A directory inside a registered project resolves to that project.
- An unregistered directory: `tt add` registers it silently when non-interactive
  (`--json`, piped, or no TTY) and prompts otherwise. Other commands, including
  `apply`, fail with `not a registered project; run tt add or tt project add`.
- A nested directory must be registered explicitly (`tt project add DIR`) to be
  its own project.
- `tt project add DIR` in a worktree maps `DIR` to the same path in the main
  checkout. It fails if that directory does not exist there. `project remove`
  maps paths the same way.

## Paths and config

- Config (the registry): `$TT_CONFIG`, else `$XDG_DATA_HOME/tt/config.toml`,
  else `~/.local/share/tt/config.toml`. If that file is missing, an existing
  legacy `$XDG_CONFIG_HOME/tt/config.toml` (default `~/.config/tt/config.toml`)
  is migrated to it on first use. `TT_CONFIG` disables the migration.
- Store: `$XDG_DATA_HOME/tt/<slug>/`, else `~/.local/share/tt/<slug>/`.

```toml
capture_target = "abc1234567" # default parent for `tt add` without --parent

[[project]]
path = "~/work/app"           # paths under HOME are written with ~/
slug = "app"
never_ask_nested = false
```

A `vault = "..."` key is deprecated: `tt` warns on stderr, ignores it, and keeps
it when the config is rewritten. Unknown keys are kept.

## Output contract

- Success: exit `0`, the JSON object on stdout.
- Runtime failure: exit `1`. With `--json`: `{"error":"<message>"}` on stdout.
  Human mode: `error: <message>` on stderr.
- Usage error (unknown flag, bad value, wrong argument count): exit `2`, usage
  on stderr, never JSON.
- Store problems: `warning: <path> (<kind>): <detail>` on stderr. They never
  fail the command.

### Task object

Task-producing commands return:

```json
{"id":"bzfca8bm5j","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null,"rank":null}
```

- `id`: the stem of the store file, 10 characters from `[0-9a-z]` when `tt`
  creates it. Other ids may use `[0-9a-z_-]`, up to 64 characters. Use it in
  `[[id]]` links.
- `state`: `open` | `done` | `cancelled`.
- `parent`: parent id or `null`. One parent maximum.
- `tags`: without a leading `#`. A `work` filter also matches `work/admin`.
- `due`: `YYYY-MM-DD`. `priority`: `high` | `med` | `low`.
- `rank`: optional place among siblings, lower first; `null` when unranked.
- `add` and `show` also return `path`: the absolute store file path. Other
  commands do not.

## Commands

### `tt add`

```
tt add [TITLE] [--title TITLE] [--parent ID] [--tag TAG]... [--due YYYY-MM-DD]
       [--priority high|med|low] [--body TEXT|-]
```

Pass the title positionally or with `--title`, not both. `--body -` reads the
body from stdin. Without `--parent`, `capture_target` is used if set, else the
project root. The new task is appended after its siblings.

```console
$ mkdir -p /tmp/tt-demo/project && cd /tmp/tt-demo/project
$ export TT_CONFIG=/tmp/tt-demo/config.toml XDG_DATA_HOME=/tmp/tt-demo/data
$ tt --json project add
{"path":"/tmp/tt-demo/project","slug":"project"}
$ tt --json add --title "Write the plan"
{"id":"8d7jfebh28","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null,"rank":0,"path":"/tmp/tt-demo/data/tt/project/8d7jfebh28.md"}
$ printf 'body from stdin\n' | tt --json add "Stdin task" --body -
{"id":"yubpl8hb0y","title":"Stdin task","state":"open","parent":null,"tags":[],"due":null,"priority":null,"rank":1,"path":"/tmp/tt-demo/data/tt/project/yubpl8hb0y.md"}
```

### `tt list`

```
tt list [--flat] [--tag TAG] [--state open|done|cancelled] [--due-today]
```

Tree by default: `{"tasks":[{...task,"children":[...]}],"revision":"..."}`.
`--flat` returns `{"tasks":[{...task}],"revision":"..."}` sorted by id, with no
`children`. A task that matches while its parent does not is promoted to a root.
Siblings are ordered by `rank` (ranked first, ascending), then by lowercased
title, then by id. `--due-today` includes overdue tasks. Filters combine with AND.

`revision` is needed by `tt apply`. Read it from the same `list` call you plan
from.

```console
$ tt --json list --flat --tag work
{"tasks":[{"id":"2mucpsinmb","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high","rank":0}],"revision":"1af83cbb529cdbca59bdb4b9283dd2ed051eece66d5c5228e76ace6926ae4861"}
```

### `tt show ID`

The task object plus `body`, `links`, `backlinks`, `children` (ids), and `path`.

```console
$ tt --json show 8d7jfebh28
{"id":"8d7jfebh28","title":"Write the plan","state":"open","parent":null,"tags":[],"due":null,"priority":null,"rank":0,"body":"","links":[],"backlinks":["2mucpsinmb"],"children":["2mucpsinmb"],"path":"/tmp/tt-demo/data/tt/project/8d7jfebh28.md"}
```

### `tt done ID` / `tt cancel ID` / `tt reopen ID`

Set the state (`reopen` sets `open`) and return the task object without `path`.

```console
$ tt --json done 2mucpsinmb
{"id":"2mucpsinmb","title":"Ship it","state":"done","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high","rank":0}
```

### `tt edit ID [--title TITLE]`

With `--title`, renames the task and returns it. The id and file name never
change. Mirror aliases in this store are rewritten (see Title sync). Without
`--title`, opens the task file in `$EDITOR` (fallback `vi`), then reloads and
returns the task. Scripts use `--title`; the editor form is for people.

```console
$ tt --json edit 2mucpsinmb --title "Ship it v2"
{"id":"2mucpsinmb","title":"Ship it v2","state":"open","parent":"8d7jfebh28","tags":["work/admin"],"due":"2026-09-17","priority":"high","rank":0}
```

### `tt move ID`

```
tt move ID [--parent ID | --root] [--to-project DIR|SLUG]
```

- Without `--to-project`: reparents within the current project. Exactly one of
  `--parent ID` or `--root` is required. The task becomes the last sibling in
  its new group. A task cannot move under itself or a descendant.
- With `--to-project DIR|SLUG`: moves the task and its whole subtree to a
  registered project. An unregistered destination is an error; it is never
  registered implicitly. Without `--parent`, or with `--root`, the moved root
  becomes unranked at the destination's root. If the destination is the current
  project, the command reparents in place.
- Ids, bodies, and `[[id]]` text are unchanged. Links left in the source may
  dangle, which is legitimate.
- A cross-project move checks for id collisions first, then writes all target
  files before removing any source file. A failure mid-way returns
  `partial move (target_written=N, source_removed=N)`; duplicate copies may
  remain.

Both kinds of move return every moved task (the root and all descendants) in id
order, plus the destination:

```json
{"tasks":[{"id":"2mucpsinmb","title":"Ship it","state":"open","parent":"8d7jfebh28","tags":[],"due":null,"priority":null,"rank":0}],"target_project":{"path":"/work/other","slug":"other"}}
```

### `tt apply`

```
tt apply [--dry-run]
```

Makes the project match a complete desired tree. Send one JSON object on stdin:

```json
{"revision":"<revision from tt --json list>","tasks":[
  {"id":"8d7jfebh28","title":"Write the plan","state":"done","tags":[],"due":null,"priority":null,"children":[
    {"id":"2mucpsinmb","title":"Ship it","tags":["work/admin"],"due":"2026-09-17","priority":"high","children":[]}
  ]},
  {"title":"New task","children":[]}
]}
```

Rules:

- `revision` and `tasks` are required. Input must be exactly one JSON object.
- `tasks` is the **whole** tree. Every task you want to keep must appear. Any
  existing task missing from the tree is **deleted**, with its body.
- Node fields: `id` (omit for a new task; ids are generated), `title` (required,
  not blank), `state` (default `open`), `tags` (default empty), `due`
  (`YYYY-MM-DD` or `null`), `priority` (`high`|`med`|`low` or `null`),
  `children`.
- An omitted `tags`, `due`, or `priority` **clears** the field. Send the full
  value on every apply.
- Order among siblings sets their `rank`.
- The whole tree is validated before any write. Errors include
  `invalid task id`, `duplicate task id`, `task not found`, `invalid state`,
  `invalid priority`, and `task title must not be empty`.
- A `revision` that no longer matches the store fails with
  `task list changed on disk; reload`, and nothing is written. Run `tt list`
  again and re-plan.
- Writes happen in this order: new tasks, updates and moves, mirror-alias
  rewrites, then deletions. Bodies and unknown frontmatter keys of kept tasks
  are preserved.

Output (stdout, `--json`):

```json
{"revision":"0b9773b3...","dry_run":false,"adds":[{"id":"pdgyhn41zi","title":"New task","parent":null}],"updates":[{"id":"8d7jfebh28","fields":["state"]}],"moves":[],"reorders":0,"deletes":[]}
```

- `adds`: `{id,title,parent}`. `updates`: `{id,fields}`. `moves`: `{id,from,to}`.
  `reorders`: a count. `deletes`: `{id,title,has_body}`.
- `dry_run: true` plans without writing. Its ids for new tasks are provisional,
  and its `revision` is the current one.
- A successful apply returns the new `revision`. Use it for the next apply.
- An I/O failure during writes exits `1` with
  `{"error":"...","applied":{...same shape...}}`. `applied` lists what was
  committed. There is no rollback.
- Human mode prints `applied: N adds, N updates, N moves, N reorders, N deletes`
  (or `planned:` for `--dry-run`).

```console
$ R=$(tt --json list | sed 's/.*"revision":"\([^"]*\)".*/\1/')
$ printf '%s' "{\"revision\":\"$R\",\"tasks\":[{\"title\":\"New task\",\"children\":[]}]}" | tt --json apply --dry-run
```

`apply` does not register an unregistered directory.

### `tt project list` / `tt project add [DIR]` / `tt project remove DIR|SLUG` / `tt project current`

- `list`: `{"projects":[{"path":"...","slug":"...","never_ask_nested":false}]}`.
- `add [DIR]`: registers `DIR`, or the starting directory when omitted. `DIR`
  must exist (`project directory does not exist`). It creates the store folder.
  Returns `{"path":"...","slug":"..."}`. Registering an existing project
  returns it.
- `remove DIR|SLUG`: unregisters only. Store files are kept. Returns
  `{"removed":{...}}`. Fails with `not a registered project: ...` if absent.
- `current`: resolves the starting directory without registering anything.
  Returns `{"registered":true|false,"path":"...","slug":"...|null","nested":"...|null"}`.
  `path` is the registered path, or the path to register when `registered` is
  false.

```console
$ tt --json project list
{"projects":[{"path":"/tmp/tt-demo/project","slug":"project","never_ask_nested":false}]}
$ tt --json project current
{"registered":true,"path":"/tmp/tt-demo/project","slug":"project","nested":null}
$ mkdir -p /tmp/tt-demo/other && tt --json project add /tmp/tt-demo/other
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

### `tt update` (alias `upgrade`)

Runs `go install github.com/vieitesss/tt/cmd/tt@main` with
`GOBIN=$HOME/.local/bin`. It needs `go` on `PATH` and replaces any `tt` binary
in `~/.local/bin`.

## On-disk format

One file per task at `<store>/<id>.md`:

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
rank: 0
---
See [[8d7jfebh28]].
```

- The file stem is the identity. A frontmatter `id` that differs from the stem
  is reported as `id mismatch`; the task loads under the stem. Files are never
  renamed, and `tt` never rewrites a file to fix a mismatch.
- A `.md` file without task frontmatter (no frontmatter, or another tool's) is
  ignored silently. A file with task frontmatter that is malformed (bad YAML,
  invalid UTF-8, or a file name that is not a valid id) is skipped and warned
  about as `malformed`; it is never rewritten.
- A parent that is missing makes the task a root (`dangling parent`). If the
  parent appears later, the task reattaches. A parent cycle removes the closing
  edge and makes that task a root (`cycle`).
- `rank` is an optional integer; the key is omitted when unranked.
- Rewrites keep unknown frontmatter keys and are atomic (temp file, then rename).
- Backlinks are computed from bodies, never stored. Link by id, not by title.
- Prefer `tt --json` over hand edits, so the index stays consistent.

### Links

Write a link as `[[{id}.md|{Title}]]`: the real file name, so it is navigable
in any editor, plus a display alias. The parser also accepts `[[id]]`,
`[[id|alias]]`, `[[id.md]]`, and `[[id.md|alias]]`, and normalizes them to the
id. Aliases never affect identity. In `--json`, `links` and `backlinks` are id
arrays.

### Title sync

When a title changes through `tt edit --title`, `tt edit` (editor form), or an
apply that renames a task, every alias in this store that equaled the old title
(a *mirror*) is rewritten to the new title. Sync is one-way and mirror-only:
aliases that differ on purpose are never touched, and no other store is
changed. A title changed outside tt (for example, in an editor on the file) is
not synced by later commands: they start cold and never write. Edit such
aliases by hand.

## Troubleshooting

- Wrong project? Resolution order: `--path`, `$TT_PATH`, then cwd, nearest
  registered ancestor first. Run `tt --json project current` to see what
  resolves, and `tt project list` to see the registry.
- Task missing from `tt list`? Its file must be `<id>.md` with a frontmatter
  `id`, `title`, and `state`. Notes and malformed files are ignored or warned
  about.
- `warning:` on stderr names a skipped or repairable file (`unreadable`,
  `malformed`, `id mismatch`, `dangling parent`, `cycle`). The command still
  exits `0`.
- `task list changed on disk; reload` from `apply`: run `tt --json list`, take
  the new `revision`, and apply again from the fresh tree.
