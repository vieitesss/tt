//! Live-update plumbing shared by the list and the launch modal.
//!
//! Both stages watch their store through [`VaultWatcher`](tt::VaultWatcher)
//! and also reconcile on a bounded clock, because the native watcher can fail
//! to start or silently drop events. Both also re-read the registry file the
//! CLI writes, and both surface the same failure once rather than on every
//! rescan.

use std::path::Path;
use std::time::{Duration, Instant};

use tt::registry;
use tt::{Config, Project};

/// How long between bounded reconciliations.
///
/// The watcher is the fast path; this rescan keeps a session correct when it
/// failed to start or dropped events. About a second, so a CLI write shows up
/// promptly at the cost of one store scan and one small registry read while
/// idle.
pub(crate) const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

/// An elapsed-time trigger for bounded reconciliation.
///
/// The event loops call `on_tick` once per iteration, and an iteration is not
/// a unit of time: a keypress or resize returns immediately, while an idle
/// poll waits out the 250 ms [`TICK`](super::app::TICK). Counting calls would
/// rescale the interval with typing speed, so this clock measures wall time
/// and fires only once the interval really elapsed.
#[derive(Debug)]
pub(crate) struct ReconcileClock {
    last: Instant,
}

impl ReconcileClock {
    /// A clock that is first due one [`RECONCILE_INTERVAL`] after `now`.
    pub(crate) fn new(now: Instant) -> Self {
        Self { last: now }
    }

    /// Whether a scan is due at `now`, consuming the due moment. Time that
    /// passed while the session was busy (in `$EDITOR`, say) is not replayed
    /// as extra scans.
    pub(crate) fn due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last) < RECONCILE_INTERVAL {
            return false;
        }
        self.last = now;
        true
    }
}

/// Remembers the failure a stage currently shows, so a condition that keeps
/// failing is reported once instead of replacing the display on every rescan.
#[derive(Debug, Default)]
pub(crate) struct FailureLatch {
    current: Option<String>,
}

impl FailureLatch {
    /// Whether `message` is new; the message becomes the current failure.
    pub(crate) fn first(&mut self, message: &str) -> bool {
        if self.current.as_deref() == Some(message) {
            return false;
        }
        self.current = Some(message.to_owned());
        true
    }

    /// Forget the current failure, so the next identical one is reported.
    pub(crate) fn clear(&mut self) {
        self.current = None;
    }

    /// The failure currently reported, if any.
    pub(crate) fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }
}

/// Whether two registry entries name the same destination directory.
///
/// Compared by normalized path, never by slug: the same directory is the same
/// destination even after it is unregistered and registered again under a new
/// slug (the caller then re-resolves to the current entry).
pub(crate) fn same_project(left: &Project, right: &Project) -> bool {
    registry::normalize(&left.path) == registry::normalize(&right.path)
}

/// The registry's current entry for `project`'s directory, if it is still
/// registered. Re-registering can change a project's slug, so callers must act
/// on this fresh entry, never on the stale clone they captured earlier.
pub(crate) fn registered_like(config: &Config, project: &Project) -> Option<Project> {
    config
        .projects
        .iter()
        .find(|current| same_project(current, project))
        .cloned()
}

/// The highlight that keeps `previous` selected after the match list changed,
/// or the old highlight clamped to the shortened list when it vanished.
pub(crate) fn retarget_highlight(
    matches: &[Project],
    highlight: usize,
    previous: Option<&Project>,
) -> usize {
    previous
        .and_then(|previous| {
            matches
                .iter()
                .position(|project| same_project(project, previous))
        })
        .unwrap_or_else(|| highlight.min(matches.len().saturating_sub(1)))
}

/// How a [`RegistryFile`] refresh ended.
pub(crate) enum RegistryRefresh {
    /// The config has no file path (tests, embedding): nothing to re-read.
    InMemory,
    /// The file was re-read; adopt this registry.
    Fresh(Config),
    /// The file could not be read, so the last good registry stands.
    ///
    /// `Some(message)` the first time this failure is surfaced, `None` while
    /// it is already on screen, so a broken file does not replace the display
    /// on every bounded rescan.
    Failed(Option<String>),
}

/// Re-reads a stage's registry file from the path its config was loaded from.
///
/// The CLI adds and removes projects from other processes, so the registry the
/// session started with is not authoritative. A read failure keeps the last
/// good registry and is remembered once, not on every bounded rescan.
#[derive(Debug, Default)]
pub(crate) struct RegistryFile {
    failure: FailureLatch,
}

impl RegistryFile {
    /// Re-read the file and update the failure latch.
    pub(crate) fn refresh(&mut self, config: &Config) -> RegistryRefresh {
        let Some(path) = config.path().map(Path::to_path_buf) else {
            return RegistryRefresh::InMemory;
        };
        match Config::load_from(Some(path)) {
            Ok(fresh) => {
                self.failure.clear();
                RegistryRefresh::Fresh(fresh)
            }
            Err(error) => {
                let message = format!("config error: {error}");
                RegistryRefresh::Failed(self.failure.first(&message).then_some(message))
            }
        }
    }

    /// The failure currently surfaced, so a stage can drop stale config
    /// feedback once the file reads again, or repeat it when a user action
    /// fails because the file is still unreadable.
    pub(crate) fn surfaced(&self) -> Option<&str> {
        self.failure.current()
    }
}
