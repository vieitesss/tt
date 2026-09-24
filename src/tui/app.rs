//! TUI state and key handling.
//!
//! [`App`] owns the vault and all view state. Key handling is deliberately
//! terminal-free so tests can drive it directly; rendering lives in
//! [`super::ui`]. Every mutation goes through the public library API, so the
//! TUI has no privileged write path.
//!
//! The list view is the only main view. It owns a single selection over the
//! flattened task tree, and a persistent preview pane follows that selection.

use std::collections::{BTreeSet, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tt::{
    registry, Config, DeleteOutcome, NewTask, Priority, Project, ShiftOutcome, TaskId, TreeNode,
    Vault, VaultIssue, VaultWatcher,
};

use super::keymap::{keymap_columns, keymap_content_lines, keymap_content_width, keymap_geometry};
use super::list::TaskList;
use super::picker::{self, FilterChoice, FilterCriterion, Picker, PickerKind};

/// How often the event loop wakes to check for external changes.
pub(crate) const TICK: Duration = Duration::from_millis(250);

/// How many [`TICK`]s a toast stays on screen: 12 ticks is about 3 seconds.
pub(crate) const TOAST_TICKS: u32 = 12;

/// A key/label pair shown in the single footer hint row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hint {
    pub(crate) key: &'static str,
    pub(crate) label: &'static str,
}

/// The footer's hint row is either plain text (for prompts and inline picker
/// status) or a styled list of key/label pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FooterLine {
    Text(String),
    Hints(&'static [Hint]),
}

const NAVIGATE_HINTS: &[Hint] = &[
    Hint {
        key: "j/k",
        label: "move",
    },
    Hint {
        key: "a",
        label: "add",
    },
    Hint {
        key: "x",
        label: "state",
    },
    Hint {
        key: "p",
        label: "projects",
    },
    Hint {
        key: "/",
        label: "find",
    },
    Hint {
        key: "?",
        label: "keys",
    },
];
const SELECTION_HINTS: &[Hint] = &[
    Hint {
        key: "tab",
        label: "un/select",
    },
    Hint {
        key: "esc",
        label: "clear",
    },
    Hint {
        key: "m",
        label: "move",
    },
    Hint {
        key: "d",
        label: "delete",
    },
    Hint {
        key: "x",
        label: "state",
    },
    Hint {
        key: "!",
        label: "priority",
    },
    Hint {
        key: "t",
        label: "tags",
    },
];
const CONFIRM_DELETE_HINTS: &[Hint] = &[
    Hint {
        key: "←/→",
        label: "select",
    },
    Hint {
        key: "enter",
        label: "confirm",
    },
    Hint {
        key: "y",
        label: "confirm",
    },
    Hint {
        key: "esc",
        label: "cancel",
    },
];
const ISSUES_HINTS: &[Hint] = &[
    Hint {
        key: "j/k",
        label: "move",
    },
    Hint {
        key: "e",
        label: "edit",
    },
    Hint {
        key: "esc",
        label: "close",
    },
];
const REGISTER_HINTS: &[Hint] = &[
    Hint {
        key: "enter",
        label: "register",
    },
    Hint {
        key: "esc",
        label: "cancel",
    },
];

/// A transient action message shown as a non-blocking toast popup.
///
/// Toasts never take focus and are not persisted; a new message replaces the
/// previous one and resets the countdown. Expiry is measured in [`TICK`]s so
/// tests never need to sleep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    /// Message text.
    pub(crate) text: String,
    /// Remaining [`TICK`]s before the toast disappears.
    pub(crate) ticks_left: u32,
}

/// What the keyboard is currently doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputMode {
    /// Navigation and hotkeys.
    Navigate,
    /// Typing a title for `a`/`A`; the new task's parent is fixed.
    Add {
        /// Parent for the new task; `None` means the vault root.
        parent: Option<TaskId>,
        /// Current sibling after which `A` inserts; `a` leaves this unset.
        insert_after: Option<TaskId>,
    },
    /// Typing a new title for `r`; the target is the cursor task from when
    /// the prompt opened, and committing cascades mirror aliases.
    Rename {
        /// Task being renamed.
        id: TaskId,
    },
    /// Typing a title for `N` quick capture.
    Capture,
    /// Typing the first tag when the vault has no defined tags yet.
    Tag,
    /// Typing a directory path for `P` to register a new project.
    RegisterPath,
    /// Confirming a destructive delete (`d`); `ids` is a snapshot of the
    /// active selection. Every descendant of those tasks dies too, so the
    /// snapshot only holds the requested roots. The highlight starts on
    /// Cancel.
    ConfirmDelete {
        /// Tasks that would be deleted.
        ids: Vec<TaskId>,
        /// Highlighted button: 0 = Delete, 1 = Cancel.
        button: usize,
    },
    /// A shared type-to-filter picker is open.
    Pick(Picker),
}

impl InputMode {
    /// Whether an unsaved buffer in this mode must block watcher reloads.
    ///
    /// Typed text worth protecting lives in the mode's buffer; pickers only
    /// hold one for search and tags.
    pub(crate) fn holds_buffer(&self) -> bool {
        match self {
            Self::Add { .. }
            | Self::Capture
            | Self::Rename { .. }
            | Self::Tag
            | Self::RegisterPath
            | Self::ConfirmDelete { .. } => true,
            Self::Pick(picker) => picker.kind.holds_buffer(),
            Self::Navigate => false,
        }
    }

    /// The prompt this mode shows, before [`App`] resolves any task labels;
    /// `None` while navigating or while a mode draws its own popup.
    pub(crate) fn prompt(&self) -> Option<Prompt> {
        match self {
            Self::Add { parent, .. } => Some(Prompt::NewTask {
                parent: parent.clone(),
            }),
            Self::Capture => Some(Prompt::Capture),
            Self::Rename { .. } => Some(Prompt::Rename),
            Self::Tag => Some(Prompt::Tag),
            Self::Pick(picker) => Some(Prompt::Picker(picker.kind.prompt())),
            Self::Navigate | Self::RegisterPath | Self::ConfirmDelete { .. } => None,
        }
    }
}

/// A prompt shape owned by [`InputMode`], resolved into text by [`App`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Prompt {
    /// Typing a title for `a`/`A`, under a fixed parent.
    NewTask {
        /// Parent for the new task; `None` means the vault root.
        parent: Option<TaskId>,
    },
    /// Typing a title for `N` quick capture.
    Capture,
    /// Typing a new title for `r`.
    Rename,
    /// Typing the first tag when the vault has no defined tags yet.
    Tag,
    /// A picker's own prompt prefix.
    Picker(&'static str),
}

/// A picker's popup content, ready for [`super::ui`] to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerPopup {
    /// Popup title.
    pub(crate) title: &'static str,
    /// One display row per live match, already formatted for the row width.
    pub(crate) entries: Vec<String>,
    /// Highlighted entry index.
    pub(crate) highlight: usize,
}

/// List application state.
pub(crate) struct App {
    /// The vault; every mutation goes through it.
    pub(crate) vault: Vault,
    /// Selected task; the preview follows it.
    pub(crate) selected: Option<TaskId>,
    /// Current keyboard mode.
    pub(crate) mode: InputMode,
    /// Text typed in the current input mode.
    pub(crate) input: String,
    /// Picker-inline status (for example search's `no matches`); action
    /// confirmations and errors live in [`App::toast`] instead.
    pub(crate) status: Option<String>,
    /// Transient action confirmation or error, auto-dismissed on tick.
    pub(crate) toast: Option<Toast>,
    /// Date used for due/overdue rendering, refreshed on every tick so a
    /// session left open across midnight stays correct.
    pub(crate) today: NaiveDate,
    /// Issues from the most recent scan, shown as a persistent badge and in
    /// the `g?` overlay. Kept out of [`App::status`] so reloads never clobber
    /// action feedback.
    pub(crate) vault_issues: Vec<VaultIssue>,
    /// Whether the vault-issue overlay is open.
    pub(crate) issues_open: bool,
    /// Whether the complete keymap overlay is open.
    pub(crate) keymap_open: bool,
    /// Scroll offset in the keymap overlay.
    pub(crate) keymap_scroll: usize,
    /// Highlighted issue in the `g?` overlay.
    pub(crate) issues_cursor: usize,
    /// Middle pane size last measured from the terminal; keymap scrolling
    /// clamps against it before drawing.
    pub(crate) middle_viewport: (u16, u16),
    /// Set by `q`/`ctrl-c`; the event loop exits when true.
    pub(crate) should_quit: bool,
    /// The flattened task tree, rebuilt on every refresh.
    pub(crate) list: TaskList,
    /// Active session-only list filter. A filtered list is flat and ignores
    /// all tree relationships.
    pub(crate) active_filter: Option<FilterCriterion>,
    /// Ids whose children are hidden in the list. In-memory only; folds are
    /// never persisted.
    pub(crate) collapsed: HashSet<TaskId>,
    /// Multi-selected rows, independent of the cursor. Marks survive folds
    /// and navigation, and are pruned when their task vanishes.
    pub(crate) marked: BTreeSet<TaskId>,
    /// First row drawn in the list pane; adjusted to keep the selection
    /// visible.
    pub(crate) list_scroll: usize,
    /// Size of the list pane as last measured from the terminal; scrolling
    /// clamps against it. Set by [`App::set_list_viewport`], never by the
    /// draw.
    pub(crate) list_viewport: (u16, u16),
    /// Registry entry for the current project; `None` for the `--vault`
    /// escape hatch.
    pub(crate) project: Option<Project>,
    /// Loaded configuration: registry, display style, and capture target.
    pub(crate) config: Config,
    /// Data directory the project stores live under; `None` disables the
    /// project picker with a status message.
    pub(crate) store_root: Option<PathBuf>,
    /// Set by `e`/`Enter`: the task file the event loop should open in
    /// `$EDITOR` after the current key press is handled. `handle_key` never
    /// spawns a process, so tests can assert the request directly.
    pending_edit: Option<PathBuf>,
    /// Set by the first `g` of a `gg` chord.
    pending_g: bool,
    watcher: Option<VaultWatcher>,
    /// Set when a watched change arrived while an unsaved buffer was open;
    /// shown on the context row until the buffer settles and reloads.
    pub(crate) external_change_pending: bool,
}

impl App {
    /// Build the app, starting a best-effort vault watcher.
    pub(crate) fn new(
        vault: Vault,
        config: Config,
        project: Option<Project>,
        store_root: Option<PathBuf>,
    ) -> Self {
        let watcher = vault.watch().ok();
        let mut app = Self {
            vault,
            selected: None,
            mode: InputMode::Navigate,
            input: String::new(),
            status: None,
            toast: None,
            today: Local::now().date_naive(),
            vault_issues: Vec::new(),
            issues_open: false,
            keymap_open: false,
            keymap_scroll: 0,
            issues_cursor: 0,
            middle_viewport: (0, 0),
            should_quit: false,
            list: TaskList::default(),
            active_filter: None,
            collapsed: HashSet::new(),
            marked: BTreeSet::new(),
            list_scroll: 0,
            list_viewport: (0, 0),
            project,
            config,
            store_root,
            pending_edit: None,
            pending_g: false,
            watcher,
            external_change_pending: false,
        };
        app.refresh();
        app
    }

    /// Whether the event loop should exit.
    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Id of the selected task, if any.
    pub(crate) fn selected_id(&self) -> Option<TaskId> {
        self.selected.clone()
    }

    /// The open picker, if any.
    pub(crate) fn picker(&self) -> Option<&Picker> {
        match &self.mode {
            InputMode::Pick(picker) => Some(picker),
            _ => None,
        }
    }

    /// The open picker's kind, if any.
    pub(crate) fn picker_kind(&self) -> Option<&PickerKind> {
        self.picker().map(|picker| &picker.kind)
    }

    /// The open picker's highlighted index, if any.
    pub(crate) fn picker_highlight(&self) -> Option<usize> {
        self.picker().map(|picker| picker.highlight)
    }

    /// Re-sync with the vault: rebuild the flattened list, keep the selected
    /// task when it still exists, otherwise fall back to the first row, and
    /// pick up the vault's current issue list for the persistent badge.
    pub(crate) fn refresh(&mut self) {
        let had_issues = !self.vault_issues.is_empty();
        // Hygiene: drop folds for tasks that no longer exist.
        self.collapsed.retain(|id| self.vault.get(id).is_some());
        // Multi-selection is id-based, so it survives folds; only vanished
        // tasks are pruned.
        self.marked.retain(|id| self.vault.get(id).is_some());
        self.rebuild_list();
        // A reload (watcher tick, editor round-trip, project switch) can
        // re-introduce a selection that a fold currently hides; unfold its
        // ancestors instead of losing the user's place.
        if let Some(id) = self.selected.clone() {
            if self.active_filter.is_none()
                && self.vault.get(&id).is_some()
                && self.list.index_of(&id).is_none()
            {
                self.unfold_ancestors(&id);
                self.rebuild_list();
            }
        }
        let still_selected = self
            .selected
            .as_ref()
            .is_some_and(|id| self.list.index_of(id).is_some());
        if !still_selected {
            self.selected = self.list.id_at(0).cloned();
            self.list_scroll = 0;
        }
        self.vault_issues = self.vault.issues().to_vec();
        if self.issues_cursor >= self.vault_issues.len() {
            self.issues_cursor = self.vault_issues.len().saturating_sub(1);
        }
        // The editor round-trip reloads the vault: when the issue the user was
        // looking at is gone, the overlay confirms and closes itself.
        if self.issues_open && had_issues && self.vault_issues.is_empty() {
            self.issues_open = false;
            self.set_toast("all vault issues resolved");
        }
        self.ensure_selection_visible();
    }

    /// Right-aligned badge for the persistent vault-issue count.
    pub(crate) fn issue_badge(&self) -> Option<String> {
        let count = self.vault_issues.len();
        (count > 0).then(|| format!("⚠ {}", issue_count_text(count)))
    }

    /// Route one key press to the active mode.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        self.handle_key_with(key, super::clipboard::copy);
    }

    pub(super) fn handle_key_with(
        &mut self,
        key: KeyEvent,
        copy: impl FnOnce(&str) -> std::io::Result<()>,
    ) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return;
        }
        // The delete confirmation is modal and handled before the issues
        // overlay so `?`/Esc can never leak into it.
        if matches!(self.mode, InputMode::ConfirmDelete { .. }) {
            self.handle_confirm_delete(key);
            return;
        }
        if self.keymap_open {
            // The keymap is modal: only scrolling, closing, and quitting are
            // active. Task hotkeys must never leak through to the list.
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.keymap_open = false,
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => self.scroll_keymap(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll_keymap(-1),
                _ => {}
            }
            return;
        }
        if self.issues_open {
            // Modal-lite: movement, edit, close, and quit act; everything else
            // (including task hotkeys like `x`) stays inert.
            match key.code {
                KeyCode::Esc => self.issues_open = false,
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => self.move_issues_cursor(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_issues_cursor(-1),
                KeyCode::Char('e') | KeyCode::Enter => self.edit_selected_issue(),
                _ => {}
            }
            return;
        }
        match self.mode {
            InputMode::Navigate => self.handle_list_with(key, copy),
            InputMode::Add { .. }
            | InputMode::Capture
            | InputMode::Rename { .. }
            | InputMode::Tag
            | InputMode::RegisterPath => {
                self.handle_input(key);
            }
            InputMode::Pick(_) => self.handle_pick(key),
            // Handled by the early modal check above; kept exhaustive.
            InputMode::ConfirmDelete { .. } => self.handle_confirm_delete(key),
        }
    }

    /// Poll the watcher: reload while idle, warn and keep the buffer while
    /// editing. Never writes to the vault.
    pub(crate) fn on_tick(&mut self) {
        // Keep "today" fresh: a session left open across midnight must still
        // render due and overdue correctly.
        self.today = Local::now().date_naive();
        // Toasts expire on tick counts, independent of the watcher; this must
        // run before the `!changed` early return.
        if let Some(toast) = &mut self.toast {
            toast.ticks_left = toast.ticks_left.saturating_sub(1);
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.ticks_left == 0)
        {
            self.toast = None;
        }
        let changed = self.watcher.as_ref().is_some_and(VaultWatcher::changed);
        if !changed {
            return;
        }
        if self.has_unsaved_buffer() {
            self.external_change_pending = true;
        } else {
            self.reload_now();
        }
    }

    /// Show a transient action message, replacing any previous toast.
    pub(crate) fn set_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            ticks_left: TOAST_TICKS,
        });
    }

    /// Prompt prefix for input modes; `None` while navigating. The `P` path
    /// prompt renders its buffer inside its own popup, not in the footer.
    pub(crate) fn prompt_prefix(&self) -> Option<String> {
        match self.mode.prompt()? {
            Prompt::NewTask { parent } => Some(format!(
                "new task under {}: ",
                self.parent_label(parent.as_ref())
            )),
            Prompt::Capture => Some(format!(
                "capture to {}: ",
                self.parent_label(self.config.capture_target.as_ref())
            )),
            Prompt::Rename => Some("rename to: ".to_owned()),
            Prompt::Tag => Some("tag: ".to_owned()),
            Prompt::Picker(prompt) => Some(prompt.to_owned()),
        }
    }

    /// The single footer hint row for the current mode. Prompts and inline
    /// picker status remain plain text; navigation, selection, and modal
    /// hints are structured so the renderer can style keys and labels.
    pub(crate) fn status_line(&self) -> FooterLine {
        if let Some(prefix) = self.prompt_prefix() {
            let prompt = format!("{prefix}{}", self.input);
            // Search keeps a status line (for example `no matches`) and a
            // selection hint visible next to the live query.
            let text = match (&self.mode, &self.status) {
                (InputMode::Pick(picker), Some(status)) if picker.kind.shows_status() => {
                    format!("{prompt}  [{status}]")
                }
                (InputMode::Pick(picker), None) => {
                    format!("{prompt}  [{}]", picker.kind.hint())
                }
                _ => prompt,
            };
            return FooterLine::Text(text);
        }
        if matches!(self.mode, InputMode::RegisterPath) {
            return FooterLine::Hints(REGISTER_HINTS);
        }
        if matches!(self.mode, InputMode::ConfirmDelete { .. }) {
            return FooterLine::Hints(CONFIRM_DELETE_HINTS);
        }
        if let Some(status) = &self.status {
            return FooterLine::Text(status.clone());
        }
        if self.issues_open {
            return FooterLine::Hints(ISSUES_HINTS);
        }
        if !self.marked.is_empty() {
            return FooterLine::Hints(SELECTION_HINTS);
        }
        FooterLine::Hints(NAVIGATE_HINTS)
    }

    /// Third footer row: project context and the sticky external-change
    /// flag. Never a transient message; toasts are a popup instead.
    pub(crate) fn context_text(&self) -> String {
        let scope = self
            .project
            .as_ref()
            .map_or_else(|| "vault".to_owned(), |project| project.slug.clone());
        let count = self.vault.len();
        let tasks = if count == 1 {
            "1 task".to_owned()
        } else {
            format!("{count} tasks")
        };
        let mut text = format!("{scope} · {tasks}");
        if let Some(filter) = &self.active_filter {
            text.push_str(&format!("  [filter {}]", filter.label()));
        }
        if self.external_change_pending {
            text.push_str("  [external change pending]");
        }
        text
    }

    /// Character length of the active input buffer (for cursor placement).
    pub(crate) fn input_buffer_len(&self) -> usize {
        self.input.chars().count()
    }

    /// Whether an unsaved buffer must block watcher reloads.
    fn has_unsaved_buffer(&self) -> bool {
        self.mode.holds_buffer()
    }

    /// The list is the only main view. `j`/`k` (and the arrows) walk the
    /// flattened tree in order, `gg`/`G` jump to the ends, and the task
    /// hotkeys operate on the selection.
    fn handle_list_with(&mut self, key: KeyEvent, copy: impl FnOnce(&str) -> std::io::Result<()>) {
        let was_pending_g = std::mem::take(&mut self.pending_g);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('J') => self.shift_selected_rank(1),
            KeyCode::Char('K') => self.shift_selected_rank(-1),
            KeyCode::Char('g') => {
                if was_pending_g {
                    self.select_index(0);
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                if !self.list.is_empty() {
                    self.select_index(self.list.len() - 1);
                }
            }
            KeyCode::Char('a') => self.start_add(false),
            KeyCode::Char('A') => self.start_add(true),
            KeyCode::Char('h') | KeyCode::Left => self.fold_selection(),
            KeyCode::Char('l') | KeyCode::Right => self.unfold_selection(),
            KeyCode::Char('N') => {
                self.mode = InputMode::Capture;
                self.input.clear();
                self.status = None;
            }
            KeyCode::Char('x') => self.toggle_done(),
            KeyCode::Char('!') => self.start_priority_pick(),
            KeyCode::Char('t') => self.start_tag_edit(),
            KeyCode::Char('f') => self.start_filter_pick(),
            KeyCode::Char('m') => self.start_move_pick(),
            KeyCode::Char('d') => self.start_delete(),
            KeyCode::Char('L') => self.start_create_link(),
            KeyCode::Char('r') => self.start_rename(),
            KeyCode::Char('y') => self.copy_selected_task_with(copy),
            KeyCode::Char('e') | KeyCode::Enter => self.start_edit(),
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('p') => self.start_project_pick(),
            KeyCode::Char('P') => self.start_register_path(),
            KeyCode::Char('o') => self.start_link_pick(),
            KeyCode::Char('?') if was_pending_g => self.open_issues(),
            KeyCode::Char('?') => self.open_keymap(),
            KeyCode::Tab => self.toggle_mark(),
            KeyCode::Esc => {
                // Selection is the innermost state: Esc clears it first, and
                // only a second Esc falls through to dismissing a toast or a
                // stale status.
                if self.marked.is_empty() {
                    self.status = None;
                    self.toast = None;
                } else {
                    self.marked.clear();
                }
            }
            _ => {}
        }
    }

    /// Move the selection `delta` rows through the flat order, clamped at the
    /// ends.
    fn move_selection(&mut self, delta: isize) {
        let Some(current) = self.selected.as_ref().and_then(|id| self.list.index_of(id)) else {
            self.select_index(0);
            return;
        };
        let last = self.list.len() as isize - 1;
        let next = (current as isize + delta).clamp(0, last) as usize;
        self.select_index(next);
    }

    /// Move the cursor task one place among its siblings. Rank changes never
    /// use or clear the multi-selection.
    fn shift_selected_rank(&mut self, delta: i32) {
        if self.active_filter.is_some() {
            self.set_toast("clear the filter to change rank");
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        match self.vault.shift_rank(&id, delta) {
            Ok(ShiftOutcome::Shifted) => self.refresh(),
            Ok(ShiftOutcome::OnlyChild) => self.set_toast("task has no siblings"),
            Ok(ShiftOutcome::AtBound) if delta < 0 => {
                self.set_toast("already first among siblings");
            }
            Ok(ShiftOutcome::AtBound) => self.set_toast("already last among siblings"),
            Err(error) => self.set_toast(format!("error: {error}")),
        }
    }

    /// Select the row at `index` (clamped) and scroll it into view.
    fn select_index(&mut self, index: usize) {
        if self.list.is_empty() {
            return;
        }
        let index = index.min(self.list.len() - 1);
        self.selected = self.list.id_at(index).cloned();
        self.ensure_selection_visible();
    }

    /// Rebuild the flattened list from the vault and the current folds.
    fn rebuild_list(&mut self) {
        self.list = match &self.active_filter {
            Some(filter) => {
                TaskList::build_filtered(&self.vault, &filter.task_filter(), self.today)
            }
            None => TaskList::build(&self.vault, &self.collapsed),
        };
    }

    /// `Tab`: toggle the cursor row in the multi-selection set. Folds never
    /// hide a mark, and the cursor itself is always a visible row.
    fn toggle_mark(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if !self.marked.remove(&id) {
            self.marked.insert(id);
        }
    }

    /// The tasks a selection action applies to: the marked ids in full-tree
    /// pre-order when anything is marked, otherwise the cursor task. Ids
    /// hidden by a fold stay in the set.
    fn action_ids(&self) -> Vec<TaskId> {
        if self.marked.is_empty() {
            return self.selected.clone().into_iter().collect();
        }
        let mut ordered = Vec::new();
        collect_marked_in_tree_order(&self.vault.tree(), &self.marked, &mut ordered);
        ordered
    }

    /// Remove every ancestor of `id` from the collapsed set, so a jump target
    /// can never land on a hidden row.
    fn unfold_ancestors(&mut self, id: &TaskId) {
        let mut current = self.vault.parent(id).cloned();
        while let Some(parent) = current {
            self.collapsed.remove(&parent);
            current = self.vault.parent(&parent).cloned();
        }
    }

    /// Select `id`, unfolding its ancestors first. Every programmatic jump
    /// (search, link, add) goes through here.
    fn select_id(&mut self, id: TaskId) {
        self.unfold_ancestors(&id);
        self.rebuild_list();
        if self.list.index_of(&id).is_some() {
            self.selected = Some(id);
        }
        self.ensure_selection_visible();
    }

    /// `h`/Left: collapse the selected parent, or jump to its parent when it
    /// has no visible children to collapse (navigator behavior).
    fn fold_selection(&mut self) {
        if self.active_filter.is_some() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        if !self.vault.children(&id).is_empty() && !self.collapsed.contains(&id) {
            self.collapsed.insert(id);
            self.rebuild_list();
            self.ensure_selection_visible();
            return;
        }
        // Already collapsed or childless: go up one level (no-op on a root).
        if let Some(parent) = self.vault.parent(&id).cloned() {
            self.select_id(parent);
        }
    }

    /// `l`/Right: expand the selected collapsed parent; a no-op otherwise.
    fn unfold_selection(&mut self) {
        if self.active_filter.is_some() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.collapsed.remove(&id) {
            self.rebuild_list();
            self.ensure_selection_visible();
        }
    }

    /// Scroll the minimum amount needed to keep the selected row inside the
    /// list pane. Applies the pure scroll clamp; drawing never mutates this
    /// state.
    pub(crate) fn ensure_selection_visible(&mut self) {
        let Some(index) = self.selected.as_ref().and_then(|id| self.list.index_of(id)) else {
            return;
        };
        self.list_scroll = ensure_selection_visible(
            self.list.len(),
            index,
            self.list_scroll,
            self.list_viewport.1 as usize,
        );
    }

    /// Record the list pane size and re-clamp the scroll so the selection
    /// stays visible. The event loop calls this before each draw; `render`
    /// itself takes `&App`.
    pub(crate) fn set_list_viewport(&mut self, width: u16, height: u16) {
        self.list_viewport = (width, height);
        self.ensure_selection_visible();
    }

    /// Record the middle pane size and re-clamp keymap scrolling. The event
    /// loop calls this on startup and resize so scrolling never gets stuck
    /// beyond the end after the terminal shrinks. The clamp is the single
    /// [`keymap_geometry`] derivation, so there is one clamp, not two.
    pub(crate) fn set_middle_viewport(&mut self, width: u16, height: u16) {
        self.middle_viewport = (width, height);
        self.keymap_scroll = self.keymap_geometry(self.keymap_scroll).scroll;
    }

    /// Popup geometry for the current middle viewport and a requested scroll.
    /// The single shared derivation behind clamping; rendering uses the same
    /// function, so the two can never disagree.
    fn keymap_geometry(&self, scroll: usize) -> super::keymap::KeymapGeometry {
        let columns = keymap_columns(self.middle_viewport.0);
        let content_lines = keymap_content_lines(columns);
        let content_width = keymap_content_width(columns);
        keymap_geometry(
            self.middle_viewport.0,
            self.middle_viewport.1,
            content_width,
            content_lines,
            scroll,
        )
    }

    /// Start the project picker (`p`), filtering over the registry.
    fn start_project_pick(&mut self) {
        if self.config.projects.is_empty() {
            self.set_toast("no registered projects");
            return;
        }
        self.mode = InputMode::Pick(Picker::new(PickerKind::Project));
        self.input.clear();
        self.status = None;
    }

    /// Registered projects matching the live query (all of them when empty).
    pub(crate) fn project_matches(&self) -> Vec<Project> {
        picker::project_matches(&self.input, &self.config.projects)
    }

    fn commit_project_pick(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let matches = self.project_matches();
        if matches.is_empty() {
            self.set_toast("no matching projects");
            return;
        }
        let project = matches[highlight.min(matches.len() - 1)].clone();
        self.switch_project(project);
    }

    /// Open another project's store and make it current, keeping the capture
    /// target and display settings.
    fn switch_project(&mut self, project: Project) {
        let Some(root) = self.store_root.clone() else {
            self.mode = InputMode::Navigate;
            self.input.clear();
            self.set_toast("no data directory is available");
            return;
        };
        match registry::open_store(&root, &project) {
            Ok(vault) => {
                self.vault = vault;
                self.watcher = self.vault.watch().ok();
                self.project = Some(project);
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.selected = None;
                self.list_scroll = 0;
                self.external_change_pending = false;
                self.refresh();
                let slug = self
                    .project
                    .as_ref()
                    .map_or_else(String::new, |project| project.slug.clone());
                self.set_toast(format!("switched to {slug}"));
            }
            Err(error) => {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.set_toast(format!("error: {error}"));
            }
        }
    }

    /// Start the `P` prompt that registers a new project by path.
    fn start_register_path(&mut self) {
        if self.store_root.is_none() {
            self.set_toast("no data directory is available");
            return;
        }
        self.mode = InputMode::RegisterPath;
        self.input.clear();
        self.status = None;
    }

    /// Commit the `P` path prompt: expand `~`, create the directory if it is
    /// missing, register it, and switch to its store. Errors toast and keep
    /// the prompt open so the path can be fixed.
    fn commit_register_path(&mut self) {
        let Some(root) = self.store_root.clone() else {
            self.mode = InputMode::Navigate;
            self.input.clear();
            self.set_toast("no data directory is available");
            return;
        };
        let Some(path) = expand_tilde(self.input.trim()) else {
            self.set_toast("cannot expand ~: HOME is not set");
            return;
        };
        if path.as_os_str().is_empty() {
            self.set_toast("path cannot be empty");
            return;
        }
        if let Err(error) = fs::create_dir_all(&path) {
            self.set_toast(format!("error: {error}"));
            return;
        }
        match registry::register_and_save_in(&mut self.config, &path, &root) {
            Ok(project) => self.switch_project(project),
            Err(error) => self.set_toast(format!("error: {error:#}")),
        }
    }

    /// Start the link picker (`o`) with the selection's outlinks and
    /// backlinks.
    fn start_link_pick(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let mut targets = self.vault.links(&id).to_vec();
        for backlink in self.vault.backlinks(&id) {
            if !targets.contains(backlink) {
                targets.push(backlink.clone());
            }
        }
        if targets.is_empty() {
            self.set_toast("no links on this task");
            return;
        }
        self.mode = InputMode::Pick(Picker::new(PickerKind::Link { targets }));
        self.input.clear();
        self.status = None;
    }

    /// Link targets matching the live query (all of them when empty).
    pub(crate) fn link_matches(&self) -> Vec<TaskId> {
        let Some(PickerKind::Link { targets }) = self.picker_kind() else {
            return Vec::new();
        };
        picker::link_matches(&self.input, targets, &self.vault)
    }

    fn commit_link_pick(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let matches = self.link_matches();
        if matches.is_empty() {
            self.set_toast("no matching links");
            return;
        }
        let target = matches[highlight.min(matches.len() - 1)].clone();
        self.mode = InputMode::Navigate;
        self.input.clear();
        if self.vault.get(&target).is_none() {
            self.set_toast("not found");
            return;
        }
        self.select_id(target);
        self.status = None;
    }

    /// Leave the open picker without committing. Search restores the
    /// selection from before it opened; any picker with a protected buffer
    /// then applies its pending reload.
    fn cancel_pick(&mut self) {
        let Some(kind) = self.picker_kind().cloned() else {
            return;
        };
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.status = None;
        let held_reload = kind.holds_buffer();
        if let PickerKind::Search { previous: Some(id) } = kind {
            self.selected = Some(id);
        }
        if held_reload {
            self.settle_pending_reload();
        }
    }

    /// The open picker's popup title and live entries, formatted for a
    /// `row_width`-cell row. `None` when no picker is open.
    ///
    /// The single seam between [`PickerKind`] and its presentation: `ui` draws
    /// whatever comes back and never matches on the kind itself.
    pub(crate) fn picker_popup(&self, row_width: usize) -> Option<PickerPopup> {
        let picker = self.picker()?;
        let entries = match &picker.kind {
            PickerKind::Search { .. } => self
                .search_matches()
                .iter()
                .map(|id| self.resolve_title(id))
                .collect(),
            PickerKind::Project => picker::project_rows(
                &self.project_matches(),
                &self.config.path_display,
                row_width,
            ),
            PickerKind::Link { .. } => self
                .link_matches()
                .iter()
                .map(|id| self.resolve_title(id))
                .collect(),
            PickerKind::Move { .. } => self
                .move_matches()
                .iter()
                .map(|target| match target {
                    None => "⌂ root".to_owned(),
                    Some(id) => self.resolve_title(id),
                })
                .collect(),
            PickerKind::Priority => self
                .priority_matches()
                .into_iter()
                .map(|priority| {
                    priority.map_or_else(|| "none".to_owned(), |value| value.to_string())
                })
                .collect(),
            PickerKind::Tags => self.tag_matches(),
            PickerKind::Filter => self
                .filter_matches()
                .into_iter()
                .map(|choice| choice.label())
                .collect(),
            PickerKind::CreateLink { .. } => self
                .create_link_matches()
                .iter()
                .map(|id| self.resolve_title(id))
                .collect(),
        };
        Some(PickerPopup {
            title: picker.kind.popup_title(),
            entries,
            highlight: picker.highlight,
        })
    }

    /// A task's live title, or `<id> (missing)` when it is not loaded.
    pub(crate) fn resolve_title(&self, id: &TaskId) -> String {
        self.vault
            .get(id)
            .map_or_else(|| format!("{id} (missing)"), |task| task.title.clone())
    }

    /// Number of matches for the active picker (0 when no picker is open).
    ///
    /// The entry count does not depend on the row width, so any width gives
    /// the same length; `0` only skips needless project-row formatting.
    fn pick_match_count(&self) -> usize {
        self.picker_popup(0).map_or(0, |popup| popup.entries.len())
    }

    /// Route shared picker transitions, then apply this client's Enter/Esc and
    /// query-feedback policies.
    fn handle_pick(&mut self, key: KeyEvent) {
        let count = self.pick_match_count();
        let action = match &mut self.mode {
            InputMode::Pick(picker) => picker.handle_input(&mut self.input, key, count),
            _ => return,
        };
        match action {
            picker::PickerInput::Cancel => self.cancel_pick(),
            picker::PickerInput::Commit => self.commit_pick(),
            picker::PickerInput::QueryChanged => self.query_changed(),
            picker::PickerInput::Continue => {}
        }
    }

    /// Search clears stale status feedback after a query edit; other picker
    /// kinds leave client status policy unchanged.
    fn query_changed(&mut self) {
        if self
            .picker_kind()
            .is_some_and(PickerKind::clears_status_on_query)
        {
            self.status = None;
        }
    }

    /// Commit the open picker using its kind-specific action.
    fn commit_pick(&mut self) {
        let Some(kind) = self.picker_kind().cloned() else {
            return;
        };
        match kind {
            PickerKind::Search { .. } => self.commit_search(),
            PickerKind::Project => self.commit_project_pick(),
            PickerKind::Link { .. } => self.commit_link_pick(),
            PickerKind::Move { .. } => self.commit_move_pick(),
            PickerKind::Priority => self.commit_priority_pick(),
            PickerKind::Tags => self.commit_tag_pick(),
            PickerKind::Filter => self.commit_filter_pick(),
            PickerKind::CreateLink { .. } => self.commit_create_link(),
        }
    }

    fn handle_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.finish_input(),
            KeyCode::Enter => {
                if matches!(self.mode, InputMode::RegisterPath) {
                    self.commit_register_path();
                } else {
                    self.commit_input();
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.push(character);
            }
            _ => {}
        }
    }

    /// Start a title search, remembering the selection for Esc.
    fn start_search(&mut self) {
        self.mode = InputMode::Pick(Picker::new(PickerKind::Search {
            previous: self.selected.clone(),
        }));
        self.input.clear();
        self.status = None;
    }

    /// Task ids whose title contains the query (case-insensitive), in full
    /// tree pre-order. Folds never hide a search match; selecting one unfolds
    /// its ancestors.
    pub(crate) fn search_matches(&self) -> Vec<TaskId> {
        if self.active_filter.is_none() {
            return picker::search_matches(&self.input, &self.vault);
        }
        let query = self.input.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        self.list
            .rows()
            .iter()
            .filter_map(|row| {
                self.vault
                    .get(&row.id)
                    .filter(|task| task.title.to_lowercase().contains(&query))
                    .map(|task| task.id.clone())
            })
            .collect()
    }

    /// Select the highlighted match and leave search. An empty match list
    /// keeps the prompt open with a `no matches` status.
    fn commit_search(&mut self) {
        self.settle_pending_reload();
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let matches = self.search_matches();
        if matches.is_empty() {
            self.status = Some("no matches".to_owned());
            return;
        }
        let selected = matches[highlight.min(matches.len() - 1)].clone();
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.select_id(selected);
        self.status = None;
    }

    pub(super) fn copy_selected_task_with(
        &mut self,
        copy: impl FnOnce(&str) -> std::io::Result<()>,
    ) {
        let Some(id) = self.selected.as_ref() else {
            self.set_toast("no task selected");
            return;
        };
        let Some(task) = self.vault.get(id) else {
            self.set_toast("task not found");
            return;
        };
        let project_path = self
            .project
            .as_ref()
            .map_or_else(|| self.vault.root(), |project| project.path.as_path());
        let paths = std::fs::canonicalize(project_path).and_then(|project| {
            let task_path = std::fs::canonicalize(self.vault.root().join(id.file_name()))?;
            Ok((project, task_path))
        });
        let (project_path, task_path) = match paths {
            Ok(paths) => paths,
            Err(error) => {
                self.set_toast(format!("error: {error}"));
                return;
            }
        };
        let metadata = format!(
            "Project path: {}\nTask file: {}\nID: {}\nTitle: {}\nState: {}",
            project_path.display(),
            task_path.display(),
            task.id,
            task.title,
            task.state
        );
        match copy(&metadata) {
            Ok(()) => self.set_toast(format!("copied task {}", task.id)),
            Err(error) => self.set_toast(format!("clipboard error: {error}")),
        }
    }

    /// Request the external editor for the selected task (`e`/`Enter`).
    ///
    /// The event loop performs the actual suspend/open/re-enter dance, so
    /// this stays terminal-free and testable.
    fn start_edit(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.vault.get(&id).is_none() {
            return;
        }
        self.pending_edit = Some(self.vault.root().join(id.file_name()));
        self.status = None;
    }

    /// Open the `g?` overlay on the first issue, closing the keymap first.
    fn open_issues(&mut self) {
        self.keymap_open = false;
        self.issues_open = true;
        self.issues_cursor = 0;
    }

    /// Open the `?` keymap, closing the issues overlay first.
    fn open_keymap(&mut self) {
        self.issues_open = false;
        self.keymap_open = true;
        self.keymap_scroll = 0;
    }

    /// Move the keymap scroll offset and clamp it to the measured viewport.
    /// Every number comes from [`keymap_geometry`].
    fn scroll_keymap(&mut self, delta: isize) {
        let requested = (self.keymap_scroll as isize + delta).max(0) as usize;
        self.keymap_scroll = self.keymap_geometry(requested).scroll;
    }

    /// Move the overlay highlight `delta` issues, clamped at the ends.
    fn move_issues_cursor(&mut self, delta: isize) {
        if self.vault_issues.is_empty() {
            return;
        }
        let last = self.vault_issues.len() as isize - 1;
        self.issues_cursor = (self.issues_cursor as isize + delta).clamp(0, last) as usize;
    }

    /// Request the external editor for the highlighted issue. The issue path
    /// is the vault file, so the existing `pending_edit` hand-off applies
    /// unchanged.
    fn edit_selected_issue(&mut self) {
        if let Some(issue) = self.vault_issues.get(self.issues_cursor) {
            self.pending_edit = Some(issue.path.clone());
        }
    }

    /// Take the pending external-edit request, if any.
    pub(crate) fn take_pending_edit(&mut self) -> Option<PathBuf> {
        self.pending_edit.take()
    }

    /// `r`: open a one-line title prompt prefilled with the cursor task's
    /// current title. Marks are ignored — only the cursor task is renamed —
    /// and they survive the prompt. The input buffer starts holding the old
    /// title with the cursor at its end (see the prompt rendering).
    fn start_rename(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(title) = self.vault.get(&id).map(|task| task.title.clone()) else {
            return;
        };
        self.mode = InputMode::Rename { id };
        self.input = title;
        self.status = None;
    }

    /// Commit the `r` prompt through the vault title-sync lifecycle. An
    /// unchanged title writes nothing; the toast appends the number of mirror
    /// files the vault rewrote.
    fn commit_rename(&mut self, id: TaskId, title: String) {
        self.settle_pending_reload();
        let Some(old_title) = self.vault.get(&id).map(|task| task.title.clone()) else {
            self.mode = InputMode::Navigate;
            self.input.clear();
            self.set_toast(format!("error: task not found: {id}"));
            return;
        };
        if title == old_title {
            self.finish_input();
            return;
        }
        let outcome = match self.vault.set_title(&id, &title) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.refresh();
                self.set_toast(format!("error: {error}"));
                return;
            }
        };
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.refresh();
        match outcome.mirror_files_updated {
            0 => self.set_toast(format!("renamed to {title}")),
            written => self.set_toast(format!(
                "renamed to {title} · {written} {} updated",
                if written == 1 { "link" } else { "links" }
            )),
        }
    }

    /// Start an add prompt: `child` adds under the selection, otherwise a
    /// sibling of the selection.
    fn start_add(&mut self, sibling: bool) {
        let insert_after = sibling.then(|| self.selected_id()).flatten();
        let parent = if sibling {
            insert_after
                .as_ref()
                .and_then(|id| self.vault.parent(id).cloned())
        } else {
            self.selected_id()
        };
        self.mode = InputMode::Add {
            parent,
            insert_after,
        };
        self.input.clear();
        self.status = None;
    }

    fn toggle_done(&mut self) {
        let ids = self.action_ids();
        if ids.is_empty() {
            return;
        }
        let selection = !self.marked.is_empty();
        let mut last_task: Option<tt::Task> = None;
        for id in &ids {
            let Some(next) = self.vault.get(id).map(|task| task.state.toggle_done()) else {
                continue;
            };
            match self.vault.set_state(id, next) {
                Ok(task) => last_task = Some(task),
                Err(error) => {
                    self.set_toast(format!("error: {error}"));
                    return;
                }
            }
        }
        self.refresh();
        if !selection && ids.len() == 1 {
            if let Some(task) = last_task {
                self.set_toast(format!("{} {}", task.state, task.title));
            }
        } else {
            let count = ids.len();
            self.set_toast(format!(
                "cycled {count} {}",
                if count == 1 { "task" } else { "tasks" }
            ));
            self.marked.clear();
        }
    }

    /// Open the session-only filter picker (`f`).
    fn start_filter_pick(&mut self) {
        self.mode = InputMode::Pick(Picker::new(PickerKind::Filter));
        self.input.clear();
        self.status = None;
    }

    /// Filter choices matching the live query.
    pub(crate) fn filter_matches(&self) -> Vec<FilterChoice> {
        if !matches!(self.picker_kind(), Some(PickerKind::Filter)) {
            return Vec::new();
        }
        picker::filter_matches(&self.input, &self.vault, self.active_filter.is_some())
    }

    /// Apply or clear the highlighted filter while retaining the selected id
    /// when it remains visible.
    fn commit_filter_pick(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let matches = self.filter_matches();
        let Some(choice) = matches.get(highlight.min(matches.len().saturating_sub(1))) else {
            return;
        };
        self.active_filter = match choice {
            FilterChoice::Clear => None,
            FilterChoice::Apply(filter) => Some(filter.clone()),
        };
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.status = None;
        self.refresh();
    }

    /// Start tag editing (`t`) for the active selection set. Existing tags
    /// use the shared picker; the first tag uses a plain prompt.
    fn start_tag_edit(&mut self) {
        if self.action_ids().is_empty() {
            return;
        }
        self.mode = if picker::all_tags(&self.vault).is_empty() {
            InputMode::Tag
        } else {
            InputMode::Pick(Picker::new(PickerKind::Tags))
        };
        self.input.clear();
        self.status = None;
    }

    /// Existing tags matching the live picker query.
    pub(crate) fn tag_matches(&self) -> Vec<String> {
        if !matches!(self.picker_kind(), Some(PickerKind::Tags)) {
            return Vec::new();
        }
        picker::tag_matches(&self.input, &self.vault)
    }

    /// Commit the tag picker: toggle the highlighted existing tag, or add a
    /// normalized non-empty query when no existing tag matches it.
    fn commit_tag_pick(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        self.settle_pending_reload();
        let matches = self.tag_matches();
        let (tag, toggle) = if matches.is_empty() {
            let tag = picker::normalize_tag(&self.input);
            if tag.is_empty() {
                return;
            }
            (tag, false)
        } else {
            (matches[highlight.min(matches.len() - 1)].clone(), true)
        };
        self.apply_tag(&tag, toggle);
    }

    /// Add `tag` to every action task, or toggle it independently when
    /// `toggle` is true.
    fn apply_tag(&mut self, tag: &str, toggle: bool) {
        let ids = self.action_ids();
        let had_marks = !self.marked.is_empty();
        let mut added = false;
        let mut removed = false;
        for id in &ids {
            let Some(task) = self.vault.get(id) else {
                continue;
            };
            let mut tags = task.tags.clone();
            if toggle && tags.iter().any(|existing| existing == tag) {
                tags.retain(|existing| existing != tag);
                removed = true;
            } else if !tags.iter().any(|existing| existing == tag) {
                tags.push(tag.to_owned());
                added = true;
            }
            if let Err(error) = self.vault.set_tags(id, tags) {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.refresh();
                self.set_toast(format!("error: {error}"));
                return;
            }
        }
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.status = None;
        self.refresh();
        let action = match (added, removed) {
            (true, true) => "toggled",
            (false, true) => "removed",
            _ => "added",
        };
        self.set_toast(format!("tag #{tag} {action}"));
        if had_marks {
            self.marked.clear();
        }
    }

    /// Commit the first-tag prompt. Blank input closes without writing.
    fn commit_tag_prompt(&mut self) {
        self.settle_pending_reload();
        let tag = picker::normalize_tag(&self.input);
        if tag.is_empty() {
            self.finish_input();
            return;
        }
        self.apply_tag(&tag, false);
    }

    /// Start the priority picker (`!`) for the active selection set.
    fn start_priority_pick(&mut self) {
        if self.action_ids().is_empty() {
            return;
        }
        self.mode = InputMode::Pick(Picker::new(PickerKind::Priority));
        self.input.clear();
        self.status = None;
    }

    /// Priority values matching the live query.
    pub(crate) fn priority_matches(&self) -> Vec<Option<Priority>> {
        if !matches!(self.picker_kind(), Some(PickerKind::Priority)) {
            return Vec::new();
        }
        picker::priority_matches(&self.input)
    }

    /// Commit the priority picker for the cursor or every marked task.
    fn commit_priority_pick(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let matches = self.priority_matches();
        if matches.is_empty() {
            return;
        }
        let priority = matches[highlight.min(matches.len() - 1)];
        let ids = self.action_ids();
        let had_marks = !self.marked.is_empty();
        for id in &ids {
            if let Err(error) = self.vault.set_priority(id, priority) {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.refresh();
                self.set_toast(format!("error: {error}"));
                return;
            }
        }
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.status = None;
        self.refresh();
        self.set_toast(format!(
            "priority {}",
            priority.map_or("none", Priority::as_str)
        ));
        if had_marks {
            self.marked.clear();
        }
    }

    /// Start the move picker (`m`) for the active selection set.
    fn start_move_pick(&mut self) {
        let ids = self.action_ids();
        if ids.is_empty() {
            return;
        }
        self.mode = InputMode::Pick(Picker::new(PickerKind::Move { moving: ids }));
        self.input.clear();
        self.status = None;
    }

    /// Move targets matching the live query: the root entry, then every task
    /// outside the moving set and its descendants.
    pub(crate) fn move_matches(&self) -> Vec<Option<TaskId>> {
        let Some(PickerKind::Move { moving }) = self.picker_kind() else {
            return Vec::new();
        };
        picker::move_matches(&self.input, &self.vault, moving)
    }

    /// Commit the move picker: reparent every moving task to the highlighted
    /// target, unfold the new parent, and select the first moved task.
    fn commit_move_pick(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let Some(PickerKind::Move { moving }) = self.picker_kind().cloned() else {
            return;
        };
        let matches = self.move_matches();
        if matches.is_empty() {
            return;
        }
        let target = matches[highlight.min(matches.len() - 1)].clone();
        for id in &moving {
            if let Err(error) = self.vault.set_parent(id, target.as_ref()) {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.refresh();
                self.set_toast(format!("error: {error}"));
                return;
            }
        }
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.status = None;
        if let Some(first) = moving.first().cloned() {
            self.select_id(first);
        }
        let count = moving.len();
        self.set_toast(format!(
            "moved {count} {}",
            if count == 1 { "task" } else { "tasks" }
        ));
        self.marked.clear();
    }

    /// `d`: ask before deleting the active selection (or the cursor task
    /// when nothing is marked).
    fn start_delete(&mut self) {
        let ids = self.action_ids();
        if ids.is_empty() {
            return;
        }
        // Cancel is the highlighted default: a bare Enter never deletes.
        self.mode = InputMode::ConfirmDelete { ids, button: 1 };
        self.status = None;
    }

    /// Questions shown by the delete confirmation: the title for a single
    /// task or a count, plus how many descendants die with them. Descendants
    /// already requested are excluded, matching what [`Vault::delete`] removes
    /// beyond the requested tasks themselves.
    pub(crate) fn confirm_delete_lines(&self) -> Vec<String> {
        let InputMode::ConfirmDelete { ids, .. } = &self.mode else {
            return Vec::new();
        };
        let descendants = self.vault.descendant_count(ids);
        let descendant_clause = format!(
            "{descendants} {}",
            if descendants == 1 {
                "descendant"
            } else {
                "descendants"
            }
        );
        match ids.as_slice() {
            [only] => {
                let title = self
                    .vault
                    .get(only)
                    .map_or_else(|| only.to_string(), |task| task.title.clone());
                if descendants == 0 {
                    vec![format!("Delete \"{title}\"?")]
                } else {
                    vec![format!("Delete \"{title}\" and its {descendant_clause}?")]
                }
            }
            _ => {
                if descendants == 0 {
                    vec![format!("Delete {} tasks?", ids.len())]
                } else {
                    vec![format!(
                        "Delete {} tasks and their {descendant_clause}?",
                        ids.len()
                    )]
                }
            }
        }
    }

    /// Labels of the delete-confirmation buttons, in display order.
    pub(crate) fn confirm_delete_buttons(&self) -> &'static [&'static str] {
        &["Delete", "Cancel"]
    }

    /// Index of the highlighted delete-confirmation button.
    pub(crate) fn confirm_delete_button(&self) -> usize {
        match &self.mode {
            InputMode::ConfirmDelete { button, .. } => *button,
            _ => 0,
        }
    }

    /// Route one key press to the delete confirmation: button navigation and
    /// activation, direct `y`/`d` confirmation, and Esc/`n` cancelling.
    fn handle_confirm_delete(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => self.move_confirm_button(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.move_confirm_button(1),
            KeyCode::Enter => self.activate_confirm_button(),
            KeyCode::Char('y') | KeyCode::Char('d') => self.commit_delete(),
            KeyCode::Esc | KeyCode::Char('n') => self.cancel_delete(),
            _ => {}
        }
    }

    /// Move the delete-confirmation highlight by `delta`, wrapping around
    /// Delete and Cancel.
    fn move_confirm_button(&mut self, delta: isize) {
        if let InputMode::ConfirmDelete { button, .. } = &mut self.mode {
            *button = (*button as isize + delta).rem_euclid(2) as usize;
        }
    }

    /// Run the highlighted delete-confirmation button.
    fn activate_confirm_button(&mut self) {
        match self.mode {
            InputMode::ConfirmDelete { button: 0, .. } => self.commit_delete(),
            InputMode::ConfirmDelete { .. } => self.cancel_delete(),
            _ => {}
        }
    }

    /// Leave the delete confirmation without deleting.
    fn cancel_delete(&mut self) {
        self.mode = InputMode::Navigate;
        self.settle_pending_reload();
    }

    /// `L`: open the link picker for the cursor task. Marks are ignored; a
    /// link is appended to the task the cursor is on. Toasting instead of
    /// opening keeps the `L` key honest on a one-task vault.
    fn start_create_link(&mut self) {
        let Some(source) = self.selected.clone() else {
            return;
        };
        if self.vault.get(&source).is_none() {
            return;
        }
        if picker::create_link_matches("", &self.vault, &source).is_empty() {
            self.set_toast("no tasks to link");
            return;
        }
        self.mode = InputMode::Pick(Picker::new(PickerKind::CreateLink { source }));
        self.input.clear();
        self.status = None;
    }

    /// Link-target candidates matching the live query: the full tree minus
    /// the source task.
    pub(crate) fn create_link_matches(&self) -> Vec<TaskId> {
        let Some(PickerKind::CreateLink { source }) = self.picker_kind() else {
            return Vec::new();
        };
        picker::create_link_matches(&self.input, &self.vault, source)
    }

    /// Commit the link picker: append `[[{id}.md|{Title}]]` to the source
    /// task's body (or the bare path when the title cannot carry an alias),
    /// toast the target's title, and stay on the source task so the new
    /// backlink count is visible in the preview.
    fn commit_create_link(&mut self) {
        let Some(highlight) = self.picker_highlight() else {
            return;
        };
        let Some(PickerKind::CreateLink { source }) = self.picker_kind().cloned() else {
            return;
        };
        let matches = self.create_link_matches();
        if matches.is_empty() {
            return;
        }
        let target = matches[highlight.min(matches.len() - 1)].clone();
        let title = self
            .vault
            .get(&target)
            .map_or_else(|| target.to_string(), |task| task.title.clone());
        let body = self
            .vault
            .get(&source)
            .map_or_else(String::new, |task| task.body.clone());
        match self
            .vault
            .set_body(&source, &append_wikilink(&body, &target, &title))
        {
            Ok(_) => {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.status = None;
                self.refresh();
                self.set_toast(format!("linked to {title}"));
            }
            Err(error) => {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.set_toast(format!("error: {error}"));
            }
        }
    }

    /// Delete the confirmed snapshot, reload from disk, clear the marks, and
    /// report what happened. On error the vault may hold a partial delete, so
    /// it is reloaded too; remaining marks survive the reload's pruning.
    fn commit_delete(&mut self) {
        let InputMode::ConfirmDelete { ids, .. } = self.mode.clone() else {
            return;
        };
        self.mode = InputMode::Navigate;
        match self.vault.delete(&ids) {
            Ok(outcome) => {
                self.reload_now();
                self.marked.clear();
                self.set_toast(delete_toast(outcome));
            }
            Err(error) => {
                self.reload_now();
                self.set_toast(format!("error: {error}"));
            }
        }
    }

    fn commit_input(&mut self) {
        let title = self.input.trim().to_owned();
        if title.is_empty() {
            // Matches `a`/`A`: a blank title cancels instead of writing.
            self.finish_input();
            return;
        }
        if let InputMode::Rename { id } = self.mode.clone() {
            self.commit_rename(id, title);
            return;
        }
        if matches!(self.mode, InputMode::Tag) {
            self.commit_tag_prompt();
            return;
        }
        let (parent, insert_after) = match &self.mode {
            InputMode::Add {
                parent,
                insert_after,
            } => (parent.clone(), insert_after.clone()),
            InputMode::Capture => (self.config.capture_target.clone(), None),
            InputMode::Navigate
            | InputMode::Rename { .. }
            | InputMode::Tag
            | InputMode::Pick(_)
            | InputMode::RegisterPath
            | InputMode::ConfirmDelete { .. } => return,
        };

        self.settle_pending_reload();

        let new_task = NewTask {
            parent,
            insert_after,
            ..NewTask::new(title)
        };
        match self.vault.add(new_task) {
            Ok(task) => {
                self.mode = InputMode::Navigate;
                self.input.clear();
                self.refresh();
                self.select_id(task.id.clone());
                self.set_toast(format!("added {}", task.id));
            }
            Err(error) => self.set_toast(format!("error: {error}")),
        }
    }

    /// Cancel the current input mode without adding anything.
    fn finish_input(&mut self) {
        self.mode = InputMode::Navigate;
        self.input.clear();
        self.settle_pending_reload();
    }

    /// After leaving an input mode, apply any change that arrived while the
    /// buffer was held.
    fn settle_pending_reload(&mut self) {
        if self.external_change_pending {
            self.reload_now();
        }
    }

    /// Rescan the vault. Issues land in [`App::vault_issues`] for the badge
    /// and overlay; transient action feedback in `status` is left alone. Both
    /// the selected task and the scroll position survive: a reload must not
    /// steal the user's place.
    ///
    /// The vault compares titles across the rescan and synchronizes mirror
    /// aliases. A sync failure is toasted but never stops the reload; a reload
    /// with no title change writes nothing.
    pub(crate) fn reload_now(&mut self) {
        let reload = self.vault.reload();
        self.external_change_pending = false;
        self.refresh();
        if let Err(error) = reload {
            self.set_toast(format!("error: {error}"));
        }
    }

    /// Rescan after the external editor exits. When the edit raised the issue
    /// count the user is told immediately; a flat or falling count stays
    /// quiet so existing action feedback (and the all-resolved toast) is not
    /// clobbered. Watcher reloads use [`App::reload_now`] instead.
    pub(crate) fn reload_after_edit(&mut self) {
        let before = self.vault_issues.len();
        self.reload_now();
        let after = self.vault_issues.len();
        if after > before {
            self.set_toast(format!("⚠ {} — press g?", issue_count_text(after)));
        }
    }

    fn parent_label(&self, id: Option<&TaskId>) -> String {
        match id {
            Some(id) => self
                .vault
                .get(id)
                .map_or_else(|| id.to_string(), |task| task.title.clone()),
            None => "root".to_owned(),
        }
    }
}

/// Append the ids of `nodes` and their descendants that are in `wanted`, in
/// full-tree pre-order. Folds are ignored, so a marked id stays ordered even
/// when its row is hidden.
fn collect_marked_in_tree_order(
    nodes: &[TreeNode<'_>],
    wanted: &BTreeSet<TaskId>,
    out: &mut Vec<TaskId>,
) {
    for node in nodes {
        if wanted.contains(&node.task.id) {
            out.push(node.task.id.clone());
        }
        collect_marked_in_tree_order(&node.children, wanted, out);
    }
}

/// Append the full link form to `body`: `[[{id}.md|{Title}]]`, or the bare
/// `[[{id}.md]]` when the title contains `|` or `]` (which would break the
/// alias syntax). The link stands alone when the body is blank; otherwise
/// the body loses extra trailing newlines and a blank line separates the
/// link.
fn append_wikilink(body: &str, target: &TaskId, title: &str) -> String {
    let file = target.file_name();
    let link = if title.contains('|') || title.contains(']') {
        format!("[[{file}]]")
    } else {
        format!("[[{file}|{title}]]")
    };
    let trimmed = body.trim_end_matches(['\n', '\r']);
    if trimmed.trim().is_empty() {
        link
    } else {
        format!("{trimmed}\n\n{link}")
    }
}

/// Toast for a completed delete: the total file count, descendants included.
fn delete_toast(outcome: DeleteOutcome) -> String {
    format!(
        "deleted {} {}",
        outcome.deleted,
        if outcome.deleted == 1 {
            "task"
        } else {
            "tasks"
        }
    )
}

/// `1 issue` / `N issues`, shared by the header badge and the edit-return
/// toast so they always agree.
fn issue_count_text(count: usize) -> String {
    match count {
        1 => "1 issue".to_owned(),
        _ => format!("{count} issues"),
    }
}

/// Expand a leading `~` in a user-typed path using `$HOME`.
///
/// Returns `None` only when the path starts with `~` and `$HOME` is unset;
/// paths without a tilde pass through unchanged. `~user` is not expanded.
pub(crate) fn expand_tilde(raw: &str) -> Option<PathBuf> {
    expand_tilde_with_home(raw, env::var_os("HOME").map(PathBuf::from).as_deref())
}

/// [`expand_tilde`] against an explicit home directory.
pub(crate) fn expand_tilde_with_home(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    if raw == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    Some(PathBuf::from(raw))
}

/// Pure scroll clamp: the first visible row that keeps `selected` inside a
/// `height`-row viewport of a list with `rows` rows.
pub(crate) fn ensure_selection_visible(
    rows: usize,
    selected: usize,
    scroll: usize,
    height: usize,
) -> usize {
    let height = height.max(1);
    let mut scroll = scroll;
    if selected < scroll {
        scroll = selected;
    } else if selected >= scroll + height {
        scroll = selected + 1 - height;
    }
    scroll.min(rows.saturating_sub(height))
}
