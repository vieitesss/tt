//! One picker state machine for every type-to-filter TUI choice.
//!
//! Query editing and highlight navigation are shared by every picker through
//! [`Picker::handle_input`]. The per-kind differences — the item source, what
//! Enter does, and what Esc restores — live on [`PickerKind`] and in the
//! clients' commit/cancel handlers.

use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tt::{
    path_display, PathDisplay, Priority, Project, TaskFilter, TaskId, TaskState, TreeNode, Vault,
};

use super::text::{pop_word, text_width, truncate_title};

/// What a picker is choosing between, and how its keys differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerKind {
    /// `/`: jump to a task whose title matches the query. `previous` is the
    /// selection restored on Esc.
    Search {
        /// Selection before the search opened.
        previous: Option<TaskId>,
    },
    /// `p`: switch to another registered project.
    Project,
    /// `o`: jump to one of the selected task's links. `targets` holds
    /// outlinks followed by backlinks, de-duplicated.
    Link {
        /// Candidate task ids.
        targets: Vec<TaskId>,
    },
    /// `m`: move the active subtree within this Project or choose another.
    Move {
        /// Top-level selected tasks; these and all their descendants are
        /// excluded from in-project parent candidates.
        moving: Vec<TaskId>,
    },
    /// `m`: choose another registered Project as the move destination.
    MoveProject {
        /// Top-level selected tasks being moved.
        moving: Vec<TaskId>,
    },
    /// `m`: choose a parent in the selected destination Project.
    MoveParent {
        /// Top-level selected tasks being moved.
        moving: Vec<TaskId>,
        /// Registered destination Project.
        project: Project,
    },
    /// `!`: set or clear priority on the active task set.
    Priority,
    /// `t`: toggle an existing tag or add a new typed tag.
    Tags,
    /// `f`: choose one session-only list filter.
    Filter,
    /// `L`: append a `[[id.md|Title]]` link to the cursor task's body.
    /// `source` is the task being linked from and is excluded from the
    /// candidates.
    CreateLink {
        /// Task that will gain the link.
        source: TaskId,
    },
}

impl PickerKind {
    /// Whether an unsaved buffer in this picker must block watcher reloads.
    /// Search and tags hold typed text worth protecting.
    pub(crate) fn holds_buffer(&self) -> bool {
        matches!(self, Self::Search { .. } | Self::Tags)
    }

    /// Whether the prompt keeps a status message (for example `no matches`)
    /// visible next to the live query; only search does.
    pub(crate) fn shows_status(&self) -> bool {
        matches!(self, Self::Search { .. })
    }

    /// Whether typing clears the current status message so stale feedback
    /// never sits next to a changed query; only search does.
    pub(crate) fn clears_status_on_query(&self) -> bool {
        matches!(self, Self::Search { .. })
    }

    /// Prompt prefix for the status line.
    pub(crate) fn prompt(&self) -> String {
        match self {
            Self::Search { .. } => "search: ".to_owned(),
            Self::Project => "project: ".to_owned(),
            Self::Link { .. } => "link: ".to_owned(),
            Self::Move { .. } => "move under: ".to_owned(),
            Self::MoveProject { .. } => "move to project: ".to_owned(),
            Self::MoveParent { project, .. } => format!("move under {}: ", project.slug),
            Self::Priority => "priority: ".to_owned(),
            Self::Tags => "tag: ".to_owned(),
            Self::Filter => "filter: ".to_owned(),
            Self::CreateLink { .. } => "link to: ".to_owned(),
        }
    }

    /// Key hint shown next to the prompt while no status is set.
    pub(crate) fn hint(&self) -> &'static str {
        match self {
            Self::Search { .. } => "↑↓ select · enter open · esc cancel",
            Self::Project => "↑↓ select · enter switch · esc cancel",
            Self::Link { .. } => "↑↓ select · enter jump · esc cancel",
            Self::Move { .. } => "↑↓ parent/project · enter choose · esc cancel",
            Self::MoveProject { .. } => "↑↓ select · enter choose · esc cancel",
            Self::MoveParent { .. } => "↑↓ parent/root · enter move · esc cancel",
            Self::Priority => "↑↓ select · enter set · esc cancel",
            Self::Tags => "↑↓ select · enter toggle/add · esc cancel",
            Self::Filter => "↑↓ select · enter apply · esc cancel",
            Self::CreateLink { .. } => "↑↓ select · enter link · esc cancel",
        }
    }

    /// Title of this picker's popup.
    pub(crate) fn popup_title(&self) -> &'static str {
        match self {
            Self::Search { .. } => "matches",
            Self::Project => "projects",
            Self::Link { .. } => "links",
            Self::Move { .. } => "move under…",
            Self::MoveProject { .. } => "move to project…",
            Self::MoveParent { .. } => "move under…",
            Self::Priority => "priority",
            Self::Tags => "tags",
            Self::Filter => "filter",
            Self::CreateLink { .. } => "link to…",
        }
    }
}

/// Result of reducing a key against the shared picker interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerInput {
    /// The owning client should apply its cancel policy.
    Cancel,
    /// The owning client should apply its commit policy.
    Commit,
    /// The query changed and client-specific feedback may need clearing.
    QueryChanged,
    /// Keep the picker open without changing its query.
    Continue,
}

/// Highlight state for one open picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picker {
    /// What is being picked.
    pub(crate) kind: PickerKind,
    /// Index into the live match list.
    pub(crate) highlight: usize,
}

impl Picker {
    /// Open a picker at the first match.
    pub(crate) fn new(kind: PickerKind) -> Self {
        Self { kind, highlight: 0 }
    }

    /// Apply the shared query-editing and highlight-navigation transitions.
    /// `match_count` is the number of candidates for the current query, used
    /// only to clamp navigation. Enter and Esc are returned to the client so
    /// App and Launch can retain their own commit/cancel policy.
    pub(crate) fn handle_input(
        &mut self,
        query: &mut String,
        key: KeyEvent,
        match_count: usize,
    ) -> PickerInput {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => PickerInput::Cancel,
            KeyCode::Enter => PickerInput::Commit,
            KeyCode::Down => {
                self.move_highlight(1, match_count);
                PickerInput::Continue
            }
            KeyCode::Up => {
                self.move_highlight(-1, match_count);
                PickerInput::Continue
            }
            KeyCode::Char('n') if control => {
                self.move_highlight(1, match_count);
                PickerInput::Continue
            }
            KeyCode::Char('p') if control => {
                self.move_highlight(-1, match_count);
                PickerInput::Continue
            }
            KeyCode::Char('w') if control => {
                pop_word(query);
                self.highlight = 0;
                PickerInput::QueryChanged
            }
            KeyCode::Backspace => {
                query.pop();
                self.highlight = 0;
                PickerInput::QueryChanged
            }
            // Every printable character, including `j`/`k`, is query text;
            // only arrows and ctrl-n/ctrl-p move the highlight.
            KeyCode::Char(character) if !control => {
                query.push(character);
                self.highlight = 0;
                PickerInput::QueryChanged
            }
            _ => PickerInput::Continue,
        }
    }

    fn move_highlight(&mut self, delta: isize, match_count: usize) {
        if match_count == 0 {
            return;
        }
        let last = match_count as isize - 1;
        self.highlight = (self.highlight as isize + delta).clamp(0, last) as usize;
    }
}

/// Priority choices matching `query`, always in high, med, low, none order.
pub(crate) fn priority_matches(query: &str) -> Vec<Option<Priority>> {
    let query = query.trim().to_lowercase();
    [
        Some(Priority::High),
        Some(Priority::Med),
        Some(Priority::Low),
        None,
    ]
    .into_iter()
    .filter(|priority| {
        let label = priority.map_or("none", Priority::as_str);
        query.is_empty() || label.contains(&query)
    })
    .collect()
}

/// One active session-only filter criterion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterCriterion {
    /// Match one lifecycle state.
    State(TaskState),
    /// Match one exact priority.
    Priority(Priority),
    /// Match one tag, including its nested descendants.
    Tag(String),
}

impl FilterCriterion {
    /// Compact label used in the picker and footer.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::State(state) => state.to_string(),
            Self::Priority(priority) => priority.to_string(),
            Self::Tag(tag) => format!("#{tag}"),
        }
    }

    /// Convert this single criterion into the vault's AND-capable filter.
    pub(crate) fn task_filter(&self) -> TaskFilter {
        match self {
            Self::State(state) => TaskFilter {
                state: Some(*state),
                ..TaskFilter::default()
            },
            Self::Priority(priority) => TaskFilter {
                priority: Some(*priority),
                ..TaskFilter::default()
            },
            Self::Tag(tag) => TaskFilter {
                tag: Some(tag.clone()),
                ..TaskFilter::default()
            },
        }
    }
}

/// One entry in the filter picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterChoice {
    /// Restore the normal tree.
    Clear,
    /// Apply one criterion.
    Apply(FilterCriterion),
}

impl FilterChoice {
    /// Human-readable picker label.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Clear => "clear filter".to_owned(),
            Self::Apply(criterion) => criterion.label(),
        }
    }
}

/// Filter-picker choices matching `query`.
pub(crate) fn filter_matches(query: &str, vault: &Vault, filter_active: bool) -> Vec<FilterChoice> {
    let mut choices = Vec::new();
    if filter_active {
        choices.push(FilterChoice::Clear);
    }
    choices.extend([
        FilterChoice::Apply(FilterCriterion::State(TaskState::Open)),
        FilterChoice::Apply(FilterCriterion::State(TaskState::Done)),
        FilterChoice::Apply(FilterCriterion::State(TaskState::Cancelled)),
        FilterChoice::Apply(FilterCriterion::Priority(Priority::High)),
        FilterChoice::Apply(FilterCriterion::Priority(Priority::Med)),
        FilterChoice::Apply(FilterCriterion::Priority(Priority::Low)),
    ]);
    choices.extend(
        all_tags(vault)
            .into_iter()
            .map(FilterCriterion::Tag)
            .map(FilterChoice::Apply),
    );
    let query = query.trim().to_lowercase();
    choices
        .into_iter()
        .filter(|choice| query.is_empty() || choice.label().to_lowercase().contains(&query))
        .collect()
}

/// Every unique normalized tag in the vault, sorted.
pub(crate) fn all_tags(vault: &Vault) -> Vec<String> {
    vault
        .tasks()
        .flat_map(|task| task.tags.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Existing tags matching the normalized query, case-insensitively.
pub(crate) fn tag_matches(query: &str, vault: &Vault) -> Vec<String> {
    let query = normalize_tag(query).to_lowercase();
    all_tags(vault)
        .into_iter()
        .filter(|tag| query.is_empty() || tag.to_lowercase().contains(&query))
        .collect()
}

/// Normalize one typed tag the same way as vault persistence.
pub(crate) fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('#').trim().to_owned()
}

/// Task ids whose title contains `query` (case-insensitive), in full tree
/// pre-order. An empty query matches nothing. Folds do not apply: search has
/// to find hidden tasks so selecting one can unfold its ancestors.
pub(crate) fn search_matches(query: &str, vault: &Vault) -> Vec<TaskId> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    collect_search_matches(&vault.tree(), &query, &mut matches);
    matches
}

/// Append ids of `nodes` and their descendants whose title matches `query`.
fn collect_search_matches(nodes: &[TreeNode<'_>], query: &str, matches: &mut Vec<TaskId>) {
    for node in nodes {
        if node.task.title.to_lowercase().contains(query) {
            matches.push(node.task.id.clone());
        }
        collect_search_matches(&node.children, query, matches);
    }
}

/// Registered projects matching `query` over slug or the path as it is shown
/// in the picker, best match first (all of them in config order when the
/// query is empty).
///
/// Ties are broken by config order rather than by path, so the ranking is
/// stable and predictable.
///
/// Matches are ranked: exact slug, slug prefix, slug substring, then a
/// substring of the displayed path. Matching the displayed path (with `$HOME`
/// collapsed to `~`) keeps the username in a raw absolute path from matching
/// every project.
pub(crate) fn project_matches(
    query: &str,
    projects: &[Project],
    display: &PathDisplay,
) -> Vec<Project> {
    let home = env::var_os("HOME").map(PathBuf::from);
    project_matches_with_home(query, projects, display, home.as_deref())
}

/// [`project_matches`] with an explicit home directory, for tests.
fn project_matches_with_home(
    query: &str,
    projects: &[Project],
    display: &PathDisplay,
    home: Option<&Path>,
) -> Vec<Project> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return projects.to_vec();
    }
    let mut matches: Vec<(u8, Project)> = projects
        .iter()
        .filter_map(|project| {
            let slug = project.slug.to_lowercase();
            let rank = if slug == query {
                0
            } else if slug.starts_with(&query) {
                1
            } else if slug.contains(&query) {
                2
            } else if path_display::shorten_with_home(&project.path, display, home)
                .to_lowercase()
                .contains(&query)
            {
                3
            } else {
                return None;
            };
            Some((rank, project.clone()))
        })
        .collect();
    matches.sort_by_key(|(rank, _)| *rank);
    matches.into_iter().map(|(_, project)| project).collect()
}

/// Minimum path cells reserved in a project row, so long slugs yield space to
/// a useful path suffix instead of pushing the path off-screen.
const PROJECT_PATH_MIN_WIDTH: usize = 8;

/// Project picker rows: a shared slug column plus each shortened project
/// path, truncated to `row_width` cells.
///
/// Presentation for the `p` picker lives here, next to [`PickerKind::Project`],
/// so `ui` only draws the strings.
pub(crate) fn project_rows(
    projects: &[Project],
    display: &PathDisplay,
    row_width: usize,
) -> Vec<String> {
    let measured = projects
        .iter()
        .map(|project| text_width(&project.slug))
        .max()
        .unwrap_or_default();
    let path_width = PROJECT_PATH_MIN_WIDTH.min(row_width.saturating_sub(2));
    let slug_width = measured.min(row_width.saturating_sub(path_width + 2));
    projects
        .iter()
        .map(|project| {
            let path = path_display::shorten(&project.path, display);
            if slug_width == 0 {
                return truncate_title(&path, row_width);
            }
            let slug = truncate_title(&project.slug, slug_width);
            let padding = " ".repeat(slug_width.saturating_sub(text_width(&slug)));
            truncate_title(&format!("{slug}{padding}  {path}"), row_width)
        })
        .collect()
}
pub(crate) fn link_matches(query: &str, targets: &[TaskId], vault: &Vault) -> Vec<TaskId> {
    let query = query.trim().to_lowercase();
    targets
        .iter()
        .filter(|id| {
            if query.is_empty() {
                return true;
            }
            let title = vault
                .get(id)
                .map_or_else(String::new, |task| task.title.to_lowercase());
            title.contains(&query) || id.to_string().contains(&query)
        })
        .cloned()
        .collect()
}

/// One choice in the first stage of the move picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveChoice {
    /// Move under the task or to the root in the current Project.
    Parent(Option<TaskId>),
    /// Continue to the registered-Project picker.
    OtherProject,
}

/// Choices in the first move picker: current-Project roots and tasks, plus an
/// optional route to another registered Project.
pub(crate) fn move_choices(
    query: &str,
    vault: &Vault,
    moving: &[TaskId],
    has_other_projects: bool,
) -> Vec<MoveChoice> {
    let mut choices: Vec<MoveChoice> = move_matches(query, vault, moving)
        .into_iter()
        .map(MoveChoice::Parent)
        .collect();
    let query = query.trim().to_lowercase();
    if has_other_projects && (query.is_empty() || "another project".contains(&query)) {
        choices.push(MoveChoice::OtherProject);
    }
    choices
}

/// Move-target candidates matching `query` over title or id (all of them when
/// the query is empty): the synthetic root entry (`None`) always first, then
/// the full tree in pre-order minus the moving tasks and every descendant of
/// one. Folds are ignored, and the root entry is never filtered out, so
/// move-to-root is always one Enter away.
pub(crate) fn move_matches(query: &str, vault: &Vault, moving: &[TaskId]) -> Vec<Option<TaskId>> {
    let mut excluded: BTreeSet<TaskId> = BTreeSet::new();
    for id in moving {
        collect_subtree_ids(vault, id, &mut excluded);
    }

    let mut matches: Vec<Option<TaskId>> = vec![None];
    matches.extend(task_matches(query, vault, &excluded).into_iter().map(Some));
    matches
}

/// Link-creation candidates matching `query` over title or id (all of them
/// when the query is empty): every task in the full tree except `source`.
/// Folds are ignored, so a hidden task can still be linked.
pub(crate) fn create_link_matches(query: &str, vault: &Vault, source: &TaskId) -> Vec<TaskId> {
    let excluded: BTreeSet<TaskId> = std::iter::once(source.clone()).collect();
    task_matches(query, vault, &excluded)
}

/// Task candidates matching `query` over title or id (all of them when the
/// query is empty): the full tree in pre-order minus `excluded`.
pub(crate) fn task_matches(query: &str, vault: &Vault, excluded: &BTreeSet<TaskId>) -> Vec<TaskId> {
    let query = query.trim().to_lowercase();
    let mut matches = Vec::new();
    collect_task_matches(&vault.tree(), &query, excluded, &mut matches);
    matches
}

/// Insert `id` and every descendant of `id` into `out`.
fn collect_subtree_ids(vault: &Vault, id: &TaskId, out: &mut BTreeSet<TaskId>) {
    if !out.insert(id.clone()) {
        return;
    }
    for child in vault.children(id) {
        collect_subtree_ids(vault, child, out);
    }
}

/// Append non-excluded tasks whose title or id matches `query`, in pre-order.
fn collect_task_matches(
    nodes: &[TreeNode<'_>],
    query: &str,
    excluded: &BTreeSet<TaskId>,
    out: &mut Vec<TaskId>,
) {
    for node in nodes {
        if !excluded.contains(&node.task.id) {
            let title = node.task.title.to_lowercase();
            if query.is_empty() || title.contains(query) || node.task.id.to_string().contains(query)
            {
                out.push(node.task.id.clone());
            }
        }
        collect_task_matches(&node.children, query, excluded, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{project_matches_with_home, Picker, PickerInput, PickerKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::{Path, PathBuf};
    use tt::{PathDisplay, Project};

    fn project(path: &str, slug: &str) -> Project {
        Project {
            path: PathBuf::from(path),
            slug: slug.to_owned(),
            never_ask_nested: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn input_edits_query_and_resets_highlight() {
        let mut picker = Picker::new(PickerKind::Project);
        let mut query = String::new();

        assert_eq!(
            picker.handle_input(&mut query, key(KeyCode::Char('j')), 3),
            PickerInput::QueryChanged
        );
        assert_eq!(query, "j");
        picker.highlight = 2;
        assert_eq!(
            picker.handle_input(&mut query, key(KeyCode::Backspace), 3),
            PickerInput::QueryChanged
        );
        assert!(query.is_empty());
        assert_eq!(picker.highlight, 0);

        picker.highlight = 2;
        assert_eq!(
            picker.handle_input(&mut query, key(KeyCode::Char('k')), 3),
            PickerInput::QueryChanged
        );
        assert_eq!(query, "k");
        assert_eq!(picker.highlight, 0);
    }

    #[test]
    fn input_ctrl_w_removes_the_previous_word_and_resets_highlight() {
        let mut picker = Picker::new(PickerKind::Project);
        let mut query = "alpha beta".to_owned();
        picker.highlight = 2;

        assert_eq!(
            picker.handle_input(
                &mut query,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                3,
            ),
            PickerInput::QueryChanged
        );
        assert_eq!(query, "alpha ");
        assert_eq!(picker.highlight, 0);
    }

    #[test]
    fn input_moves_highlight_with_clamped_arrows_and_control_aliases() {
        let mut picker = Picker::new(PickerKind::Project);
        let mut query = String::new();

        for (event, expected) in [
            (key(KeyCode::Up), 0),
            (key(KeyCode::Down), 1),
            (KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL), 2),
            (key(KeyCode::Down), 2),
            (KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL), 1),
            (key(KeyCode::Up), 0),
            (key(KeyCode::Up), 0),
        ] {
            assert_eq!(
                picker.handle_input(&mut query, event, 3),
                PickerInput::Continue
            );
            assert_eq!(picker.highlight, expected);
        }

        picker.highlight = 1;
        picker.handle_input(&mut query, key(KeyCode::Down), 0);
        assert_eq!(
            picker.highlight, 1,
            "empty results cannot move the highlight"
        );
    }

    #[test]
    fn input_returns_commit_and_cancel_for_client_policy() {
        let mut picker = Picker::new(PickerKind::Project);
        let mut query = "projects".to_owned();

        assert_eq!(
            picker.handle_input(&mut query, key(KeyCode::Enter), 1),
            PickerInput::Commit
        );
        assert_eq!(
            picker.handle_input(&mut query, key(KeyCode::Esc), 1),
            PickerInput::Cancel
        );
        assert_eq!(query, "projects");
    }

    #[test]
    fn project_matches_ranks_the_exact_slug_above_home_path_matches() {
        // Every path lives under a home directory whose name contains `rp`
        // (`vieitesrpi`). Matching the raw path would leave the `rp` project
        // last; collapsing home to `~` removes the false positives.
        let home = Path::new("/home/vieitesrpi");
        let display = PathDisplay::default();
        let projects = vec![
            project("/home/vieitesrpi/personal/tt-3", "tt-3"),
            project("/home/vieitesrpi/personal/dotfiles", "dotfiles"),
            project(
                "/home/vieitesrpi/personal/prefapp-backstage",
                "prefapp-backstage",
            ),
            project("/home/vieitesrpi/personal/gitops-k8s", "gitops-k8s"),
            project("/home/vieitesrpi/personal/agent-radar", "agent-radar"),
            project("/home/vieitesrpi/personal/contx", "contx"),
            project("/home/vieitesrpi/personal/rp", "rp"),
        ];

        let matches = project_matches_with_home("rp", &projects, &display, Some(home));

        assert_eq!(
            matches.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            ["rp"],
            "home is collapsed so only the real rp project matches"
        );
    }

    #[test]
    fn project_matches_ranks_exact_then_prefix_then_substring() {
        let display = PathDisplay::default();
        let projects = vec![
            project("/xapp", "xapp"),
            project("/apple", "apple"),
            project("/app", "app"),
        ];

        let matches = project_matches_with_home("app", &projects, &display, None);

        assert_eq!(
            matches.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            ["app", "apple", "xapp"],
            "exact beats prefix beats substring, config order as tie-breaker"
        );
    }

    #[test]
    fn project_matches_empty_query_keeps_config_order() {
        let display = PathDisplay::default();
        let projects = vec![
            project("/one", "gamma"),
            project("/two", "alpha"),
            project("/three", "beta"),
        ];

        let matches = project_matches_with_home("   ", &projects, &display, None);

        assert_eq!(
            matches.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
            ["gamma", "alpha", "beta"],
            "an empty query lists every project in config order"
        );
    }
}
