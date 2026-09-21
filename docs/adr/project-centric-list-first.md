# Project-centric central store and list-first TUI

`tt` v2 pivots from "one vault you point at" to **projects**: a project is a
directory the user already works in, registered in `~/.config/tt/config.toml`,
and its tasks live in an invisible central store
(`$XDG_DATA_HOME/tt/<slug>/`, else `~/.local/share/tt/<slug>/`). Resolution
walks up from `--path`/`$TT_PATH`/cwd and takes the nearest registered project;
an unregistered directory can be registered on first use, and a directory
nested inside a project resolves to it unless the user asks for a separate
project (with a persisted never-ask rule). The TUI becomes a list-first outline
of the current project's tree; the spatial map, camera, root tab bar, and node
boxes are deleted. Editing moved to `$EDITOR`, and links are demoted to the
preview plus an `o` jump picker.

This supersedes the map-first direction. The repository has no git history, so
the deleted map/outline code is unrecoverable; that is accepted — the list is
the product.

## Considered Options

- **Per-project vault in the working directory (`.tt/`)** — rejected: pollutes
  working trees, invites accidental commits, and forces every project to be
  writable.
- **One global vault for everything** — rejected: no project scoping, and one
  noisy tree for unrelated work.
- **Keep the map alongside the list** — rejected: two navigation models to
  maintain; the spatial layout was the expensive part and the list covers the
  daily flow.
- **Keep in-app rename/body editing** — rejected: `$EDITOR` is better at text,
  and the two write paths diverged from external edits.
- **Always prompt before registering** — rejected: prompts would break
  `--json`/piped usage; a non-interactive `tt add` registers silently so agents
  can capture without ceremony.

## Consequences

- The config file is now a registry; writes are atomic and unknown keys
  (including the legacy `vault=`) survive rewrites. `vault=` is warn-and-ignore.
- The store is invisible: users never browse or organize it, and files stay
  normalized and id-named.
- `--json` stays additive-only: task objects and links/backlinks are unchanged;
  `tt project *` adds its own shapes.
- `--vault`/`$TT_VAULT` stays as a hidden escape hatch that opens a folder
  directly and skips the registry.
- The TUI and the CLI share the same resolution and write paths; the TUI shows
  launch modals where the CLI prompts.
