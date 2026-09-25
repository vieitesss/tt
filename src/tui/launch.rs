//! Pre-App launch modal: the questions shown before the list when the
//! starting directory is unregistered or nested.
//!
//! The modal is a small state machine over [`LaunchState`]; [`super::mod`]
//! drives it until it is ready, then hands the resolved project to
//! [`super::app::App`]. Unregistered directories can bind to a different
//! already-registered project (`Projects` / `--projects`) instead of
//! registering cwd.

use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tt::resolve::Resolution;
use tt::{registry, Config, Project};

use super::app::expand_tilde;
use super::picker::{self, Picker, PickerKind};
use super::text::pop_word;

/// What the pre-App launch modal is asking, or what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchState {
    /// No question: open this project.
    Ready(Project),
    /// Ask whether to register the unregistered start directory.
    Register {
        /// Directory that would become a project.
        path: PathBuf,
    },
    /// Ask what to do with a directory nested inside a project.
    Nested {
        /// Parent project that currently owns the directory.
        project: Project,
        /// Nested start directory.
        dir: PathBuf,
    },
    /// Pick an already-registered project. `back` is the unregistered start
    /// directory to restore when Esc cancels; `None` means `--projects`, so
    /// Esc quits.
    Pick {
        /// Unregistered start directory to return to, if any.
        back: Option<PathBuf>,
        /// Highlight into the live match list.
        picker: Picker,
        /// Live filter query.
        query: String,
    },
    /// Register a directory that is not cwd. `back` is as in [`LaunchState::Pick`].
    RegisterDir {
        /// Unregistered start directory to return to, if any.
        back: Option<PathBuf>,
        /// Typed path.
        input: String,
    },
}

/// Pre-App launch modal shown before the list when the starting directory is
/// unregistered (offer to register it) or nested (offer to manage it as its
/// own project).
///
/// Left/Right (or `h`/`l`/Tab) move the highlighted button and Enter activates
/// it; Esc cancels. The `y`/`n`/`p` keys, and `!` for the nested question, stay
/// available as direct shortcuts that ignore the highlight.
pub(crate) struct Launch {
    config: Config,
    state: LaunchState,
    store_root: PathBuf,
    error: Option<String>,
    quit: bool,
    /// Index of the highlighted button into [`Launch::buttons`].
    button: usize,
}

impl Launch {
    /// Build the launch state from a resolution.
    pub(crate) fn new(config: Config, resolution: Resolution, store_root: PathBuf) -> Self {
        let state = match resolution {
            Resolution::Registered { project, nested } => match nested {
                Some(dir) if !project.never_ask_nested => LaunchState::Nested { project, dir },
                _ => LaunchState::Ready(project),
            },
            Resolution::Unregistered { path } => LaunchState::Register { path },
        };
        Self {
            config,
            state,
            store_root,
            error: None,
            quit: false,
            button: 0,
        }
    }

    /// Skip the register/nested questions and open the project picker (or the
    /// register-path prompt when the registry is empty). Esc quits.
    pub(crate) fn new_projects(
        config: Config,
        resolution: Resolution,
        store_root: PathBuf,
    ) -> Self {
        let current = resolution.project().cloned();
        let mut launch = Self::new(config, resolution, store_root);
        launch.enter_projects(None, current.as_ref());
        launch
    }

    /// Human-readable lines for the modal (empty when already ready or when a
    /// picker / path prompt has replaced the question).
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = match &self.state {
            LaunchState::Ready(_) | LaunchState::Pick { .. } | LaunchState::RegisterDir { .. } => {
                Vec::new()
            }
            LaunchState::Register { path } => vec![format!(
                "{} is not a registered project. Register it with tt?",
                path.display()
            )],
            LaunchState::Nested { project, dir } => vec![
                format!("{} is inside project {}.", dir.display(), project.slug),
                "Manage it as a separate project?".to_owned(),
                format!("! = never ask again inside {}", project.slug),
            ],
        };
        if let Some(error) = &self.error {
            if !matches!(
                self.state,
                LaunchState::Pick { .. } | LaunchState::RegisterDir { .. }
            ) {
                lines.push(format!("error: {error}"));
            }
        }
        lines
    }

    /// Labels of the buttons for the active question, in display order; empty
    /// when no question is open.
    pub(crate) fn buttons(&self) -> &'static [&'static str] {
        match &self.state {
            LaunchState::Ready(_) | LaunchState::Pick { .. } | LaunchState::RegisterDir { .. } => {
                &[]
            }
            LaunchState::Register { .. } => &["Yes", "Projects", "Cancel"],
            LaunchState::Nested { .. } => &["Register parent", "Register separately", "Cancel"],
        }
    }

    /// Whether the register question uses key-box buttons (`[y] Yes`) rather
    /// than `[ label ]`.
    pub(crate) fn key_box_buttons(&self) -> bool {
        matches!(self.state, LaunchState::Register { .. })
    }

    /// Index of the highlighted button.
    pub(crate) fn button_index(&self) -> usize {
        self.button
    }

    /// Live project picker, when that overlay is open.
    pub(crate) fn project_pick(&self) -> Option<(&str, usize, Vec<Project>)> {
        let LaunchState::Pick { query, picker, .. } = &self.state else {
            return None;
        };
        Some((
            query.as_str(),
            picker.highlight,
            picker::project_matches(query, &self.config.projects, self.path_display()),
        ))
    }

    /// Typed path in the empty-registry register prompt.
    pub(crate) fn path_input(&self) -> Option<&str> {
        match &self.state {
            LaunchState::RegisterDir { input, .. } => Some(input.as_str()),
            _ => None,
        }
    }

    /// Overlay error (picker / path prompt), if any.
    pub(crate) fn overlay_error(&self) -> Option<&str> {
        match &self.state {
            LaunchState::Pick { .. } | LaunchState::RegisterDir { .. } => self.error.as_deref(),
            _ => None,
        }
    }

    /// Path display settings from the loaded config.
    pub(crate) fn path_display(&self) -> &tt::PathDisplay {
        &self.config.path_display
    }

    /// Whether a project is resolved and the app can start.
    pub(crate) fn ready(&self) -> bool {
        matches!(self.state, LaunchState::Ready(_))
    }

    /// Whether the user declined and the run should end.
    pub(crate) fn should_quit(&self) -> bool {
        self.quit
    }

    /// Route one key press to the active launch question.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        match &self.state {
            LaunchState::Ready(_) => return,
            LaunchState::Pick { .. } => {
                self.handle_launch_pick(key);
                return;
            }
            LaunchState::RegisterDir { .. } => {
                self.handle_launch_path(key);
                return;
            }
            LaunchState::Register { .. } | LaunchState::Nested { .. } => {}
        }
        // Button navigation and activation are shared by every question and
        // take precedence over the direct shortcut keys.
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                self.move_button(-1);
                return;
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.move_button(1);
                return;
            }
            KeyCode::Enter => {
                self.activate_button();
                return;
            }
            _ => {}
        }
        match &self.state {
            LaunchState::Ready(_) | LaunchState::Pick { .. } | LaunchState::RegisterDir { .. } => {}
            LaunchState::Register { path } => {
                let path = path.clone();
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => self.register(path),
                    KeyCode::Char('p') | KeyCode::Char('P') => {
                        self.enter_projects(Some(path), None)
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.quit = true,
                    _ => {}
                }
            }
            LaunchState::Nested { project, dir } => {
                let project = project.clone();
                let dir = dir.clone();
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => self.register(dir),
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        self.state = LaunchState::Ready(project);
                    }
                    KeyCode::Char('!') => self.never_ask(&project),
                    // Cancelling the nested question ends the run; `n` above
                    // still keeps the parent as before.
                    KeyCode::Esc => self.quit = true,
                    _ => {}
                }
            }
        }
    }

    /// Move the button highlight by `delta`, wrapping around the choices.
    fn move_button(&mut self, delta: isize) {
        let count = self.buttons().len();
        if count == 0 {
            return;
        }
        self.button = (self.button as isize + delta).rem_euclid(count as isize) as usize;
    }

    /// Run the highlighted button.
    fn activate_button(&mut self) {
        match &self.state {
            LaunchState::Ready(_) | LaunchState::Pick { .. } | LaunchState::RegisterDir { .. } => {}
            LaunchState::Register { path } => {
                let path = path.clone();
                match self.button {
                    0 => self.register(path),
                    1 => self.enter_projects(Some(path), None),
                    _ => self.quit = true,
                }
            }
            LaunchState::Nested { project, dir } => {
                let project = project.clone();
                let dir = dir.clone();
                match self.button {
                    0 => self.state = LaunchState::Ready(project),
                    1 => self.register(dir),
                    _ => self.quit = true,
                }
            }
        }
    }

    /// Consume the modal, returning the config and the resolved project.
    pub(crate) fn into_parts(self) -> (Config, Project) {
        let LaunchState::Ready(project) = self.state else {
            unreachable!("launch is only consumed once a project is ready");
        };
        (self.config, project)
    }

    /// Open the project picker, or the register-path prompt when none exist.
    fn enter_projects(&mut self, back: Option<PathBuf>, current: Option<&Project>) {
        self.error = None;
        if self.config.projects.is_empty() {
            self.state = LaunchState::RegisterDir {
                back,
                input: String::new(),
            };
            return;
        }
        let matches = picker::project_matches("", &self.config.projects, &self.config.path_display);
        let highlight = current
            .and_then(|current| {
                matches
                    .iter()
                    .position(|project| project.slug == current.slug)
            })
            .unwrap_or(0);
        self.state = LaunchState::Pick {
            back,
            picker: Picker {
                kind: PickerKind::Project,
                highlight,
            },
            query: String::new(),
        };
    }

    fn handle_launch_pick(&mut self, key: KeyEvent) {
        let count = match &self.state {
            LaunchState::Pick { query, .. } => {
                picker::project_matches(query, &self.config.projects, &self.config.path_display)
                    .len()
            }
            _ => return,
        };
        let action = match &mut self.state {
            LaunchState::Pick { query, picker, .. } => picker.handle_input(query, key, count),
            _ => return,
        };
        match action {
            picker::PickerInput::Cancel => self.leave_overlay(),
            picker::PickerInput::Commit => self.commit_pick(),
            picker::PickerInput::QueryChanged => self.error = None,
            picker::PickerInput::Continue => {}
        }
    }

    fn handle_launch_path(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.leave_overlay(),
            KeyCode::Enter => self.commit_path(),
            KeyCode::Backspace => {
                if let LaunchState::RegisterDir { input, .. } = &mut self.state {
                    input.pop();
                }
                self.error = None;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let LaunchState::RegisterDir { input, .. } = &mut self.state {
                    pop_word(input);
                }
                self.error = None;
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let LaunchState::RegisterDir { input, .. } = &mut self.state {
                    input.push(character);
                }
                self.error = None;
            }
            _ => {}
        }
    }

    fn commit_pick(&mut self) {
        let LaunchState::Pick { query, picker, .. } = &self.state else {
            return;
        };
        let matches =
            picker::project_matches(query, &self.config.projects, &self.config.path_display);
        if matches.is_empty() {
            self.error = Some("no matching projects".to_owned());
            return;
        }
        let project = matches[picker.highlight.min(matches.len() - 1)].clone();
        self.state = LaunchState::Ready(project);
        self.error = None;
    }

    fn commit_path(&mut self) {
        let LaunchState::RegisterDir { input, .. } = &self.state else {
            return;
        };
        let Some(path) = expand_tilde(input.trim()) else {
            self.error = Some("cannot expand ~: HOME is not set".to_owned());
            return;
        };
        if path.as_os_str().is_empty() {
            self.error = Some("path cannot be empty".to_owned());
            return;
        }
        if let Err(error) = fs::create_dir_all(&path) {
            self.error = Some(format!("{error}"));
            return;
        }
        match registry::register_and_save_in(&mut self.config, &path, &self.store_root) {
            Ok(project) => {
                self.state = LaunchState::Ready(project);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn leave_overlay(&mut self) {
        let back = match &self.state {
            LaunchState::Pick { back, .. } | LaunchState::RegisterDir { back, .. } => back.clone(),
            _ => return,
        };
        match back {
            Some(path) => {
                self.state = LaunchState::Register { path };
                self.button = 0;
                self.error = None;
            }
            None => self.quit = true,
        }
    }

    /// Register `path` and make it the resolved project.
    fn register(&mut self, path: PathBuf) {
        let outcome = registry::register_and_save_in(&mut self.config, &path, &self.store_root);
        match outcome {
            Ok(project) => {
                self.state = LaunchState::Ready(project);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    /// Persist the never-ask rule for `project` and keep using it.
    fn never_ask(&mut self, project: &Project) {
        if let Err(error) =
            registry::set_never_ask(&mut self.config, project).context("saving the config")
        {
            self.error = Some(format!("{error:#}"));
            return;
        }
        self.state = LaunchState::Ready(project.clone());
    }
}
