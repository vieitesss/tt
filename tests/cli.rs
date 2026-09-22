//! CLI smoke tests: spawn the real binary and assert on the `--json` contract.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn base_command(vault: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tt"));
    command
        .arg("--vault")
        .arg(vault)
        .arg("--json")
        .env_remove("TT_VAULT")
        .env("TT_CONFIG", vault.join("no-config.toml"));
    command
}

/// An isolated project environment: temp config file, temp `XDG_DATA_HOME`,
/// and a temp project directory used as the working directory.
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

    fn root(&self) -> &Path {
        self._root.path()
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

    /// Where the store for `slug` lives.
    fn store_dir(&self, slug: &str) -> PathBuf {
        self.data.join("tt").join(slug)
    }
}

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("run tt");
    assert!(
        output.status.success(),
        "tt failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn run_json(command: &mut Command) -> Value {
    let output = run(command);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn add_task(vault: &Path, title: &str, extra_args: &[&str]) -> Value {
    let mut command = base_command(vault);
    command
        .arg("add")
        .arg("--title")
        .arg(title)
        .args(extra_args);
    run_json(&mut command)
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().expect("id in JSON").to_owned()
}

fn append_body(vault: &Path, id: &str, body: &str) {
    let path = vault.join(format!("{id}.md"));
    let mut document = fs::read_to_string(&path).expect("read task file");
    document.push_str(body);
    fs::write(&path, document).expect("write task file");
}

fn collect_ids(value: &Value) -> Vec<String> {
    fn walk(nodes: &[Value], ids: &mut Vec<String>) {
        for node in nodes {
            ids.push(node["id"].as_str().expect("id in JSON").to_owned());
            if let Some(children) = node["children"].as_array() {
                walk(children, ids);
            }
        }
    }

    let mut ids = Vec::new();
    if let Some(tasks) = value["tasks"].as_array() {
        walk(tasks, &mut ids);
    }
    ids
}

#[test]
fn add_prints_task_json_and_writes_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let value = add_task(dir.path(), "A", &[]);

    assert_eq!(value["state"], "open");
    assert!(value["parent"].is_null());
    assert_eq!(value["tags"], serde_json::json!([]));
    assert!(value["due"].is_null());
    assert!(value["priority"].is_null());
    assert_eq!(value["rank"], 0);

    let id = id_of(&value);
    assert_eq!(id.len(), 10);
    assert!(id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    assert!(dir.path().join(format!("{id}.md")).is_file());
}

#[test]
fn json_reports_null_rank_for_an_unranked_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        dir.path().join("unranked01.md"),
        "---\nid: unranked01\ntitle: Old task\nstate: open\n---\n",
    )
    .expect("write task");

    let value = run_json(base_command(dir.path()).args(["show", "unranked01"]));

    assert!(value["rank"].is_null());
}

#[test]
fn add_nests_child_with_flags() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent_id = id_of(&add_task(dir.path(), "Parent", &[]));
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();

    let child = add_task(
        dir.path(),
        "Child",
        &[
            "--parent",
            &parent_id,
            "--tag",
            "work/admin",
            "--due",
            &today,
            "--priority",
            "high",
        ],
    );

    assert_eq!(child["parent"], parent_id);
    assert_eq!(child["tags"], serde_json::json!(["work/admin"]));
    assert_eq!(child["due"], today);
    assert_eq!(child["priority"], "high");
}

#[test]
fn list_defaults_to_tree_with_nested_child() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent_id = id_of(&add_task(dir.path(), "Parent", &[]));
    let child_id = id_of(&add_task(dir.path(), "Child", &["--parent", &parent_id]));

    let value = run_json(base_command(dir.path()).arg("list"));
    let tasks = value["tasks"].as_array().expect("tasks array");
    let parent = tasks
        .iter()
        .find(|task| task["id"] == parent_id.as_str())
        .expect("parent at top level");

    assert_eq!(parent["children"][0]["id"], child_id);
    assert!(
        tasks.iter().all(|task| task["id"] != child_id.as_str()),
        "the child must be nested, not top-level"
    );
}

#[test]
fn list_flat_filters_by_nested_tag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent_id = id_of(&add_task(dir.path(), "Parent", &[]));
    let child_id = id_of(&add_task(
        dir.path(),
        "Child",
        &["--parent", &parent_id, "--tag", "work/admin"],
    ));

    let value = run_json(base_command(dir.path()).args(["list", "--flat", "--tag", "work"]));
    let tasks = value["tasks"].as_array().expect("tasks array");

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], child_id);
    assert!(
        tasks[0].get("children").is_none(),
        "flat tasks have no children key"
    );
}

#[test]
fn list_with_state_and_due_today_returns_child() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent_id = id_of(&add_task(dir.path(), "Parent", &[]));
    let today = chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let child_id = id_of(&add_task(
        dir.path(),
        "Child",
        &["--parent", &parent_id, "--due", &today],
    ));

    let value = run_json(base_command(dir.path()).args(["list", "--state", "open", "--due-today"]));
    let ids = collect_ids(&value);

    assert!(ids.contains(&child_id));
    assert!(
        !ids.contains(&parent_id),
        "the parent has no due date and must be filtered out"
    );
}

#[test]
fn show_includes_body_links_backlinks_and_children() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent_id = id_of(&add_task(dir.path(), "Parent", &[]));
    let dangling = "dangling01";
    append_body(dir.path(), &parent_id, &format!("[[{dangling}]]"));
    let child_id = id_of(&add_task(dir.path(), "Child", &["--parent", &parent_id]));

    let value = run_json(base_command(dir.path()).args(["show", &parent_id]));
    assert_eq!(value["body"], format!("[[{dangling}]]"));
    assert_eq!(value["links"], serde_json::json!([dangling]));
    assert_eq!(value["children"], serde_json::json!([child_id]));
    assert_eq!(value["backlinks"], serde_json::json!([]));

    append_body(dir.path(), &child_id, &format!("[[{parent_id}]]"));
    let value = run_json(base_command(dir.path()).args(["show", &parent_id]));
    assert_eq!(value["backlinks"], serde_json::json!([child_id]));
}

#[test]
fn show_normalizes_rich_link_forms_to_ids() {
    let dir = tempfile::tempdir().expect("temp dir");
    let target_id = id_of(&add_task(dir.path(), "Target", &[]));
    let source_id = id_of(&add_task(dir.path(), "Source", &[]));
    append_body(
        dir.path(),
        &source_id,
        &format!("see [[{target_id}.md|Target title]]"),
    );

    let value = run_json(base_command(dir.path()).args(["show", &source_id]));
    assert_eq!(
        value["body"],
        format!("see [[{target_id}.md|Target title]]")
    );
    assert_eq!(value["links"], serde_json::json!([target_id]));
    assert_eq!(value["backlinks"], serde_json::json!([]));
    assert!(
        value.get("link_aliases").is_none(),
        "no alias keys in the JSON contract: {value}"
    );

    let target = run_json(base_command(dir.path()).args(["show", &target_id]));
    assert_eq!(target["backlinks"], serde_json::json!([source_id]));
}

#[test]
fn cli_edit_title_never_rewrites_linking_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let target_id = id_of(&add_task(dir.path(), "Old title", &[]));
    let source_id = id_of(&add_task(dir.path(), "Source", &[]));
    append_body(
        dir.path(),
        &source_id,
        &format!("[[{target_id}.md|Old title]]"),
    );
    let source_path = dir.path().join(format!("{source_id}.md"));
    let before = fs::read_to_string(&source_path).expect("read");

    let edited =
        run_json(base_command(dir.path()).args(["edit", &target_id, "--title", "New title"]));
    assert_eq!(edited["title"], "New title");

    assert_eq!(
        fs::read_to_string(&source_path).expect("read"),
        before,
        "the CLI never title-syncs; only a live TUI reload does"
    );
}

#[test]
fn state_commands_and_edit_roundtrip() {
    let dir = tempfile::tempdir().expect("temp dir");
    let task_id = id_of(&add_task(dir.path(), "Original", &[]));

    let done = run_json(base_command(dir.path()).args(["done", &task_id]));
    assert_eq!(done["state"], "done");
    assert_eq!(done["id"], task_id);

    let shown = run_json(base_command(dir.path()).args(["show", &task_id]));
    assert_eq!(shown["state"], "done");
    assert_eq!(shown["title"], "Original");

    let reopened = run_json(base_command(dir.path()).args(["reopen", &task_id]));
    assert_eq!(reopened["state"], "open");

    let cancelled = run_json(base_command(dir.path()).args(["cancel", &task_id]));
    assert_eq!(cancelled["state"], "cancelled");

    let edited = run_json(base_command(dir.path()).args(["edit", &task_id, "--title", "Renamed"]));
    assert_eq!(edited["title"], "Renamed");

    let shown = run_json(base_command(dir.path()).args(["show", &task_id]));
    assert_eq!(shown["title"], "Renamed");
    assert_eq!(shown["state"], "cancelled");
}

#[test]
fn unknown_id_fails_with_json_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut command = base_command(dir.path());
    command.args(["show", "missing000"]);

    let output = command.output().expect("run tt");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    let value: Value = serde_json::from_slice(&output.stdout).expect("error JSON on stdout");
    assert!(
        value["error"]
            .as_str()
            .expect("error string")
            .contains("missing000"),
        "unexpected error: {value}"
    );
}

#[test]
fn invalid_id_fails_with_json_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut command = base_command(dir.path());
    command.args(["show", "bad/id"]);

    let output = command.output().expect("run tt");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    let value: Value = serde_json::from_slice(&output.stdout).expect("error JSON on stdout");
    assert!(value["error"]
        .as_str()
        .expect("error string")
        .contains("invalid task id"));
}

#[test]
fn human_list_is_plain_text() {
    let dir = tempfile::tempdir().expect("temp dir");
    add_task(dir.path(), "Human readable", &[]);

    let output = run(Command::new(env!("CARGO_BIN_EXE_tt"))
        .arg("--vault")
        .arg(dir.path())
        .arg("list")
        .env_remove("TT_VAULT")
        .env("TT_CONFIG", dir.path().join("no-config.toml")));

    let text = String::from_utf8(output.stdout).expect("utf-8 output");
    assert!(!text.trim().is_empty());
    assert!(
        !text.trim_start().starts_with('{'),
        "human output must not be JSON: {text}"
    );
    assert!(text.contains("Human readable"));
}

#[test]
fn tt_vault_env_resolves_the_vault() {
    let dir = tempfile::tempdir().expect("temp dir");
    let value = run_json(
        Command::new(env!("CARGO_BIN_EXE_tt"))
            .args(["add", "--title", "Via env", "--json"])
            .env("TT_VAULT", dir.path())
            .env("TT_CONFIG", dir.path().join("no-config.toml")),
    );

    let id = id_of(&value);
    assert!(dir.path().join(format!("{id}.md")).is_file());
}

#[test]
fn legacy_vault_key_is_warned_about_and_ignored() {
    let sandbox = Sandbox::new();
    let legacy = sandbox.root().join("legacy");
    fs::create_dir_all(&legacy).expect("legacy dir");
    fs::create_dir_all(sandbox.config.parent().expect("config parent")).expect("config dir");
    fs::write(&sandbox.config, format!("vault = {:?}\n", legacy)).expect("write config");

    let output = sandbox
        .command()
        .args(["--json", "add", "--title", "Legacy ignored"])
        .output()
        .expect("run tt");
    assert!(
        output.status.success(),
        "tt failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("task JSON");
    let id = id_of(&value);

    assert!(
        sandbox
            .store_dir("project")
            .join(format!("{id}.md"))
            .is_file(),
        "the task must go to the central store"
    );
    assert!(
        !legacy.join(format!("{id}.md")).exists(),
        "the legacy vault must not receive files"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("deprecated"),
        "expected a deprecation warning: {stderr}"
    );

    // Rewriting the config must keep the legacy key.
    let config_text = fs::read_to_string(&sandbox.config).expect("read config");
    assert!(
        config_text.contains("vault = "),
        "the key must survive: {config_text}"
    );
}

#[test]
fn capture_target_config_is_used_for_add_without_parent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let parent_id = id_of(&add_task(dir.path(), "Parent", &[]));
    let config_path = dir.path().join("capture.toml");
    fs::write(&config_path, format!("capture_target = \"{parent_id}\"\n")).expect("write config");

    let value = run_json(
        Command::new(env!("CARGO_BIN_EXE_tt"))
            .arg("--vault")
            .arg(dir.path())
            .args(["add", "--title", "Captured", "--json"])
            .env_remove("TT_VAULT")
            .env("TT_CONFIG", &config_path),
    );

    assert_eq!(value["parent"], parent_id);
}

#[test]
fn add_body_from_stdin() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut command = base_command(dir.path());
    command
        .args(["add", "--title", "Stdin body", "--body", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().expect("spawn tt");
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(b"hello from stdin\n").expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait for tt");
    assert!(output.status.success());

    let value: Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let id = id_of(&value);
    let shown = run_json(base_command(dir.path()).args(["show", &id]));
    assert_eq!(shown["body"], "hello from stdin\n");
}

#[test]
fn project_add_list_and_remove_round_trip() {
    let sandbox = Sandbox::new();

    let added = run_json(sandbox.command().args(["--json", "project", "add"]));
    let expected = sandbox.project.canonicalize().expect("canonical project");
    assert_eq!(added["slug"], "project");
    assert_eq!(added["path"], expected.to_string_lossy().as_ref());
    assert!(sandbox.store_dir("project").is_dir(), "store dir created");

    let listed = run_json(sandbox.command().args(["--json", "project", "list"]));
    let projects = listed["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["slug"], "project");
    assert_eq!(projects[0]["never_ask_nested"], false);

    // An explicit directory registers under its own basename.
    let other = sandbox.root().join("other");
    fs::create_dir_all(&other).expect("other dir");
    let added = run_json(
        sandbox
            .command()
            .args(["--json", "project", "add"])
            .arg(&other),
    );
    assert_eq!(added["slug"], "other");

    // Remove by slug; the other project stays.
    let removed = run_json(
        sandbox
            .command()
            .args(["--json", "project", "remove", "project"]),
    );
    assert_eq!(removed["removed"]["slug"], "project");
    let listed = run_json(sandbox.command().args(["--json", "project", "list"]));
    let projects = listed["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["slug"], "other");

    // Remove by path.
    let removed = run_json(
        sandbox
            .command()
            .args(["--json", "project", "remove"])
            .arg(&other),
    );
    assert_eq!(removed["removed"]["slug"], "other");
    let listed = run_json(sandbox.command().args(["--json", "project", "list"]));
    assert!(listed["projects"].as_array().expect("projects").is_empty());
}

#[test]
fn projects_flag_is_rejected_with_a_subcommand() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["--json", "--projects", "list"])
        .output()
        .expect("run tt");
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("error JSON");
    assert!(
        value["error"]
            .as_str()
            .expect("error string")
            .contains("--projects"),
        "unexpected error: {value}"
    );
}

#[test]
fn projects_short_flag_is_rejected_with_a_subcommand() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["--json", "-p", "list"])
        .output()
        .expect("run tt");
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("error JSON");
    assert!(
        value["error"]
            .as_str()
            .expect("error string")
            .contains("--projects"),
        "unexpected error: {value}"
    );
}

#[test]
fn unregistered_add_silently_registers_and_writes_to_the_store() {
    let sandbox = Sandbox::new();
    let value = run_json(
        sandbox
            .command()
            .args(["--json", "add", "--title", "First"]),
    );
    let id = id_of(&value);

    assert!(
        sandbox
            .store_dir("project")
            .join(format!("{id}.md"))
            .is_file(),
        "the task must land in the store"
    );
    assert!(
        !sandbox.project.join(format!("{id}.md")).exists(),
        "the project directory must stay clean"
    );

    let config_text = fs::read_to_string(&sandbox.config).expect("read config");
    assert!(config_text.contains("[[project]]"), "{config_text}");
    assert!(config_text.contains("slug = \"project\""), "{config_text}");
}

#[test]
fn unregistered_read_fails_with_the_locked_error() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["--json", "list"])
        .output()
        .expect("run tt");

    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).expect("error JSON on stdout");
    let error = value["error"].as_str().expect("error string");
    assert!(error.contains("not a registered project"), "{error}");
    assert!(error.contains("tt project add"), "{error}");
}

#[test]
fn ancestor_resolution_finds_the_project_from_a_subdirectory() {
    let sandbox = Sandbox::new();
    let added = run_json(
        sandbox
            .command()
            .args(["--json", "add", "--title", "Root task"]),
    );
    let id = id_of(&added);
    let sub = sandbox.project.join("sub");
    fs::create_dir_all(&sub).expect("sub dir");

    let listed = run_json(
        sandbox
            .command()
            .args(["--json", "list", "--path"])
            .arg(&sub),
    );
    assert_eq!(collect_ids(&listed), vec![id]);
}

#[test]
fn nested_add_in_json_mode_uses_the_parent_project_without_prompting() {
    let sandbox = Sandbox::new();
    run_json(
        sandbox
            .command()
            .args(["--json", "add", "--title", "Parent task"]),
    );
    let sub = sandbox.project.join("sub");
    fs::create_dir_all(&sub).expect("sub dir");

    let value = run_json(
        sandbox
            .command()
            .args(["--json", "add", "--title", "Nested task", "--path"])
            .arg(&sub),
    );
    assert_eq!(value["title"], "Nested task");

    let listed = run_json(sandbox.command().args(["--json", "project", "list"]));
    assert_eq!(
        listed["projects"].as_array().expect("projects").len(),
        1,
        "nested dirs are not registered without an interactive choice"
    );

    let store = sandbox.store_dir("project");
    assert_eq!(
        fs::read_dir(&store).expect("store").count(),
        2,
        "both tasks live in the parent store"
    );
}

#[test]
fn edit_without_title_opens_editor_and_prints_the_task() {
    let sandbox = Sandbox::new();
    let added = run_json(
        sandbox
            .command()
            .args(["--json", "add", "--title", "Editable"]),
    );
    let id = id_of(&added);

    let value = run_json(
        sandbox
            .command()
            .args(["--json", "edit", &id])
            .env("EDITOR", "/usr/bin/true"),
    );
    assert_eq!(value["id"], id);
    assert_eq!(value["title"], "Editable");
}

#[cfg(unix)]
#[test]
fn edit_without_title_runs_the_editor_and_shows_edited_content() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let added = run_json(
        sandbox
            .command()
            .args(["--json", "add", "--title", "Editable"]),
    );
    let id = id_of(&added);

    // A stub editor that appends to its last argument, so `$EDITOR` with an
    // argument exercises the whitespace splitting.
    let script = sandbox.root().join("editor.sh");
    fs::write(
        &script,
        "#!/bin/sh\nfor path in \"$@\"; do :; done\nprintf '\\nfrom the editor\\n' >> \"$path\"\n",
    )
    .expect("write editor stub");
    let mut permissions = fs::metadata(&script).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).expect("chmod");

    let editor = format!("{} --flag", script.display());
    let value = run_json(
        sandbox
            .command()
            .args(["--json", "edit", &id])
            .env("EDITOR", &editor),
    );
    assert_eq!(value["id"], id);

    let shown = run_json(sandbox.command().args(["--json", "show", &id]));
    assert!(
        shown["body"]
            .as_str()
            .expect("body")
            .contains("from the editor"),
        "edited body missing: {shown}"
    );
}
