# `tt apply` takes the complete desired task tree

`tt apply` reads one JSON object, `{revision, tasks}`, from stdin. `tasks` is
the complete desired tree. Go computes the diff (adds, updates, moves, reorders,
deletes), validates the whole tree before any write, and writes in this order:
adds, updates and moves, alias rewrites, deletes. `revision` is the Store hash
that the client planned from (`tt list --json`). A mismatch writes nothing.
`--dry-run` returns the same plan without writing. The client parses input and
renders output; the diff and the writes stay in Go.

## Considered Options

- **Per-change commands from the client** — rejected: many writes, no
  all-or-nothing validation, and the client would have to compute the diff.
- **Operation lists from the client** — rejected: the client would need to know
  the ordering and rank rules that Go already enforces.

## Consequences

- A task missing from the tree is deleted, with its body. Clients send every task
  they keep, and omitted `tags`, `due`, or `priority` clear those fields.
- Writes are not transactional. An I/O failure reports what was committed
  (`applied`) and does not roll back.
- Dry-run ids for new tasks are provisional.
- The revision guards against concurrent edits. On mismatch, reload and plan again.
