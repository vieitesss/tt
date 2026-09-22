//! CLI implementation: argument definitions and command handlers.
//!
//! The CLI is a thin client of the `tt` library: every mutation goes through
//! [`tt::Vault`], so it shares its write path with the TUI. Machine output
//! (`--json`) is a public contract; see `SKILL.md`.
//!
//! Projects resolve through the central registry: `--path` > `$TT_PATH` >
//! cwd, nearest registered ancestor wins. `--vault` (and `$TT_VAULT`) remain
//! as a hidden escape hatch that opens a folder directly and skips the
//! registry. `--projects` opens the TUI project picker without registering
//! cwd and is only valid without a subcommand.

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::Cli;
use tt::config::env_vault;
use tt::registry;
use tt::resolve::{self, Resolution};
use tt::{
    Config, NewTask, Priority, Project, Task, TaskFilter, TaskId, TaskState, TreeNode, Vault,
};

/// Contextual add target for quick capture.
const STDIN_BODY: &str = "-";

/// Error for commands that need a registered project and do not get one.
const UNREGISTERED: &str = "not a registered project; run tt add or tt project add";

/// Subcommands of `tt`.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Create a task.
    Add(AddArgs),
    /// List tasks as a tree or a flat list.
    List(ListArgs),
    /// Show one task in detail.
    Show(IdArgs),
    /// Mark a task done.
    Done(IdArgs),
    /// Mark a task cancelled.
    Cancel(IdArgs),
    /// Reopen a done or cancelled task.
    Reopen(IdArgs),
    /// Rename a task, or open its file in `$EDITOR`.
    Edit(EditArgs),
    /// List, add, or remove registered projects.
    Project(ProjectArgs),
}

/// `tt add`
#[derive(Debug, Args)]
pub(crate) struct AddArgs {
    /// Task title (alternative to `--title`).
    #[arg(value_name = "TITLE")]
    positional_title: Option<String>,

    /// Task title.
    #[arg(long, value_name = "TITLE", conflicts_with = "positional_title")]
    title: Option<String>,

    /// Parent task id; defaults to the configured capture target, else root.
    #[arg(long, value_name = "ID")]
    parent: Option<String>,

    /// Tag; repeat for multiple tags. A leading `#` is optional.
    #[arg(long = "tag", value_name = "TAG")]
    tag: Vec<String>,

    /// Due date.
    #[arg(long, value_name = "YYYY-MM-DD")]
    due: Option<String>,

    /// Priority.
    #[arg(long, value_enum, value_name = "high|med|low")]
    priority: Option<PriorityArg>,

    /// Body markdown, or `-` to read it from stdin.
    #[arg(long, value_name = "TEXT")]
    body: Option<String>,
}

/// `tt list`
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Flat list instead of a tree.
    #[arg(long)]
    flat: bool,

    /// Only tasks carrying this tag (nested tags match).
    #[arg(long, value_name = "TAG")]
    tag: Option<String>,

    /// Only tasks in this state.
    #[arg(long, value_enum, value_name = "open|done|cancelled")]
    state: Option<StateArg>,

    /// Only tasks due today or earlier.
    #[arg(long)]
    due_today: bool,
}

/// `tt show|done|cancel|reopen <ID>`
#[derive(Debug, Args)]
pub(crate) struct IdArgs {
    /// Task id.
    #[arg(value_name = "ID")]
    id: String,
}

/// `tt edit <ID>`
#[derive(Debug, Args)]
pub(crate) struct EditArgs {
    /// Task id.
    #[arg(value_name = "ID")]
    id: String,

    /// New title; omit to open the task file in `$EDITOR`.
    #[arg(long, value_name = "TITLE")]
    title: Option<String>,
}

/// `tt project`
#[derive(Debug, Args)]
pub(crate) struct ProjectArgs {
    #[command(subcommand)]
    command: ProjectCommand,
}

/// Subcommands of `tt project`.
#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// List registered projects.
    List,
    /// Register a directory as a project.
    Add(ProjectPathArgs),
    /// Unregister a project (by directory or slug).
    Remove(ProjectPathArgs),
}

/// `tt project add|remove [DIR]`
#[derive(Debug, Args)]
struct ProjectPathArgs {
    /// Directory (add) or directory/slug (remove).
    #[arg(value_name = "DIR")]
    path: Option<PathBuf>,
}

/// CLI-facing priority values.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum PriorityArg {
    High,
    Med,
    Low,
}

impl From<PriorityArg> for Priority {
    fn from(value: PriorityArg) -> Self {
        match value {
            PriorityArg::High => Self::High,
            PriorityArg::Med => Self::Med,
            PriorityArg::Low => Self::Low,
        }
    }
}

/// CLI-facing state values.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum StateArg {
    Open,
    Done,
    Cancelled,
}

impl From<StateArg> for TaskState {
    fn from(value: StateArg) -> Self {
        match value {
            StateArg::Open => Self::Open,
            StateArg::Done => Self::Done,
            StateArg::Cancelled => Self::Cancelled,
        }
    }
}

/// How a command treats an unregistered or nested starting directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartPolicy {
    /// `tt add` may register a new project, prompting when interactive.
    MayRegister,
    /// Read-only commands never prompt and fail on an unregistered start.
    ReadOnly,
}

/// Answer to the nested-directory prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NestedAnswer {
    /// Manage the nested directory as its own project.
    Separate,
    /// Keep using the parent project.
    Parent,
    /// Keep using the parent project and stop asking for its nested dirs.
    NeverAsk,
}

/// Run the CLI, mapping errors to the documented output and exit codes.
pub(crate) fn run(cli: &Cli) -> ExitCode {
    match execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if cli.json {
                println!("{}", serde_json::json!({ "error": format!("{error:#}") }));
            } else {
                eprintln!("error: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}

fn execute(cli: &Cli) -> Result<()> {
    if cli.projects && cli.command.is_some() {
        bail!("--projects is only valid without a subcommand");
    }

    let mut config = Config::load().context("loading config")?;
    if config.deprecated_vault.is_some() {
        eprintln!(
            "warning: the `vault` config key is deprecated and ignored; use `tt project add` to register projects"
        );
    }

    if let Some(Command::Project(args)) = cli.command.as_ref() {
        return project_command(cli, &mut config, args);
    }

    // Bare `tt` opens the TUI, which owns resolution so it can show the
    // launch modals for unregistered and nested directories.
    if cli.command.is_none() {
        return run_tui(cli, config);
    }

    let policy = if matches!(cli.command, Some(Command::Add(_))) {
        StartPolicy::MayRegister
    } else {
        StartPolicy::ReadOnly
    };
    let mut vault = open_vault(cli, &mut config, policy)?;

    // Skipped files are reported, never silently ignored; stderr keeps stdout
    // valid JSON.
    for issue in vault.issues() {
        eprintln!("warning: {issue}");
    }

    let json = cli.json;
    match cli.command.as_ref() {
        Some(Command::Add(args)) => add(&mut vault, &config, args, json),
        Some(Command::List(args)) => list(&vault, args, json),
        Some(Command::Show(args)) => show(&vault, args, json),
        Some(Command::Done(args)) => set_state(&mut vault, args, TaskState::Done, json),
        Some(Command::Cancel(args)) => set_state(&mut vault, args, TaskState::Cancelled, json),
        Some(Command::Reopen(args)) => set_state(&mut vault, args, TaskState::Open, json),
        Some(Command::Edit(args)) => edit(&mut vault, args, json),
        Some(Command::Project(_)) | None => {
            unreachable!("the TUI and project commands are handled before opening a vault")
        }
    }
}

/// Start the TUI: the `--vault`/`$TT_VAULT` hatch runs the vault directly,
/// otherwise the start directory resolves against the registry and the TUI
/// shows launch modals as needed. `--projects` skips those questions and
/// opens the project picker.
fn run_tui(cli: &Cli, config: Config) -> Result<()> {
    if let Some(path) = cli.vault.as_deref() {
        let vault =
            Vault::open(path).with_context(|| format!("opening vault {}", path.display()))?;
        return crate::tui::run_vault(vault, config);
    }
    if let Some(path) = env_vault() {
        let vault =
            Vault::open(&path).with_context(|| format!("opening vault {}", path.display()))?;
        return crate::tui::run_vault(vault, config);
    }
    let start =
        resolve::start_dir(cli.path.as_deref()).context("determining the starting directory")?;
    let resolution = resolve::resolve(&config, &start);
    if cli.projects {
        crate::tui::run_projects_picker(config, resolution)
    } else {
        crate::tui::run_project(config, resolution)
    }
}

/// Open the vault for this invocation.
///
/// `--vault` and `$TT_VAULT` win and skip the registry entirely. Otherwise the
/// starting directory (`--path`, `$TT_PATH`, cwd) resolves against the
/// registry: the nearest registered ancestor wins, `MayRegister` policies may
/// register an unregistered start (silently when non-interactive, prompting
/// when interactive), and read-only policies fail with [`UNREGISTERED`].
/// Nested directories keep resolving to the parent project unless an
/// interactive `tt add` chooses otherwise.
fn open_vault(cli: &Cli, config: &mut Config, policy: StartPolicy) -> Result<Vault> {
    if let Some(path) = cli.vault.as_deref() {
        return Vault::open(path).with_context(|| format!("opening vault {}", path.display()));
    }
    if let Some(path) = env_vault() {
        return Vault::open(&path).with_context(|| format!("opening vault {}", path.display()));
    }

    let start =
        resolve::start_dir(cli.path.as_deref()).context("determining the starting directory")?;
    match resolve::resolve(config, &start) {
        Resolution::Registered { project, nested } => {
            let project = match nested {
                Some(nested_dir)
                    if policy == StartPolicy::MayRegister
                        && !project.never_ask_nested
                        && is_interactive(cli) =>
                {
                    match ask_nested(&project, &nested_dir)? {
                        NestedAnswer::Separate => registry::register_and_save(config, &nested_dir)?,
                        NestedAnswer::Parent => project,
                        NestedAnswer::NeverAsk => {
                            set_never_ask(config, &project)?;
                            project
                        }
                    }
                }
                _ => project,
            };
            open_store(&project)
        }
        Resolution::Unregistered { path } => {
            if policy == StartPolicy::ReadOnly {
                bail!(UNREGISTERED);
            }
            let project = if is_interactive(cli) {
                if ask_register(&path)? {
                    register_project(config, &path)?
                } else {
                    bail!(UNREGISTERED);
                }
            } else {
                register_project(config, &path)?
            };
            open_store(&project)
        }
    }
}

/// Register `path` and persist the config.
fn register_project(config: &mut Config, path: &Path) -> Result<Project> {
    Ok(registry::register_and_save(config, path)?)
}

/// Open the invisible store folder for `project`.
fn open_store(project: &Project) -> Result<Vault> {
    let data = registry::data_dir().context("cannot determine the data directory")?;
    let store = registry::store_path(&data, &project.slug);
    registry::open_store(&data, project)
        .with_context(|| format!("opening the project store {}", store.display()))
}

/// Persist the "never ask again for nested dirs" rule for `project`.
fn set_never_ask(config: &mut Config, project: &Project) -> Result<()> {
    registry::set_never_ask(config, project).context("saving the config")
}

/// Whether prompts are allowed for this invocation.
fn is_interactive(cli: &Cli) -> bool {
    !cli.json && io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Ask a question on stderr and return the raw answer.
fn ask(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush().context("flushing the prompt")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("reading the answer")?;
    Ok(answer)
}

/// Ask whether to register an unregistered directory as a new project.
fn ask_register(path: &Path) -> Result<bool> {
    let answer = ask(&format!(
        "register {} as a tt project? [y/N] ",
        path.display()
    ))?;
    Ok(parse_register_answer(&answer))
}

/// Ask whether a nested directory becomes its own project.
fn ask_nested(project: &Project, nested_dir: &Path) -> Result<NestedAnswer> {
    let answer = ask(&format!(
        "{} is inside project {}; manage it as a separate project? [y/N/!] (! = never ask inside {}) ",
        nested_dir.display(),
        project.slug,
        project.slug
    ))?;
    Ok(parse_nested_answer(&answer))
}

/// `y`/`yes` (case-insensitive) registers; anything else declines.
fn parse_register_answer(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// `y`/`yes` = separate, `!`/`never` = never ask, anything else = parent.
fn parse_nested_answer(raw: &str) -> NestedAnswer {
    match raw.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => NestedAnswer::Separate,
        "!" | "never" => NestedAnswer::NeverAsk,
        _ => NestedAnswer::Parent,
    }
}

/// `tt project list|add|remove`
fn project_command(cli: &Cli, config: &mut Config, args: &ProjectArgs) -> Result<()> {
    match &args.command {
        ProjectCommand::List => {
            if cli.json {
                let payload = ProjectsJson {
                    projects: config.projects.iter().map(project_json).collect(),
                };
                print_json(&payload)
            } else {
                for project in &config.projects {
                    println!("{}\t{}", project.slug, project.path.display());
                }
                Ok(())
            }
        }
        ProjectCommand::Add(path_args) => {
            let path = match &path_args.path {
                Some(path) => path.clone(),
                None => resolve::start_dir(cli.path.as_deref())
                    .context("determining the starting directory")?,
            };
            if !path.is_dir() {
                bail!("project directory does not exist: {}", path.display());
            }
            let project = register_project(config, &path)?;
            if cli.json {
                print_json(&ProjectRefJson {
                    path: project.path.to_string_lossy().into_owned(),
                    slug: project.slug,
                })
            } else {
                println!("registered {}  {}", project.slug, project.path.display());
                Ok(())
            }
        }
        ProjectCommand::Remove(path_args) => {
            let Some(raw) = &path_args.path else {
                bail!("a directory or slug is required");
            };
            let removed = registry::remove(config, raw)
                .or_else(|| registry::remove_slug(config, raw.to_string_lossy().as_ref()));
            let Some(project) = removed else {
                bail!("not a registered project: {}", raw.display());
            };
            config.save().context("saving the config")?;
            if cli.json {
                print_json(&RemovedJson {
                    removed: project_json(&project),
                })
            } else {
                println!("unregistered {}  {}", project.slug, project.path.display());
                Ok(())
            }
        }
    }
}

fn add(vault: &mut Vault, config: &Config, args: &AddArgs, json: bool) -> Result<()> {
    let title = args
        .title
        .as_ref()
        .or(args.positional_title.as_ref())
        .context("a title is required: pass `--title` or a positional title")?
        .clone();

    let parent = match args.parent.as_deref() {
        Some(raw) => Some(parse_id(raw)?),
        None => config.capture_target.clone(),
    };

    let due = match args.due.as_deref() {
        Some(raw) => Some(parse_date(raw)?),
        None => None,
    };

    let body = match args.body.as_deref() {
        Some(STDIN_BODY) => read_stdin()?,
        Some(text) => text.to_owned(),
        None => String::new(),
    };

    let task = vault
        .add(NewTask {
            title,
            parent,
            insert_after: None,
            tags: args.tag.clone(),
            due,
            priority: args.priority.map(Priority::from),
            body,
        })
        .context("adding the task")?;

    print_task(&task, json, "added")
}

fn list(vault: &Vault, args: &ListArgs, json: bool) -> Result<()> {
    let filter = TaskFilter {
        tag: args.tag.clone(),
        state: args.state.map(TaskState::from),
        priority: None,
        due_today: args.due_today,
    };
    let today = Local::now().date_naive();

    if args.flat {
        let tasks = vault.filter(&filter, today);
        if json {
            let payload = FlatJson {
                tasks: tasks.iter().map(|task| TaskJson::new(task)).collect(),
            };
            print_json(&payload)
        } else {
            for task in tasks {
                println!("{}", human_line(task));
            }
            Ok(())
        }
    } else {
        let tree = vault.filter_tree(&filter, today);
        if json {
            let payload = TreeJson {
                tasks: tree.iter().map(TaskNodeJson::from_tree).collect(),
            };
            print_json(&payload)
        } else {
            for node in &tree {
                print_tree(node, 0);
            }
            Ok(())
        }
    }
}

fn show(vault: &Vault, args: &IdArgs, json: bool) -> Result<()> {
    let id = parse_id(&args.id)?;
    let task = vault
        .get(&id)
        .with_context(|| format!("task not found: {id}"))?;

    if json {
        let payload = ShowJson {
            task: TaskJson::new(task),
            body: &task.body,
            links: id_strings(vault.links(&id)),
            backlinks: id_strings(vault.backlinks(&id)),
            children: id_strings(vault.children(&id)),
        };
        print_json(&payload)
    } else {
        println!("{}", human_show(vault, task));
        Ok(())
    }
}

fn set_state(vault: &mut Vault, args: &IdArgs, state: TaskState, json: bool) -> Result<()> {
    let id = parse_id(&args.id)?;
    let task = vault.set_state(&id, state)?;
    print_task(&task, json, state.as_str())
}

fn edit(vault: &mut Vault, args: &EditArgs, json: bool) -> Result<()> {
    let id = parse_id(&args.id)?;

    // `--title` keeps the agent-first rename contract.
    if let Some(title) = &args.title {
        let task = vault.set_title(&id, title)?;
        return print_task(&task, json, "edited");
    }

    let task_path = vault
        .get(&id)
        .map(|_| vault.root().join(format!("{id}.md")))
        .with_context(|| format!("task not found: {id}"))?;
    crate::editor::open(&task_path)?;

    // The editor may have changed anything; rescan before answering.
    vault.reload();
    let task = vault
        .get(&id)
        .with_context(|| format!("task not found after editing: {id}"))?;
    print_task(task, json, "edited")
}

fn parse_id(raw: &str) -> Result<TaskId> {
    TaskId::parse(raw).with_context(|| format!("invalid task id: {raw}"))
}

fn parse_date(raw: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .with_context(|| format!("invalid due date {raw:?}, expected YYYY-MM-DD"))
}

fn read_stdin() -> Result<String> {
    let mut buffer = String::new();
    std::io::stdin()
        .read_to_string(&mut buffer)
        .context("reading body from stdin")?;
    Ok(buffer)
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(value).context("serializing JSON")?
    );
    Ok(())
}

fn print_task(task: &Task, json: bool, human_prefix: &str) -> Result<()> {
    if json {
        print_json(&TaskJson::new(task))
    } else {
        println!("{human_prefix} {}  {}", task.id, task.title);
        Ok(())
    }
}

fn print_tree(node: &TreeNode<'_>, depth: usize) {
    println!(
        "{:indent$}{}",
        "",
        human_line(node.task),
        indent = depth * 2
    );
    for child in &node.children {
        print_tree(child, depth + 1);
    }
}

fn human_line(task: &Task) -> String {
    format!("{} {}  {}", state_marker(task.state), task.id, task.title)
}

fn state_marker(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "[ ]",
        TaskState::Done => "[x]",
        TaskState::Cancelled => "[-]",
    }
}

fn human_show(vault: &Vault, task: &Task) -> String {
    let mut lines = vec![
        task.title.clone(),
        format!("id: {}", task.id),
        format!("state: {}", task.state),
        format!("parent: {}", describe_one(vault, task.parent.as_ref())),
        format!(
            "tags: {}",
            if task.tags.is_empty() {
                "-".to_owned()
            } else {
                task.tags.join(", ")
            }
        ),
        format!(
            "due: {}",
            task.due
                .map_or_else(|| "-".to_owned(), |due| due.to_string())
        ),
        format!(
            "priority: {}",
            task.priority
                .map_or_else(|| "-".to_owned(), |priority| priority.to_string())
        ),
        format!(
            "children: {}",
            describe_ids(vault, vault.children(&task.id))
        ),
        format!("links: {}", describe_ids(vault, vault.links(&task.id))),
        format!(
            "backlinks: {}",
            describe_ids(vault, vault.backlinks(&task.id))
        ),
    ];

    if !task.body.trim().is_empty() {
        lines.push(String::new());
        lines.push(task.body.trim_end().to_owned());
    }
    lines.join("\n")
}

fn describe_one(vault: &Vault, id: Option<&TaskId>) -> String {
    id.map_or_else(
        || "-".to_owned(),
        |id| describe_ids(vault, std::slice::from_ref(id)),
    )
}

fn describe_ids(vault: &Vault, ids: &[TaskId]) -> String {
    if ids.is_empty() {
        return "-".to_owned();
    }
    ids.iter()
        .map(|id| match vault.get(id) {
            Some(task) => format!("{} ({id})", task.title),
            None => id.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn id_strings(ids: &[TaskId]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

fn project_json(project: &Project) -> ProjectJson {
    ProjectJson {
        path: project.path.to_string_lossy().into_owned(),
        slug: project.slug.clone(),
        never_ask_nested: project.never_ask_nested,
    }
}

/// Task object in the JSON contract: the frontmatter fields, no body.
#[derive(Debug, Serialize)]
struct TaskJson {
    id: String,
    title: String,
    state: &'static str,
    parent: Option<String>,
    tags: Vec<String>,
    due: Option<String>,
    priority: Option<&'static str>,
    rank: Option<i32>,
}

impl TaskJson {
    fn new(task: &Task) -> Self {
        Self {
            id: task.id.to_string(),
            title: task.title.clone(),
            state: task.state.as_str(),
            parent: task.parent.as_ref().map(ToString::to_string),
            tags: task.tags.clone(),
            due: task.due.map(|due| due.format("%Y-%m-%d").to_string()),
            priority: task.priority.map(Priority::as_str),
            rank: task.rank,
        }
    }
}

#[derive(Debug, Serialize)]
struct TreeJson {
    tasks: Vec<TaskNodeJson>,
}

#[derive(Debug, Serialize)]
struct TaskNodeJson {
    #[serde(flatten)]
    task: TaskJson,
    children: Vec<TaskNodeJson>,
}

impl TaskNodeJson {
    fn from_tree(node: &TreeNode<'_>) -> Self {
        Self {
            task: TaskJson::new(node.task),
            children: node.children.iter().map(Self::from_tree).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct FlatJson {
    tasks: Vec<TaskJson>,
}

#[derive(Debug, Serialize)]
struct ShowJson<'a> {
    #[serde(flatten)]
    task: TaskJson,
    body: &'a str,
    links: Vec<String>,
    backlinks: Vec<String>,
    children: Vec<String>,
}

/// One registered project in `tt project list`.
#[derive(Debug, Serialize)]
struct ProjectJson {
    path: String,
    slug: String,
    never_ask_nested: bool,
}

/// `tt project list` payload.
#[derive(Debug, Serialize)]
struct ProjectsJson {
    projects: Vec<ProjectJson>,
}

/// `tt project add` payload.
#[derive(Debug, Serialize)]
struct ProjectRefJson {
    path: String,
    slug: String,
}

/// `tt project remove` payload.
#[derive(Debug, Serialize)]
struct RemovedJson {
    removed: ProjectJson,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_answer_accepts_only_yes() {
        assert!(parse_register_answer("y"));
        assert!(parse_register_answer(" YES "));
        assert!(!parse_register_answer("n"));
        assert!(!parse_register_answer(""));
    }

    #[test]
    fn nested_answer_maps_three_choices() {
        assert_eq!(parse_nested_answer("y"), NestedAnswer::Separate);
        assert_eq!(parse_nested_answer("YES"), NestedAnswer::Separate);
        assert_eq!(parse_nested_answer("!"), NestedAnswer::NeverAsk);
        assert_eq!(parse_nested_answer("never"), NestedAnswer::NeverAsk);
        assert_eq!(parse_nested_answer("n"), NestedAnswer::Parent);
        assert_eq!(parse_nested_answer(""), NestedAnswer::Parent);
    }
}
