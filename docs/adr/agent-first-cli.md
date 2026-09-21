# Agent-first CLI as a stable interface

The app ships a headless CLI covering the full task lifecycle (add, list/query, done/cancel/reopen, link, tag, edit, re-parent) with stable, machine-readable output (`--json`), plus a `SKILL.md` so AI agents can operate the vault without the TUI. The CLI is a *public contract*, not a debug tool: both the TUI and agents are clients of the same vault library.

## Consequences

- CLI output stability matters as much as the TUI's UX — breaking `--json` shape breaks agents.
- All mutation logic lives in the vault library; the TUI must not have privileged write paths the CLI lacks.
- The skill file ships in the repo and documents the CLI as the agent interface; keeping it in sync is part of changing the CLI.

## Considered Options

- **TUI-only, CLI later** — rejected: retrofitting a stable headless interface onto TUI-coupled logic is much harder than building library-first.
