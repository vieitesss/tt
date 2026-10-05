//! Integration tests for the vault file watcher (node 6).

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use tt::{Config, Task, TaskId, TaskState, Vault, VaultIssueKind, VaultWatcher};

fn parse_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("valid id")
}

fn task_document(id: &str, title: &str) -> String {
    Task::new(parse_id(id), title).to_document()
}

/// Drain the short burst of history macOS FSEvents replays as the stream
/// starts, including writes that predate the watch (verified with a raw-event
/// probe). Without this, a later `wait` would consume a setup event instead of
/// the change under test.
fn drain_startup_burst(watcher: &VaultWatcher) {
    while watcher.wait(Duration::from_millis(400)).expect("wait") {}
}

/// Wait for the watcher's signal, then reload, so each step proves the CLI's
/// write really woke the live view instead of being seen by luck.
fn signals_and_reload(watcher: &VaultWatcher, vault: &mut Vault, what: &str) {
    assert!(
        watcher
            .wait(Duration::from_secs(5))
            .expect("wait for signal"),
        "{what} must wake the watcher"
    );
    vault.reload().expect("reload");
}

/// An isolated project environment: temp config file, temp `XDG_DATA_HOME`,
/// and a temp project directory used as the working directory. The same shape
/// as the CLI sandbox, so these tests exercise the real atomic write paths.
struct Sandbox {
    _root: tempfile::TempDir,
    config: PathBuf,
    data: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let config = root.path().join("config").join("config.toml");
        let data = root.path().join("data");
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("project dir");
        Self {
            _root: root,
            config,
            data,
            project,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tt"));
        command
            .env("TT_CONFIG", &self.config)
            .env("XDG_DATA_HOME", &self.data)
            .env_remove("TT_VAULT")
            .env_remove("TT_PATH")
            .current_dir(&self.project);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().expect("run tt");
        assert!(
            output.status.success(),
            "tt failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Where the store for `slug` lives.
    fn store_dir(&self, slug: &str) -> PathBuf {
        self.data.join("tt").join(slug)
    }
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
    let mut vault = Vault::open(dir.path()).expect("open vault");
    let watcher = VaultWatcher::start(dir.path()).expect("start watcher");

    // Start watching before creating the task, then drain the burst FSEvents
    // replays for folder creation and the create itself; the assertion below
    // is about the reload's reads, not about setup noise.
    fs::write(
        dir.path().join("readtest01.md"),
        task_document("readtest01", "Read me"),
    )
    .expect("write task");
    assert!(
        watcher
            .wait(Duration::from_secs(3))
            .expect("wait for signal"),
        "creating a task signals"
    );
    drain_startup_burst(&watcher);

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

#[test]
fn cli_add_signals_the_watcher_and_reload_sees_it() {
    let sandbox = Sandbox::new();
    sandbox.run(&["project", "add"]);
    let store = sandbox.store_dir("project");
    let mut vault = Vault::open(&store).expect("open vault");
    let watcher = VaultWatcher::start(&store).expect("start watcher");
    drain_startup_burst(&watcher);

    sandbox.run(&["add", "--title", "From the CLI"]);

    signals_and_reload(&watcher, &mut vault, "a CLI add");
    assert_eq!(vault.len(), 1);
    assert_eq!(vault.tasks().next().expect("task").title, "From the CLI");
}

#[test]
fn a_cli_add_reaches_two_watcher_views_of_the_same_store() {
    let sandbox = Sandbox::new();
    sandbox.run(&["project", "add"]);
    let store = sandbox.store_dir("project");
    let mut first = Vault::open(&store).expect("open first view");
    let mut second = Vault::open(&store).expect("open second view");
    let first_watcher = VaultWatcher::start(&store).expect("watch first");
    let second_watcher = VaultWatcher::start(&store).expect("watch second");
    drain_startup_burst(&first_watcher);
    drain_startup_burst(&second_watcher);

    sandbox.run(&["add", "--title", "Shared store task"]);

    signals_and_reload(&first_watcher, &mut first, "a CLI add");
    signals_and_reload(&second_watcher, &mut second, "a CLI add");
    assert_eq!(
        first.tasks().next().expect("first task").title,
        "Shared store task"
    );
    assert_eq!(
        second.tasks().next().expect("second task").title,
        "Shared store task"
    );
}

#[test]
fn cli_state_edit_and_move_reach_a_watching_vault() {
    let sandbox = Sandbox::new();
    sandbox.run(&["project", "add"]);
    let store = sandbox.store_dir("project");
    let mut vault = Vault::open(&store).expect("open vault");
    let watcher = VaultWatcher::start(&store).expect("start watcher");
    drain_startup_burst(&watcher);

    let output = sandbox.run(&["add", "--title", "CLI target", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("add json");
    let id = value["id"].as_str().expect("task id").to_owned();
    signals_and_reload(&watcher, &mut vault, "cli add");
    assert!(vault.get(&parse_id(&id)).is_some());

    sandbox.run(&["done", &id]);
    signals_and_reload(&watcher, &mut vault, "cli done");
    assert_eq!(
        vault.get(&parse_id(&id)).expect("task").state,
        TaskState::Done
    );

    sandbox.run(&["edit", &id, "--title", "Renamed by the CLI"]);
    signals_and_reload(&watcher, &mut vault, "cli edit");
    assert_eq!(
        vault.get(&parse_id(&id)).expect("task").title,
        "Renamed by the CLI"
    );

    let output = sandbox.run(&["add", "--title", "CLI parent", "--json"]);
    let parent: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parent json");
    let parent_id = parent["id"].as_str().expect("parent id").to_owned();
    signals_and_reload(&watcher, &mut vault, "cli add (parent)");

    sandbox.run(&["move", &id, "--parent", &parent_id]);
    signals_and_reload(&watcher, &mut vault, "cli move --parent");
    assert_eq!(
        vault.parent(&parse_id(&id)),
        Some(&parse_id(&parent_id)),
        "the CLI's in-project move is visible after the reload"
    );

    sandbox.run(&["move", &id, "--root"]);
    signals_and_reload(&watcher, &mut vault, "cli move --root");
    assert_eq!(vault.parent(&parse_id(&id)), None);
}

#[test]
fn a_cli_registration_is_visible_through_the_config_path_the_tui_reloads() {
    let sandbox = Sandbox::new();
    sandbox.run(&["project", "add"]);

    let config = Config::load_from(Some(sandbox.config.clone())).expect("load registry");
    assert_eq!(config.projects.len(), 1);
    assert_eq!(config.projects[0].slug, "project");

    sandbox.run(&["project", "remove", "project"]);
    let config = Config::load_from(Some(sandbox.config.clone())).expect("load registry");
    assert!(
        config.projects.is_empty(),
        "the CLI's removal is visible through the same path the TUI reloads"
    );
}
