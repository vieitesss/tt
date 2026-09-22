//! ToTask (`tt`) library.
//!
//! The vault library is the single source of truth for all task mutation. The
//! CLI ([`tt`](../../tt/index.html) binary) and the TUI are thin clients over
//! the public APIs defined here; neither has a privileged write path.
//!
//! Implementation arrives milestone by milestone:
//! task model and frontmatter (node 2), vault CRUD (node 3), index (node 4),
//! watcher (node 6), and TUI (nodes 7–8).

pub mod config;
mod fsutil;
mod index;
pub mod model;
pub mod path_display;
pub mod registry;
pub mod resolve;
pub mod vault;
pub mod watcher;

pub use config::{Config, ConfigError, PathDisplay, PathDisplayStyle, Project};
pub use index::{TaskFilter, TreeNode};
pub use model::{IdError, ParseError, Priority, Task, TaskId, TaskState};
pub use path_display::{shorten, shorten_with_home};
pub use registry::RegistryError;
pub use resolve::{resolve, Resolution};
pub use vault::{
    DeleteOutcome, NewTask, ShiftOutcome, Vault, VaultError, VaultIssue, VaultIssueKind,
};
pub use watcher::{VaultWatcher, WatchError, DEBOUNCE};

/// Crate version, as reported by the `tt` binary.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
