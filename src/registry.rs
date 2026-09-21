//! Central store registry: project slugs and their store folders.
//!
//! A project is a directory the user works in; its tasks live in an invisible
//! central store under the data directory (`$XDG_DATA_HOME/tt/<slug>`, else
//! `~/.local/share/tt/<slug>`). The config file maps project paths to slugs;
//! users never browse the store, and stores stay normalized and id-named.
//!
//! Registration is idempotent: registering a path that is already a project
//! returns the existing entry. Slugs are the directory basename, with
//! `-2`, `-3`, ... appended when another registered project or an existing
//! store folder already claims the name.

use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::{non_empty_env, Config, ConfigError, Project};
use crate::vault::{Vault, VaultError};

/// Environment variable naming the XDG data directory.
pub const DATA_ENV: &str = "XDG_DATA_HOME";

/// Errors from registering or moving projects.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// No data directory can be determined.
    #[error("cannot determine the data directory: set $XDG_DATA_HOME or $HOME")]
    NoDataDir,
    /// The project's store folder could not be created.
    #[error("cannot create the store folder {path}: {source}")]
    Io {
        /// Store folder that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// Errors from registering a project and persisting the registry together.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The project could not be registered.
    #[error("registering the project: {0}")]
    Register(#[from] RegistryError),
    /// The config could not be saved after the project was registered.
    #[error("saving the config: {0}")]
    Save(#[from] ConfigError),
}

/// The `tt` data directory: `$XDG_DATA_HOME/tt` else
/// `$HOME/.local/share/tt`.
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(
        non_empty_env(DATA_ENV).as_deref(),
        non_empty_env("HOME").as_deref(),
    )
}

/// [`data_dir`] against explicit environment values. Empty values are the
/// caller's concern; [`data_dir`] passes [`None`] for them.
pub(crate) fn data_dir_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(xdg) = xdg {
        return Some(PathBuf::from(xdg).join("tt"));
    }
    home.map(|home| PathBuf::from(home).join(".local").join("share").join("tt"))
}

/// Folder where a project's tasks live.
pub fn store_path(data_dir: &Path, slug: &str) -> PathBuf {
    data_dir.join(slug)
}

/// Open the invisible store folder for `project` under `data_dir`.
///
/// # Errors
///
/// Returns [`VaultError::Io`] when the store folder cannot be created or read.
pub fn open_store(data_dir: &Path, project: &Project) -> Result<Vault, VaultError> {
    Vault::open(store_path(data_dir, &project.slug))
}

/// Absolute, canonicalized form of `path` when possible.
///
/// Canonicalization resolves `..` and symlinks; a path that does not exist yet
/// is made absolute against the current directory instead.
pub fn normalize(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

/// Register `path`, creating its store folder, and return the entry.
///
/// # Errors
///
/// Returns [`RegistryError::NoDataDir`] when neither `$XDG_DATA_HOME` nor
/// `$HOME` is set, or [`RegistryError::Io`] when the store folder cannot be
/// created.
pub fn register(config: &mut Config, path: &Path) -> Result<Project, RegistryError> {
    let data_dir = data_dir().ok_or(RegistryError::NoDataDir)?;
    register_in(config, path, &data_dir)
}

/// [`register`] against an explicit data directory.
///
/// # Errors
///
/// Returns [`RegistryError::Io`] when the store folder cannot be created.
pub fn register_in(
    config: &mut Config,
    path: &Path,
    data_dir: &Path,
) -> Result<Project, RegistryError> {
    let path = normalize(path);
    if let Some(existing) = config.projects.iter().find(|project| project.path == path) {
        return Ok(existing.clone());
    }

    let taken: BTreeSet<&str> = config
        .projects
        .iter()
        .map(|project| project.slug.as_str())
        .collect();
    let slug = slug_for(&path, &taken, data_dir);
    let store = store_path(data_dir, &slug);
    fs::create_dir_all(&store).map_err(|source| RegistryError::Io {
        path: store,
        source,
    })?;

    let project = Project {
        path,
        slug,
        never_ask_nested: false,
    };
    config.projects.push(project.clone());
    Ok(project)
}

/// [`register`] plus persisting the updated config.
///
/// # Errors
///
/// Returns [`SessionError::Register`] when there is no data directory or the
/// store folder cannot be created, and [`SessionError::Save`] when the config
/// cannot be written.
pub fn register_and_save(config: &mut Config, path: &Path) -> Result<Project, SessionError> {
    let data_dir = data_dir().ok_or(RegistryError::NoDataDir)?;
    register_and_save_in(config, path, &data_dir)
}

/// [`register_and_save`] against an explicit data directory.
///
/// # Errors
///
/// Returns [`SessionError::Register`] when the store folder cannot be created,
/// and [`SessionError::Save`] when the config cannot be written.
pub fn register_and_save_in(
    config: &mut Config,
    path: &Path,
    data_dir: &Path,
) -> Result<Project, SessionError> {
    let project = register_in(config, path, data_dir)?;
    config.save()?;
    Ok(project)
}

/// Persist the "never ask again for nested dirs" rule for `project`.
///
/// A project that is not registered in `config` is a no-op.
///
/// # Errors
///
/// Returns [`ConfigError`] when the config cannot be written.
pub fn set_never_ask(config: &mut Config, project: &Project) -> Result<(), ConfigError> {
    if let Some(entry) = config
        .projects
        .iter_mut()
        .find(|entry| entry.path == project.path)
    {
        entry.never_ask_nested = true;
        config.save()?;
    }
    Ok(())
}

/// Unregister the project at `path`, returning the removed entry.
pub fn remove(config: &mut Config, path: &Path) -> Option<Project> {
    let path = normalize(path);
    let index = config
        .projects
        .iter()
        .position(|project| project.path == path)?;
    Some(config.projects.remove(index))
}

/// Unregister the project with `slug`, returning the removed entry.
pub fn remove_slug(config: &mut Config, slug: &str) -> Option<Project> {
    let index = config
        .projects
        .iter()
        .position(|project| project.slug == slug)?;
    Some(config.projects.remove(index))
}

/// Slug candidates: the directory name, then `-2`, `-3`, ... until neither a
/// registered slug nor an existing store folder claims it.
pub fn slug_for(path: &Path, taken: &BTreeSet<&str>, data_dir: &Path) -> String {
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_owned();
    let mut candidate = base.clone();
    let mut suffix = 1u32;
    while taken.contains(candidate.as_str()) || store_path(data_dir, &candidate).exists() {
        suffix += 1;
        candidate = format!("{base}-{suffix}");
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_dir(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("create project dir");
        path
    }

    #[test]
    fn slug_collisions_get_numeric_suffixes() {
        let root = tempfile::tempdir().expect("temp dir");
        let data = root.path().join("data");
        let first_path = project_dir(root.path(), "a/pkg");
        let second_path = project_dir(root.path(), "b/pkg");

        let mut config = Config::default();
        let first = register_in(&mut config, &first_path, &data).expect("register");
        let second = register_in(&mut config, &second_path, &data).expect("register");

        assert_eq!(first.slug, "pkg");
        assert_eq!(second.slug, "pkg-2");
        assert!(data.join("pkg").is_dir(), "store dir created");
        assert!(data.join("pkg-2").is_dir(), "second store dir created");
        assert!(first.path.is_absolute(), "absolute path stored");
        assert_eq!(config.projects.len(), 2);
    }

    #[test]
    fn orphan_store_folder_collides_too() {
        let root = tempfile::tempdir().expect("temp dir");
        let data = root.path().join("data");
        fs::create_dir_all(data.join("pkg")).expect("pre-create orphan store");
        let target = project_dir(root.path(), "pkg");

        let mut config = Config::default();
        let project = register_in(&mut config, &target, &data).expect("register");
        assert_eq!(project.slug, "pkg-2");
        assert!(data.join("pkg-2").is_dir());
    }

    #[test]
    fn registering_twice_is_idempotent() {
        let root = tempfile::tempdir().expect("temp dir");
        let data = root.path().join("data");
        let target = project_dir(root.path(), "pkg");

        let mut config = Config::default();
        let first = register_in(&mut config, &target, &data).expect("register");
        let again = register_in(&mut config, &target, &data).expect("register again");

        assert_eq!(first, again);
        assert_eq!(config.projects.len(), 1);
    }

    #[test]
    fn remove_unregisters_by_path_and_slug() {
        let root = tempfile::tempdir().expect("temp dir");
        let data = root.path().join("data");
        let first_path = project_dir(root.path(), "one");
        let second_path = project_dir(root.path(), "two");

        let mut config = Config::default();
        register_in(&mut config, &first_path, &data).expect("register");
        register_in(&mut config, &second_path, &data).expect("register");

        let removed = remove(&mut config, &first_path).expect("remove by path");
        assert_eq!(removed.slug, "one");
        assert!(remove(&mut config, &first_path).is_none(), "already gone");

        let removed = remove_slug(&mut config, "two").expect("remove by slug");
        assert_eq!(removed.path, normalize(&second_path));
        assert!(config.projects.is_empty());
    }

    #[test]
    fn never_ask_nested_persists_through_save_and_load() {
        let root = tempfile::tempdir().expect("temp dir");
        let data = root.path().join("data");
        let target = project_dir(root.path(), "pkg");
        let config_path = root.path().join("tt").join("config.toml");

        let mut config = Config::load_from(Some(config_path.clone())).expect("load");
        let project = register_in(&mut config, &target, &data).expect("register");
        config.projects[0].never_ask_nested = true;
        config.save().expect("save");

        let reloaded = Config::load_from(Some(config_path)).expect("reload");
        assert_eq!(reloaded.projects.len(), 1);
        assert!(reloaded.projects[0].never_ask_nested);
        assert_eq!(reloaded.projects[0].slug, project.slug);
        assert_eq!(reloaded.projects[0].path, project.path);
    }

    #[test]
    fn facade_registers_saves_opens_and_never_asks() {
        let root = tempfile::tempdir().expect("temp dir");
        let data = root.path().join("data");
        let target = project_dir(root.path(), "pkg");
        let config_path = root.path().join("tt").join("config.toml");

        let mut config = Config::load_from(Some(config_path.clone())).expect("load");
        let project = register_and_save_in(&mut config, &target, &data).expect("register");
        let vault = open_store(&data, &project).expect("open store");
        assert_eq!(vault.root(), data.join("pkg").as_path());

        set_never_ask(&mut config, &project).expect("set never ask");
        assert!(config.projects[0].never_ask_nested);

        let reloaded = Config::load_from(Some(config_path)).expect("reload");
        assert_eq!(reloaded.projects.len(), 1);
        assert!(reloaded.projects[0].never_ask_nested, "saved by the facade");
    }

    #[test]
    fn set_never_ask_ignores_unregistered_projects() {
        let root = tempfile::tempdir().expect("temp dir");
        let mut config = Config::load_from(None).expect("load");
        let stranger = Project {
            path: root.path().join("stranger"),
            slug: "stranger".to_owned(),
            never_ask_nested: false,
        };

        set_never_ask(&mut config, &stranger).expect("no-op");
        assert!(config.projects.is_empty());
    }

    #[test]
    fn data_dir_prefers_xdg_then_home() {
        let root = tempfile::tempdir().expect("temp dir");
        let xdg = root.path().join("xdg");
        let home = root.path().join("home");

        assert_eq!(
            data_dir_from(Some(xdg.as_os_str()), Some(home.as_os_str())),
            Some(xdg.join("tt"))
        );
        assert_eq!(
            data_dir_from(None, Some(home.as_os_str())),
            Some(home.join(".local").join("share").join("tt"))
        );
        assert_eq!(data_dir_from(None, None), None);
    }
}
