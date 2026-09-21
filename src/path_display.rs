//! Shortening project paths for the list header.
//!
//! Three styles, all replacing `$HOME` with `~` when the path is inside it:
//!
//! - `home`: full path (`~/work/app`);
//! - `short`: fish-prompt default — every parent segment becomes its first
//!   character and the last segment stays full (`~/w/app`);
//! - `tail N`: like `short`, but the last N segments stay full
//!   (`~/a/b/gamma/delta` for `N = 2`).
//!
//! The header always shows the registered project path, never the invisible
//! store folder.

use std::env;
use std::path::{Component, Path, PathBuf};

use crate::config::{PathDisplay, PathDisplayStyle};

/// Shorten `path` using the current `$HOME` for `~` substitution.
pub fn shorten(path: &Path, display: &PathDisplay) -> String {
    let home = env::var_os("HOME").map(PathBuf::from);
    shorten_with_home(path, display, home.as_deref())
}

/// [`shorten`] with an explicit home directory, for tests.
pub fn shorten_with_home(path: &Path, display: &PathDisplay, home: Option<&Path>) -> String {
    let (prefix, segments) = split(path, home);
    match display.style {
        PathDisplayStyle::Home => format!("{prefix}{}", segments.join("/")),
        PathDisplayStyle::Short => fish(prefix, &segments, 1),
        PathDisplayStyle::Tail => fish(prefix, &segments, display.tail.max(1)),
    }
}

/// Split `path` into a display prefix (`~`, `~/`, `/`, or empty) and its
/// non-root components.
fn split(path: &Path, home: Option<&Path>) -> (&'static str, Vec<String>) {
    if let Some(home) = home {
        if path == home {
            return ("~", Vec::new());
        }
        if let Ok(rest) = path.strip_prefix(home) {
            let segments = names(rest);
            if !segments.is_empty() {
                return ("~/", segments);
            }
        }
    }
    let prefix = if path.is_absolute() { "/" } else { "" };
    (prefix, names(path))
}

/// Component names, keeping `..` and dropping roots and `.`.
fn names(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_owned()),
            _ => None,
        })
        .collect()
}

/// Fish-style shortening: abbreviate every segment before the last `full` to
/// its first character.
fn fish(prefix: &str, segments: &[String], full: usize) -> String {
    if segments.is_empty() {
        return if prefix.is_empty() {
            ".".to_owned()
        } else {
            prefix.to_owned()
        };
    }
    let keep = full.min(segments.len());
    let abbreviated = segments.len() - keep;
    let mut parts: Vec<String> = segments
        .iter()
        .take(abbreviated)
        .map(|segment| {
            segment
                .chars()
                .next()
                .map_or_else(String::new, String::from)
        })
        .collect();
    parts.extend(segments.iter().skip(abbreviated).cloned());
    format!("{prefix}{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/me";

    fn at(style: PathDisplayStyle, tail: usize) -> PathDisplay {
        PathDisplay { style, tail }
    }

    fn render(path: &str, display: &PathDisplay) -> String {
        shorten_with_home(Path::new(path), display, Some(Path::new(HOME)))
    }

    #[test]
    fn short_uses_tilde_for_home_and_one_letter_parents() {
        let display = at(PathDisplayStyle::Short, 1);
        assert_eq!(render("/home/me/work/app", &display), "~/w/app");
        assert_eq!(render("/home/me", &display), "~");
        assert_eq!(render("/home/me/one", &display), "~/one");
        assert_eq!(render("/opt/very/deep/app", &display), "/o/v/d/app");
        assert_eq!(render("/", &display), "/");
    }

    #[test]
    fn home_style_keeps_full_segments() {
        let display = at(PathDisplayStyle::Home, 1);
        assert_eq!(render("/home/me/work/app", &display), "~/work/app");
        assert_eq!(render("/home/me", &display), "~");
        assert_eq!(render("/opt/x", &display), "/opt/x");
    }

    #[test]
    fn tail_keeps_the_last_n_segments_full() {
        let display = at(PathDisplayStyle::Tail, 2);
        assert_eq!(
            render("/home/me/alpha/beta/gamma/delta", &display),
            "~/a/b/gamma/delta"
        );
        assert_eq!(render("/home/me/alpha/beta", &display), "~/alpha/beta");
        assert_eq!(render("/opt/alpha/beta/gamma", &display), "/o/a/beta/gamma");
    }

    #[test]
    fn tail_with_one_matches_short() {
        let short = at(PathDisplayStyle::Short, 1);
        let tail = at(PathDisplayStyle::Tail, 1);
        for path in ["/home/me/a/b/c", "/opt/x/y", "/home/me", "/"] {
            assert_eq!(
                render(path, &short),
                render(path, &tail),
                "styles must agree for {path}"
            );
        }
    }

    #[test]
    fn prefix_matching_is_component_wise_and_home_is_optional() {
        let display = at(PathDisplayStyle::Short, 1);
        // `/home/mexico` is not inside `/home/me`.
        assert_eq!(render("/home/mexico/x", &display), "/h/m/x");
        // Without a home directory the path stays absolute.
        assert_eq!(
            shorten_with_home(Path::new("/home/me/a/b"), &display, None),
            "/h/m/a/b"
        );
        // Relative paths keep their relative prefix.
        assert_eq!(render("alpha/beta", &display), "a/beta");
    }
}
