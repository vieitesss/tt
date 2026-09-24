//! Configuration file loading and atomic saving.
//!
//! The config file is TOML. Its location is `$TT_CONFIG` when set, otherwise
//! `config.toml` in the tt data directory. A missing file is fine; an unreadable
//! or invalid file is an error.
//!
//! ```toml
//! capture_target = "abc1234567" # optional default parent for quick capture
//!
//! [path_display]
//! style = "short"               # "home" | "short" | "tail"
//! tail = 1                      # segments kept when style = "tail"
//!
//! [[project]]
//! path = "~/work/app"
//! slug = "app"
//! never_ask_nested = false
//! ```
//!
//! Every key the app does not know is preserved verbatim when the file is
//! rewritten, including the legacy `vault = "..."` key. Rewrites are atomic:
//! content goes to a temporary file in the config folder and is renamed over
//! the target.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{IdError, TaskId};

/// Environment variable naming the config file.
pub const CONFIG_ENV: &str = "TT_CONFIG";

/// Environment variable naming the legacy vault folder.
pub const VAULT_ENV: &str = "TT_VAULT";

/// A directory registered as a `tt` project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Absolute, canonicalized project path.
    pub path: PathBuf,
    /// Store folder name under the data directory.
    pub slug: String,
    /// Whether nested directories of this project stop prompting.
    pub never_ask_nested: bool,
}

/// How project paths are shortened for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PathDisplayStyle {
    /// `$HOME` becomes `~`; everything else is kept.
    Home,
    /// Fish-style one-letter directories, with `~` for home.
    #[default]
    Short,
    /// Keep the last `tail` path segments, with `~` for home.
    Tail,
}

/// `[path_display]` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathDisplay {
    /// Shortening strategy.
    pub style: PathDisplayStyle,
    /// Segments kept when `style` is [`PathDisplayStyle::Tail`].
    pub tail: usize,
}

impl Default for PathDisplay {
    fn default() -> Self {
        Self {
            style: PathDisplayStyle::Short,
            tail: 1,
        }
    }
}

/// Loaded `tt` configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The legacy `vault=` value, if the file still has one. Kept only so the
    /// CLI can warn and so rewrites preserve the key; it never selects the
    /// vault folder in v2.
    pub deprecated_vault: Option<PathBuf>,
    /// Default parent for quick capture, if any.
    pub capture_target: Option<TaskId>,
    /// Project path shortening for the list header.
    pub path_display: PathDisplay,
    /// Registered projects, in file order.
    pub projects: Vec<Project>,
    /// Where this config was loaded from; `None` disables saving.
    path: Option<PathBuf>,
    /// Every other top-level key, preserved verbatim on save.
    extra: toml::Table,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            deprecated_vault: None,
            capture_target: None,
            path_display: PathDisplay::default(),
            projects: Vec::new(),
            path: None,
            extra: toml::Table::new(),
        }
    }
}

/// Serde view of the config file.
///
/// Most keys are handled by the derive; `capture_target`, `[path_display]`,
/// and `project`/`[[project]]` stay raw [`toml::Value`]s so
/// [`Config::load_from`] can report the precise per-key errors
/// ([`ConfigError::Invalid`] and [`ConfigError::CaptureTarget`]) that the CLI
/// is expected to show. Every unknown top-level key lands in `extra` and is
/// written back verbatim.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct ConfigFile {
    /// Raw `capture_target`, validated by [`parse_capture_target`].
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_target: Option<toml::Value>,
    /// Raw `[path_display]`, validated by [`parse_path_display`].
    #[serde(skip_serializing_if = "Option::is_none")]
    path_display: Option<toml::Value>,
    /// Raw `project` or `[[project]]`, validated by [`parse_projects`].
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<toml::Value>,
    /// The legacy `vault` key, kept only to warn and to round-trip.
    #[serde(rename = "vault", skip_serializing_if = "Option::is_none")]
    deprecated_vault: Option<PathBuf>,
    /// Every other top-level key, preserved verbatim.
    #[serde(flatten)]
    extra: toml::Table,
}

impl From<&Config> for ConfigFile {
    fn from(config: &Config) -> Self {
        Self {
            capture_target: config
                .capture_target
                .as_ref()
                .map(|id| toml::Value::String(id.to_string())),
            path_display: (config.path_display != PathDisplay::default())
                .then(|| path_display_value(&config.path_display)),
            project: (!config.projects.is_empty())
                .then(|| toml::Value::Array(config.projects.iter().map(project_value).collect())),
            deprecated_vault: config.deprecated_vault.clone(),
            extra: config.extra.clone(),
        }
    }
}

/// Errors from loading or saving configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file exists but could not be read or written.
    #[error("config filesystem error at {path}: {source}")]
    Io {
        /// Config file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// The config file is not valid TOML.
    #[error("invalid config {path}: {source}")]
    Toml {
        /// Config file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::de::Error,
    },
    /// A known key has the wrong type or shape.
    #[error("invalid {key} in {path}: {detail}")]
    Invalid {
        /// Config file path.
        path: PathBuf,
        /// Dotted key name.
        key: &'static str,
        /// What is wrong with the value.
        detail: String,
    },
    /// The config table could not be serialized.
    #[error("cannot serialize config {path}: {source}")]
    Serialize {
        /// Config file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: toml::ser::Error,
    },
    /// Saving was requested for a config with no file location.
    #[error("config file location is unknown; cannot save")]
    NoConfigPath,
    /// `capture_target` is not a valid task id.
    #[error("invalid capture_target in {path}: {source}")]
    CaptureTarget {
        /// Config file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: IdError,
    },
}

impl Config {
    /// Load configuration from `$TT_CONFIG` or the default path.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a config file exists but cannot be read,
    /// is not valid TOML, or has an invalid known key.
    pub fn load() -> Result<Self, ConfigError> {
        let override_path = non_empty_env(CONFIG_ENV).map(PathBuf::from);
        let path = crate::registry::data_dir().map(|dir| dir.join("config.toml"));
        Self::load_selected(override_path, path, legacy_config_path())
    }

    fn load_selected(
        override_path: Option<PathBuf>,
        default_path: Option<PathBuf>,
        legacy: Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        if let Some(path) = override_path {
            return Self::load_from(Some(path));
        }
        let Some(path) = default_path else {
            return Self::load_from(None);
        };
        Self::load_or_migrate(path, legacy)
    }

    fn load_or_migrate(path: PathBuf, legacy: Option<PathBuf>) -> Result<Self, ConfigError> {
        if path.exists() {
            return Self::load_from(Some(path));
        }
        let Some(legacy) = legacy.filter(|legacy| legacy.is_file()) else {
            return Self::load_from(Some(path));
        };
        let config = Self::load_from(Some(legacy))?;
        Self::finish_migration(config, path)
    }

    fn finish_migration(mut config: Self, path: PathBuf) -> Result<Self, ConfigError> {
        config.path = Some(path.clone());
        if config.save_new()? {
            crate::registry::warn_nonportable_projects(&config.projects);
            Ok(config)
        } else {
            // Another process created the shared config after our initial
            // check. Its config is authoritative; never replace it with legacy.
            Self::load_from(Some(path))
        }
    }

    /// Load from an explicit location, or with no location at all.
    ///
    /// Splitting this out keeps unit tests free of environment mutation; the
    /// returned config still remembers `path` so [`Config::save`] can write
    /// the file (creating it on first save).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the file exists but cannot be read or
    /// parsed.
    pub fn load_from(path: Option<PathBuf>) -> Result<Self, ConfigError> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let mut config = Self {
            path: Some(path.clone()),
            ..Self::default()
        };

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(config),
            Err(source) => return Err(ConfigError::Io { path, source }),
        };

        let file: ConfigFile = toml::from_str(&text).map_err(|source| ConfigError::Toml {
            path: path.clone(),
            source,
        })?;

        config.capture_target = parse_capture_target(file.capture_target, &path)?;
        config.deprecated_vault = file.deprecated_vault;
        config.path_display = parse_path_display(file.path_display, &path)?;
        config.projects = parse_projects(file.project, &path)?;
        config.extra = file.extra;
        Ok(config)
    }

    /// The path this config was loaded from (or will be saved to).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// A config with no file location and the given registered projects.
    ///
    /// Useful for constructing a registry in-process (tests, embedding); the
    /// returned config cannot be saved until it has a path.
    pub fn with_projects(projects: Vec<Project>) -> Self {
        Self {
            projects,
            ..Self::default()
        }
    }

    /// Write the config back, preserving unknown keys and atomically replacing
    /// the file (creating parent directories on first save).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::NoConfigPath`] when the config has no location,
    /// or [`ConfigError::Io`] / [`ConfigError::Serialize`] on failure.
    pub fn save(&self) -> Result<(), ConfigError> {
        let (path, text) = self.serialized()?;
        crate::fsutil::write_atomic(&path, &text, true)
            .map_err(|source| ConfigError::Io { path, source })
    }

    fn save_new(&self) -> Result<bool, ConfigError> {
        let (path, text) = self.serialized()?;
        crate::fsutil::write_atomic_new(&path, &text, true)
            .map_err(|source| ConfigError::Io { path, source })
    }

    fn serialized(&self) -> Result<(PathBuf, String), ConfigError> {
        let Some(path) = &self.path else {
            return Err(ConfigError::NoConfigPath);
        };
        let table = toml::Table::try_from(ConfigFile::from(self)).map_err(|source| {
            ConfigError::Serialize {
                path: path.clone(),
                source,
            }
        })?;
        let text = toml::to_string(&table).map_err(|source| ConfigError::Serialize {
            path: path.clone(),
            source,
        })?;
        Ok((path.clone(), text))
    }
}

fn legacy_config_path() -> Option<PathBuf> {
    if let Some(xdg) = non_empty_env("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("tt").join("config.toml"));
    }
    non_empty_env("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("tt")
            .join("config.toml")
    })
}

fn encode_project_path(path: &Path) -> String {
    encode_project_path_with_home(path, non_empty_env("HOME").map(PathBuf::from).as_deref())
}

fn encode_project_path_with_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(relative) = home.and_then(|home| path_relative_to_home(path, home)) {
        return if relative.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", relative.to_string_lossy())
        };
    }
    path.to_string_lossy().into_owned()
}

fn expand_project_path(path: &str) -> PathBuf {
    expand_project_path_with_home(path, non_empty_env("HOME").map(PathBuf::from).as_deref())
}

pub(crate) fn canonicalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn path_is_under_home(path: &Path, home: &Path) -> bool {
    path_relative_to_home(path, home).is_some()
}

fn path_relative_to_home(path: &Path, home: &Path) -> Option<PathBuf> {
    let canonical_home = canonicalize_path(home);
    if let Ok(canonical_path) = path.canonicalize() {
        return canonical_path
            .strip_prefix(canonical_home)
            .ok()
            .map(Path::to_path_buf);
    }
    path.strip_prefix(home)
        .or_else(|_| path.strip_prefix(canonical_home))
        .ok()
        .map(Path::to_path_buf)
}

fn expand_project_path_with_home(path: &str, home: Option<&Path>) -> PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = home {
            return if path == "~" {
                home.to_path_buf()
            } else {
                home.join(&path[2..])
            };
        }
    }
    PathBuf::from(path)
}

/// The legacy `vault` folder from `$TT_VAULT`, if set (empty counts as
/// unset). Kept for the hidden `--vault` escape hatch; the v2 registry does
/// not consult it.
pub fn env_vault() -> Option<PathBuf> {
    non_empty_env(VAULT_ENV).map(PathBuf::from)
}

/// Environment values that are set but empty count as unset.
pub(crate) fn non_empty_env(name: &str) -> Option<std::ffi::OsString> {
    let value = env::var_os(name)?;
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn invalid(path: &Path, key: &'static str, detail: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        path: path.to_owned(),
        key,
        detail: detail.into(),
    }
}

fn string_value(value: toml::Value, key: &'static str, path: &Path) -> Result<String, ConfigError> {
    match value {
        toml::Value::String(text) => Ok(text),
        _ => Err(invalid(path, key, "expected a string")),
    }
}

fn parse_capture_target(
    value: Option<toml::Value>,
    path: &Path,
) -> Result<Option<TaskId>, ConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let raw = string_value(value, "capture_target", path)?;
    TaskId::parse(raw.trim())
        .map(Some)
        .map_err(|source| ConfigError::CaptureTarget {
            path: path.to_owned(),
            source,
        })
}

fn parse_path_display(value: Option<toml::Value>, path: &Path) -> Result<PathDisplay, ConfigError> {
    let Some(value) = value else {
        return Ok(PathDisplay::default());
    };
    let toml::Value::Table(table) = value else {
        return Err(invalid(path, "path_display", "expected a table"));
    };
    let mut display = PathDisplay::default();
    for (key, value) in &table {
        match key.as_str() {
            "style" => {
                let toml::Value::String(style) = value else {
                    return Err(invalid(path, "path_display.style", "expected a string"));
                };
                display.style = match style.as_str() {
                    "home" => PathDisplayStyle::Home,
                    "short" => PathDisplayStyle::Short,
                    "tail" => PathDisplayStyle::Tail,
                    other => {
                        return Err(invalid(
                            path,
                            "path_display.style",
                            format!("unknown style {other:?}; expected home, short, or tail"),
                        ));
                    }
                };
            }
            "tail" => {
                let toml::Value::Integer(tail) = value else {
                    return Err(invalid(
                        path,
                        "path_display.tail",
                        "expected a non-negative integer",
                    ));
                };
                display.tail = usize::try_from(*tail).map_err(|_| {
                    invalid(path, "path_display.tail", "expected a non-negative integer")
                })?;
            }
            other => {
                return Err(invalid(
                    path,
                    "path_display",
                    format!("unknown key {other:?}"),
                ));
            }
        }
    }
    if display.style == PathDisplayStyle::Tail && display.tail == 0 {
        return Err(invalid(
            path,
            "path_display.tail",
            "tail must be at least 1",
        ));
    }
    Ok(display)
}

fn parse_projects(value: Option<toml::Value>, path: &Path) -> Result<Vec<Project>, ConfigError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if let toml::Value::Array(entries) = &value {
        return entries
            .iter()
            .map(|entry| parse_project(entry, path))
            .collect();
    }
    Ok(vec![parse_project(&value, path)?])
}

fn parse_project(value: &toml::Value, path: &Path) -> Result<Project, ConfigError> {
    let toml::Value::Table(table) = value else {
        return Err(invalid(path, "project", "expected a table"));
    };
    let project_path = match table.get("path") {
        Some(toml::Value::String(text)) => expand_project_path(text),
        Some(_) => return Err(invalid(path, "project.path", "expected a string")),
        None => return Err(invalid(path, "project.path", "missing")),
    };
    let slug = match table.get("slug") {
        Some(toml::Value::String(text)) if !text.is_empty() => text.clone(),
        Some(toml::Value::String(_)) => {
            return Err(invalid(path, "project.slug", "must not be empty"));
        }
        Some(_) => return Err(invalid(path, "project.slug", "expected a string")),
        None => return Err(invalid(path, "project.slug", "missing")),
    };
    let never_ask_nested = match table.get("never_ask_nested") {
        Some(toml::Value::Boolean(value)) => *value,
        Some(_) => {
            return Err(invalid(
                path,
                "project.never_ask_nested",
                "expected a boolean",
            ));
        }
        None => false,
    };
    Ok(Project {
        path: project_path,
        slug,
        never_ask_nested,
    })
}

fn project_value(project: &Project) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "path".to_owned(),
        toml::Value::String(encode_project_path(&project.path)),
    );
    table.insert("slug".to_owned(), toml::Value::String(project.slug.clone()));
    table.insert(
        "never_ask_nested".to_owned(),
        toml::Value::Boolean(project.never_ask_nested),
    );
    toml::Value::Table(table)
}

fn path_display_value(display: &PathDisplay) -> toml::Value {
    let mut table = toml::Table::new();
    let style = match display.style {
        PathDisplayStyle::Home => "home",
        PathDisplayStyle::Short => "short",
        PathDisplayStyle::Tail => "tail",
    };
    table.insert("style".to_owned(), toml::Value::String(style.to_owned()));
    table.insert("tail".to_owned(), toml::Value::Integer(display.tail as i64));
    toml::Value::Table(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("tt").join("config.toml")
    }

    #[test]
    fn home_project_paths_encode_and_expand_per_host() {
        let old_home = Path::new("/old/home");
        let new_home = Path::new("/new/home");
        let encoded =
            encode_project_path_with_home(Path::new("/old/home/work/app"), Some(old_home));
        assert_eq!(encoded, "~/work/app");
        assert_eq!(
            expand_project_path_with_home(&encoded, Some(new_home)),
            PathBuf::from("/new/home/work/app")
        );
        assert_eq!(
            encode_project_path_with_home(Path::new("/outside/project"), Some(old_home)),
            "/outside/project"
        );
        assert_eq!(
            expand_project_path_with_home("/outside/project", Some(new_home)),
            PathBuf::from("/outside/project")
        );
    }

    #[test]
    fn missing_config_loads_defaults_and_remembers_its_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        let config = Config::load_from(Some(path.clone())).expect("load");

        assert_eq!(config.deprecated_vault, None);
        assert_eq!(config.capture_target, None);
        assert_eq!(config.path_display, PathDisplay::default());
        assert!(config.projects.is_empty());
        assert_eq!(config.path(), Some(path.as_path()));
        assert_eq!(Config::load_from(None).expect("no path").path(), None);
    }

    #[test]
    fn explicit_config_override_skips_legacy_migration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let override_path = dir.path().join("custom.toml");
        let default_path = dir.path().join("shared").join("config.toml");
        let legacy = dir.path().join("legacy").join("config.toml");
        fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("mkdir");
        fs::write(&legacy, r#"capture_target = "abc1234567""#).expect("legacy config");

        let loaded = Config::load_selected(
            Some(override_path.clone()),
            Some(default_path.clone()),
            Some(legacy),
        )
        .expect("load override");

        assert_eq!(loaded.path(), Some(override_path.as_path()));
        assert!(!default_path.exists());
        assert!(loaded.capture_target.is_none());
    }

    #[test]
    fn first_shared_load_migrates_legacy_preferences_and_never_overwrites_shared() {
        let dir = tempfile::tempdir().expect("temp dir");
        let legacy = dir.path().join("old").join("config.toml");
        let shared = dir.path().join("data").join("tt").join("config.toml");
        fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("mkdir");
        fs::write(
            &legacy,
            r#"capture_target = "abc1234567"
[[project]]
path = "~/work/app"
slug = "stable-slug"
never_ask_nested = true
"#,
        )
        .expect("write legacy config");

        let migrated =
            Config::load_or_migrate(shared.clone(), Some(legacy.clone())).expect("migrate");
        assert_eq!(migrated.projects[0].slug, "stable-slug");
        assert!(migrated.projects[0].never_ask_nested);
        assert_eq!(
            migrated.capture_target,
            Some(TaskId::parse("abc1234567").expect("id"))
        );
        let migrated_text = fs::read_to_string(&shared).expect("shared config");
        assert!(migrated_text.contains("~/work/app"));

        fs::write(&shared, r#"capture_target = "def1234567""#).expect("replace shared");
        let existing = Config::load_or_migrate(shared.clone(), Some(legacy)).expect("load shared");
        assert_eq!(
            existing.capture_target,
            Some(TaskId::parse("def1234567").expect("id"))
        );
    }

    #[test]
    fn migration_collision_loads_the_config_created_by_the_racing_writer() {
        let dir = tempfile::tempdir().expect("temp dir");
        let legacy = dir.path().join("old.toml");
        let shared = dir.path().join("data").join("config.toml");
        fs::write(&legacy, r#"capture_target = "abc1234567""#).expect("legacy config");
        let migrated = Config::load_from(Some(legacy)).expect("load legacy");

        // Simulate the winner creating the shared config after the initial
        // missing-file check, but before migration's atomic no-replace create.
        fs::create_dir_all(shared.parent().expect("shared parent")).expect("mkdir");
        fs::write(&shared, r#"capture_target = "def1234567""#).expect("winner config");

        let loaded = Config::finish_migration(migrated, shared.clone()).expect("load winner");
        assert_eq!(
            loaded.capture_target,
            Some(TaskId::parse("def1234567").expect("id"))
        );
        assert_eq!(loaded.path(), Some(shared.as_path()));
        assert_eq!(
            fs::read_to_string(shared).expect("read winner"),
            r#"capture_target = "def1234567""#
        );
    }

    #[cfg(unix)]
    #[test]
    fn project_path_under_symlinked_home_serializes_as_relative() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("temp dir");
        let actual_home = dir.path().join("real-home");
        let home_alias = dir.path().join("home-alias");
        let project_path = actual_home.join("work").join("app");
        fs::create_dir_all(&project_path).expect("project dir");
        symlink(&actual_home, &home_alias).expect("home symlink");

        assert_eq!(
            encode_project_path_with_home(&project_path, Some(&home_alias)),
            "~/work/app"
        );
        assert_eq!(
            encode_project_path_with_home(&home_alias.join("not-created"), Some(&home_alias)),
            "~/not-created"
        );
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        let config = Config {
            path: Some(path.clone()),
            ..Config::default()
        };
        config.save().expect("save");
        assert!(path.is_file());
    }

    #[test]
    fn save_without_a_path_fails() {
        assert!(matches!(
            Config::default().save(),
            Err(ConfigError::NoConfigPath)
        ));
    }

    #[test]
    fn path_display_parses_and_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "[path_display]\nstyle = \"tail\"\ntail = 3\n").expect("write");

        let config = Config::load_from(Some(path.clone())).expect("load");
        assert_eq!(config.path_display.style, PathDisplayStyle::Tail);
        assert_eq!(config.path_display.tail, 3);

        config.save().expect("save");
        let reloaded = Config::load_from(Some(path)).expect("reload");
        assert_eq!(reloaded.path_display, config.path_display);
    }

    #[test]
    fn invalid_path_display_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "[path_display]\nstyle = \"fancy\"\n").expect("write");
        let error = Config::load_from(Some(path)).expect_err("invalid style");
        assert!(matches!(error, ConfigError::Invalid { .. }));
        let message = error.to_string();
        assert!(message.contains("path_display.style"), "{message}");
        assert!(message.contains("unknown style"), "{message}");
    }

    #[test]
    fn invalid_capture_target_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "capture_target = \"NOT VALID\"\n").expect("write");
        let error = Config::load_from(Some(path)).expect_err("invalid capture target");
        assert!(matches!(error, ConfigError::CaptureTarget { .. }));
        assert!(error.to_string().contains("capture_target"), "{error}");
    }

    #[test]
    fn rewrite_preserves_unknown_keys_and_the_legacy_vault() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &path,
            "vault = \"/tmp/legacy\"\ncapture_target = \"abc1234567\"\ntheme = \"dark\"\n\n[extra]\nnested = true\n",
        )
        .expect("write");

        let mut config = Config::load_from(Some(path.clone())).expect("load");
        assert_eq!(
            config.deprecated_vault.as_deref(),
            Some(Path::new("/tmp/legacy"))
        );
        assert_eq!(
            config.capture_target,
            Some(TaskId::parse("abc1234567").expect("id"))
        );

        config.projects.push(Project {
            path: PathBuf::from("/p/a"),
            slug: "a".to_owned(),
            never_ask_nested: true,
        });
        config.save().expect("save");

        let text = fs::read_to_string(&path).expect("read");
        for expected in [
            "vault = \"/tmp/legacy\"",
            "capture_target = \"abc1234567\"",
            "theme = \"dark\"",
            "nested = true",
            "[[project]]",
            "never_ask_nested = true",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in: {text}");
        }

        let reloaded = Config::load_from(Some(path)).expect("reload");
        assert_eq!(reloaded.deprecated_vault, config.deprecated_vault);
        assert_eq!(reloaded.capture_target, config.capture_target);
        assert_eq!(reloaded.projects, config.projects);
    }

    #[test]
    fn project_entries_round_trip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &path,
            "[[project]]\npath = \"/home/me/one\"\nslug = \"one\"\n\n[[project]]\npath = \"/home/me/two\"\nslug = \"two\"\nnever_ask_nested = true\n",
        )
        .expect("write");

        let config = Config::load_from(Some(path.clone())).expect("load");
        assert_eq!(config.projects.len(), 2);
        assert_eq!(config.projects[0].slug, "one");
        assert!(!config.projects[0].never_ask_nested);
        assert_eq!(config.projects[1].slug, "two");
        assert!(config.projects[1].never_ask_nested);

        config.save().expect("save");
        let reloaded = Config::load_from(Some(path)).expect("reload");
        assert_eq!(reloaded.projects, config.projects);
    }

    #[test]
    fn invalid_project_entry_is_an_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "[[project]]\npath = \"/p/a\"\n").expect("write");
        assert!(matches!(
            Config::load_from(Some(path)),
            Err(ConfigError::Invalid { .. })
        ));
    }

    #[test]
    fn single_table_project_entry_is_accepted_and_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = temp_config(&dir);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "[project]\npath = \"/p/one\"\nslug = \"one\"\n").expect("write");

        let config = Config::load_from(Some(path.clone())).expect("load");
        assert_eq!(config.projects.len(), 1);
        assert_eq!(config.projects[0].slug, "one");

        config.save().expect("save");
        let reloaded = Config::load_from(Some(path)).expect("reload");
        assert_eq!(reloaded.projects, config.projects);
    }
}
