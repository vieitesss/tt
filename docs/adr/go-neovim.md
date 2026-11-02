# Go CLI with a Neovim plugin replaces the Rust TUI and desktop app

tt is a Go CLI (`cmd/tt`, `internal/...`) built from the repository root. Its
interactive client is a Neovim plugin (`plugin/tt.lua`, `lua/tt/`) that shows a
project's tasks as an editable buffer. `:w` sends the whole tree to `tt apply`
(see [declarative-apply](declarative-apply.md)). The Rust CLI, the ratatui TUI,
and the macOS desktop app are removed; their sources and designs stay in git
history at `8947b0b`.

## Why

- The owner wants to use tt from Neovim directly, so Neovim replaces the TUI as the UI.
- The owner chose to write the CLI in Go.
- The owner chose to remove the desktop app. It stays in git history.

## Considered Options

- **Keep the Rust TUI** — not chosen: Neovim replaces the TUI as the UI.
- **Go + Bubble Tea TUI** — the second choice in [rust-ratatui](rust-ratatui.md). Not chosen: Neovim replaces the TUI as the UI.
- **Keep the desktop client** — not chosen: the owner removed it; its design is in git history.

## Consequences

- The CLI is the only write path. The plugin calls `tt --json` and never edits
  task files.
- The plugin needs Neovim 0.10 or newer and a `tt` binary: `bin/tt` from the
  lazy.nvim `build` step, or `tt` on `PATH`.
- Existing stores and the registry stay readable. `--json` changes are additive.
- `--projects` and the TUI move flow are gone. `:TT projects` and
  `tt move --to-project` replace them.
