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
use crate::model::{normalize_tags, ParseError, Priority, Task, TaskId, TaskState};
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
            tags: Vec::new(),
            due: None,
            priority: None,
            body: String::new(),
        }
    }
}

/// Result of a [`Vault::delete`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeleteOutcome {
    /// Task files that were removed, descendants included.
    pub deleted: usize,
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
        vault.reload();
        Ok(vault)
    }

    /// Rescan the vault folder, replacing the in-memory cache and index, and
    /// return the issues found.
    ///
    /// This is the only way to pick up external edits; the cache and the index
    /// are fully disposable.
    pub fn reload(&mut self) -> Vec<VaultIssue> {
        let (tasks, scan_issues) = scan(&self.root);
        self.tasks = tasks;
        self.scan_issues = scan_issues;
        self.rebuild_index();
        self.issues.clone()
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

    /// Root task ids in display order (lowercased title, then id).
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

    /// Create a task, assign it a fresh id, and persist it atomically.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::EmptyTitle`] for a blank title, or
    /// [`VaultError::Io`] when the file cannot be written. On error the
    /// in-memory cache is left unchanged.
    pub fn add(&mut self, new: NewTask) -> Result<Task, VaultError> {
        let NewTask {
            title,
            parent,
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
        self.persist(task)
    }

    /// Set the state of an existing task and persist it.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when the file cannot be written.
    pub fn set_state(&mut self, id: &TaskId, state: TaskState) -> Result<Task, VaultError> {
        let mut task = self.cloned(id)?;
        task.state = state;
        self.persist(task)
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
        let mut task = self.cloned(id)?;
        task.priority = priority;
        self.persist(task)
    }

    /// Replace an existing task's tags, normalizing them before persistence.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::NotFound`] for an unknown id or
    /// [`VaultError::Io`] when the file cannot be written.
    pub fn set_tags(&mut self, id: &TaskId, tags: Vec<String>) -> Result<Task, VaultError> {
        let mut task = self.cloned(id)?;
        task.tags = normalize_tags(tags);
        self.persist(task)
    }

    /// Set the title of an existing task and persist it.
    ///
    /// This is a direct rename: unlike [`Vault::sync_mirror_aliases`] it does
    /// not touch links that alias this task.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::EmptyTitle`] for a blank title,
    /// [`VaultError::NotFound`] for an unknown id, or [`VaultError::Io`] when
    /// the file cannot be written.
    pub fn set_title(&mut self, id: &TaskId, title: &str) -> Result<Task, VaultError> {
        if title.trim().is_empty() {
            return Err(VaultError::EmptyTitle);
        }
        let mut task = self.cloned(id)?;
        task.title = title.to_owned();
        self.persist(task)
    }

    /// Rewrite mirror aliases after a reload observed title changes.
    ///
    /// `old_titles` is the id → title map from before the reload. Ids present
    /// in both that map and the vault whose title changed form the diff; a
    /// link whose alias equals a changed target's old title (after trimming
    /// the alias) is a **mirror** and is rewritten to the new title. The
    /// qualifier matters: contextual aliases, bare links, links in code, and
    /// dangling or cross-store targets (which have no old title here) are
    /// never touched. Each affected body is persisted atomically through
    /// [`Vault::set_body`], so unknown frontmatter and every non-alias byte
    /// survive.
    ///
    /// The diff is applied once per id against the old titles, so a rename
    /// chain `A → B, B → C` in one snapshot rewrites each alias to its own
    /// target's new title and never chases transitively. Returns the number
    /// of files written — 0 when nothing changed, which is what makes a
    /// cascade reload diff clean and keeps the watcher from looping.
    ///
    /// This method never snapshots titles itself: a cold scan (CLI, fresh
    /// TUI start) has no previous index and therefore never writes.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::Io`] when a rewrite cannot be persisted. Files
    /// written before the failure stay written; the in-memory cache matches
    /// disk.
    pub fn sync_mirror_aliases(
        &mut self,
        old_titles: &BTreeMap<TaskId, String>,
    ) -> Result<usize, VaultError> {
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
            self.set_body(&source, &rewritten)?;
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
        let mut task = self.cloned(id)?;
        task.body = body.to_owned();
        self.persist(task)
    }

    /// Move an existing task under `new_parent`, or to the vault root when
    /// `new_parent` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::InvalidParent`] when `new_parent` is the task
    /// itself or one of its descendants (the strict tree forbids cycles),
    /// [`VaultError::NotFound`] for an unknown task or an unknown parent, or
    /// [`VaultError::Io`] when the file cannot be written.
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
        task.parent = new_parent.cloned();
        self.persist(task)
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

    fn path_for(&self, id: &TaskId) -> PathBuf {
        self.root.join(format!("{id}.md"))
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
        .filter(|path| path.is_file() && path.extension() == Some(OsStr::new("md")))
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

        let updated = vault.set_title(&task.id, "New title").expect("set title");
        assert_eq!(updated.title, "New title");
        assert_eq!(updated.body, "keep this body\n");
        assert_eq!(updated.state, TaskState::Open);

        let stored = Task::from_document(&read_task_file(&vault, &task.id)).expect("parse");
        assert_eq!(stored.title, "New title");
        assert_eq!(stored.body, "keep this body\n");
        assert_eq!(stored.state, TaskState::Open);
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

        let issues = vault.reload();

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

        let issues = vault.reload();

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

        let issues = vault.reload();

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

        let issues = vault.reload();

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

        vault.reload();

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

        vault.reload();

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

        vault.reload();

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

        vault.reload();

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

        vault.reload();

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
        vault.reload();

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
        vault.reload();
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
        vault.reload();

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
        vault.reload();
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
        vault.reload();

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
        vault.reload();

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
