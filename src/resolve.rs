//! Resolve a working directory to a registered project.
//!
//! Resolution is purely structural: walk the ancestors of the starting
//! directory, nearest first, and take the first registered project. A start
//! that is strictly inside the project is reported as `nested` so interactive
//! flows can ask once whether to manage it as its own project; nothing here
//! prompts or touches the filesystem beyond path normalization.

use std::env;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::{Config, Project};
use crate::registry::normalize;

/// Environment variable naming the starting directory for resolution.
pub const PATH_ENV: &str = "TT_PATH";

/// The starting directory: `explicit` (`--path`), then `$TT_PATH`, then the
/// current working directory. Empty environment values count as unset.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when the current directory cannot
/// be determined.
pub fn start_dir(explicit: Option<&Path>) -> io::Result<PathBuf> {
    start_dir_from(explicit, env::var_os(PATH_ENV).as_deref(), env::current_dir)
}

/// [`start_dir`] against explicit values: `explicit` (`--path`), then
/// `tt_path` (`$TT_PATH`), then `cwd`. An empty `tt_path` counts as unset.
///
/// `cwd` is lazy so the working directory is only read when it is actually
/// the answer; tests pass a closure and never touch the process environment.
pub(crate) fn start_dir_from(
    explicit: Option<&Path>,
    tt_path: Option<&OsStr>,
    cwd: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(value) = tt_path.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    cwd()
}

/// Outcome of resolving a starting directory against the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A registered project was found at or above the starting directory.
    Registered {
        /// The registered project (the nearest ancestor wins).
        project: Project,
        /// The normalized starting directory when it is strictly inside the
        /// project; `None` for an exact match.
        nested: Option<PathBuf>,
    },
    /// No ancestor of the starting directory is registered.
    Unregistered {
        /// Normalized starting directory.
        path: PathBuf,
    },
}

impl Resolution {
    /// The resolved project, if any.
    pub fn project(&self) -> Option<&Project> {
        match self {
            Self::Registered { project, .. } => Some(project),
            Self::Unregistered { .. } => None,
        }
    }

    /// The starting directory when it was strictly inside a project.
    pub fn nested(&self) -> Option<&Path> {
        match self {
            Self::Registered { nested, .. } => nested.as_deref(),
            Self::Unregistered { .. } => None,
        }
    }
}

/// Resolve `start` (already selected from `--path`, `$TT_PATH`, or the current
/// directory) against the registered projects in `config`.
///
/// Project paths are normalized before comparison, so hand-edited entries
/// with `..` or a trailing slash still match. `never_ask_nested` never changes
/// the result; it only tells interactive flows whether to ask.
pub fn resolve(config: &Config, start: &Path) -> Resolution {
    let start = normalize(start);
    let projects: Vec<(PathBuf, &Project)> = config
        .projects
        .iter()
        .map(|project| (normalize(&project.path), project))
        .collect();

    for ancestor in start.ancestors() {
        if let Some((path, project)) = projects.iter().find(|(path, _)| path == ancestor) {
            let nested = (*path != start).then(|| start.clone());
            return Resolution::Registered {
                project: (*project).clone(),
                nested,
            };
        }
    }

    Resolution::Unregistered { path: start }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn project(path: &Path, slug: &str) -> Project {
        Project {
            path: path.to_path_buf(),
            slug: slug.to_owned(),
            never_ask_nested: false,
        }
    }

    fn config_with(projects: Vec<Project>) -> Config {
        Config::with_projects(projects)
    }

    /// A temp root with `proj/` and `proj/sub/`, plus a sibling named
    /// `proj-other/` that shares a prefix but is not inside the project.
    fn layout() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("proj/sub/deep")).expect("create project tree");
        fs::create_dir_all(root.join("proj-other/deep")).expect("create sibling tree");
        (dir, root)
    }

    #[test]
    fn exact_project_path_resolves_without_nested() {
        let (_dir, root) = layout();
        let config = config_with(vec![project(&root.join("proj"), "proj")]);

        let result = resolve(&config, &root.join("proj"));
        assert_eq!(
            result,
            Resolution::Registered {
                project: project(&root.join("proj"), "proj"),
                nested: None,
            }
        );
        assert_eq!(result.project().expect("project").slug, "proj");
        assert_eq!(result.nested(), None);
    }

    #[test]
    fn descendant_resolves_to_the_ancestor_and_reports_nested() {
        let (_dir, root) = layout();
        let config = config_with(vec![project(&root.join("proj"), "proj")]);
        let start = root.join("proj/sub/deep");

        let result = resolve(&config, &start);
        assert_eq!(result.project().expect("project").slug, "proj");
        assert_eq!(result.nested(), Some(normalize(&start).as_path()));
    }

    #[test]
    fn nearest_registered_ancestor_wins() {
        let (_dir, root) = layout();
        let config = config_with(vec![
            project(&root.join("proj"), "proj"),
            project(&root.join("proj/sub"), "sub"),
        ]);
        let start = root.join("proj/sub/deep");

        let result = resolve(&config, &start);
        assert_eq!(
            result.project().expect("project").slug,
            "sub",
            "the deeper registration is the nearest"
        );
        assert_eq!(result.nested(), Some(normalize(&start).as_path()));

        // An exact hit on the deeper registration has no nested dir.
        let result = resolve(&config, &root.join("proj/sub"));
        assert_eq!(result.project().expect("project").slug, "sub");
        assert_eq!(result.nested(), None);
    }

    #[test]
    fn unregistered_path_is_reported() {
        let (_dir, root) = layout();
        let config = config_with(vec![project(&root.join("proj"), "proj")]);
        let start = root.join("elsewhere/deep");

        let result = resolve(&config, &start);
        assert_eq!(
            result,
            Resolution::Unregistered {
                path: normalize(&start),
            }
        );
        assert_eq!(result.project(), None);
        assert_eq!(result.nested(), None);
    }

    #[test]
    fn unregistered_sibling_with_a_shared_prefix_does_not_hit() {
        let (_dir, root) = layout();
        let config = config_with(vec![project(&root.join("proj"), "proj")]);

        let result = resolve(&config, &root.join("proj-other/deep"));
        assert!(
            matches!(result, Resolution::Unregistered { .. }),
            "path components must match exactly: {result:?}"
        );
    }

    #[test]
    fn never_ask_nested_is_data_only_and_does_not_change_the_hit() {
        let (_dir, root) = layout();
        let mut entry = project(&root.join("proj"), "proj");
        entry.never_ask_nested = true;
        let config = config_with(vec![entry]);

        let result = resolve(&config, &root.join("proj/sub"));
        assert_eq!(result.project().expect("project").slug, "proj");
        assert!(
            result.project().expect("project").never_ask_nested,
            "the flag surfaces on the project for callers to consult"
        );
        assert!(
            result.nested().is_some(),
            "the nested directory is still reported"
        );
    }

    #[test]
    fn hand_edited_project_paths_are_normalized_before_comparison() {
        let (_dir, root) = layout();
        // Both entries normalize to the same directory; file order decides.
        let config = config_with(vec![
            project(&root.join("proj").join("."), "dotted"),
            project(&root.join("proj/sub/.."), "dotted-up"),
        ]);
        let result = resolve(&config, &root.join("proj"));
        assert_eq!(result.project().expect("project").slug, "dotted");
        assert_eq!(result.nested(), None, "normalized to an exact match");

        // A config whose only entry uses `..` still resolves.
        let config = config_with(vec![project(&root.join("proj/sub/.."), "dotted-up")]);
        let result = resolve(&config, &root.join("proj"));
        assert_eq!(result.project().expect("project").slug, "dotted-up");
        assert_eq!(result.nested(), None);
    }

    #[test]
    fn start_dir_prefers_the_explicit_path() {
        let start = start_dir(Some(Path::new("/explicit"))).expect("start dir");
        assert_eq!(start, PathBuf::from("/explicit"));
    }

    #[test]
    fn start_dir_falls_back_to_tt_path_then_cwd() {
        let cwd = || Ok(PathBuf::from("/cwd"));

        assert_eq!(
            start_dir_from(
                Some(Path::new("/explicit")),
                Some(OsStr::new("/from/env")),
                cwd
            )
            .expect("start dir"),
            PathBuf::from("/explicit")
        );
        assert_eq!(
            start_dir_from(None, Some(OsStr::new("/from/env")), cwd).expect("start dir"),
            PathBuf::from("/from/env")
        );
        assert_eq!(
            start_dir_from(None, Some(OsStr::new("")), cwd).expect("start dir"),
            PathBuf::from("/cwd"),
            "an empty TT_PATH counts as unset"
        );
        assert_eq!(
            start_dir_from(None, None, cwd).expect("start dir"),
            PathBuf::from("/cwd")
        );
    }
}
