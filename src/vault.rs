//! Vault storage: one markdown file per task.
//!
//! The vault folder is the source of truth; the in-memory map is a cache that
//! [`Vault::reload`] rebuilds from disk at any time. Markdown files that are
//! not task documents — no `---` frontmatter block, or frontmatter without a
//! task `id` — are ordinary notes and are ignored; files that claim a task id
//! but cannot be parsed are skipped and reported as [`VaultIssue`]s — they are
//! never rewritten, renamed, or deleted, so external edits cannot be
//! destroyed.
//!
//! Writes are atomic: content is written to a temporary file in the vault
//! folder and renamed over the target.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use thiserror::Error;

use crate::index::{
    display_order, rewrite_mirror_aliases, tag_matches, Index, TaskFilter, TreeNode,
};
use crate::model::{normalize_tags, ParseError, Priority, Task, TaskId, TaskState, TASK_EXTENSION};
use crate::watcher::{VaultWatcher, WatchError};

/// Errors from vault operations.
#[derive(Debug, Error)]
pub enum VaultError {
    /// No task with the given id exists in the vault.
    #[error("task not found: {0}")]
    NotFound(TaskId),
    /// A task title was empty or whitespace-only.
    #[error("task title must not be empty")]
    EmptyTitle,
    /// The requested parent would put the task inside its own subtree (itself
    /// or one of its descendants), which the strict tree forbids.
    #[error("invalid parent {parent} for task {id}: a task cannot parent itself or a descendant")]
    InvalidParent {
        /// Task that would gain the parent.
        id: TaskId,
        /// Rejected parent.
        parent: TaskId,
    },
    /// A filesystem operation failed.
    #[error("filesystem error at {path}: {source}")]
    Io {
        /// Path the operation targeted.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Mirror synchronization failed after the title or reloaded cache was
    /// already committed. Earlier mirror writes remain committed.
    #[error(
        "partial commit (title_committed={title_committed}, reload_committed={reload_committed}); {mirror_files_updated} mirror file(s) updated before failure at {path}: {source}"
    )]
    PartialCommit {
        /// Whether a title update was persisted by `set_title`.
        title_committed: bool,
        /// Whether a reload scan was applied to the in-memory cache.
        reload_committed: bool,
        /// Number of mirror files successfully updated before the failure.
        mirror_files_updated: usize,
        /// Mirror file whose rewrite failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// Category of a [`VaultIssue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VaultIssueKind {
    /// The file could not be parsed as a task document.
    Malformed,
    /// Another file already declared the same task id. Retained for API
    /// stability; scans key tasks by file stem, so this can no longer occur.
    DuplicateId,
    /// The file stem and the frontmatter `id` differ. The task still loads,
    /// keyed by the file stem; the declared id is reported here and preserved
    /// verbatim on rewrite (the file is never renamed).
    IdMismatch,
    /// The file (or the vault folder) could not be read.
    Unreadable,
    /// A parent id points at a task that does not exist; the task renders as
    /// a root and reattaches automatically if that id appears later.
    DanglingParent,
    /// A parent cycle was detected; one edge was dropped to break it and the
    /// repeated task renders as a root.
    Cycle,
}

impl VaultIssueKind {
    /// Short human-readable label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::DuplicateId => "duplicate id",
            Self::IdMismatch => "id mismatch",
            Self::Unreadable => "unreadable",
            Self::DanglingParent => "dangling parent",
            Self::Cycle => "cycle",
        }
    }
}

/// A problem found while scanning the vault.
///
/// Issues never stop a scan: every other file still loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultIssue {
    /// File (or folder) the issue refers to.
    pub path: PathBuf,
    /// Issue category.
    pub kind: VaultIssueKind,
    /// Human-readable detail.
    pub detail: String,
}

impl VaultIssue {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        kind: VaultIssueKind,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for VaultIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}): {}",
            self.path.display(),
            self.kind.as_str(),
            self.detail
        )
    }
}

/// Fields for creating a task with [`Vault::add`].
#[derive(Debug, Clone)]
pub struct NewTask {
    /// Title; must not be empty.
    pub title: String,
    /// Parent task id, if this is a sub-task.
    pub parent: Option<TaskId>,
    /// Destination sibling after which to insert. When absent, or when the id
    /// is not in the destination sibling group, the task is appended.
    pub insert_after: Option<TaskId>,
    /// Tags, with or without a leading `#`; normalized on add.
    pub tags: Vec<String>,
    /// Due date, if any.
    pub due: Option<NaiveDate>,
    /// Priority, if any.
    pub priority: Option<Priority>,
    /// Free markdown body; may contain `[[id]]` links.
    pub body: String,
}

impl NewTask {
    /// A task with only a title; every optional field unset.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            parent: None,
            insert_after: None,
            tags: Vec::new(),
            due: None,
            priority: None,
            body: String::new(),
        }
    }
}

/// Result of moving a task one place among its siblings with
/// [`Vault::shift_rank`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftOutcome {
    /// The task exchanged places with an adjacent sibling.
    Shifted,
    /// The task was already at the requested edge of its sibling group.
    AtBound,
    /// The task has no sibling to move past.
    OnlyChild,
}

/// Result of a [`Vault::delete`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeleteOutcome {
    /// Task files that were removed, descendants included.
    pub deleted: usize,
}

/// Result of changing a task title.
#[derive(Debug, Clone, PartialEq)]
pub struct RenameOutcome {
    /// The renamed task after any self-link aliases were synchronized.
    pub task: Task,
    /// Number of files whose mirror aliases were rewritten.
    pub mirror_files_updated: usize,
}

/// A folder of one-file-per-task markdown documents.
#[derive(Debug)]
pub struct Vault {
    root: PathBuf,
    tasks: BTreeMap<TaskId, Task>,
    index: Index,
    scan_issues: Vec<VaultIssue>,
    issues: Vec<VaultIssue>,
}

struct MirrorSyncError {
    path: PathBuf,
    source: std::io::Error,
    mirror_files_updated: usize,
}

impl Vault {
    /// Open the vault at `root`, creating the folder if it does not exist, and
    /// scan it once.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::Io`] when the folder cannot be created.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|source| VaultError::Io {
            path: root.clone(),
            source,
        })?;

        let mut vault = Self {
            root,
            tasks: BTreeMap::new(),
            index: Index::default(),
            scan_issues: Vec::new(),
            issues: Vec::new(),
        };
        vault.reload()?;
        Ok(vault)
    }

    /// Rescan the vault folder, replacing the in-memory cache and index, and
    /// return the issues found. Existing tasks whose title changed since the
    /// previous scan have their mirror aliases synchronized before this
    /// returns.
    ///
    /// A cold open has no previous titles to compare and never writes. This is
    /// the only way to pick up external edits; the cache and the index are
    /// fully disposable.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::PartialCommit`] when a mirror alias rewrite
    /// cannot be persisted. The error reports that the reload cache was
    /// committed, the number of earlier rewrites, and the failing path.
    pub fn reload(&mut self) -> Result<Vec<VaultIssue>, VaultError> {
        self.reload_with_writer(|vault, id, body| vault.set_body(id, body).map(|_| ()))
    }

    fn reload_with_writer(
        &mut self,
        mut write_body: impl FnMut(&mut Self, &TaskId, &str) -> Result<(), VaultError>,
    ) -> Result<Vec<VaultIssue>, VaultError> {
        let old_titles: BTreeMap<TaskId, String> = self
            .tasks
            .values()
            .map(|task| (task.id.clone(), task.title.clone()))
            .collect();
        let (tasks, scan_issues) = scan(&self.root);
        self.tasks = tasks;
        self.scan_issues = scan_issues;
        self.rebuild_index();
        self.sync_mirror_aliases(&old_titles, &mut write_body)
            .map_err(|error| VaultError::PartialCommit {
                title_committed: false,
                reload_committed: true,
                mirror_files_updated: error.mirror_files_updated,
                path: error.path,
                source: error.source,
            })?;
        Ok(self.issues.clone())
    }

    /// Rebuild the derived graph structures and refresh index issues, keeping
    /// the issues found by the most recent filesystem scan.
    fn rebuild_index(&mut self) {
        let (index, index_issues) = Index::build(&self.tasks, &self.root);
        self.index = index;
        self.issues = self.scan_issues.clone();
        self.issues.extend(index_issues);
    }

    /// Issues found during the most recent scan.
    pub fn issues(&self) -> &[VaultIssue] {
        &self.issues
    }

    /// The vault folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Start watching this vault's folder for external changes.
    ///
    /// # Errors
    ///
    /// Returns [`WatchError::Notify`] when the folder cannot be watched.
    pub fn watch(&self) -> Result<VaultWatcher, WatchError> {
        VaultWatcher::start(self.root())
    }

    /// All loaded tasks, ordered by id.
    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// All loaded task ids, ordered.
    pub fn ids(&self) -> impl Iterator<Item = &TaskId> {
        self.tasks.keys()
    }

    /// Number of loaded tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether no task currently loads.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Look up a task by id.
    pub fn get(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Root task ids in display order (rank, then lowercased title and id).
    pub fn roots(&self) -> &[TaskId] {
        self.index.roots()
    }

    /// Child task ids of `id` in display order.
    pub fn children(&self, id: &TaskId) -> &[TaskId] {
        self.index.children(id)
    }

    /// Effective parent of `id` after missing or cyclic edges were dropped.
    pub fn parent(&self, id: &TaskId) -> Option<&TaskId> {
        self.index.parent(id)
    }

    /// Outgoing `[[id]]` links from the body of `id`, in body order and
    /// de-duplicated. Targets may be missing: dangling links are allowed.
    pub fn links(&self, id: &TaskId) -> &[TaskId] {
        self.index.links(id)
    }

    /// Ids of tasks whose body links to `id`, in id order.
    pub fn backlinks(&self, id: &TaskId) -> &[TaskId] {
        self.index.backlinks(id)
    }

    /// Root tasks with their subtrees, in display order.
    pub fn tree(&self) -> Vec<TreeNode<'_>> {
        self.index
            .roots()
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .map(|task| self.subtree(task))
            .collect()
    }

    /// Tasks matching `filter` as of `today`, assembled into a forest with the
    /// same parent edges: a matching task whose parent does not match becomes
    /// a root, and non-matching tasks are omitted entirely.
    pub fn filter_tree(&self, filter: &TaskFilter, today: NaiveDate) -> Vec<TreeNode<'_>> {
        let matching: BTreeSet<TaskId> = self
            .filter(filter, today)
            .into_iter()
            .map(|task| task.id.clone())
            .collect();

        let mut roots: Vec<&Task> = matching
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .filter(|task| match self.index.parent(&task.id) {
                Some(parent) => !matching.contains(parent),
                None => true,
            })
            .collect();
        roots.sort_by(|a, b| display_order(&self.tasks, &a.id, &b.id));

        roots
            .into_iter()
            .map(|task| self.subtree_filtered(task, &matching))
            .collect()
    }

    /// `(done, total)` over non-cancelled descendants of `id`, excluding `id`
    /// itself. Cancelled tasks are not counted but their descendants are.
    pub fn rollup(&self, id: &TaskId) -> (usize, usize) {
        let mut done = 0usize;
        let mut total = 0usize;
        let mut visited: BTreeSet<TaskId> = BTreeSet::new();
        let mut stack: Vec<TaskId> = self.index.children(id).to_vec();

        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if let Some(task) = self.tasks.get(&current) {
                match task.state {
                    TaskState::Open => total += 1,
                    TaskState::Done => {
                        done += 1;
                        total += 1;
                    }
                    TaskState::Cancelled => {}
                }
                stack.extend(self.index.children(&task.id).iter().cloned());
            }
        }
        (done, total)
    }

    /// Tasks matching every set filter as of `today`, in id order.
    ///
    /// `today` is injected rather than read from the clock: callers decide
    /// what "today" means, and tests can pin it.
    pub fn filter(&self, filter: &TaskFilter, today: NaiveDate) -> Vec<&Task> {
        let tag_query = filter
            .tag
            .as_deref()
            .map(|tag| tag.trim().trim_start_matches('#').trim())
            .filter(|tag| !tag.is_empty());

        self.tasks
            .values()
            .filter(|task| {
                let state_ok = filter.state.is_none_or(|state| task.state == state);
                let priority_ok = filter
                    .priority
                    .is_none_or(|priority| task.priority == Some(priority));
                let tag_ok = tag_query
                    .is_none_or(|query| task.tags.iter().any(|tag| tag_matches(tag, query)));
                let due_ok = !filter.due_today || task.due.is_some_and(|due| due <= today);
                state_ok && priority_ok && tag_ok && due_ok
            })
            .collect()
    }

    fn subtree<'a>(&'a self, task: &'a Task) -> TreeNode<'a> {
        TreeNode {
            task,
            children: self
                .index
                .children(&task.id)
                .iter()
                .filter_map(|id| self.tasks.get(id))
                .map(|child| self.subtree(child))
                .collect(),
        }
    }

    fn subtree_filtered<'a>(&'a self, task: &'a Task, matching: &BTreeSet<TaskId>) -> TreeNode<'a> {
        TreeNode {
            task,
            children: self
                .index
                .children(&task.id)
                .iter()
                .filter(|id| matching.contains(*id))
                .filter_map(|id| self.tasks.get(id))
                .map(|child| self.subtree_filtered(child, matching))
                .collect(),
        }
    }

    /// Create a task, assign it a fresh id, and place it in its destination
    /// sibling group. Existing display order is materialized as consecutive
    /// ranks; the new task appends unless [`NewTask::insert_after`] names a
    /// destination sibling.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::EmptyTitle`] for a blank title, or
    /// [`VaultError::Io`] when a file cannot be written. Rank writes completed
    /// before an I/O failure stay written, and the in-memory cache matches
    /// disk.
    pub fn add(&mut self, new: NewTask) -> Result<Task, VaultError> {
        let NewTask {
            title,
            parent,
            insert_after,
            tags,
            due,
            priority,
            body,
        } = new;
        if title.trim().is_empty() {
            return Err(VaultError::EmptyTitle);
        }

        // Never reuse an id that is loaded *or* that names an existing file,
        // including files that were skipped as malformed.
        let id = TaskId::generate(|candidate| {
            self.tasks.contains_key(candidate) || self.path_for(candidate).exists()
        });

        let mut task = Task::new(id, title);
        task.parent = parent;
        task.tags = normalize_tags(tags);
        task.due = due;
        task.priority = priority;
        task.body = body;

        let siblings = self.destination_siblings(task.parent.as_ref());
        let insertion = insert_after
            .as_ref()
            .and_then(|after| siblings.iter().position(|id| id == after))
            .map_or(siblings.len(), |index| index + 1);
        for (index, sibling) in siblings.iter().enumerate() {
            let rank = index + usize::from(index >= insertion);
            let mut sibling = self.cloned(sibling)?;
            sibling.rank = Some(rank as i32);
            self.persist(sibling)?;
        }
        task.rank = Some(insertion as i32);
        self.persist(task)
    }

    /// Move a task one place earlier (`delta < 0`) or later (`delta > 0`)
    /// among its siblings, materializing consecutive ranks for the group.
    /// A zero delta is an at-bound no-op. Bound and only-child outcomes never
    /// write.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when a rank cannot be persisted. Rank writes
    /// completed before an I/O failure stay written, and the in-memory cache
    /// matches disk.
    pub fn shift_rank(&mut self, id: &TaskId, delta: i32) -> Result<ShiftOutcome, VaultError> {
        self.cloned(id)?;
        if delta == 0 {
            return Ok(ShiftOutcome::AtBound);
        }
        let mut siblings = self.sibling_ids(id);
        if siblings.len() == 1 {
            return Ok(ShiftOutcome::OnlyChild);
        }
        let current = siblings
            .iter()
            .position(|sibling| sibling == id)
            .expect("a loaded task is present in its effective sibling group");
        let target = if delta < 0 {
            current.checked_sub(1)
        } else if delta > 0 && current + 1 < siblings.len() {
            Some(current + 1)
        } else {
            None
        };
        let Some(target) = target else {
            return Ok(ShiftOutcome::AtBound);
        };
        siblings.swap(current, target);
        self.persist_ranks(&siblings)?;
        Ok(ShiftOutcome::Shifted)
    }

    /// Set the state of an existing task and persist it.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when the file cannot be written.
    pub fn set_state(&mut self, id: &TaskId, state: TaskState) -> Result<Task, VaultError> {
        self.edit(id, |task| task.state = state)
    }

    /// Set or clear the priority of an existing task and persist it.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when the file cannot be written.
    pub fn set_priority(
        &mut self,
        id: &TaskId,
        priority: Option<Priority>,
    ) -> Result<Task, VaultError> {
        self.edit(id, |task| task.priority = priority)
    }

    /// Replace an existing task's tags, normalizing them before persistence.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when the file cannot be written.
    pub fn set_tags(&mut self, id: &TaskId, tags: Vec<String>) -> Result<Task, VaultError> {
        let tags = normalize_tags(tags);
        self.edit(id, |task| task.tags = tags)
    }

    /// Set the title of an existing task and synchronize mirror aliases that
    /// pointed to its previous title.
    ///
    /// Contextual aliases, bare links, and links in code remain untouched. The
    /// returned count is the number of files whose mirror aliases were
    /// rewritten. The title and any earlier alias writes remain applied if a
    /// later alias rewrite fails.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::EmptyTitle`] for a blank title,
    /// [`VaultError::NotFound`] for an unknown id, [`VaultError::Io`] when
    /// the task file cannot be written, or [`VaultError::PartialCommit`] when
    /// the title was persisted but a mirror file could not be written. The
    /// latter reports prior mirror successes and the failing path.
    pub fn set_title(&mut self, id: &TaskId, title: &str) -> Result<RenameOutcome, VaultError> {
        if title.trim().is_empty() {
            return Err(VaultError::EmptyTitle);
        }
        let old_title = self.cloned(id)?.title;
        self.edit(id, |task| task.title = title.to_owned())?;
        let old_titles = BTreeMap::from([(id.clone(), old_title)]);
        let mirror_files_updated = self
            .sync_mirror_aliases(&old_titles, &mut |vault, source, body| {
                vault.set_body(source, body).map(|_| ())
            })
            .map_err(|error| VaultError::PartialCommit {
                title_committed: true,
                reload_committed: false,
                mirror_files_updated: error.mirror_files_updated,
                path: error.path,
                source: error.source,
            })?;
        Ok(RenameOutcome {
            task: self.cloned(id)?,
            mirror_files_updated,
        })
    }

    /// Rewrite mirror aliases for title changes in `old_titles`, whose values
    /// are the titles before those changes. A link is a mirror only when its
    /// alias equals the target's previous title (after trimming); contextual
    /// aliases, bare links, links in code, and dangling or cross-store targets
    /// are never touched. Alias spans are rewritten in place, preserving
    /// unknown frontmatter and every non-alias byte.
    ///
    /// Each title diff is applied once, so a chain `A → B, B → C` never
    /// chases transitively. Returns the number of files written, and writes
    /// nothing when there is no title diff or no stale mirror.
    ///
    /// Files written before a failure stay written; the in-memory cache
    /// matches disk.
    fn sync_mirror_aliases(
        &mut self,
        old_titles: &BTreeMap<TaskId, String>,
        write_body: &mut impl FnMut(&mut Self, &TaskId, &str) -> Result<(), VaultError>,
    ) -> Result<usize, MirrorSyncError> {
        let mut diffs: BTreeMap<TaskId, (String, String)> = BTreeMap::new();
        for (id, task) in &self.tasks {
            let Some(old) = old_titles.get(id) else {
                continue;
            };
            if old != &task.title {
                diffs.insert(id.clone(), (old.clone(), task.title.clone()));
            }
        }
        if diffs.is_empty() {
            return Ok(0);
        }

        // A self-link is a backlink of itself, so this covers it too.
        let mut sources: BTreeSet<TaskId> = BTreeSet::new();
        for id in diffs.keys() {
            sources.extend(self.index.backlinks(id).iter().cloned());
        }

        let mut written = 0usize;
        for source in sources {
            let Some(body) = self.tasks.get(&source).map(|task| task.body.clone()) else {
                continue;
            };
            let Some(rewritten) = rewrite_mirror_aliases(&body, &diffs) else {
                continue;
            };
            if let Err(error) = write_body(self, &source, &rewritten) {
                let (path, source) = match error {
                    VaultError::Io { path, source } => (path, source),
                    _ => unreachable!("source task exists in cache"),
                };
                return Err(MirrorSyncError {
                    path,
                    source,
                    mirror_files_updated: written,
                });
            }
            written += 1;
        }
        Ok(written)
    }

    /// Replace the body of an existing task and persist it.
    ///
    /// The body is stored verbatim (whitespace is not trimmed) and may be
    /// empty. Links in the new body are re-indexed by the rebuild that follows
    /// the write.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when the file cannot be written.
    pub fn set_body(&mut self, id: &TaskId, body: &str) -> Result<Task, VaultError> {
        self.edit(id, |task| task.body = body.to_owned())
    }

    /// Move an existing task under `new_parent`, or to the vault root when
    /// `new_parent` is `None`. The task appends to the destination sibling
    /// group, whose ranks are materialized; a ranked source group is closed
    /// up after the move.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::InvalidParent`] when `new_parent` is the task
    /// itself or one of its descendants (the strict tree forbids cycles),
    /// [`VaultError::NotFound`] for an unknown task or an unknown parent, or
    /// [`VaultError::Io`] when a file cannot be written. Rank writes completed
    /// before an I/O failure stay written, and the in-memory cache matches
    /// disk.
    pub fn set_parent(
        &mut self,
        id: &TaskId,
        new_parent: Option<&TaskId>,
    ) -> Result<Task, VaultError> {
        let mut task = self.cloned(id)?;
        if let Some(parent) = new_parent {
            if self.is_descendant(parent, id) {
                return Err(VaultError::InvalidParent {
                    id: id.clone(),
                    parent: parent.clone(),
                });
            }
            if !self.tasks.contains_key(parent) {
                return Err(VaultError::NotFound(parent.clone()));
            }
        }

        let old_parent = self.index.parent(id).cloned();
        let old_siblings = self.sibling_ids(id);
        let old_group_had_ranks = old_siblings.iter().any(|sibling| {
            self.tasks
                .get(sibling)
                .is_some_and(|task| task.rank.is_some())
        });
        let same_group = old_parent.as_ref() == new_parent;

        let mut destination = self.destination_siblings(new_parent);
        destination.retain(|sibling| sibling != id);
        destination.push(id.clone());
        task.parent = new_parent.cloned();
        for (rank, sibling) in destination.iter().enumerate() {
            let mut sibling = if sibling == id {
                task.clone()
            } else {
                self.cloned(sibling)?
            };
            sibling.rank = Some(rank as i32);
            self.persist(sibling)?;
        }

        if !same_group && old_group_had_ranks {
            let remaining: Vec<TaskId> = old_siblings
                .into_iter()
                .filter(|sibling| sibling != id)
                .collect();
            self.persist_ranks(&remaining)?;
        }
        self.cloned(id)
    }

    /// Delete tasks and their whole subtrees.
    ///
    /// The doomed set is each loaded requested task plus every descendant in
    /// the effective forest; descendants are never reparented or kept alive,
    /// so callers that want to keep a subtree move it out first with
    /// [`Vault::set_parent`]. A descendant requested on its own is already
    /// covered by an ancestor's closure, so marking both dedupes naturally.
    /// Unknown ids are skipped, which makes a bulk delete best-effort.
    /// `[[id]]` links to deleted tasks are left alone: dangling links are
    /// legitimate.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::Io`] when a file removal fails. Files removed
    /// before the failure stay removed, and the in-memory cache is left
    /// consistent with the files.
    pub fn delete(&mut self, ids: &[TaskId]) -> Result<DeleteOutcome, VaultError> {
        let doomed = self.closure(&self.doomed_ids(ids));
        if doomed.is_empty() {
            return Ok(DeleteOutcome::default());
        }

        let mut removed: Vec<TaskId> = Vec::new();
        for id in &doomed {
            let path = self.path_for(id);
            match fs::remove_file(&path) {
                Ok(()) => removed.push(id.clone()),
                // The goal (no file) is already met; keep going.
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    removed.push(id.clone());
                }
                Err(source) => {
                    for id in &removed {
                        self.tasks.remove(id);
                    }
                    self.rebuild_index();
                    return Err(VaultError::Io { path, source });
                }
            }
        }
        for id in &doomed {
            self.tasks.remove(id);
        }
        self.rebuild_index();
        Ok(DeleteOutcome {
            deleted: doomed.len(),
        })
    }

    /// How many tasks a [`Vault::delete`] of `ids` would remove beyond the
    /// requested tasks themselves: every descendant not already in `ids`,
    /// without touching disk. Callers use it for confirmation copy, so the
    /// preview and the outcome can never disagree.
    pub fn descendant_count(&self, ids: &[TaskId]) -> usize {
        let requested = self.doomed_ids(ids);
        self.closure(&requested).len() - requested.len()
    }

    /// The requested ids that are actually loaded; unknown ids are skipped so
    /// a bulk delete stays best-effort.
    fn doomed_ids(&self, ids: &[TaskId]) -> BTreeSet<TaskId> {
        ids.iter()
            .filter(|id| self.tasks.contains_key(*id))
            .cloned()
            .collect()
    }

    /// The full descendant closure of `seeds` in the effective forest: every
    /// seed plus all of its descendants. The visited set keeps the walk finite
    /// even if a corrupt index ever exposed a child cycle.
    fn closure(&self, seeds: &BTreeSet<TaskId>) -> BTreeSet<TaskId> {
        let mut closure: BTreeSet<TaskId> = BTreeSet::new();
        let mut stack: Vec<TaskId> = seeds.iter().cloned().collect();
        while let Some(id) = stack.pop() {
            if !closure.insert(id.clone()) {
                continue;
            }
            stack.extend(self.children(&id).iter().cloned());
        }
        closure
    }

    /// Current destination sibling group for a new or reparented task. A
    /// missing requested parent makes the task an effective root.
    fn destination_siblings(&self, parent: Option<&TaskId>) -> Vec<TaskId> {
        match parent.filter(|parent| self.tasks.contains_key(*parent)) {
            Some(parent) => self.index.children(parent).to_vec(),
            None => self.index.roots().to_vec(),
        }
    }

    /// Effective sibling group for a loaded task, in current display order.
    fn sibling_ids(&self, id: &TaskId) -> Vec<TaskId> {
        match self.index.parent(id) {
            Some(parent) => self.index.children(parent).to_vec(),
            None => self.index.roots().to_vec(),
        }
    }

    /// Persist `ids` with consecutive ranks matching their slice order.
    fn persist_ranks(&mut self, ids: &[TaskId]) -> Result<(), VaultError> {
        for (rank, id) in ids.iter().enumerate() {
            let mut task = self.cloned(id)?;
            task.rank = Some(rank as i32);
            self.persist(task)?;
        }
        Ok(())
    }

    fn path_for(&self, id: &TaskId) -> PathBuf {
        self.root.join(id.file_name())
    }

    /// Apply `change` to a loaded task, persist it, and return the new value.
    ///
    /// The single write seam for field setters: load a clone, run the change,
    /// then persist disk-first and rebuild the index. [`Vault::cloned`] guards
    /// the id, so an unknown task never reaches `change`.
    fn edit(&mut self, id: &TaskId, change: impl FnOnce(&mut Task)) -> Result<Task, VaultError> {
        let mut task = self.cloned(id)?;
        change(&mut task);
        self.persist(task)
    }

    fn cloned(&self, id: &TaskId) -> Result<Task, VaultError> {
        self.tasks
            .get(id)
            .cloned()
            .ok_or_else(|| VaultError::NotFound(id.clone()))
    }

    /// Whether `candidate` is `ancestor` itself or one of its descendants in
    /// the effective forest.
    fn is_descendant(&self, candidate: &TaskId, ancestor: &TaskId) -> bool {
        if candidate == ancestor {
            return true;
        }
        let mut current = self.parent(candidate);
        while let Some(parent) = current {
            if parent == ancestor {
                return true;
            }
            current = self.parent(parent);
        }
        false
    }

    /// Write the task to disk first, then update the cache, so a failed write
    /// never leaves memory and disk diverged.
    fn persist(&mut self, task: Task) -> Result<Task, VaultError> {
        let path = self.path_for(&task.id);
        crate::fsutil::write_atomic(&path, &task.to_document(), false).map_err(|source| {
            VaultError::Io {
                path: path.clone(),
                source,
            }
        })?;
        self.tasks.insert(task.id.clone(), task.clone());
        self.rebuild_index();
        Ok(task)
    }
}

/// Scan the vault root (non-recursively) for `*.md` files.
///
/// Paths are visited in sorted order so issues are deterministic. The file
/// stem is the task identity: the task loads under the stem, and a frontmatter
/// `id` that differs is reported as [`VaultIssueKind::IdMismatch`] and then
/// preserved verbatim on rewrite. Files without a `---` frontmatter opener,
/// frontmatter blocks without an `id`, and files whose stem is not a valid
/// task id are not tasks and are skipped (the latter with a
/// [`VaultIssueKind::Malformed`] issue).
fn scan(root: &Path) -> (BTreeMap<TaskId, Task>, Vec<VaultIssue>) {
    let mut tasks = BTreeMap::new();
    let mut issues = Vec::new();

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) => {
            issues.push(VaultIssue::new(
                root,
                VaultIssueKind::Unreadable,
                format!("cannot read vault folder: {source}"),
            ));
            return (tasks, issues);
        }
    };

    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension() == Some(OsStr::new(TASK_EXTENSION)))
        .collect();
    paths.sort();

    for path in paths {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) => {
                issues.push(VaultIssue::new(
                    &path,
                    VaultIssueKind::Unreadable,
                    format!("cannot read file: {source}"),
                ));
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                issues.push(VaultIssue::new(
                    &path,
                    VaultIssueKind::Malformed,
                    "file is not valid UTF-8",
                ));
                continue;
            }
        };
        let mut task = match Task::from_document(&text) {
            Ok(task) => task,
            // No frontmatter block, or frontmatter with no task `id`, means
            // this is an ordinary markdown note sharing the folder, not a
            // task file.
            Err(ParseError::MissingFrontmatter | ParseError::ForeignFrontmatter) => continue,
            Err(error) => {
                issues.push(VaultIssue::new(
                    &path,
                    VaultIssueKind::Malformed,
                    error.to_string(),
                ));
                continue;
            }
        };

        let stem = path.file_stem().and_then(OsStr::to_str);
        let stem_id = stem.and_then(|stem| TaskId::parse(stem).ok());
        let Some(stem_id) = stem_id else {
            issues.push(VaultIssue::new(
                &path,
                VaultIssueKind::Malformed,
                match stem {
                    Some(stem) => format!("file name {stem:?} is not a valid task id"),
                    None => "file name has no usable stem".to_owned(),
                },
            ));
            continue;
        };

        if stem_id != task.declared_id {
            issues.push(VaultIssue::new(
                &path,
                VaultIssueKind::IdMismatch,
                format!(
                    "frontmatter id {} does not match the file name {stem_id}",
                    task.declared_id
                ),
            ));
        }

        task.id = stem_id;
        tasks.insert(task.id.clone(), task);
    }

    (tasks, issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    fn read_task_file(vault: &Vault, id: &TaskId) -> String {
        fs::read_to_string(vault.root().join(format!("{id}.md"))).expect("read task file")
    }

    fn parse_id(value: &str) -> TaskId {
        TaskId::parse(value).expect("valid id")
    }

    #[test]
    fn add_persists_one_file_per_task() {
        let (_dir, mut vault) = open_vault();

        let first = vault.add(NewTask::new("First task")).expect("add first");
        let second = vault
            .add(NewTask {
                parent: Some(first.id.clone()),
                tags: vec!["#work".to_owned(), "work".to_owned()],
                due: NaiveDate::from_ymd_opt(2026, 1, 2),
                priority: Some(Priority::High),
                body: "See [[later]]".to_owned(),
                ..NewTask::new("Second task")
            })
            .expect("add second");

        assert_ne!(first.id, second.id);
        assert_eq!(vault.len(), 2);
        assert_eq!(second.tags, vec!["work".to_owned()]);
        assert_eq!(second.parent, Some(first.id.clone()));
        assert_eq!(second.priority, Some(Priority::High));

        for task in [&first, &second] {
            let path = vault.root().join(format!("{}.md", task.id));
            assert!(path.is_file(), "expected file at {}", path.display());
            let stored = Task::from_document(&read_task_file(&vault, &task.id)).expect("parse");
            assert_eq!(&stored, task);
        }
    }

    #[test]
    fn add_appends_instead_of_title_sorting() {
        let (_dir, mut vault) = open_vault();
        let zebra = vault.add(NewTask::new("Zebra")).expect("add zebra");
        let apple = vault.add(NewTask::new("Apple")).expect("add apple");

        assert_eq!(vault.roots(), &[zebra.id.clone(), apple.id.clone()]);
        assert_eq!(vault.get(&zebra.id).expect("zebra").rank, Some(0));
        assert_eq!(vault.get(&apple.id).expect("apple").rank, Some(1));
    }

    #[test]
    fn add_can_insert_immediately_after_a_sibling() {
        let (_dir, mut vault) = open_vault();
        let zebra = vault.add(NewTask::new("Zebra")).expect("add zebra");
        let apple = vault.add(NewTask::new("Apple")).expect("add apple");
        let mango = vault
            .add(NewTask {
                insert_after: Some(zebra.id.clone()),
                ..NewTask::new("Mango")
            })
            .expect("insert mango");

        assert_eq!(
            vault.roots(),
            &[zebra.id.clone(), mango.id.clone(), apple.id.clone()]
        );
        for (rank, id) in vault.roots().iter().enumerate() {
            assert_eq!(vault.get(id).expect("task").rank, Some(rank as i32));
        }
    }

    #[test]
    fn shift_rank_moves_one_slot_and_persists_consecutive_ranks() {
        let (_dir, mut vault) = open_vault();
        let alpha = vault.add(NewTask::new("Alpha")).expect("add alpha");
        let beta = vault.add(NewTask::new("Beta")).expect("add beta");
        let charlie = vault.add(NewTask::new("Charlie")).expect("add charlie");

        let outcome = vault.shift_rank(&alpha.id, 1).expect("shift down");

        assert_eq!(outcome, ShiftOutcome::Shifted);
        assert_eq!(
            vault.roots(),
            &[beta.id.clone(), alpha.id.clone(), charlie.id.clone()]
        );
        for (expected, id) in vault.roots().iter().enumerate() {
            assert_eq!(vault.get(id).expect("task").rank, Some(expected as i32));
            let stored = Task::from_document(&read_task_file(&vault, id)).expect("parse");
            assert_eq!(stored.rank, Some(expected as i32));
        }
    }

    #[test]
    fn shift_rank_at_a_bound_or_without_a_sibling_does_not_materialize_ranks() {
        let (dir, mut vault) = open_vault();
        let alpha = Task::new(parse_id("alpha00001"), "Alpha");
        let beta = Task::new(parse_id("beta000001"), "Beta");
        let parent = Task::new(parse_id("parent0001"), "Parent");
        let mut only_child = Task::new(parse_id("child00001"), "Only child");
        only_child.parent = Some(parent.id.clone());
        for task in [&alpha, &beta, &parent, &only_child] {
            fs::write(
                dir.path().join(format!("{}.md", task.id)),
                task.to_document(),
            )
            .expect("write task");
        }
        vault.reload().expect("reload");
        let before_alpha = read_task_file(&vault, &alpha.id);
        let before_child = read_task_file(&vault, &only_child.id);

        assert_eq!(
            vault.shift_rank(&alpha.id, -1).expect("at first"),
            ShiftOutcome::AtBound
        );
        assert_eq!(
            vault.shift_rank(&only_child.id, 1).expect("only child"),
            ShiftOutcome::OnlyChild
        );
        assert_eq!(read_task_file(&vault, &alpha.id), before_alpha);
        assert_eq!(read_task_file(&vault, &only_child.id), before_child);
        assert_eq!(vault.get(&alpha.id).expect("alpha").rank, None);
        assert_eq!(vault.get(&beta.id).expect("beta").rank, None);
        assert_eq!(vault.get(&only_child.id).expect("child").rank, None);
    }

    #[test]
    fn set_state_changes_only_the_target_task() {
        let (_dir, mut vault) = open_vault();
        let target = vault
            .add(NewTask {
                body: "body [[link]]\n".to_owned(),
                tags: vec!["work".to_owned()],
                ..NewTask::new("Keep me")
            })
            .expect("add target");
        let other = vault.add(NewTask::new("Untouched")).expect("add other");

        let other_before = read_task_file(&vault, &other.id);
        let before = read_task_file(&vault, &target.id);
        let updated = vault
            .set_state(&target.id, TaskState::Done)
            .expect("set state");

        assert_eq!(updated.state, TaskState::Done);
        assert_eq!(updated.title, "Keep me");
        assert_eq!(updated.body, "body [[link]]\n");
        assert_eq!(updated.tags, vec!["work".to_owned()]);

        let after = read_task_file(&vault, &target.id);
        assert_ne!(before, after, "state change should touch the file");
        let after_task = Task::from_document(&after).expect("parse");
        assert_eq!(after_task.state, TaskState::Done);
        assert_eq!(after_task.title, "Keep me");
        assert_eq!(after_task.body, "body [[link]]\n");
        assert_eq!(after_task.tags, vec!["work".to_owned()]);
        assert_eq!(read_task_file(&vault, &other.id), other_before);
    }

    #[test]
    fn set_priority_persists_and_round_trips() {
        let (_dir, mut vault) = open_vault();
        let task = vault.add(NewTask::new("Prioritize me")).expect("add");

        let updated = vault
            .set_priority(&task.id, Some(Priority::High))
            .expect("set priority");
        assert_eq!(updated.priority, Some(Priority::High));

        let stored = Task::from_document(&read_task_file(&vault, &task.id)).expect("parse");
        assert_eq!(stored.priority, Some(Priority::High));

        vault.set_priority(&task.id, None).expect("clear priority");
        let reopened = Vault::open(vault.root()).expect("reopen vault");
        assert_eq!(reopened.get(&task.id).expect("task").priority, None);
    }

    #[test]
    fn set_tags_normalizes_persists_and_round_trips() {
        let (_dir, mut vault) = open_vault();
        let task = vault.add(NewTask::new("Tag me")).expect("add");

        let updated = vault
            .set_tags(
                &task.id,
                vec![" #work ".to_owned(), "work".to_owned(), "#home".to_owned()],
            )
            .expect("set tags");
        assert_eq!(updated.tags, vec!["work".to_owned(), "home".to_owned()]);

        let reopened = Vault::open(vault.root()).expect("reopen vault");
        assert_eq!(
            reopened.get(&task.id).expect("task").tags,
            vec!["work".to_owned(), "home".to_owned()]
        );
    }

    #[test]
    fn set_title_changes_only_the_title() {
        let (_dir, mut vault) = open_vault();
        let task = vault
            .add(NewTask {
                body: "keep this body\n".to_owned(),
                ..NewTask::new("Old title")
            })
            .expect("add");

        let outcome = vault.set_title(&task.id, "New title").expect("set title");
        assert_eq!(outcome.task.title, "New title");
        assert_eq!(outcome.task.body, "keep this body\n");
        assert_eq!(outcome.task.state, TaskState::Open);
        assert_eq!(outcome.mirror_files_updated, 0);

        let stored = Task::from_document(&read_task_file(&vault, &task.id)).expect("parse");
        assert_eq!(stored.title, "New title");
        assert_eq!(stored.body, "keep this body\n");
        assert_eq!(stored.state, TaskState::Open);
    }

    #[test]
    fn set_title_keeps_the_rename_when_a_mirror_write_fails() {
        let (dir, mut vault) = open_vault();
        let target = vault.add(NewTask::new("Old title")).expect("add target");
        let first_mirror = vault
            .add(NewTask {
                body: format!("[[{}.md|Old title]]", target.id),
                ..NewTask::new("First mirror")
            })
            .expect("add first mirror");
        let later_mirror = vault
            .add(NewTask {
                body: format!("[[{}.md|Old title]]", target.id),
                ..NewTask::new("Later mirror")
            })
            .expect("add later mirror");
        let (successful, failing) = if first_mirror.id < later_mirror.id {
            (first_mirror, later_mirror)
        } else {
            (later_mirror, first_mirror)
        };
        let failing_path = dir.path().join(format!("{}.md", failing.id));
        fs::remove_file(&failing_path).expect("remove mirror file");
        fs::create_dir(&failing_path).expect("block mirror file");

        let error = vault
            .set_title(&target.id, "New title")
            .expect_err("mirror write should fail");

        assert!(matches!(
            error,
            VaultError::PartialCommit {
                title_committed: true,
                reload_committed: false,
                mirror_files_updated: 1,
                path,
                ..
            } if path == failing_path
        ));
        assert_eq!(vault.get(&target.id).expect("target").title, "New title");
        assert_eq!(
            vault.get(&successful.id).expect("successful mirror").body,
            format!("[[{}.md|New title]]", target.id),
            "earlier mirror writes remain committed"
        );
        assert_eq!(
            vault.get(&failing.id).expect("failing mirror").body,
            format!("[[{}.md|Old title]]", target.id),
            "failed mirror writes leave its cached content unchanged"
        );
        assert_eq!(
            Task::from_document(&read_task_file(&vault, &target.id))
                .expect("stored target")
                .title,
            "New title",
            "the title remains persisted before the mirror failure"
        );
    }

    #[test]
    fn reload_reports_committed_cache_and_prior_alias_writes_on_failure() {
        let (dir, mut vault) = open_vault();
        let target = vault.add(NewTask::new("Old title")).expect("add target");
        let first_mirror = vault
            .add(NewTask {
                body: format!("[[{}.md|Old title]]", target.id),
                ..NewTask::new("First mirror")
            })
            .expect("add first mirror");
        let later_mirror = vault
            .add(NewTask {
                body: format!("[[{}.md|Old title]]", target.id),
                ..NewTask::new("Later mirror")
            })
            .expect("add later mirror");
        let (successful, failing) = if first_mirror.id < later_mirror.id {
            (first_mirror, later_mirror)
        } else {
            (later_mirror, first_mirror)
        };
        let target_path = dir.path().join(format!("{}.md", target.id));
        let contents = fs::read_to_string(&target_path).expect("read target");
        assert!(
            contents.contains("Old title"),
            "target document: {contents:?}"
        );
        fs::write(
            &target_path,
            contents.replace("Old title", "External title"),
        )
        .expect("edit title externally");
        let failing_path = dir.path().join(format!("{}.md", failing.id));

        let error = vault
            .reload_with_writer(|vault, id, body| {
                if id == &failing.id {
                    return Err(VaultError::Io {
                        path: failing_path.clone(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "injected write failure",
                        ),
                    });
                }
                vault.set_body(id, body).map(|_| ())
            })
            .expect_err("mirror write should fail");

        assert!(matches!(
            error,
            VaultError::PartialCommit {
                title_committed: false,
                reload_committed: true,
                mirror_files_updated: 1,
                path,
                ..
            } if path == failing_path
        ));
        assert_eq!(
            vault.get(&target.id).expect("reloaded target").title,
            "External title"
        );
        assert_eq!(
            vault.get(&successful.id).expect("successful mirror").body,
            format!("[[{}.md|External title]]", target.id)
        );
    }

    #[test]
    fn set_body_changes_only_the_body() {
        let (_dir, mut vault) = open_vault();
        let task = vault
            .add(NewTask {
                tags: vec!["work".to_owned()],
                ..NewTask::new("Keep my title")
            })
            .expect("add");

        let updated = vault
            .set_body(&task.id, "new body\n\nwith lines\n")
            .expect("set body");

        assert_eq!(updated.body, "new body\n\nwith lines\n");
        assert_eq!(updated.title, "Keep my title");
        assert_eq!(updated.tags, vec!["work".to_owned()]);
        assert_eq!(updated.state, TaskState::Open);

        let stored = Task::from_document(&read_task_file(&vault, &task.id)).expect("parse");
        assert_eq!(stored.body, "new body\n\nwith lines\n");
        assert_eq!(stored.title, "Keep my title");
    }

    #[test]
    fn set_body_can_clear_the_body() {
        let (_dir, mut vault) = open_vault();
        let task = vault
            .add(NewTask {
                body: "some body".to_owned(),
                ..NewTask::new("Has body")
            })
            .expect("add");

        let updated = vault.set_body(&task.id, "").expect("clear body");

        assert_eq!(updated.body, "");
        let stored = Task::from_document(&read_task_file(&vault, &task.id)).expect("parse");
        assert_eq!(stored.body, "");
    }

    #[test]
    fn set_body_reindexes_links_and_backlinks() {
        let (_dir, mut vault) = open_vault();
        let target = vault.add(NewTask::new("Target")).expect("add target");
        let source = vault.add(NewTask::new("Source")).expect("add source");
        assert!(vault.links(&source.id).is_empty());
        assert!(vault.backlinks(&target.id).is_empty());

        vault
            .set_body(&source.id, &format!("see [[{}]]", target.id))
            .expect("set body");

        assert_eq!(vault.links(&source.id), &[target.id.clone()][..]);
        assert!(vault.backlinks(&target.id).contains(&source.id));
    }

    #[test]
    fn malformed_files_are_skipped_reported_and_never_rewritten() {
        let (dir, mut vault) = open_vault();
        let good = vault.add(NewTask::new("Good")).expect("add good");
        let garbage_path = dir.path().join("garbage.md");
        fs::write(
            &garbage_path,
            "---\nid: garbage001\nstate: open\n---\nmissing title\n",
        )
        .expect("write garbage");
        let garbage_before = fs::read(&garbage_path).expect("read garbage");

        let issues = vault.reload().expect("reload");

        assert_eq!(vault.len(), 1);
        assert!(vault.get(&good.id).is_some());
        let malformed: Vec<_> = issues
            .iter()
            .filter(|issue| issue.kind == VaultIssueKind::Malformed)
            .collect();
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].path, garbage_path);

        vault
            .set_title(&good.id, "Good, edited")
            .expect("edit good");
        assert_eq!(
            fs::read(&garbage_path).expect("read garbage"),
            garbage_before
        );
    }

    #[test]
    fn frontmatter_less_markdown_is_ignored_silently() {
        let (dir, mut vault) = open_vault();
        let good = vault.add(NewTask::new("Good")).expect("add good");
        fs::write(
            dir.path().join("CONTEXT.md"),
            "# A note\n\nOrdinary markdown, not a task.\n",
        )
        .expect("write note");
        fs::write(dir.path().join("SKILL.md"), "plain text, no frontmatter\n").expect("write note");

        let issues = vault.reload().expect("reload");

        assert!(issues.is_empty(), "notes must not be issues: {issues:?}");
        assert_eq!(vault.len(), 1);
        assert!(vault.get(&good.id).is_some());
    }

    #[test]
    fn foreign_frontmatter_without_an_id_is_ignored_silently() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: tt\ndescription: a skill\n---\nbody\n",
        )
        .expect("write skill");
        fs::write(
            dir.path().join("note.md"),
            "---\ntitle: A note\ntags: [x]\n---\nNot a task.\n",
        )
        .expect("write note");

        let issues = vault.reload().expect("reload");

        assert!(
            issues.is_empty(),
            "foreign frontmatter must be silent: {issues:?}"
        );
        assert!(vault.is_empty());
    }

    #[test]
    fn broken_frontmatter_is_still_reported() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("broken.md"),
            "---\nid: broken0001\nstate: open\n---\nmissing title\n",
        )
        .expect("write broken");

        let issues = vault.reload().expect("reload");

        assert!(vault.is_empty());
        let malformed: Vec<_> = issues
            .iter()
            .filter(|issue| issue.kind == VaultIssueKind::Malformed)
            .collect();
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].path, dir.path().join("broken.md"));
    }

    #[test]
    fn repo_root_reports_no_issues_for_its_notes() {
        // The repo root mixes CONTEXT.md/SKILL.md with milestone task files;
        // only files with task frontmatter may load, and none may be issues.
        let vault = Vault::open(env!("CARGO_MANIFEST_DIR")).expect("open repo root");
        assert!(
            vault.issues().is_empty(),
            "unexpected issues: {:?}",
            vault.issues()
        );
    }

    #[test]
    fn set_body_rejects_unknown_tasks() {
        let (_dir, mut vault) = open_vault();
        let missing = parse_id("doesnot123");

        assert!(matches!(
            vault.set_body(&missing, "body"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn same_declared_id_in_two_files_loads_both_by_stem() {
        let (dir, mut vault) = open_vault();
        let document = "---\nid: abc1234567\ntitle: Original\nstate: open\n---\n";
        fs::write(dir.path().join("aaa.md"), document).expect("write aaa");
        fs::write(dir.path().join("bbb.md"), document).expect("write bbb");

        vault.reload().expect("reload");

        assert_eq!(vault.len(), 2, "each file is a task keyed by its stem");
        assert_eq!(vault.get(&parse_id("aaa")).unwrap().title, "Original");
        assert_eq!(vault.get(&parse_id("bbb")).unwrap().title, "Original");
        assert!(
            vault.get(&parse_id("abc1234567")).is_none(),
            "the shared frontmatter id is not a key"
        );
        let mismatches: Vec<_> = vault
            .issues()
            .iter()
            .filter(|issue| issue.kind == VaultIssueKind::IdMismatch)
            .collect();
        assert_eq!(mismatches.len(), 2, "both files name a different id");
        assert_eq!(mismatches[0].path, dir.path().join("aaa.md"));
        assert_eq!(mismatches[1].path, dir.path().join("bbb.md"));
    }

    #[test]
    fn id_mismatch_is_reported_and_files_key_by_stem() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("wrongname.md"),
            "---\nid: abc1234567\ntitle: Loaded\nstate: open\n---\n",
        )
        .expect("write");

        vault.reload().expect("reload");

        let stem = parse_id("wrongname");
        assert_eq!(vault.len(), 1);
        let task = vault.get(&stem).expect("keyed by the file stem");
        assert_eq!(task.declared_id, parse_id("abc1234567"));
        assert!(
            vault.get(&parse_id("abc1234567")).is_none(),
            "the frontmatter id is not a key"
        );
        let mismatches: Vec<_> = vault
            .issues()
            .iter()
            .filter(|issue| issue.kind == VaultIssueKind::IdMismatch)
            .collect();
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].path, dir.path().join("wrongname.md"));
        assert!(mismatches[0].detail.contains("abc1234567"));
        assert!(mismatches[0].detail.contains("wrongname"));
    }

    #[test]
    fn files_with_invalid_stems_are_malformed_not_tasks() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("UPPER.md"),
            "---\nid: upper00001\ntitle: Upper\nstate: open\n---\n",
        )
        .expect("write");

        vault.reload().expect("reload");

        assert!(vault.is_empty(), "an invalid stem must not become a task");
        let malformed: Vec<_> = vault
            .issues()
            .iter()
            .filter(|issue| issue.kind == VaultIssueKind::Malformed)
            .collect();
        assert_eq!(malformed.len(), 1);
        assert_eq!(malformed[0].path, dir.path().join("UPPER.md"));
    }

    #[test]
    fn open_creates_a_missing_vault_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("nested").join("vault");

        let vault = Vault::open(&root).expect("open vault");

        assert!(root.is_dir());
        assert!(vault.is_empty());
        assert!(vault.issues().is_empty());
    }

    #[test]
    fn non_markdown_files_are_ignored() {
        let (dir, mut vault) = open_vault();
        fs::write(dir.path().join("notes.txt"), "not a task").expect("write txt");
        fs::write(dir.path().join("readme"), "not a task either").expect("write readme");

        vault.reload().expect("reload");

        assert!(vault.is_empty());
        assert!(vault.issues().is_empty());
    }

    #[test]
    fn reload_picks_up_external_edits() {
        let (_dir, mut vault) = open_vault();
        let task = vault.add(NewTask::new("Original")).expect("add");
        let edited = Task {
            title: "Edited externally".to_owned(),
            ..task.clone()
        };
        fs::write(
            vault.root().join(format!("{}.md", task.id)),
            edited.to_document(),
        )
        .expect("external write");

        vault.reload().expect("reload");

        assert_eq!(
            vault.get(&task.id).expect("still loaded").title,
            "Edited externally"
        );
    }

    #[test]
    fn rewriting_preserves_unknown_frontmatter_keys() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("aaa.md"),
            "---\nid: abc1234567\ntitle: T\nstate: open\ncustom: keep-me\n---\n",
        )
        .expect("write");
        vault.reload().expect("reload");

        let id = parse_id("aaa");
        vault.set_title(&id, "T2").expect("set title");

        let contents = fs::read_to_string(dir.path().join("aaa.md")).expect("read");
        assert!(contents.contains("custom: keep-me"));
        assert!(
            contents.contains("id: abc1234567"),
            "a mismatched declared id must survive a rewrite: {contents}"
        );
        assert!(contents.contains("title: T2"));
        assert!(Task::from_document(&contents)
            .expect("parse")
            .extra
            .contains_key("custom"));
    }

    #[test]
    fn mutating_unknown_tasks_fails() {
        let (_dir, mut vault) = open_vault();
        let missing = parse_id("doesnot123");

        assert!(matches!(
            vault.set_state(&missing, TaskState::Done),
            Err(VaultError::NotFound(_))
        ));
        assert!(matches!(
            vault.set_title(&missing, "x"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn empty_titles_are_rejected() {
        let (_dir, mut vault) = open_vault();
        assert!(matches!(
            vault.add(NewTask::new("   ")),
            Err(VaultError::EmptyTitle)
        ));

        let task = vault.add(NewTask::new("Real")).expect("add");
        assert!(matches!(
            vault.set_title(&task.id, "\t"),
            Err(VaultError::EmptyTitle)
        ));
    }

    #[test]
    fn set_parent_appends_to_the_destination_and_closes_the_old_rank_gap() {
        let (_dir, mut vault) = open_vault();
        let moving = vault.add(NewTask::new("Moving")).expect("add moving");
        let parent = vault.add(NewTask::new("Parent")).expect("add parent");
        let zebra = vault
            .add(NewTask {
                parent: Some(parent.id.clone()),
                ..NewTask::new("Zebra")
            })
            .expect("add zebra");
        let apple = vault
            .add(NewTask {
                parent: Some(parent.id.clone()),
                ..NewTask::new("Apple")
            })
            .expect("add apple");

        vault
            .set_parent(&moving.id, Some(&parent.id))
            .expect("reparent");

        assert_eq!(
            vault.children(&parent.id),
            &[zebra.id.clone(), apple.id.clone(), moving.id.clone()]
        );
        assert_eq!(vault.get(&zebra.id).expect("zebra").rank, Some(0));
        assert_eq!(vault.get(&apple.id).expect("apple").rank, Some(1));
        assert_eq!(vault.get(&moving.id).expect("moving").rank, Some(2));
        assert_eq!(
            vault.get(&parent.id).expect("remaining root").rank,
            Some(0),
            "the ranked old group is closed up"
        );
    }

    #[test]
    fn set_parent_moves_a_task_and_can_clear_it() {
        let (_dir, mut vault) = open_vault();
        let root = vault.add(NewTask::new("Root")).expect("add root");
        let other = vault.add(NewTask::new("Other root")).expect("add other");
        let child = vault
            .add(NewTask {
                parent: Some(root.id.clone()),
                ..NewTask::new("Child")
            })
            .expect("add child");

        let moved = vault
            .set_parent(&child.id, Some(&other.id))
            .expect("move child");
        assert_eq!(moved.parent, Some(other.id.clone()));
        assert!(vault.children(&other.id).contains(&child.id));
        assert!(!vault.children(&root.id).contains(&child.id));
        let stored = Task::from_document(&read_task_file(&vault, &child.id)).expect("parse");
        assert_eq!(stored.parent, Some(other.id.clone()));

        let detached = vault.set_parent(&child.id, None).expect("detach child");
        assert_eq!(detached.parent, None);
        assert!(vault.roots().contains(&child.id));
        let stored = Task::from_document(&read_task_file(&vault, &child.id)).expect("parse");
        assert_eq!(stored.parent, None);
    }

    #[test]
    fn set_parent_rejects_self_and_descendants() {
        let (_dir, mut vault) = open_vault();
        let root = vault.add(NewTask::new("Root")).expect("add root");
        let child = vault
            .add(NewTask {
                parent: Some(root.id.clone()),
                ..NewTask::new("Child")
            })
            .expect("add child")
            .id;
        let grandchild = vault
            .add(NewTask {
                parent: Some(child.clone()),
                ..NewTask::new("Grandchild")
            })
            .expect("add grandchild")
            .id;

        assert!(matches!(
            vault.set_parent(&root.id, Some(&root.id)),
            Err(VaultError::InvalidParent { .. })
        ));
        assert!(matches!(
            vault.set_parent(&root.id, Some(&grandchild)),
            Err(VaultError::InvalidParent { .. })
        ));
        assert!(matches!(
            vault.set_parent(&child, Some(&grandchild)),
            Err(VaultError::InvalidParent { .. })
        ));

        // Moving a task to an ancestor is legal: it does not create a cycle.
        let moved = vault
            .set_parent(&grandchild, Some(&root.id))
            .expect("move up");
        assert_eq!(moved.parent, Some(root.id.clone()));
        assert_eq!(vault.parent(&grandchild), Some(&root.id));
    }

    #[test]
    fn set_parent_rejects_unknown_tasks_and_parents() {
        let (_dir, mut vault) = open_vault();
        let task = vault.add(NewTask::new("Task")).expect("add");
        let missing = parse_id("doesnot123");

        assert!(matches!(
            vault.set_parent(&missing, None),
            Err(VaultError::NotFound(_))
        ));
        assert!(matches!(
            vault.set_parent(&task.id, Some(&missing)),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn set_parent_preserves_unknown_frontmatter() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("aaa.md"),
            "---\nid: abc1234567\ntitle: T\nstate: open\ncustom: keep-me\n---\n",
        )
        .expect("write");
        vault.reload().expect("reload");
        let id = parse_id("aaa");
        let root = vault.add(NewTask::new("Root")).expect("add root");

        vault.set_parent(&id, Some(&root.id)).expect("set parent");

        let contents = fs::read_to_string(dir.path().join("aaa.md")).expect("read");
        assert!(contents.contains("custom: keep-me"));
        assert!(contents.contains(&format!("parent: {}", root.id)));
        assert!(Task::from_document(&contents)
            .expect("parse")
            .extra
            .contains_key("custom"));
    }

    #[test]
    fn delete_removes_files_and_skips_unknown_ids() {
        let (_dir, mut vault) = open_vault();
        let keep = vault.add(NewTask::new("Keep")).expect("add keep");
        let drop = vault.add(NewTask::new("Drop")).expect("add drop");

        let outcome = vault
            .delete(&[drop.id.clone(), parse_id("doesnot123")])
            .expect("delete");

        assert_eq!(outcome, DeleteOutcome { deleted: 1 });
        assert!(vault.get(&keep.id).is_some());
        assert!(vault.get(&drop.id).is_none());
        assert!(!vault.root().join(format!("{}.md", drop.id)).exists());
        assert!(vault.root().join(format!("{}.md", keep.id)).exists());

        assert_eq!(vault.delete(&[]).expect("empty"), DeleteOutcome::default());
        assert_eq!(
            vault
                .delete(&[parse_id("othermissin")])
                .expect("unknown only"),
            DeleteOutcome::default()
        );
    }

    #[test]
    fn delete_tolerates_a_file_that_is_already_gone() {
        let (_dir, mut vault) = open_vault();
        let gone = vault.add(NewTask::new("Gone")).expect("add gone").id;
        let child = vault
            .add(NewTask {
                parent: Some(gone.clone()),
                ..NewTask::new("Child")
            })
            .expect("add child")
            .id;
        // Remove the parent behind the vault's back; the cache still has it.
        fs::remove_file(vault.root().join(format!("{gone}.md"))).expect("remove file");

        let outcome = vault.delete(std::slice::from_ref(&gone)).expect("delete");

        assert_eq!(outcome, DeleteOutcome { deleted: 2 });
        assert!(vault.get(&gone).is_none());
        assert!(vault.get(&child).is_none());
        assert!(!vault.root().join(format!("{child}.md")).exists());
    }

    #[test]
    fn delete_removes_the_whole_subtree_without_reparenting() {
        let (_dir, mut vault) = open_vault();
        let root = vault.add(NewTask::new("Root")).expect("add root").id;
        let middle = vault
            .add(NewTask {
                parent: Some(root.clone()),
                ..NewTask::new("Middle")
            })
            .expect("add middle")
            .id;
        let child = vault
            .add(NewTask {
                parent: Some(middle.clone()),
                ..NewTask::new("Child")
            })
            .expect("add child")
            .id;

        let outcome = vault
            .delete(std::slice::from_ref(&middle))
            .expect("delete middle");

        assert_eq!(outcome, DeleteOutcome { deleted: 2 });
        assert!(vault.get(&middle).is_none());
        assert!(vault.get(&child).is_none(), "descendants die with the task");
        assert!(vault.get(&root).is_some(), "ancestors survive");
        assert!(vault.children(&root).is_empty(), "nothing is reparented");
        assert!(!vault.root().join(format!("{middle}.md")).exists());
        assert!(!vault.root().join(format!("{child}.md")).exists());
    }

    #[test]
    fn delete_of_a_root_removes_its_subtree_and_leaves_other_roots_alone() {
        let (_dir, mut vault) = open_vault();
        let root = vault.add(NewTask::new("Root")).expect("add root").id;
        let child = vault
            .add(NewTask {
                parent: Some(root.clone()),
                ..NewTask::new("Child")
            })
            .expect("add child")
            .id;
        let other = vault.add(NewTask::new("Other")).expect("add other").id;

        let outcome = vault
            .delete(std::slice::from_ref(&root))
            .expect("delete root");

        assert_eq!(outcome, DeleteOutcome { deleted: 2 });
        assert!(vault.get(&root).is_none());
        assert!(vault.get(&child).is_none());
        assert!(vault.get(&other).is_some(), "unrelated roots survive");
        assert!(vault.roots().contains(&other));
        assert!(!vault.root().join(format!("{root}.md")).exists());
        assert!(!vault.root().join(format!("{child}.md")).exists());
        assert!(vault.root().join(format!("{other}.md")).exists());
    }

    #[test]
    fn multi_delete_with_parent_and_child_dedupes_the_closure() {
        let (_dir, mut vault) = open_vault();
        let grandparent = vault.add(NewTask::new("Grandparent")).expect("add").id;
        let parent = vault
            .add(NewTask {
                parent: Some(grandparent.clone()),
                ..NewTask::new("Parent")
            })
            .expect("add")
            .id;
        let child = vault
            .add(NewTask {
                parent: Some(parent.clone()),
                ..NewTask::new("Child")
            })
            .expect("add")
            .id;
        let grandchild = vault
            .add(NewTask {
                parent: Some(child.clone()),
                ..NewTask::new("Grandchild")
            })
            .expect("add")
            .id;
        let bystander = vault.add(NewTask::new("Bystander")).expect("add").id;

        assert_eq!(
            vault.descendant_count(&[parent.clone(), child.clone()]),
            1,
            "only the grandchild is not itself requested"
        );
        let outcome = vault
            .delete(&[parent.clone(), child.clone()])
            .expect("delete");

        assert_eq!(
            outcome,
            DeleteOutcome { deleted: 3 },
            "parent, child, and grandchild, each exactly once"
        );
        assert!(vault.get(&parent).is_none());
        assert!(vault.get(&child).is_none());
        assert!(vault.get(&grandchild).is_none());
        assert!(vault.get(&grandparent).is_some(), "the ancestor survives");
        assert!(vault.get(&bystander).is_some(), "unrelated tasks survive");
        assert!(!vault.root().join(format!("{parent}.md")).exists());
        assert!(!vault.root().join(format!("{child}.md")).exists());
        assert!(!vault.root().join(format!("{grandchild}.md")).exists());
    }

    #[test]
    fn multi_delete_merges_overlapping_subtrees() {
        let (_dir, mut vault) = open_vault();
        let a = vault.add(NewTask::new("A")).expect("add").id;
        let b = vault
            .add(NewTask {
                parent: Some(a.clone()),
                ..NewTask::new("B")
            })
            .expect("add")
            .id;
        let c = vault
            .add(NewTask {
                parent: Some(b.clone()),
                ..NewTask::new("C")
            })
            .expect("add")
            .id;
        let survivor = vault.add(NewTask::new("Survivor")).expect("add").id;

        // `a` already covers `c`; the closure must count every file once.
        let outcome = vault.delete(&[a.clone(), c.clone()]).expect("delete chain");

        assert_eq!(outcome, DeleteOutcome { deleted: 3 });
        assert_eq!(vault.len(), 1, "only the survivor remains");
        assert!(vault.get(&a).is_none());
        assert!(vault.get(&b).is_none());
        assert!(vault.get(&c).is_none());
        assert!(vault.get(&survivor).is_some());
    }

    #[test]
    fn delete_follows_the_effective_tree_for_dangling_parents() {
        let (_dir, mut vault) = open_vault();
        let doomed = vault.add(NewTask::new("Doomed")).expect("add").id;
        let child = vault
            .add(NewTask {
                parent: Some(doomed.clone()),
                ..NewTask::new("Child")
            })
            .expect("add")
            .id;
        // Corrupt the doomed task's parent pointer to an id that is not loaded.
        let path = vault.root().join(format!("{doomed}.md"));
        let contents = fs::read_to_string(&path).expect("read");
        fs::write(
            &path,
            contents.replace("state: open", "state: open\nparent: missing0001"),
        )
        .expect("write");
        vault.reload().expect("reload");

        let outcome = vault.delete(std::slice::from_ref(&doomed)).expect("delete");

        assert_eq!(outcome, DeleteOutcome { deleted: 2 });
        assert!(vault.get(&doomed).is_none());
        assert!(
            vault.get(&child).is_none(),
            "the dangling task's subtree dies with it"
        );
        assert!(!vault.root().join(format!("{doomed}.md")).exists());
        assert!(!vault.root().join(format!("{child}.md")).exists());
    }

    #[test]
    fn delete_terminates_on_a_corrupt_parent_cycle() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("aaaa.md"),
            "---\nid: aaaa\ntitle: A\nstate: open\nparent: bbbb\n---\n",
        )
        .expect("write aaaa");
        fs::write(
            dir.path().join("bbbb.md"),
            "---\nid: bbbb\ntitle: B\nstate: open\nparent: aaaa\n---\n",
        )
        .expect("write bbbb");
        fs::write(
            dir.path().join("cccc.md"),
            "---\nid: cccc\ntitle: C\nstate: open\nparent: aaaa\n---\n",
        )
        .expect("write cccc");
        vault.reload().expect("reload");
        assert!(!vault.issues().is_empty(), "the cycle is reported");

        // The index breaks the cycle at aaaa, which still roots the subtree
        // over bbbb and cccc; the closure walk terminates.
        let outcome = vault
            .delete(&[parse_id("aaaa")])
            .expect("delete within a cycle");

        assert_eq!(outcome, DeleteOutcome { deleted: 3 });
        assert!(vault.get(&parse_id("aaaa")).is_none());
        assert!(vault.get(&parse_id("bbbb")).is_none());
        assert!(vault.get(&parse_id("cccc")).is_none());
        assert!(!dir.path().join("aaaa.md").exists());
        assert!(!dir.path().join("bbbb.md").exists());
        assert!(!dir.path().join("cccc.md").exists());
    }

    #[test]
    fn multi_delete_terminates_when_the_input_covers_a_cycle() {
        let (dir, mut vault) = open_vault();
        fs::write(
            dir.path().join("dddd.md"),
            "---\nid: dddd\ntitle: D\nstate: open\nparent: eeee\n---\n",
        )
        .expect("write dddd");
        fs::write(
            dir.path().join("eeee.md"),
            "---\nid: eeee\ntitle: E\nstate: open\nparent: dddd\n---\n",
        )
        .expect("write eeee");
        fs::write(
            dir.path().join("cccc.md"),
            "---\nid: cccc\ntitle: C\nstate: open\nparent: dddd\n---\n",
        )
        .expect("write cccc");
        vault.reload().expect("reload");

        let outcome = vault
            .delete(&[parse_id("dddd"), parse_id("eeee")])
            .expect("delete a doomed cycle");

        assert_eq!(
            outcome,
            DeleteOutcome { deleted: 3 },
            "the cycle plus its descendant"
        );
        assert!(vault.get(&parse_id("dddd")).is_none());
        assert!(vault.get(&parse_id("eeee")).is_none());
        assert!(vault.get(&parse_id("cccc")).is_none());
    }

    #[test]
    fn descendant_count_matches_what_delete_removes() {
        let (_dir, mut vault) = open_vault();
        let root = vault.add(NewTask::new("Root")).expect("add").id;
        let child = vault
            .add(NewTask {
                parent: Some(root.clone()),
                ..NewTask::new("Child")
            })
            .expect("add")
            .id;
        vault
            .add(NewTask {
                parent: Some(child.clone()),
                ..NewTask::new("Grandchild")
            })
            .expect("add");

        assert_eq!(vault.descendant_count(std::slice::from_ref(&root)), 2);
        assert_eq!(vault.descendant_count(std::slice::from_ref(&child)), 1);
        assert_eq!(vault.descendant_count(&[]), 0);
        assert_eq!(vault.descendant_count(&[parse_id("doesnot123")]), 0);
        assert_eq!(
            vault.descendant_count(&[root.clone(), child.clone()]),
            1,
            "requested descendants are not counted twice"
        );

        let outcome = vault.delete(&[root]).expect("delete");
        assert_eq!(outcome.deleted, 3);
    }

    #[test]
    fn delete_never_rewrites_surviving_files() {
        let (_dir, mut vault) = open_vault();
        let doomed = vault.add(NewTask::new("Doomed")).expect("add").id;
        let keep = vault.add(NewTask::new("Keep")).expect("add").id;
        let path = vault.root().join(format!("{keep}.md"));
        let before = format!("---\nid: {keep}\ntitle: Keep\nstate: open\ncustom: keep-me\n---\n");
        fs::write(&path, &before).expect("write keep");
        vault.reload().expect("reload");

        vault.delete(&[doomed]).expect("delete");

        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            before,
            "surviving files are untouched, unknown keys included"
        );
    }

    #[test]
    fn delete_leaves_links_to_deleted_ids_dangling() {
        let (_dir, mut vault) = open_vault();
        let target = vault.add(NewTask::new("Target")).expect("add target");
        let source = vault
            .add(NewTask {
                body: format!("see [[{}]]", target.id),
                ..NewTask::new("Source")
            })
            .expect("add source");

        vault
            .delete(std::slice::from_ref(&target.id))
            .expect("delete target");

        assert_eq!(vault.links(&source.id), &[target.id.clone()][..]);
        assert!(vault.get(&target.id).is_none());
        let stored = Task::from_document(&read_task_file(&vault, &source.id)).expect("parse");
        assert_eq!(
            stored.body,
            format!("see [[{}]]", target.id),
            "the source file is never rewritten"
        );
    }
}
