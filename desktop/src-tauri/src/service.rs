//! Tauri-free task service: the single backend seam over the `tt` library.
//!
//! Everything the desktop app does to Projects, Stores, and Tasks goes through
//! [`TtService`]. It is deliberately free of Tauri types so it can be unit
//! tested against temporary fixtures with explicit config/store paths. The
//! Tauri command layer in [`crate::commands`] only translates IPC calls.
//!
//! Facts this module encodes (see `docs/design/macos-client.md`):
//!
//! - A registered Project maps to a Store under the tt data directory; the
//!   desktop never treats the Project directory (or the app cwd) as the task
//!   store.
//! - Every task mutation goes through existing [`Vault`] methods; the service
//!   only re-reads the store first so a stale cache cannot clobber external
//!   edits at the field level.
//! - Description drafts keep the raw file revision they started from; a save
//!   re-reads the file and refuses when it changed or disappeared. This is
//!   conflict detection, not transactional safety against legacy writers.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use serde::Serialize;
use tt::registry;
use tt::vault::{Vault, VaultError};
use tt::{Config, NewTask, Priority, Project, Task, TaskFilter, TaskId, TaskState, TreeNode};

/// Errors returned to the frontend.
///
/// `code` is a stable machine-readable discriminant; `message` is the human
/// text. Description-save conflicts additionally carry the current disk
/// revision and body so the UI can offer "Reload latest" without guessing.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct ServiceError {
    /// Stable error code.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
    /// Extra data for a stale-save conflict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictInfo>,
}

/// What changed under a refused description save.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    /// `changed` when the file exists with different bytes, `deleted` when it
    /// is gone.
    pub reason: String,
    /// Revision the file has on disk now (`None` when deleted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,
    /// Body parsed from the current disk document, when it still parses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_body: Option<String>,
}

impl ServiceError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            conflict: None,
        }
    }

    fn no_data_dir() -> Self {
        Self::new(
            "no_data_dir",
            "cannot determine the data directory: set $XDG_DATA_HOME or $HOME",
        )
    }

    fn no_project() -> Self {
        Self::new("no_project", "no project is open")
    }

    fn not_found(id: impl std::fmt::Display) -> Self {
        Self::new("not_found", format!("task not found: {id}"))
    }

    fn unknown_project(slug: &str) -> Self {
        Self::new("unknown_project", format!("no registered project {slug:?}"))
    }

    fn conflict(
        reason: &str,
        current_revision: Option<String>,
        current_body: Option<String>,
    ) -> Self {
        Self {
            code: "stale_draft".to_owned(),
            message: if reason == "deleted" {
                "the task file was deleted outside the app".to_owned()
            } else {
                "the task file changed on disk".to_owned()
            },
            conflict: Some(ConflictInfo {
                reason: reason.to_owned(),
                current_revision,
                current_body,
            }),
        }
    }

    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::new(
            "io",
            format!("filesystem error at {}: {source}", path.display()),
        )
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }
}

impl From<VaultError> for ServiceError {
    fn from(error: VaultError) -> Self {
        let code = match &error {
            VaultError::NotFound(_) => "not_found",
            VaultError::EmptyTitle => "empty_title",
            VaultError::InvalidParent { .. } => "invalid_parent",
            VaultError::SameStore => "same_store",
            VaultError::IdCollision { .. } => "id_collision",
            VaultError::PartialMove { .. } | VaultError::PartialCommit { .. } => "partial_commit",
            VaultError::InvalidTaskFile { .. } | VaultError::Io { .. } => "io",
        };
        Self::new(code, error.to_string())
    }
}

impl From<tt::ConfigError> for ServiceError {
    fn from(error: tt::ConfigError) -> Self {
        Self::new("config", error.to_string())
    }
}

impl From<tt::RegistryError> for ServiceError {
    fn from(error: tt::RegistryError) -> Self {
        match error {
            tt::RegistryError::NoDataDir => Self::no_data_dir(),
            other => Self::new("io", other.to_string()),
        }
    }
}

/// A registered Project as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    /// Store slug, the stable handle the UI selects by.
    pub slug: String,
    /// Absolute project directory.
    pub path: String,
    /// Directory name, for disambiguating same-name projects.
    pub name: String,
}

/// A task row in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNodeDto {
    /// Task identity.
    pub id: String,
    /// Live title.
    pub title: String,
    /// `open | done | cancelled`.
    pub state: String,
    /// ISO date, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// `high | med | low`, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// Tags without a leading `#`.
    pub tags: Vec<String>,
    /// Effective parent, when the task is not a root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Whether the task has children in the effective forest.
    pub has_children: bool,
    /// Done descendants (0 for leaves).
    pub done: usize,
    /// Total non-cancelled descendants (0 for leaves).
    pub total: usize,
    /// Children in core display order.
    pub children: Vec<TaskNodeDto>,
}

/// A lightweight task reference used for links, backlinks, children, and the
/// capture target. `title` is `None` for a dangling reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRefDto {
    /// Task identity.
    pub id: String,
    /// Live title, when the task loads in this store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A store issue, surfaced verbatim and never repaired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDto {
    /// File (or folder) the issue refers to.
    pub path: String,
    /// Issue category label.
    pub kind: String,
    /// Human-readable detail.
    pub detail: String,
}

/// The whole project-scoped state the tree view renders from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDto {
    /// Every registered Project, in registry order.
    pub projects: Vec<ProjectDto>,
    /// The open Project, when one is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectDto>,
    /// Sticky store issues for the open Project.
    pub issues: Vec<IssueDto>,
    /// The task forest in core display order.
    pub tree: Vec<TaskNodeDto>,
    /// Number of loaded tasks.
    pub task_count: usize,
    /// The configured capture target, when it resolves in the open store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_target: Option<TaskRefDto>,
    /// Changes whenever any loaded task document changes; the UI uses it to
    /// notice external edits without comparing whole trees.
    pub revision: String,
}

/// Everything the details pane needs for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetailDto {
    /// Task identity.
    pub id: String,
    /// Live title.
    pub title: String,
    /// `open | done | cancelled`.
    pub state: String,
    /// ISO date, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// `high | med | low`, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// Tags without a leading `#`.
    pub tags: Vec<String>,
    /// Effective parent, when the task is not a root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Open Project slug.
    pub project_slug: String,
    /// Absolute project directory.
    pub project_path: String,
    /// Absolute task file path.
    pub file_path: String,
    /// Raw revision of the task file, the draft baseline.
    pub revision: String,
    /// Raw markdown body.
    pub body: String,
    /// Outgoing wikilinks in body order.
    pub links: Vec<TaskRefDto>,
    /// Tasks whose body links here, in id order.
    pub backlinks: Vec<TaskRefDto>,
    /// Direct children in display order.
    pub children: Vec<TaskRefDto>,
    /// Done descendants.
    pub done: usize,
    /// Total non-cancelled descendants.
    pub total: usize,
}

/// A project with its open Store.
#[derive(Debug)]
struct OpenStore {
    project: Project,
    vault: Vault,
}

/// The one backend service behind every command.
#[derive(Debug)]
pub struct TtService {
    /// Explicit config path for tests; `None` follows the tt environment.
    config_path: Option<PathBuf>,
    /// tt data directory, when it can be determined.
    data_dir: Option<PathBuf>,
    /// Last loaded config (registry + capture target).
    config: Config,
    /// The open Project and its store, when one is open.
    open: Option<OpenStore>,
}

impl TtService {
    /// Production constructor: follow `$TT_CONFIG`, `$XDG_DATA_HOME`, and
    /// `$HOME` exactly as the terminal client does.
    pub fn from_env() -> Self {
        Self {
            config_path: None,
            data_dir: registry::data_dir(),
            config: Config::default(),
            open: None,
        }
    }

    /// Test constructor with explicit paths, so no test touches the user's
    /// real registry or stores.
    ///
    /// # Errors
    ///
    /// Returns a config error when `config_path` exists and cannot be read.
    pub fn with_paths(config_path: PathBuf, data_dir: PathBuf) -> Result<Self, ServiceError> {
        let config = Config::load_from(Some(config_path.clone()))?;
        Ok(Self {
            config_path: Some(config_path),
            data_dir: Some(data_dir),
            config,
            open: None,
        })
    }

    fn data_dir(&self) -> Result<&Path, ServiceError> {
        self.data_dir
            .as_deref()
            .ok_or_else(ServiceError::no_data_dir)
    }

    /// Re-read the config so external registry edits are visible.
    ///
    /// # Errors
    ///
    /// Returns a config error when the file exists and cannot be parsed.
    pub fn refresh_config(&mut self) -> Result<(), ServiceError> {
        let config = match &self.config_path {
            Some(path) => Config::load_from(Some(path.clone()))?,
            None => Config::load()?,
        };
        self.config = config;
        Ok(())
    }

    /// Re-read the open Store from disk, refreshing the core cache.
    fn reload_store(&mut self) -> Result<(), ServiceError> {
        if let Some(open) = self.open.as_mut() {
            open.vault.reload()?;
        }
        Ok(())
    }

    /// Initial call: load the registry, no Project open.
    ///
    /// # Errors
    ///
    /// Returns a config error when the registry cannot be read.
    pub fn bootstrap(&mut self) -> Result<SnapshotDto, ServiceError> {
        self.refresh_config()?;
        self.snapshot()
    }

    /// Open a registered Project by slug and return the new snapshot.
    ///
    /// # Errors
    ///
    /// Returns `unknown_project` when the slug is not registered, or an I/O
    /// error when the project directory or store folder is unusable.
    pub fn open_project(&mut self, slug: &str) -> Result<SnapshotDto, ServiceError> {
        self.refresh_config()?;
        let project = self
            .config
            .projects
            .iter()
            .find(|project| project.slug == slug)
            .cloned()
            .ok_or_else(|| ServiceError::unknown_project(slug))?;
        let data_dir = self.data_dir()?.to_path_buf();
        let vault = registry::open_store(&data_dir, &project)?;
        self.open = Some(OpenStore { project, vault });
        self.snapshot()
    }

    /// Re-read the registry and the open Store; used by the 1 s reconciliation.
    ///
    /// # Errors
    ///
    /// Propagates config or store read failures.
    pub fn refresh(&mut self) -> Result<SnapshotDto, ServiceError> {
        self.refresh_config()?;
        self.reload_store()?;
        self.snapshot()
    }

    /// Register a Project directory and persist the registry.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the store folder cannot be created, or a
    /// config error when the registry cannot be saved.
    pub fn register_project(&mut self, path: &Path) -> Result<SnapshotDto, ServiceError> {
        self.refresh_config()?;
        let data_dir = self.data_dir()?.to_path_buf();
        registry::register_in(&mut self.config, path, &data_dir)?;
        self.config.save()?;
        self.snapshot()
    }

    /// Unregister a Project by slug, keeping its tasks on disk and its store
    /// folder untouched.
    ///
    /// # Errors
    ///
    /// Returns a config error when the registry cannot be saved.
    pub fn unregister_project(&mut self, slug: &str) -> Result<SnapshotDto, ServiceError> {
        self.refresh_config()?;
        let removed = registry::remove_slug(&mut self.config, slug);
        if removed.is_some() {
            self.config.save()?;
        }
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.project.slug == slug)
        {
            self.open = None;
        }
        self.snapshot()
    }

    fn project_dto(project: &Project) -> ProjectDto {
        ProjectDto {
            slug: project.slug.clone(),
            path: project.path.to_string_lossy().into_owned(),
            name: project
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| project.slug.clone()),
        }
    }

    /// Build the project-scoped snapshot the tree view renders.
    ///
    /// # Errors
    ///
    /// This never reads disk beyond what is already cached, so it currently
    /// cannot fail; the `Result` keeps the command layer uniform.
    pub fn snapshot(&self) -> Result<SnapshotDto, ServiceError> {
        let projects: Vec<ProjectDto> =
            self.config.projects.iter().map(Self::project_dto).collect();
        let Some(open) = &self.open else {
            return Ok(SnapshotDto {
                projects,
                project: None,
                issues: Vec::new(),
                tree: Vec::new(),
                task_count: 0,
                capture_target: None,
                revision: String::new(),
            });
        };

        let vault = &open.vault;
        let tree: Vec<TaskNodeDto> = vault
            .tree()
            .iter()
            .map(|node| node_dto(vault, node))
            .collect();
        let issues = vault
            .issues()
            .iter()
            .map(|issue| IssueDto {
                path: issue.path.to_string_lossy().into_owned(),
                kind: issue.kind.as_str().to_owned(),
                detail: issue.detail.clone(),
            })
            .collect();
        let capture_target = self
            .config
            .capture_target
            .as_ref()
            .and_then(|id| vault.get(id))
            .map(|task| TaskRefDto {
                id: task.id.to_string(),
                title: Some(task.title.clone()),
            });

        Ok(SnapshotDto {
            projects,
            project: Some(Self::project_dto(&open.project)),
            issues,
            tree,
            task_count: vault.len(),
            capture_target,
            revision: store_revision(vault),
        })
    }

    /// Full details for one task, including the raw body and its disk
    /// revision. Re-reads the store first so the body is disk truth.
    ///
    /// # Errors
    ///
    /// Returns `no_project` when none is open or `not_found` for an unknown id.
    pub fn task_detail(&mut self, id: &str) -> Result<TaskDetailDto, ServiceError> {
        let id = parse_id(id)?;
        self.reload_store()?;
        let open = self.open.as_ref().ok_or_else(ServiceError::no_project)?;
        let task = open
            .vault
            .get(&id)
            .ok_or_else(|| ServiceError::not_found(&id))?;
        let file = open.vault.root().join(id.file_name());
        let revision = match fs::read(&file) {
            Ok(bytes) => revision_hex(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(ServiceError::io(&file, error)),
        };
        Ok(detail_dto(open, task, revision))
    }

    /// Save a description draft, refusing when the file changed or vanished
    /// since the draft's baseline revision.
    ///
    /// # Errors
    ///
    /// Returns `stale_draft` with conflict details when the disk revision does
    /// not equal `base_revision`.
    pub fn save_body(
        &mut self,
        id: &str,
        body: &str,
        base_revision: &str,
    ) -> Result<TaskDetailDto, ServiceError> {
        let id = parse_id(id)?;
        self.reload_store()?;
        let open = self.open.as_ref().ok_or_else(ServiceError::no_project)?;
        let file = open.vault.root().join(id.file_name());
        let bytes = match fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ServiceError::conflict("deleted", None, None));
            }
            Err(error) => return Err(ServiceError::io(&file, error)),
        };
        let current = revision_hex(&bytes);
        if current != base_revision {
            let current_body = String::from_utf8(bytes)
                .ok()
                .and_then(|text| Task::from_document(&text).ok())
                .map(|task| task.body);
            return Err(ServiceError::conflict(
                "changed",
                Some(current),
                current_body,
            ));
        }
        if open.vault.get(&id).is_none() {
            return Err(ServiceError::not_found(&id));
        }
        self.open
            .as_mut()
            .expect("checked above")
            .vault
            .set_body(&id, body)?;
        self.task_detail(id.as_ref())
    }

    /// Create a root task, a sub-task, or a capture-target task.
    ///
    /// `capture` uses `config.capture_target` and is only offered by the UI
    /// when that target resolves in the open store; an explicit root
    /// (`parent_id = None`, `capture = false`) never falls back to it.
    ///
    /// # Errors
    ///
    /// Returns `not_found` for an unknown parent, `capture_target_unavailable`
    /// when capture has no usable target, or `empty_title`.
    pub fn add_task(
        &mut self,
        title: &str,
        parent_id: Option<&str>,
        capture: bool,
    ) -> Result<SnapshotDto, ServiceError> {
        self.reload_store()?;
        let parent = self.resolve_new_parent(parent_id, capture)?;
        self.open
            .as_mut()
            .ok_or_else(ServiceError::no_project)?
            .vault
            .add(NewTask {
                parent,
                ..NewTask::new(title)
            })?;
        self.snapshot()
    }

    fn resolve_new_parent(
        &self,
        parent_id: Option<&str>,
        capture: bool,
    ) -> Result<Option<TaskId>, ServiceError> {
        let vault = &self
            .open
            .as_ref()
            .ok_or_else(ServiceError::no_project)?
            .vault;
        if capture {
            let target = self
                .config
                .capture_target
                .clone()
                .filter(|target| vault.get(target).is_some())
                .ok_or_else(|| {
                    ServiceError::new(
                        "capture_target_unavailable",
                        "the configured capture target is not a task in this project",
                    )
                })?;
            return Ok(Some(target));
        }
        match parent_id {
            Some(raw) => {
                let id = parse_id(raw)?;
                if vault.get(&id).is_none() {
                    return Err(ServiceError::not_found(raw));
                }
                Ok(Some(id))
            }
            None => Ok(None),
        }
    }

    /// Apply a task mutation to the open store and return the new snapshot.
    fn mutate(
        &mut self,
        f: impl FnOnce(&mut Vault) -> Result<(), ServiceError>,
    ) -> Result<SnapshotDto, ServiceError> {
        self.reload_store()?;
        f(&mut self
            .open
            .as_mut()
            .ok_or_else(ServiceError::no_project)?
            .vault)?;
        self.snapshot()
    }

    /// Set a task's lifecycle state.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn set_state(&mut self, id: &str, state: &str) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        let state = parse_state(state)?;
        self.mutate(|vault| {
            vault.set_state(&id, state)?;
            Ok(())
        })
    }

    /// Rename a task, cascading mirror aliases through core.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn set_title(&mut self, id: &str, title: &str) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        self.mutate(|vault| {
            vault.set_title(&id, title)?;
            Ok(())
        })
    }

    /// Replace a task's tags, normalized by core.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn set_tags(&mut self, id: &str, tags: Vec<String>) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        self.mutate(|vault| {
            vault.set_tags(&id, tags)?;
            Ok(())
        })
    }

    /// Set or clear a task's priority.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn set_priority(
        &mut self,
        id: &str,
        priority: Option<&str>,
    ) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        let priority = priority.map(parse_priority).transpose()?;
        self.mutate(|vault| {
            vault.set_priority(&id, priority)?;
            Ok(())
        })
    }

    /// Set or clear a task's due date.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn set_due(&mut self, id: &str, due: Option<&str>) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        let due = due.map(parse_due).transpose()?;
        self.mutate(|vault| {
            vault.set_due(&id, due)?;
            Ok(())
        })
    }

    /// Reparent a task inside its Project, or move it to the Project root.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn set_parent(
        &mut self,
        id: &str,
        parent_id: Option<&str>,
    ) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        let parent = parent_id.map(parse_id).transpose()?;
        self.mutate(|vault| {
            vault.set_parent(&id, parent.as_ref())?;
            Ok(())
        })
    }

    /// Move a task one place earlier (`delta < 0`) or later among siblings.
    ///
    /// # Errors
    ///
    /// Propagates core validation and I/O failures.
    pub fn shift_rank(&mut self, id: &str, delta: i32) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        self.mutate(|vault| {
            vault.shift_rank(&id, delta)?;
            Ok(())
        })
    }

    /// Move a task subtree into another registered Project and switch to it.
    /// A target equal to the current Project reparents in place.
    ///
    /// # Errors
    ///
    /// Returns `unknown_project` for a bad slug, or propagates core move
    /// failures (including `partial_commit`).
    pub fn move_to_project(
        &mut self,
        id: &str,
        target_slug: &str,
        parent_id: Option<&str>,
    ) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        self.reload_store()?;
        let parent = parent_id.map(parse_id).transpose()?;
        let current_slug = self
            .open
            .as_ref()
            .ok_or_else(ServiceError::no_project)?
            .project
            .slug
            .clone();
        if current_slug == target_slug {
            return self.set_parent(id.as_ref(), parent.as_ref().map(TaskId::as_ref));
        }

        let target_project = self
            .config
            .projects
            .iter()
            .find(|project| project.slug == target_slug)
            .cloned()
            .ok_or_else(|| ServiceError::unknown_project(target_slug))?;
        let data_dir = self.data_dir()?.to_path_buf();
        let mut target = registry::open_store(&data_dir, &target_project)?;
        {
            let open = self.open.as_mut().ok_or_else(ServiceError::no_project)?;
            open.vault.move_to(&mut target, &[id], parent.as_ref())?;
        }
        self.open = Some(OpenStore {
            project: target_project,
            vault: target,
        });
        self.snapshot()
    }

    /// How many descendants a delete of `id` would remove beyond the task
    /// itself, for the confirmation copy.
    ///
    /// # Errors
    ///
    /// Returns `no_project` when none is open.
    pub fn delete_count(&mut self, id: &str) -> Result<usize, ServiceError> {
        let id = parse_id(id)?;
        self.reload_store()?;
        let vault = &self
            .open
            .as_ref()
            .ok_or_else(ServiceError::no_project)?
            .vault;
        if vault.get(&id).is_none() {
            return Err(ServiceError::not_found(&id));
        }
        Ok(vault.descendant_count(std::slice::from_ref(&id)))
    }

    /// Delete a task and its whole subtree.
    ///
    /// # Errors
    ///
    /// Propagates core I/O failures.
    pub fn delete_task(&mut self, id: &str) -> Result<SnapshotDto, ServiceError> {
        let id = parse_id(id)?;
        self.mutate(|vault| {
            vault.delete(std::slice::from_ref(&id))?;
            Ok(())
        })
    }

    /// Search the open Project's titles in core display order, intersected
    /// with the core state/priority/tag/due filter.
    ///
    /// # Errors
    ///
    /// Returns `no_project` when none is open or `invalid_input` for a bad
    /// filter value.
    pub fn search(
        &self,
        query: &str,
        state: Option<&str>,
        tag: Option<&str>,
        priority: Option<&str>,
        due_today: bool,
    ) -> Result<Vec<String>, ServiceError> {
        let vault = &self
            .open
            .as_ref()
            .ok_or_else(ServiceError::no_project)?
            .vault;
        let filter = build_filter(state, tag, priority, due_today)?;
        let active = filter.tag.is_some()
            || filter.state.is_some()
            || filter.priority.is_some()
            || filter.due_today;
        let query = query.trim().to_lowercase();
        if query.is_empty() && !active {
            return Ok(Vec::new());
        }
        let matching: Option<BTreeSet<TaskId>> = active.then(|| {
            vault
                .filter(&filter, Local::now().date_naive())
                .into_iter()
                .map(|task| task.id.clone())
                .collect()
        });
        let mut ids: Vec<TaskId> = Vec::new();
        collect_search_matches(&vault.tree(), &query, matching.as_ref(), &mut ids);
        Ok(ids.iter().map(ToString::to_string).collect())
    }

    /// The exact metadata payload the terminal client copies, optionally with
    /// a blank line and the saved markdown body appended.
    ///
    /// # Errors
    ///
    /// Returns `no_project` when none is open or `not_found` for an unknown id.
    pub fn copy_payload(&mut self, id: &str, include_body: bool) -> Result<String, ServiceError> {
        let id = parse_id(id)?;
        self.reload_store()?;
        let open = self.open.as_ref().ok_or_else(ServiceError::no_project)?;
        let task = open
            .vault
            .get(&id)
            .ok_or_else(|| ServiceError::not_found(&id))?;
        let project_path = absolute(&open.project.path);
        let task_path = absolute(&open.vault.root().join(id.file_name()));
        let mut text = format!(
            "Project path: {}\nTask file: {}\nID: {}\nTitle: {}\nState: {}",
            project_path.display(),
            task_path.display(),
            task.id,
            task.title,
            task.state,
        );
        if include_body {
            text.push_str("\n\n");
            text.push_str(&task.body);
        }
        Ok(text)
    }
}

/// Build the details payload for `task` from the open store.
fn detail_dto(open: &OpenStore, task: &Task, revision: String) -> TaskDetailDto {
    let vault = &open.vault;
    let reference = |id: &TaskId| TaskRefDto {
        id: id.to_string(),
        title: vault.get(id).map(|target| target.title.clone()),
    };
    let (done, total) = if vault.children(&task.id).is_empty() {
        (0, 0)
    } else {
        vault.rollup(&task.id)
    };
    TaskDetailDto {
        id: task.id.to_string(),
        title: task.title.clone(),
        state: task.state.as_str().to_owned(),
        due: task.due.map(|due| due.format("%Y-%m-%d").to_string()),
        priority: task.priority.map(|priority| priority.as_str().to_owned()),
        tags: task.tags.clone(),
        parent_id: vault.parent(&task.id).map(ToString::to_string),
        project_slug: open.project.slug.clone(),
        project_path: open.project.path.to_string_lossy().into_owned(),
        file_path: vault
            .root()
            .join(task.id.file_name())
            .to_string_lossy()
            .into_owned(),
        revision,
        body: task.body.clone(),
        links: vault.links(&task.id).iter().map(reference).collect(),
        backlinks: vault.backlinks(&task.id).iter().map(reference).collect(),
        children: vault.children(&task.id).iter().map(reference).collect(),
        done,
        total,
    }
}

/// Convert a core tree node into the wire row shape.
fn node_dto(vault: &Vault, node: &TreeNode<'_>) -> TaskNodeDto {
    let task = node.task;
    let (done, total) = if node.children.is_empty() {
        (0, 0)
    } else {
        vault.rollup(&task.id)
    };
    TaskNodeDto {
        id: task.id.to_string(),
        title: task.title.clone(),
        state: task.state.as_str().to_owned(),
        due: task.due.map(|due| due.format("%Y-%m-%d").to_string()),
        priority: task.priority.map(|priority| priority.as_str().to_owned()),
        tags: task.tags.clone(),
        parent_id: vault.parent(&task.id).map(ToString::to_string),
        has_children: !node.children.is_empty(),
        done,
        total,
        children: node
            .children
            .iter()
            .map(|child| node_dto(vault, child))
            .collect(),
    }
}

/// Append ids of tree nodes whose title contains `query` (or every node when
/// the query is empty) and that pass `matching`, in pre-order.
fn collect_search_matches(
    nodes: &[TreeNode<'_>],
    query: &str,
    matching: Option<&BTreeSet<TaskId>>,
    out: &mut Vec<TaskId>,
) {
    for node in nodes {
        let id = &node.task.id;
        let title_ok = query.is_empty() || node.task.title.to_lowercase().contains(query);
        let filter_ok = matching.is_none_or(|set| set.contains(id));
        if title_ok && filter_ok {
            out.push(id.clone());
        }
        collect_search_matches(&node.children, query, matching, out);
    }
}

/// Core `TaskFilter` from optional wire values.
fn build_filter(
    state: Option<&str>,
    tag: Option<&str>,
    priority: Option<&str>,
    due_today: bool,
) -> Result<TaskFilter, ServiceError> {
    Ok(TaskFilter {
        tag: tag
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(ToOwned::to_owned),
        state: state.map(parse_state).transpose()?,
        priority: priority.map(parse_priority).transpose()?,
        due_today,
    })
}

fn parse_id(raw: &str) -> Result<TaskId, ServiceError> {
    TaskId::parse(raw.trim()).map_err(|error| ServiceError::invalid(error.to_string()))
}

fn parse_state(raw: &str) -> Result<TaskState, ServiceError> {
    match raw.trim() {
        "open" => Ok(TaskState::Open),
        "done" => Ok(TaskState::Done),
        "cancelled" => Ok(TaskState::Cancelled),
        other => Err(ServiceError::invalid(format!(
            "unknown state {other:?}; expected open, done, or cancelled"
        ))),
    }
}

fn parse_priority(raw: &str) -> Result<Priority, ServiceError> {
    match raw.trim() {
        "high" => Ok(Priority::High),
        "med" => Ok(Priority::Med),
        "low" => Ok(Priority::Low),
        other => Err(ServiceError::invalid(format!(
            "unknown priority {other:?}; expected high, med, or low"
        ))),
    }
}

fn parse_due(raw: &str) -> Result<NaiveDate, ServiceError> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .map_err(|_| ServiceError::invalid(format!("invalid date {raw:?}; expected YYYY-MM-DD")))
}

/// Canonicalize when possible, keeping the original path otherwise.
fn absolute(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Stable FNV-1a 64-bit hash, rendered as hex.
fn revision_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Changes whenever any loaded task document changes.
fn store_revision(vault: &Vault) -> String {
    let mut material = Vec::new();
    for task in vault.tasks() {
        material.extend_from_slice(task.id.as_ref().as_bytes());
        material.push(0);
        material.extend_from_slice(task.to_document().as_bytes());
        material.push(0xff);
    }
    revision_hex(&material)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        config: PathBuf,
        data: PathBuf,
        project: PathBuf,
        service: TtService,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let config = dir.path().join("config.toml");
            let data = dir.path().join("data");
            let project = dir.path().join("my-project");
            fs::create_dir_all(&project).expect("project dir");
            let mut service = TtService::with_paths(config.clone(), data.clone()).expect("service");
            service.register_project(&project).expect("register");
            Self {
                _dir: dir,
                config,
                data,
                project,
                service,
            }
        }

        fn slug(&self) -> String {
            self.service.config.projects[0].slug.clone()
        }

        fn store(&self) -> PathBuf {
            self.data.join(self.slug())
        }

        fn open(&mut self) -> SnapshotDto {
            self.service
                .open_project(&self.slug())
                .expect("open project")
        }
    }

    fn child_ids(node: &TaskNodeDto) -> Vec<String> {
        node.children.iter().map(|child| child.id.clone()).collect()
    }

    #[test]
    fn registered_project_opens_its_store_under_the_data_dir() {
        let mut fixture = Fixture::new();
        let snapshot = fixture.open();

        let project = snapshot.project.expect("open project");
        assert_eq!(project.path, absolute(&fixture.project).to_string_lossy());
        assert_eq!(project.name, "my-project");

        fixture.service.add_task("Hello", None, false).expect("add");
        let store = fixture.store();
        let files: Vec<String> = fs::read_dir(&store)
            .expect("store dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(files.len(), 1, "one task file in the store, got {files:?}");
        assert!(
            !fixture
                .project
                .read_dir()
                .expect("project dir")
                .any(|entry| entry
                    .expect("entry")
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "md")),
            "the project directory must not become the store"
        );
    }

    #[test]
    fn tree_uses_core_order_and_rollups() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("beta", None, false).expect("beta");
        let beta_id = task_id(&fixture, "beta");
        fixture
            .service
            .add_task("Alpha", None, false)
            .expect("alpha");
        let alpha_id = task_id(&fixture, "Alpha");

        let bogus = fixture
            .service
            .add_task("nope", Some("nonexistent1"), false);
        assert_eq!(bogus.expect_err("bogus parent").code, "not_found");

        fixture
            .service
            .add_task("kid", Some(&alpha_id), false)
            .expect("kid");
        let kid_id = task_id(&fixture, "kid");
        fixture.service.set_state(&kid_id, "done").expect("done");

        let snapshot = fixture.service.snapshot().expect("snapshot");
        let ids: Vec<String> = snapshot.tree.iter().map(|node| node.id.clone()).collect();
        assert_eq!(
            ids,
            vec![beta_id.clone(), alpha_id.clone()],
            "add materializes ranks in insertion order, and the desktop keeps core order"
        );

        let alpha_node = snapshot
            .tree
            .iter()
            .find(|node| node.id == alpha_id)
            .expect("alpha node");
        assert_eq!(child_ids(alpha_node), vec![kid_id]);
        assert_eq!((alpha_node.done, alpha_node.total), (1, 1));
        assert!(alpha_node.has_children);

        let beta_node = snapshot
            .tree
            .iter()
            .find(|node| node.id == beta_id)
            .expect("beta node");
        assert_eq!((beta_node.done, beta_node.total), (0, 0));
        assert!(!beta_node.has_children);
    }

    fn task_id(fixture: &Fixture, title: &str) -> String {
        fixture
            .service
            .open
            .as_ref()
            .expect("open")
            .vault
            .tasks()
            .find(|task| task.title == title)
            .expect("task")
            .id
            .to_string()
    }

    #[test]
    fn mutations_round_trip_through_the_store() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("Work", None, false).expect("add");
        let id = task_id(&fixture, "Work");

        fixture
            .service
            .set_title(&id, "Work renamed")
            .expect("rename");
        fixture.service.set_state(&id, "done").expect("done");
        fixture
            .service
            .set_tags(&id, vec!["work".into(), "urgent".into()])
            .expect("tags");
        fixture
            .service
            .set_priority(&id, Some("high"))
            .expect("priority");
        fixture
            .service
            .set_due(&id, Some("2026-10-31"))
            .expect("due");
        fixture
            .service
            .add_task("Sub", Some(&id), false)
            .expect("sub");
        let child_id = task_id(&fixture, "Sub");

        fixture.service.set_parent(&child_id, None).expect("root");
        fixture
            .service
            .set_priority(&child_id, None)
            .expect("clear priority");
        fixture.service.set_due(&child_id, None).expect("clear due");

        let detail = fixture.service.task_detail(&id).expect("detail");
        assert_eq!(detail.title, "Work renamed");
        assert_eq!(detail.state, "done");
        assert_eq!(detail.tags, vec!["work", "urgent"]);
        assert_eq!(detail.priority.as_deref(), Some("high"));
        assert_eq!(detail.due.as_deref(), Some("2026-10-31"));
        assert!(detail.children.is_empty(), "child was reparented to root");

        let child_detail = fixture.service.task_detail(&child_id).expect("child");
        assert_eq!(child_detail.parent_id, None);
        assert_eq!(child_detail.priority, None);
        assert_eq!(child_detail.due, None);
    }

    #[test]
    fn reorder_moves_a_task_among_its_siblings_and_reports_bounds() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("One", None, false).expect("one");
        fixture.service.add_task("Two", None, false).expect("two");
        let one = task_id(&fixture, "One");
        let two = task_id(&fixture, "Two");

        assert_eq!(
            fixture.service.shift_rank(&two, -1).expect("reorder").tree[0].id,
            two,
            "Two moves above One"
        );
        assert_eq!(
            fixture.service.shift_rank(&two, -1).expect("bound").tree[0].id,
            two,
            "already at the top: no reorder"
        );
        assert_eq!(
            fixture.service.shift_rank(&two, 1).expect("down").tree[1].id,
            two
        );
        assert_eq!(
            fixture
                .service
                .shift_rank(&one, 1)
                .expect("down again")
                .tree[0]
                .id,
            two,
            "One moves below Two"
        );
    }

    #[test]
    fn delete_reports_descendants_and_removes_the_subtree() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("Root", None, false).expect("root");
        let root = task_id(&fixture, "Root");
        fixture
            .service
            .add_task("Child", Some(&root), false)
            .expect("child");
        let child = task_id(&fixture, "Child");
        fixture
            .service
            .add_task("Grandchild", Some(&child), false)
            .expect("grandchild");
        let grandchild = task_id(&fixture, "Grandchild");

        assert_eq!(fixture.service.delete_count(&root).expect("count"), 2);
        let snapshot = fixture.service.delete_task(&root).expect("delete");
        assert_eq!(snapshot.task_count, 0);
        assert!(!fixture.store().join(format!("{grandchild}.md")).exists());
    }

    #[test]
    fn explicit_root_ignores_capture_target_and_capture_uses_it() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture
            .service
            .add_task("Anchor", None, false)
            .expect("anchor");
        let anchor = task_id(&fixture, "Anchor");
        fixture.service.config.capture_target = Some(TaskId::parse(&anchor).expect("id"));

        let captured = fixture
            .service
            .add_task("Captured", None, true)
            .expect("capture");
        assert_eq!(captured.task_count, 2);
        let captured_id = task_id(&fixture, "Captured");
        assert_eq!(
            fixture
                .service
                .task_detail(&captured_id)
                .expect("detail")
                .parent_id,
            Some(anchor.clone())
        );

        let root = fixture.service.add_task("Root", None, false).expect("root");
        assert_eq!(root.task_count, 3);
        let root_id = task_id(&fixture, "Root");
        assert_eq!(
            fixture
                .service
                .task_detail(&root_id)
                .expect("detail")
                .parent_id,
            None
        );
    }

    #[test]
    fn copy_payload_is_exact_and_body_inclusive_variant_appends() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture
            .service
            .add_task("Copy me", None, false)
            .expect("add");
        let id = task_id(&fixture, "Copy me");
        fixture
            .service
            .set_body_for_test(&id, "line one\nline two\n")
            .expect("body");

        let project = absolute(&fixture.project);
        let file = absolute(&fixture.store().join(format!("{id}.md")));
        let expected = format!(
            "Project path: {}\nTask file: {}\nID: {id}\nTitle: Copy me\nState: open",
            project.display(),
            file.display()
        );
        assert_eq!(
            fixture.service.copy_payload(&id, false).expect("copy"),
            expected
        );
        assert_eq!(
            fixture.service.copy_payload(&id, true).expect("copy body"),
            format!("{expected}\n\nline one\nline two\n")
        );
    }

    #[test]
    fn malformed_files_are_reported_and_left_untouched() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("Good", None, false).expect("add");

        let broken = fixture.store().join("broken0001.md");
        fs::write(&broken, "---\nid: broken0001\ntitle: [oops\n---\nbody\n").expect("write");
        let notes = fixture.store().join("notes.md");
        fs::write(&notes, "# just a note\n").expect("write");

        let snapshot = fixture.service.refresh().expect("refresh");
        let issues = &snapshot.issues;
        assert_eq!(
            issues.len(),
            1,
            "notes are tolerated, malformed tasks reported: {issues:?}"
        );
        assert_eq!(issues[0].kind, "malformed");
        assert!(issues[0].path.ends_with("broken0001.md"));
        assert_eq!(snapshot.task_count, 1, "the good task still loads");
        assert_eq!(
            fs::read_to_string(&broken).expect("read"),
            "---\nid: broken0001\ntitle: [oops\n---\nbody\n",
            "malformed files are never rewritten"
        );
        assert!(notes.exists(), "plain notes are tolerated");
    }

    #[test]
    fn stale_save_refuses_and_preserves_external_bytes_and_unknown_fields() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("Draft", None, false).expect("add");
        let id = task_id(&fixture, "Draft");
        let detail = fixture.service.task_detail(&id).expect("detail");
        let file = fixture.store().join(format!("{id}.md"));

        // An external editor changes the body and adds an unknown field.
        let external = format!(
            "{}extra: keep-me\n---\nexternally written\n",
            fs::read_to_string(&file)
                .expect("read")
                .split("---\n")
                .take(2)
                .collect::<Vec<_>>()
                .join("---\n")
        );
        fs::write(&file, &external).expect("external write");

        let error = fixture
            .service
            .save_body(&id, "my draft", &detail.revision)
            .expect_err("stale save must refuse");
        assert_eq!(error.code, "stale_draft");
        assert_eq!(error.conflict.as_ref().expect("conflict").reason, "changed");
        assert_eq!(
            fs::read_to_string(&file).expect("read"),
            external,
            "external bytes are untouched"
        );

        // Reload latest, edit again, and save successfully; the unknown field
        // round-trips because the base revision now matches disk.
        let latest = fixture.service.task_detail(&id).expect("reload detail");
        assert_eq!(latest.body, "externally written\n");
        let saved = fixture
            .service
            .save_body(&id, "integrated\n", &latest.revision)
            .expect("fresh save");
        assert_eq!(saved.body, "integrated\n");
        assert!(
            fs::read_to_string(&file)
                .expect("read")
                .contains("extra: keep-me"),
            "unknown frontmatter survives"
        );
    }

    #[test]
    fn stale_save_refuses_when_the_file_was_deleted() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("Gone", None, false).expect("add");
        let id = task_id(&fixture, "Gone");
        let detail = fixture.service.task_detail(&id).expect("detail");
        fs::remove_file(fixture.store().join(format!("{id}.md"))).expect("delete");

        let error = fixture
            .service
            .save_body(&id, "draft", &detail.revision)
            .expect_err("deleted file must refuse");
        assert_eq!(error.code, "stale_draft");
        assert_eq!(error.conflict.expect("conflict").reason, "deleted");
    }

    #[test]
    fn move_between_projects_carries_the_subtree_and_switches() {
        let mut fixture = Fixture::new();
        let other = fixture._dir.path().join("other-project");
        fs::create_dir_all(&other).expect("other dir");
        fixture.service.register_project(&other).expect("register");
        fixture.open();
        fixture.service.add_task("Root", None, false).expect("root");
        let root = task_id(&fixture, "Root");
        fixture
            .service
            .add_task("Child", Some(&root), false)
            .expect("child");
        let child = task_id(&fixture, "Child");

        let snapshot = fixture
            .service
            .move_to_project(&root, "other-project", None)
            .expect("move");
        assert_eq!(
            snapshot.project.as_ref().expect("project").slug,
            "other-project",
            "the UI follows the moved subtree"
        );
        assert_eq!(snapshot.task_count, 2);
        let moved = fixture.service.task_detail(&child).expect("child moved");
        assert_eq!(moved.project_slug, "other-project");
        assert!(
            fixture
                .data
                .join("other-project")
                .join(format!("{root}.md"))
                .exists(),
            "task file lands in the target store"
        );
        assert!(!fixture.store().join(format!("{root}.md")).exists());
    }

    #[test]
    fn search_matches_titles_in_core_order_and_respects_tag_rules() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture
            .service
            .add_task("Alpha plan", None, false)
            .expect("alpha");
        let alpha = task_id(&fixture, "Alpha plan");
        fixture
            .service
            .add_task("planning", None, false)
            .expect("planning");
        fixture
            .service
            .add_task("Unrelated", None, false)
            .expect("unrelated");
        fixture
            .service
            .set_tags(&alpha, vec!["work/admin".into()])
            .expect("tags");

        let hits = fixture
            .service
            .search("plan", None, None, None, false)
            .expect("search");
        assert_eq!(hits.len(), 2);
        assert!(
            hits.contains(&alpha),
            "case-insensitive title match in pre-order"
        );

        let tagged = fixture
            .service
            .search("", None, Some("work"), None, false)
            .expect("tag search");
        assert_eq!(
            tagged,
            vec![alpha],
            "descendant tag matches the parent query"
        );
        assert!(fixture
            .service
            .search("", None, Some("work/admin/x"), None, false)
            .expect("narrow tag")
            .is_empty());
        assert!(fixture
            .service
            .search("", None, None, None, false)
            .expect("empty")
            .is_empty());
    }

    #[test]
    fn unregister_keeps_tasks_and_closes_the_store() {
        let mut fixture = Fixture::new();
        fixture.open();
        fixture.service.add_task("Keep", None, false).expect("add");
        let store = fixture.store();
        let before: Vec<_> = fs::read_dir(&store).expect("store").collect();

        let snapshot = fixture
            .service
            .unregister_project("my-project")
            .expect("unregister");
        assert!(snapshot.project.is_none());
        assert!(snapshot.projects.is_empty());
        let after: Vec<_> = fs::read_dir(&store).expect("store").collect();
        assert_eq!(before.len(), after.len(), "tasks stay on disk");
        assert!(fixture.config.exists(), "registry was saved");
        let saved = fs::read_to_string(&fixture.config).expect("read config");
        assert!(
            !saved.contains("[[project]]"),
            "entry removed from config: {saved}"
        );
    }

    #[test]
    fn opening_an_unknown_project_errors() {
        let mut fixture = Fixture::new();
        let error = fixture.service.open_project("nope").expect_err("unknown");
        assert_eq!(error.code, "unknown_project");
    }

    // Test-only helper: route a body change through the public save path with
    // a fresh baseline.
    impl TtService {
        fn set_body_for_test(&mut self, id: &str, body: &str) -> Result<(), ServiceError> {
            let detail = self.task_detail(id)?;
            self.save_body(id, body, &detail.revision)?;
            Ok(())
        }
    }
}
