# tt

tt keeps a task list for each project you work in. You register a directory
as a project; its tasks live outside the directory, one markdown file per task.
Linked git worktrees of one repository share one task list.

Two clients use the same store:

- the `tt` CLI, with `--json` output for scripts and agents (see [SKILL.md](SKILL.md));
- a Neovim plugin that edits a project's tasks as a buffer.

## Requirements

- Go, to build the CLI.
- Neovim 0.10 or newer, for the plugin.

## Install

With [lazy.nvim](https://github.com/folke/lazy.nvim):

```lua
{
  "vieitesss/tt",
  build = "go build -o bin/tt ./cmd/tt",
}
```

`build` compiles `bin/tt` in the plugin directory on install and update.

Set the binary explicitly if needed:

```lua
require("tt").setup({ bin = "/path/to/tt" })
```

The plugin uses the first binary it finds: the `bin` option, then `bin/tt` in
the plugin directory, then `tt` on `PATH`.

## Commands

| Command | Action |
| --- | --- |
| `:TT` | Open the buffer for the current project. An unregistered project asks to be registered. |
| `:TT add <title>` | Add a task to the project without opening the buffer. |
| `:TT projects` | Pick a registered project and open its buffer. |

## The buffer

The buffer is named `tt://<slug>`. Each task is one line; nesting is two spaces
per level:

```
/0bkhzyq60e - [ ] Write the plan !high @2026-09-17
  /xfm9c3bkyu - [x] Ship it #work/admin
/gzsr1pq9fz - [ ] New task
```

- `/<id>` is the task identity. It is concealed in the buffer. A line without
  an id is a new task.
- `[ ]` open, `[x]` done, `[-]` cancelled.
- Trailing metadata: `#tag`, `!high`, `!med` or `!low`, `@YYYY-MM-DD`,
  `@today`, `@tomorrow`.

Edit the lines like text:

- Add a task: type a new line.
- Delete a task: delete its line. Deleting a parent does not delete its
  children. Surviving indented lines attach to the preceding less-indented line;
  if none exists, they become roots. Outdent a line fully to move it to the root.
- Rename: edit the title.
- Reorder: move lines (`dd`, `p`, `yyp`).
- Reparent: indent or outdent the line.

`:w` parses the buffer, shows a summary (adds, updates, moves, reorders, and
deletions, with `[HAS BODY]` on deleted tasks that have a body), and asks
Apply or Cancel. Apply writes the changes and reloads the buffer. A parse error
names the line and leaves the buffer unsaved. If the list changed on disk, the
save fails: reload with `:e!`.

Task bodies and unknown fields are not shown in the buffer; the CLI keeps them.

## Keys

| Key | Action |
| --- | --- |
| `<CR>` | Open the task's file. Save the buffer first if the line is new. |
| `g<Space>` | Toggle done: open becomes done; done or cancelled becomes open. |
| `g?` | Show a short key and edit hint. |

## Check

```sh
just check
```

Checks formatting (gofmt, StyLua), runs `go vet`, the Go tests, and the headless
Neovim tests. It builds `bin/tt` first. It needs `just`, `go`, `stylua`, and `nvim`.
