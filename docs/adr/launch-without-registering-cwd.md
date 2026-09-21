# Bind a TUI session without registering cwd

Bare `tt` in an unregistered directory still asks whether to register cwd, but
the TUI can bind to a different already-registered Project instead of quitting.
A session always has one current Project — there is no homeless TUI. `--projects`
opens that same picker (including from inside a Project). Nested-directory launch
is unchanged. Skipping cwd is one-shot; an empty registry falls through to the
register-path prompt.

## Considered Options

- **Homeless / "no project mode"** — rejected: the List is a project's tree; a
  TUI with no Project is a second product.
- **Remember this path is not a Project** — rejected: a negative registry is a
  new domain object; `tt --projects` is the opt-in skip.
- **Silent nested open** — rejected once the existing nested modal was in view;
  Register separately stays.
