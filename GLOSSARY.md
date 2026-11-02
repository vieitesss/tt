# TT

TT organizes personal work into Projects and Tasks. Existing domain terms retain their definitions in [CONTEXT.md](./CONTEXT.md); this glossary records newly agreed terms.

## Language

**Worktree**:
A linked working directory of a git repository, created by `git worktree add`. It shares its repository with the main checkout. In TT, all worktrees of one repository use the same Project and Store.
_Avoid_: clone, copy

**Main checkout**:
The working directory that owns the repository's common git directory (`<repo>/.git`). TT registers a repository's Project at this root and maps each worktree's directory to the same relative path inside it.
_Avoid_: primary, root repo
