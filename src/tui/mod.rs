//! TUI: the list view and the terminal event loop.
//!
//! The list is the only main view; a persistent preview pane follows its
//! selection. The TUI is a client of the public library API only; every
//! mutation goes through [`tt::Vault`], so it shares its write path with the
//! CLI. On external changes it reloads only when no edit buffer is open, per
//! the no-clobber contract documented on [`tt::VaultWatcher`].
//!
//! Two entry points: [`run_vault`] for the `--vault` escape hatch, and
//! [`run_project`] for a resolved start directory, which first shows the
//! launch modal for unregistered or nested directories. [`run_projects_picker`]
//! is `--projects`: skip those questions and pick a project.

mod app;
mod launch;
mod list;
mod markdown;
mod picker;
#[cfg(test)]
mod tests;
mod ui;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::DefaultTerminal;
use tt::{registry, resolve::Resolution, Config, Vault};

use app::{App, TICK};
use launch::Launch;

/// Run the TUI against an already opened vault (the `--vault` escape hatch).
///
/// # Errors
///
/// Returns an error when the terminal cannot be initialized or drawing fails.
pub(crate) fn run_vault(vault: Vault, config: Config) -> Result<()> {
    let store_root = registry::data_dir();
    let mut app = App::new(vault, config, None, store_root);
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    let loop_result = event_loop(&mut terminal, &mut app);
    let restore_result = ratatui::try_restore().context("restoring the terminal");
    loop_result.and(restore_result)
}

/// Run the TUI for a resolved start directory, showing the launch modal first
/// when the directory is unregistered or nested.
///
/// # Errors
///
/// Returns an error when the data directory cannot be determined, the
/// terminal cannot be initialized, or drawing fails.
pub(crate) fn run_project(config: Config, resolution: Resolution) -> Result<()> {
    run_project_from(config, resolution, false)
}

/// Run the TUI starting at the project picker (`tt --projects`).
///
/// # Errors
///
/// Returns an error when the data directory cannot be determined, the
/// terminal cannot be initialized, or drawing fails.
pub(crate) fn run_projects_picker(config: Config, resolution: Resolution) -> Result<()> {
    run_project_from(config, resolution, true)
}

fn run_project_from(config: Config, resolution: Resolution, projects: bool) -> Result<()> {
    let store_root = registry::data_dir().context("cannot determine the data directory")?;
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    let loop_result = run_project_inner(
        &mut terminal,
        config,
        resolution,
        store_root.clone(),
        projects,
    );
    let restore_result = ratatui::try_restore().context("restoring the terminal");
    loop_result.and(restore_result)
}

fn run_project_inner(
    terminal: &mut DefaultTerminal,
    config: Config,
    resolution: Resolution,
    store_root: PathBuf,
    projects: bool,
) -> Result<()> {
    let mut launch = if projects {
        Launch::new_projects(config, resolution, store_root.clone())
    } else {
        Launch::new(config, resolution, store_root.clone())
    };
    while !launch.ready() && !launch.should_quit() {
        terminal.draw(|frame| ui::render_launch(frame, &launch))?;
        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    launch.handle_key(key);
                }
            }
        }
    }
    if launch.should_quit() {
        return Ok(());
    }

    let (config, project) = launch.into_parts();
    let store = registry::store_path(&store_root, &project.slug);
    let vault = registry::open_store(&store_root, &project)
        .with_context(|| format!("opening the project store {}", store.display()))?;
    let mut app = App::new(vault, config, Some(project), Some(store_root));
    event_loop(terminal, &mut app)
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    sync_list_viewport(terminal, app)?;
    while !app.should_quit() {
        terminal.draw(|frame| ui::render(frame, app))?;
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                Event::Resize(_, _) => sync_list_viewport(terminal, app)?,
                _ => {}
            }
        }
        if let Some(path) = app.take_pending_edit() {
            edit_in_editor(terminal, app, &path)?;
        }
        app.on_tick();
    }
    Ok(())
}

/// Tell the app how big the list pane is, using the same layout `render`
/// draws, so scrolling clamps from events instead of during the draw.
fn sync_list_viewport(terminal: &DefaultTerminal, app: &mut App) -> Result<()> {
    let size = terminal.size().context("reading the terminal size")?;
    let list = ui::layout(Rect::new(0, 0, size.width, size.height)).list;
    app.set_list_viewport(list.width, list.height);
    Ok(())
}

/// Suspend the TUI, run `$EDITOR` on `path`, re-enter, and reload.
///
/// The editor owns the file; tt only rescans afterwards. Terminal restore and
/// re-init happen even when the editor fails, so the TUI is never left in raw
/// mode or on the alternate screen.
fn edit_in_editor(terminal: &mut DefaultTerminal, app: &mut App, path: &Path) -> Result<()> {
    ratatui::try_restore().context("suspending the TUI")?;
    let result = crate::editor::open(path);
    *terminal = ratatui::try_init().context("re-entering the TUI")?;
    app.reload_after_edit();
    if let Err(error) = result {
        app.set_toast(format!("editor error: {error:#}"));
    }
    Ok(())
}
