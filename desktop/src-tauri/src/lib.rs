//! TT Desktop backend: the Tauri shell around [`service::TtService`].
//!
//! The shell only translates IPC calls into service calls, runs native dialogs
//! and clipboard access, and keeps the store work off the main thread. All
//! product logic lives in [`service`], which is Tauri-free and unit tested.
//!
//! Two windows coexist: the main window and the menu bar popover. Each owns an
//! independent [`TtService`], so their open Projects never fight.

pub mod commands;
pub mod menubar;
pub mod service;

use std::sync::{Mutex, MutexGuard};

use tauri::Manager;

use service::TtService;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

/// Which independent service instance a window label owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSlot {
    /// The main window's Project.
    Main,
    /// The menu bar popover's Project, deliberately independent of the main
    /// window's, with its own drafts and baseline revisions.
    Menubar,
}

/// Map a window label to its service slot. Total by construction: any label
/// that is not the popover shares the main window's service, so a future
/// window cannot end up without one.
pub fn service_slot(window_label: &str) -> ServiceSlot {
    if window_label == menubar::MENUBAR_LABEL {
        ServiceSlot::Menubar
    } else {
        ServiceSlot::Main
    }
}

/// Shared, serialized application state.
///
/// The service is synchronous and owns the in-memory [`tt::Vault`]; one mutex
/// per slot serializes every command that touches that slot, while the two
/// windows stay independent of each other.
pub struct AppState {
    main: Mutex<TtService>,
    menubar: Mutex<TtService>,
}

impl AppState {
    fn new() -> Self {
        Self {
            main: Mutex::new(TtService::from_env()),
            menubar: Mutex::new(TtService::from_env()),
        }
    }

    /// The service for a window label. Two calls for the same slot return the
    /// same instance; different slots never share state.
    pub fn service(&self, window_label: &str) -> &Mutex<TtService> {
        match service_slot(window_label) {
            ServiceSlot::Main => &self.main,
            ServiceSlot::Menubar => &self.menubar,
        }
    }

    /// Lock the slot for a window label, recovering from a poisoned mutex: a
    /// command that panicked mid-mutation must not disable the app.
    pub fn lock(&self, window_label: &str) -> MutexGuard<'_, TtService> {
        self.service(window_label)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Global shortcut that toggles the popover: ⌥⌘T.
fn toggle_shortcut() -> tauri_plugin_global_shortcut::Shortcut {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
    Shortcut::new(Some(Modifiers::ALT | Modifiers::SUPER), Code::KeyT)
}

/// Build and run the desktop application.
///
/// # Panics
///
/// Panics when Tauri cannot set up the event loop, which is unrecoverable.
pub fn run() {
    let shortcut = toggle_shortcut();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, pressed, event| {
                    if pressed == &shortcut
                        && event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed
                    {
                        // No tray rect on a key press: reuse the last click's.
                        // Showing also activates the app, which must happen on
                        // the main thread; `run_on_main_thread` runs inline when
                        // the hotkey is already delivered there.
                        let handle = app.clone();
                        let _ = app.run_on_main_thread(move || menubar::toggle(&handle, None));
                    }
                })
                .build(),
        )
        .setup(move |app| {
            app.manage(AppState::new());
            menubar::setup(app.handle())?;
            // ⌥⌘T is best effort: another app may already own the shortcut,
            // and that must not stop the app from launching with its tray icon.
            if let Err(error) = app.global_shortcut().register(shortcut) {
                eprintln!("tt-desktop: could not register the ⌥⌘T global shortcut: {error}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // The popover is transient: it hides as soon as it stops being the
            // focused window (clicking anywhere else, or another app).
            if window.label() == menubar::MENUBAR_LABEL
                && matches!(event, tauri::WindowEvent::Focused(false))
            {
                menubar::hide_after_focus_loss(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::open_project,
            commands::refresh,
            commands::register_project,
            commands::unregister_project,
            commands::task_detail,
            commands::save_body,
            commands::add_task,
            commands::set_state,
            commands::set_title,
            commands::set_tags,
            commands::set_priority,
            commands::set_due,
            commands::set_parent,
            commands::shift_rank,
            commands::move_to_project,
            commands::delete_count,
            commands::delete_task,
            commands::search,
            commands::copy_task,
            commands::copy_text,
            commands::open_external,
            commands::close_main_window,
            commands::hide_menubar,
            commands::set_tray_attention,
            commands::open_in_desktop,
            commands::open_desktop,
            commands::quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building TT Desktop")
        .run(|app, event| {
            // Dock icon click with no visible window: bring the main window
            // back instead of doing nothing.
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    menubar::show_main(app);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{service_slot, ServiceSlot};

    #[test]
    fn only_the_popover_label_gets_its_own_service_slot() {
        assert_eq!(service_slot("menubar"), ServiceSlot::Menubar);
        assert_eq!(service_slot("main"), ServiceSlot::Main);
        // An unknown label must never end up without a service.
        assert_eq!(service_slot("settings"), ServiceSlot::Main);
        assert_eq!(service_slot(""), ServiceSlot::Main);
    }
}
