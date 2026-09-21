//! One picker state machine for every type-to-filter TUI choice.
//!
//! All interactions are the same: type to filter, arrows (or
//! `ctrl-n`/`ctrl-p`) to move a highlight, Enter to commit, Esc to cancel. The
//! per-kind differences — the item source, what Enter does, and what Esc
//! restores — live on [`PickerKind`] and in the commit/cancel handlers in
//! [`super::app`].

use std::collections::BTreeSet;

use tt::{Priority, Project, TaskFilter, TaskId, TaskState, TreeNode, Vault};

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
    /// `m`: move the active selection under another task or the root.
    Move {
        /// Ids being moved, in action order; these and all their descendants
        /// are excluded from the candidate list.
        moving: Vec<TaskId>,
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
    pub(crate) fn prompt(&self) -> &'static str {
        match self {
            Self::Search { .. } => "search: ",
            Self::Project => "project: ",
            Self::Link { .. } => "link: ",
            Self::Move { .. } => "move under: ",
            Self::Priority => "priority: ",
            Self::Tags => "tag: ",
            Self::Filter => "filter: ",
            Self::CreateLink { .. } => "link to: ",
        }
    }

    /// Key hint shown next to the prompt while no status is set.
    pub(crate) fn hint(&self) -> &'static str {
        match self {
            Self::Search { .. } => "↑↓ select · enter open · esc cancel",
            Self::Project => "↑↓ select · enter switch · esc cancel",
            Self::Link { .. } => "↑↓ select · enter jump · esc cancel",
            Self::Move { .. } => "↑↓ select · enter move · esc cancel",
            Self::Priority => "↑↓ select · enter set · esc cancel",
            Self::Tags => "↑↓ select · enter toggle/add · esc cancel",
            Self::Filter => "↑↓ select · enter apply · esc cancel",
            Self::CreateLink { .. } => "↑↓ select · enter link · esc cancel",
        }
    }
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

    /// Whether this is the `/` title search.
    pub(crate) fn is_search(&self) -> bool {
        matches!(self.kind, PickerKind::Search { .. })
    }

    /// Whether this is the `p` project picker.
    pub(crate) fn is_project(&self) -> bool {
        matches!(self.kind, PickerKind::Project)
    }

    /// Whether this is the `o` link picker.
    pub(crate) fn is_link(&self) -> bool {
        matches!(self.kind, PickerKind::Link { .. })
    }

    /// Whether this is the `m` move picker.
    pub(crate) fn is_move(&self) -> bool {
        matches!(self.kind, PickerKind::Move { .. })
    }

    /// Whether this is the `!` priority picker.
    pub(crate) fn is_priority(&self) -> bool {
        matches!(self.kind, PickerKind::Priority)
    }

    /// Whether this is the `t` tag picker.
    pub(crate) fn is_tags(&self) -> bool {
        matches!(self.kind, PickerKind::Tags)
    }

    /// Whether this is the `f` filter picker.
    pub(crate) fn is_filter(&self) -> bool {
        matches!(self.kind, PickerKind::Filter)
    }

    /// Whether this is the `L` link-creation picker.
    pub(crate) fn is_create_link(&self) -> bool {
        matches!(self.kind, PickerKind::CreateLink { .. })
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

/// Registered projects matching `query` over slug or path (all of them when
/// the query is empty).
pub(crate) fn project_matches(query: &str, projects: &[Project]) -> Vec<Project> {
    let query = query.trim().to_lowercase();
    projects
        .iter()
        .filter(|project| {
            query.is_empty()
                || project.slug.to_lowercase().contains(&query)
                || project
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&query)
        })
        .cloned()
        .collect()
}

/// Link targets matching `query` over title or id (all of them when the query
/// is empty).
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
