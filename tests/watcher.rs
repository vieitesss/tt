//! Integration tests for the vault file watcher (node 6).

use std::fs;
use std::time::Duration;

use tt::{Task, TaskId, Vault, VaultIssueKind, VaultWatcher};

fn parse_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("valid id")
}

fn task_document(id: &str, title: &str) -> String {
    Task::new(parse_id(id), title).to_document()
}

#[test]
fn watcher_signals_after_external_edits_and_reload_sees_them() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut vault = Vault::open(dir.path()).expect("open vault");
    let watcher = VaultWatcher::start(dir.path()).expect("start watcher");
    let id = parse_id("watched001");

    fs::write(
        dir.path().join("watched001.md"),
        task_document("watched001", "Created"),
    )
    .expect("external create");
    assert!(
        watcher
            .wait(Duration::from_secs(3))
            .expect("wait for signal"),
        "watcher should signal an external create"
    );
    assert!(
        vault.get(&id).is_none(),
        "the cache stays stale until the caller reloads"
    );

    vault.reload().expect("reload");
    assert_eq!(vault.get(&id).expect("loaded").title, "Created");

    fs::write(
        dir.path().join("watched001.md"),
        task_document("watched001", "Modified"),
    )
    .expect("external modify");
    assert!(
        watcher
            .wait(Duration::from_secs(3))
            .expect("wait for signal"),
        "watcher should signal an external modify"
    );
    vault.reload().expect("reload");
    assert_eq!(vault.get(&id).expect("loaded").title, "Modified");
}

#[test]
fn watch_triggered_reload_never_rewrites_a_malformed_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let garbage_path = dir.path().join("garbage.md");
    fs::write(
        &garbage_path,
        "---\nid: garbage001\nstate: open\n---\nmissing title\n",
    )
    .expect("write garbage");
    let garbage_before = fs::read(&garbage_path).expect("read garbage");

    let mut vault = Vault::open(dir.path()).expect("open vault");
    let watcher = VaultWatcher::start(dir.path()).expect("start watcher");

    fs::write(
        dir.path().join("good000001.md"),
        task_document("good000001", "Good"),
    )
    .expect("write good");
    assert!(
        watcher
            .wait(Duration::from_secs(3))
            .expect("wait for signal"),
        "watcher should signal the new file"
    );

    let issues = vault.reload().expect("reload");

    assert_eq!(
        fs::read(&garbage_path).expect("read garbage"),
        garbage_before,
        "malformed files must never be rewritten"
    );
    assert!(vault.get(&parse_id("good000001")).is_some());
    assert!(issues
        .iter()
        .any(|issue| issue.kind == VaultIssueKind::Malformed));
}

#[test]
fn reload_reads_do_not_signal_but_writes_do() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        dir.path().join("readtest01.md"),
        task_document("readtest01", "Read me"),
    )
    .expect("write task");
    let mut vault = Vault::open(dir.path()).expect("open vault");
    let watcher = VaultWatcher::start(dir.path()).expect("start watcher");

    // Reading the vault is what `Vault::reload` does on every scan; it must
    // not look like an external change, or the TUI reloads in a loop.
    vault.reload().expect("reload");
    assert!(
        !watcher
            .wait(Duration::from_millis(800))
            .expect("wait for signal"),
        "reading the vault must not signal a change"
    );

    // A real external write still signals.
    fs::write(
        dir.path().join("readtest01.md"),
        task_document("readtest01", "Modified"),
    )
    .expect("external modify");
    assert!(
        watcher
            .wait(Duration::from_secs(3))
            .expect("wait for signal"),
        "an external write must still signal a change"
    );
}

#[test]
fn watcher_start_fails_for_a_missing_folder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("does-not-exist");

    assert!(VaultWatcher::start(&missing).is_err());
}
