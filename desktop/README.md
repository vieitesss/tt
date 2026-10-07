# TT Desktop

A macOS companion app for `tt`. It is a second client over the same Projects,
Stores, and Tasks as the terminal client — no import step, no separate task
database, no daemon. It links the `tt` library directly and calls its public
APIs, so both clients always agree on task rules.

```
desktop/
├── src/                 React + TypeScript frontend
│   ├── menubar/         menu bar popover (own Project, own draft)
│   └── ...              main window
├── src-tauri/           Tauri 2 Rust shell (path-depends on ../.. = the tt crate)
└── scripts/             icon generation, isolated smoke launcher
```

## Requirements

- macOS 26 or newer (the UI uses current WebKit CSS directly: anchor positioning, `field-sizing`, `@starting-style`), Xcode Command Line Tools (`xcode-select --install`)
- Rust (stable or nightly ≥ 1.90)
- Node.js 22 or newer, npm 10 or newer

## Run

```sh
cd desktop
npm install
npm run tauri:dev        # Vite dev server + the app, hot reload
```

## Test and check

```sh
cd desktop
npm run typecheck        # tsc --noEmit
npm test                 # Vitest + Testing Library
npm run build            # typecheck + production frontend build

cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test               # backend service tests (isolated temp fixtures)
```

The backend tests and the UI tests use temporary Projects/Stores; they never
touch your real `~/.local/share/tt` data.

## Main window

The main window is two columns: one sidebar tree (Projects, with the open
Project's Tasks nested under it) and the selected Task details. Search,
filters, and New task live in that sidebar, scoped to the open Project;
clicking another Project opens it (a dirty draft is asked about first), and
collapsing the open Project only hides its Tasks. **⌘B** collapses the sidebar
to a rail, **⌘N** opens the composer, **⌘F** focuses search, and
**↑/↓/←/→** walk the tree (**Enter** opens a Project or selects a Task).

## Menu bar popover

TT Desktop is also a menu bar app: one process, one tray icon, and a 360×480
popover for quick review and capture.

- Left-click the tray icon, or press **⌥⌘T** anywhere, to toggle the popover.
  It opens under the tray icon and hides on Escape, on losing focus, or on
  ⌥⌘T again. Hiding keeps the popover alive, so its drafts survive. The
  shortcut is best effort: if another app already owns ⌥⌘T, TT logs that and
  continues without it (the tray icon always works).
- Closing the main window hides it instead of quitting; the Dock icon is shown
  only while the main window is visible, and the app quits from the popover footer
  (“Quit”) or the app menu. Any click on the tray icon opens the popover; its footer
  also has “Open TT” for the main window.
- The tray icon gains a dot while the popover's Project has an open Task that
  is overdue or due today.
- The popover holds its own Project, independent of the main window's.
  **⌥1–⌥9** switch to the Nth registered Project and the last choice is
  remembered; the header switcher does the same by mouse.
- Quick add: **Enter** adds a top-level Task, **Tab** adds a subtask of the
  currently expanded Task (the field says which).
- Click a row to expand it inline: quiet metadata, rendered Markdown, and
  Edit / Copy task / Open in TT. **↑/↓** move, **Enter** or **Space** expands,
  **⌘N** or **/** focuses quick add.
- Editing follows the same contract as the main window: title plus Markdown
  source, **⌘↩** saves, **Esc** cancels, a dirty Escape asks first
  (fail-closed), saves are revision-checked, and a stale or deleted file keeps
  the draft with Reload latest / Copy draft.
- Completing checks and strikes a row, which then collapses away after about
  2 s with a 5 s **Undo**; “Show done” in the footer reveals finished Tasks.
- Live: 1 s reconciliation while visible, 15 s while hidden so the tray dot
  stays current. A refresh never resets an open draft, and rows changed by an
  external write briefly highlight.
- **Open in TT** shows the main window with that Project and Task selected,
  subject to the main window's own dirty-draft guard.

## Build the app bundle

```sh
cd desktop
npm run tauri:build      # == tauri build --bundles app
```

Artifact: `desktop/src-tauri/target/release/bundle/macos/TT Desktop.app`
(unsigned, ad-hoc linker signature only — fine to launch locally, not for
distribution). Launch it with:

```sh
open "desktop/src-tauri/target/release/bundle/macos/TT Desktop.app"
```

The icon set in `src-tauri/icons/` is generated from `app-icon.png` (drawn by
`scripts/make-icon.mjs`, no image libraries): `npm run icon`. The tray icons
(`tray-icon@2x.png`, `tray-icon-attention@2x.png` — plain and with the
attention dot) come from `npm run tray-icons`; both are 44 px because tray-icon
scales whatever it is handed to 18 pt tall, so the larger source is the one
that stays crisp on a Retina menu bar (Tauri has no multi-representation icon
support, so no 1× companion is shipped).

## Smoke test against isolated data

`scripts/smoke-app.sh` creates a throwaway `TT_CONFIG`/`XDG_DATA_HOME`, seeds
one registered project with two tasks, and launches the built app against it —
never your real registry:

```sh
./scripts/smoke-app.sh              # release bundle
./scripts/smoke-app.sh --debug      # debug bundle (faster to rebuild)
./scripts/smoke-app.sh --binary     # raw binary, prints its own data paths
```

## Dependency pins

`tauri` is pinned to `=2.11.5` (matching `@tauri-apps/api` 2.11.1), with the
`tray-icon` and `macos-private-api` features on (menu bar icon and the
transparent popover). Its transitive crates (`tauri-runtime`,
`tauri-runtime-wry`, `tauri-utils`, `tauri-macros`, `tauri-codegen`,
`tauri-plugin`) are pinned in `src-tauri/Cargo.lock` to the compatible
2.11/2.9/2.6 line. The plugins are `dialog =2.7.3`,
`clipboard-manager =2.3.3`, `opener =2.5.5`, and
`global-shortcut =2.3.2` (⌥⌘T) — the newest releases whose `tauri` requirement
still accepts 2.11 (2.4.0 needs 2.12). **Do not run `cargo update`**: newer
2.12 transitive crates break `tauri` 2.11.5 at compile time.

## Where data lives

The app follows `tt` exactly: the registry/Stores are
`$XDG_DATA_HOME/tt` or `~/.local/share/tt`, the config is
`$TT_CONFIG` or `<data dir>/config.toml`. The app cwd is never treated as a
Project; Projects are opened explicitly from the sidebar.

## First-version scope

In: one nested Projects/Tasks sidebar tree with folds and rollups, rendered
Markdown with
explicit source editing, task/sub-task creation, rename, state, tags, priority,
due, reparent, reorder, subtree move between Projects, subtree delete with a
descendant count, metadata/task copy, project-scoped search and filters, store
issue reporting, ~1 s reconciliation with stale-draft protection, plus the menu
bar popover (tray icon, ⌥⌘T, per-Project quick review and capture, Open in TT).

Out: all-project review (in either window; the popover shows one Project at a
time), drag-and-drop reordering, signing/notarization/updater, cross-platform
packaging.
