//! Pre-App launch modal: the questions shown before the list when the
//! starting directory is unregistered or nested.
//!
//! The modal is a small state machine over [`LaunchState`]; [`super::mod`]
//! drives it until it is ready, then hands the resolved project to
//! [`super::app::App`].

use std::path::PathBuf;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent};
use tt::resolve::Resolution;
use tt::{registry, Config, Project};

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
}

/// Pre-App launch modal shown before the list when the starting directory is
/// unregistered (offer to register it) or nested (offer to manage it as its
/// own project).
///
/// Left/Right (or `h`/`l`/Tab) move the highlighted button and Enter activates
/// it; Esc cancels. The `y`/`n` keys, and `!` for the nested question, stay
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

    /// Human-readable lines for the modal (empty when already ready).
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = match &self.state {
            LaunchState::Ready(_) => Vec::new(),
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
            lines.push(format!("error: {error}"));
        }
        lines
    }

    /// Labels of the buttons for the active question, in display order; empty
    /// when no question is open.
    pub(crate) fn buttons(&self) -> &'static [&'static str] {
        match &self.state {
            LaunchState::Ready(_) => &[],
            LaunchState::Register { .. } => &["Yes", "Cancel"],
            LaunchState::Nested { .. } => &["Register parent", "Register separately", "Cancel"],
        }
    }

    /// Index of the highlighted button.
    pub(crate) fn button_index(&self) -> usize {
        self.button
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
        if self.buttons().is_empty() {
            return;
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
            LaunchState::Ready(_) => {}
            LaunchState::Register { path } => {
                let path = path.clone();
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => self.register(path),
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
            LaunchState::Ready(_) => {}
            LaunchState::Register { path } => {
                let path = path.clone();
                match self.button {
                    0 => self.register(path),
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
