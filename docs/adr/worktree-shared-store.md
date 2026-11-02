# Linked worktrees share the main checkout's project and store

All linked worktrees of one git repository use one project and one task store.
A linked worktree first searches for a registration inside its own directory.
If none is found, it maps to the same relative path in the main checkout, found
from the git common directory, and searches from there. A directory outside git
resolves by path alone. New tasks record no worktree or branch information, so a
task reads the same from every worktree.

## Considered Options

- **One store per worktree** — rejected: a task added in one worktree would be
  invisible in the next, so the project would not have one list.
- **Worktree path recorded on each task** — rejected: the record goes stale when
  a worktree is removed.

## Consequences

- An explicit registration inside a worktree overrides the main checkout's
  project for that directory.
- Registering a directory that exists only in a worktree fails; register it from
  the main checkout.
- Removing a worktree does not touch its tasks.
