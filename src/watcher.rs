//! File watching for the vault.
//!
//! [`VaultWatcher`] watches a vault folder with `notify` and emits one
//! debounced signal per burst of filesystem events. The watcher only reports;
//! it never writes to the vault, so it cannot clobber user data.
//!
//! # TUI contract (node 7)
//!
//! On a debounced change:
//!
//! - with no unsaved edit buffer, call [`Vault::reload`](crate::Vault::reload)
//!   and re-render;
//! - with an unsaved edit buffer, keep the buffer and show a stale warning;
//!   never save over the external bytes automatically.
//!
//! Reload only discards the in-memory cache; unparsable files stay untouched
//! on disk (they are skipped and reported as issues), so no external edit can
//! be destroyed by watching.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

/// Quiet period after the last filesystem event before a change is signalled.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// Errors from starting or waiting on a watcher.
#[derive(Debug, Error)]
pub enum WatchError {
    /// The folder could not be watched.
    #[error("cannot watch vault {path}: {source}")]
    Notify {
        /// Watched folder.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: notify::Error,
    },
    /// The watcher thread stopped because the watcher was dropped.
    #[error("vault watcher stopped")]
    Stopped,
}

/// Watches one vault folder and reports debounced changes.
pub struct VaultWatcher {
    /// Kept alive: dropping this stops the underlying watch.
    _watcher: RecommendedWatcher,
    signals: Receiver<()>,
}

impl VaultWatcher {
    /// Start watching `root` non-recursively.
    ///
    /// Events are coalesced: a burst of filesystem events produces one signal
    /// after [`DEBOUNCE`] of quiet.
    ///
    /// # Errors
    ///
    /// Returns [`WatchError::Notify`] when the folder cannot be watched.
    pub fn start(root: impl AsRef<Path>) -> Result<Self, WatchError> {
        let root = root.as_ref().to_path_buf();
        let (raw_tx, raw_rx) = mpsc::channel::<()>();

        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                // Opening/reading the vault (which every reload does) emits
                // access events; signalling on them would make a reload look
                // like an external change and loop forever. Only real content
                // changes count.
                if matches!(
                    event.kind,
                    EventKind::Any
                        | EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_)
                ) {
                    let _ = raw_tx.send(());
                }
            })
            .map_err(|source| WatchError::Notify {
                path: root.clone(),
                source,
            })?;
        watcher
            .watch(&root, RecursiveMode::NonRecursive)
            .map_err(|source| WatchError::Notify {
                path: root.clone(),
                source,
            })?;

        let (signal_tx, signal_rx) = mpsc::channel();
        thread::spawn(move || debounce_loop(&raw_rx, &signal_tx, DEBOUNCE));

        Ok(Self {
            _watcher: watcher,
            signals: signal_rx,
        })
    }

    /// Block until a debounced change arrives, or `timeout` elapses.
    ///
    /// Returns `true` when a change is pending, `false` on timeout.
    ///
    /// # Errors
    ///
    /// Returns [`WatchError::Stopped`] when the watcher thread has ended.
    pub fn wait(&self, timeout: Duration) -> Result<bool, WatchError> {
        match self.signals.recv_timeout(timeout) {
            Ok(()) => Ok(true),
            Err(RecvTimeoutError::Timeout) => Ok(false),
            Err(RecvTimeoutError::Disconnected) => Err(WatchError::Stopped),
        }
    }

    /// Non-blocking check for a pending debounced change.
    ///
    /// A stopped watcher reports `false`; use [`VaultWatcher::wait`] when the
    /// difference matters.
    pub fn changed(&self) -> bool {
        match self.signals.try_recv() {
            Ok(()) => true,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => false,
        }
    }
}

/// Coalesce raw events: after the first event, wait for `interval` of quiet
/// before signalling once. Ends when either channel closes.
fn debounce_loop(raw: &Receiver<()>, signal: &Sender<()>, interval: Duration) {
    loop {
        if raw.recv().is_err() {
            return;
        }
        loop {
            match raw.recv_timeout(interval) {
                Ok(()) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if signal.send(()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_coalesces_a_burst_into_one_signal() {
        let (raw_tx, raw_rx) = mpsc::channel();
        let (signal_tx, signal_rx) = mpsc::channel();
        let handle =
            thread::spawn(move || debounce_loop(&raw_rx, &signal_tx, Duration::from_millis(50)));

        raw_tx.send(()).expect("send");
        raw_tx.send(()).expect("send");
        raw_tx.send(()).expect("send");

        assert_eq!(
            signal_rx.recv_timeout(Duration::from_secs(2)),
            Ok(()),
            "the burst should produce exactly one signal"
        );
        assert_eq!(
            signal_rx.recv_timeout(Duration::from_millis(200)),
            Err(RecvTimeoutError::Timeout),
            "no extra signal after the burst"
        );

        drop(raw_tx);
        handle.join().expect("debounce thread should end");
    }
}
