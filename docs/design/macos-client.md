# macOS desktop client — first version

Status: implemented and locally verified on branch `feat/macos-client`; uncommitted.

## Confirmed product boundary

- Companion to existing tt: the desktop and terminal clients share Projects, Tasks, and task rules. No import step, separate task database, or desktop-only task types.
- First audience: the user's personal daily use on macOS. Public distribution and cross-platform support are not commitments for v1.
- Everyday workflow: choose a Project, browse its Task tree, read/edit a Task, add and organize Tasks, and complete Tasks.
- Copy metadata uses the existing five-line block: Project path, Task file, ID, Title, State. Copy task adds the Markdown body after a blank line.
- Two columns: one sidebar tree — Projects, with the open Project's Tasks nested under it — and the selected Task details. Standard mouse and keyboard navigation; collapsible sidebar.
- Rendered Markdown by default; explicit source editing with Save/Cancel. Title, state, tags, due date, and priority use ordinary form controls.
- Search inside the selected Project. Global capture and cross-Project review/search are deferred.

## Minimal task-first redesign

User feedback after the first usable build: fewer UI elements at first sight; this is a tasks manager. The redesign preserves functionality and the pane structure, but uses progressive disclosure instead of exposing every form and command.

- One sidebar, one tree: Projects and the open Project's Tasks share a single sidebar landmark. The service holds one open Project, so the tree is an accordion: only the open Project is expanded; clicking another opens it through the existing open-project path (dirty-draft guard included), and collapsing the open Project only hides its Tasks. Search, filters, and New task move into that sidebar, scoped to the open Project, with the same progressive disclosure as before. One keyboard cursor walks Project and Task rows in visual order: landing on a Project row does not open it (Enter/→ does), ←/→ fold and unfold, and ← from a root Task returns to its Project row.
- Projects show names and selection, not registry slugs, paths, and unregister buttons on every row. Add/manage controls remain discoverable and keyboard-accessible.
- The main pane prioritizes the task list. Search stays easy to reach; state/tag/priority/due filters appear only when opened. Active filters remain visible through a count or summary even when closed.
- A clear New task action reveals the composer. Root is the default; subtask/capture placement is available intentionally, not an always-visible dropdown.
- Selected task shows a readable title, completion/reopen action, any populated metadata as quiet text, and the Markdown description. Edit description is explicit. Blank metadata fields are not the default view.
- Properties editing, rename, copy actions, cancel/delete, reordering/moving, relationships, and file/ID information live in clearly named secondary disclosures/actions. Empty relationship panels and raw disk paths are absent at first sight.
- Calm macOS typography and spacing, restrained borders, one accent, and light/dark support. No dashboard, counters with no task meaning, decorative cards, new task semantics, or new UI dependencies.
- Disclosure controls have names, expanded state, and keyboard access. Menu-like popovers dismiss on Escape/outside interaction and do not leak across task/project navigation. No fabricated ARIA menu role without its keyboard behavior.
- Existing dirty-draft, saving, stale/deleted recovery, confirmation, shortcuts, issue-reporting, and exact clipboard behavior remain. Safety/error recovery is never hidden by cosmetic simplification.

Verification must assert that advanced controls are initially hidden but all original actions remain reachable, and drive the finished native bundle only against isolated Stores.

## Menu bar popover

User request: something less intrusive than the full window for daily use and quick review. Decisions confirmed with the user (Shape round 1; prototype skipped at the user's request, "we'll improve it later"):

- Lives in TT Desktop itself: one process with a menu bar (tray) icon. Closing the main window hides it instead of quitting; the Dock icon is shown only while the main window is visible. Any click on the tray icon opens the popover (no native tray menu); the popover footer has Open TT and Quit, and the app menu also quits.
- A ~360×480 borderless popover anchored under the tray icon, native translucent popover material, light/dark. Opens on left click or the global shortcut ⌥⌘T; hides on Escape or when it loses focus. Its state (including drafts) survives hide/show.
- One Project at a time, independent of the main window's Project, with a header switcher that remembers the last choice. ⌥1–⌥9 switch to the Nth registered Project.
- Open Tasks as a collapsible tree. Completing animates the checkbox and strike-through, then the row collapses out after ~2 s with an Undo. A "Show done" toggle shows finished Tasks.
- Clicking a Task expands it inline: quiet metadata, rendered Markdown. Edit in place for title and description (source textarea, ⌘↩ save, Esc cancel with discard guard), with the same revision-checked saves and stale/deleted recovery as the main window. Tags, priority, due, moving, and deleting stay in the main window via **Open in TT**, which shows the main window with that Project and Task selected (subject to its dirty-draft guards).
- Actions: complete/reopen, quick add (Enter = top-level Task; Tab = subtask of the expanded Task), copy task, Open in TT. Nothing destructive.
- Tray icon is a monochrome template icon; it gains a dot when the popover's Project has an open Task that is overdue or due today.
- Live: refresh when shown and every second while visible; slower background refresh keeps the dot current. Rows changed by an external write briefly highlight. Small CSS-only animations (open, check, row enter/exit, expand, dropdown unroll, dialog and toast enter/exit, sliding sidebar selection bar, press feedback) on shared motion tokens (`src/motion.css`). Under `prefers-reduced-motion` movement (slide, scale, rotate, clip) is removed and plain opacity fades stay, including the completed row's fade-out during the Undo window. The minimum macOS is 26 so current WebKit CSS (anchor positioning, `field-sizing`, `@starting-style`) is used without fallbacks. No UI libraries added; added dependencies: `tauri-plugin-global-shortcut`, plus macOS-only `objc2`/`objc2-app-kit` (already transitive via Tauri) for safe app activation.

## Implementation assumptions

- Tauri 2 desktop shell, React/TypeScript frontend, and a direct path dependency on the existing Rust tt library. No shelling out to tt, independent data store, or daemon.
- Standalone `desktop/` package, leaving the terminal client's install path unchanged. Build a local unsigned macOS app bundle; signing/notarization/public distribution are not first-version goals.
- Read current files before mutations. A description draft retains its base revision; stale saves fail without discarding the draft, with reload/copy options. Automatic refresh must not reset drafts. This is conflict detection, not a transaction guarantee against simultaneous legacy writers.
- Existing tt config/store locations remain authoritative. GUI selects registered Projects explicitly rather than treating app launch cwd as a Project.
- Project registration/removal, full task state cycle, task/subtask creation, subtree move/delete, sibling reordering, metadata editing, links/backlinks, and Store issue reporting are available inside the app.

## Verification

Used temporary Projects/Stores throughout. Core Rust tests: 453 pass; desktop backend tests: 15 pass; frontend tests: 135 pass after the nested-sidebar change (114 before it). Rust fmt/clippy, frontend typecheck/production build, and npm audit passed (zero reported vulnerabilities). Built the local release `TT Desktop.app`.

Native macOS checks covered Project/Task navigation, exact metadata clipboard output, completion, Markdown saving, unchanged-draft navigation, stale-save conflict recovery through real IPC, and retaining externally deleted drafts when recovery/window close is cancelled. The user's real Tasks were never test fixtures.

All accepted original-app and redesign Spec/Standards/Debt review findings were fixed. The minimal redesign was verified with native macOS workflow checks and bounded browser geometry tests, including popup resizing and task-row alignment. The final rebuilt local app was launch-smoked against an isolated Store; source remains uncommitted. Detailed evidence and session history live in the main checkout's ignored `.agents/ledger.md` and `.agents/reports/`.

## Evidence

Repository exploration at `10ca17a` found:

- The Rust library is independent of terminal UI and is the shared task-mutation boundary (`src/lib.rs`). The CLI is not a full GUI integration surface: several mutation operations are unavailable non-interactively.
- The existing metadata clipboard payload is exactly `Project path`, `Task file`, `ID`, `Title`, and `State`, with absolute paths (`src/tui/app.rs:1407`). Whether the desktop adds a body-inclusive action is an open product decision.
- Atomic file replacement does not prevent lost updates. Cached read-modify-write operations have no shared lock or compare-and-swap contract; concurrent external edits need an explicit design.
- Shared stores default to `~/.local/share/tt`; adopting a separate macOS data location would violate the companion boundary unless tt itself migrated.
- No existing app bundle, GUI, IPC service, or signing workflow.

Full research report: main checkout `.agents/reports/macos-client-facts-research.md`. The implementation assumptions above were announced when the user requested building.
