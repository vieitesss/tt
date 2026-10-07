//! Tauri command layer.
//!
//! Every command is `#[tauri::command(async)]` so blocking store scans and
//! native dialogs never run on the main thread. Commands only translate IPC
//! payloads and delegate to [`TtService`].
//!
//! Store commands take the calling `WebviewWindow` and operate on that
//! window's service slot, so the menu bar popover keeps a Project of its own
//! (see [`crate::AppState`]).

use std::sync::MutexGuard;

use tauri::{AppHandle, Emitter, State, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::menubar;
use crate::service::{ServiceError, SnapshotDto, TaskDetailDto, TtService};
use crate::AppState;

fn lock<'a>(state: &'a State<'_, AppState>, window: &WebviewWindow) -> MutexGuard<'a, TtService> {
    state.lock(window.label())
}

fn dialog_error(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::new("dialog", error.to_string())
}

/// Whether `url` uses one of the schemes the native opener may be asked to
/// open. Rust calls bypass capabilities, so this is the enforcement point.
pub(crate) fn is_allowed_external_url(url: &str) -> bool {
    url.trim()
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .is_some_and(|scheme| matches!(scheme.as_str(), "http" | "https" | "mailto"))
}

/// Load the registry without opening a Project.
///
/// # Errors
///
/// Returns a config error when the registry cannot be read.
#[tauri::command(async)]
pub fn bootstrap(
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).bootstrap()
}

/// Open a registered Project by slug.
///
/// # Errors
///
/// Returns `unknown_project` or an I/O error.
#[tauri::command(async)]
pub fn open_project(
    slug: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).open_project(&slug)
}

/// Re-read the registry and the open Store.
///
/// # Errors
///
/// Propagates config or store read failures.
#[tauri::command(async)]
pub fn refresh(
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).refresh()
}

/// Pick a folder with the native dialog and register it as a Project.
/// Returns `None` when the user cancels.
///
/// # Errors
///
/// Returns a dialog or I/O error.
#[tauri::command(async)]
pub fn register_project(
    app: AppHandle,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<Option<SnapshotDto>, ServiceError> {
    let Some(folder) = app
        .dialog()
        .file()
        .set_title("Choose a project folder")
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let path = folder.into_path().map_err(dialog_error)?;
    lock(&app_state, &window).register_project(&path).map(Some)
}

/// Unregister a Project, keeping its Store files.
///
/// # Errors
///
/// Returns a config error when the registry cannot be saved.
#[tauri::command(async)]
pub fn unregister_project(
    slug: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).unregister_project(&slug)
}

/// Full details for one task.
///
/// # Errors
///
/// Returns `no_project` or `not_found`.
#[tauri::command(async)]
pub fn task_detail(
    id: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<TaskDetailDto, ServiceError> {
    lock(&app_state, &window).task_detail(&id)
}

/// Save a description draft against its baseline revision.
///
/// # Errors
///
/// Returns `stale_draft` when the file changed or vanished.
#[tauri::command(async)]
pub fn save_body(
    id: String,
    body: String,
    base_revision: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<TaskDetailDto, ServiceError> {
    lock(&app_state, &window).save_body(&id, &body, &base_revision)
}

/// Create a root / sub / capture task.
///
/// # Errors
///
/// Propagates core validation failures.
#[tauri::command(async)]
pub fn add_task(
    title: String,
    parent_id: Option<String>,
    capture: bool,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).add_task(&title, parent_id.as_deref(), capture)
}

/// Set a task's lifecycle state.
///
/// # Errors
///
/// Propagates invalid state and I/O failures.
#[tauri::command(async)]
pub fn set_state(
    id: String,
    state: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).set_state(&id, &state)
}

/// Rename a task.
///
/// # Errors
///
/// Propagates core validation failures.
#[tauri::command(async)]
pub fn set_title(
    id: String,
    title: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).set_title(&id, &title)
}

/// Replace a task's tags.
///
/// # Errors
///
/// Propagates core validation failures.
#[tauri::command(async)]
pub fn set_tags(
    id: String,
    tags: Vec<String>,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).set_tags(&id, tags)
}

/// Set or clear a task's priority.
///
/// # Errors
///
/// Propagates invalid priority and I/O failures.
#[tauri::command(async)]
pub fn set_priority(
    id: String,
    priority: Option<String>,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).set_priority(&id, priority.as_deref())
}

/// Set or clear a task's due date.
///
/// # Errors
///
/// Propagates invalid dates and I/O failures.
#[tauri::command(async)]
pub fn set_due(
    id: String,
    due: Option<String>,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).set_due(&id, due.as_deref())
}

/// Reparent a task inside its Project.
///
/// # Errors
///
/// Propagates invalid parent and I/O failures.
#[tauri::command(async)]
pub fn set_parent(
    id: String,
    parent_id: Option<String>,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).set_parent(&id, parent_id.as_deref())
}

/// Move a task one place earlier (`delta < 0`) or later among siblings.
///
/// # Errors
///
/// Propagates core validation failures.
#[tauri::command(async)]
pub fn shift_rank(
    id: String,
    delta: i32,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).shift_rank(&id, delta)
}

/// Move a task subtree to another Project and switch to it.
///
/// # Errors
///
/// Propagates `unknown_project` and core move failures.
#[tauri::command(async)]
pub fn move_to_project(
    id: String,
    target_slug: String,
    parent_id: Option<String>,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).move_to_project(&id, &target_slug, parent_id.as_deref())
}

/// Count the descendants a delete would remove.
///
/// # Errors
///
/// Returns `no_project` or `not_found`.
#[tauri::command(async)]
pub fn delete_count(
    id: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<usize, ServiceError> {
    lock(&app_state, &window).delete_count(&id)
}

/// Delete a task and its subtree.
///
/// # Errors
///
/// Propagates core I/O failures.
#[tauri::command(async)]
pub fn delete_task(
    id: String,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<SnapshotDto, ServiceError> {
    lock(&app_state, &window).delete_task(&id)
}

/// Project-scoped title search plus core state/tag/priority/due filters.
///
/// # Errors
///
/// Returns `no_project` or `invalid_input`.
#[tauri::command(async)]
pub fn search(
    query: String,
    state: Option<String>,
    tag: Option<String>,
    priority: Option<String>,
    due_today: bool,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<Vec<String>, ServiceError> {
    lock(&app_state, &window).search(
        &query,
        state.as_deref(),
        tag.as_deref(),
        priority.as_deref(),
        due_today,
    )
}

/// Copy the exact metadata payload (optionally with the body) to the native
/// clipboard and return it.
///
/// # Errors
///
/// Returns `clipboard` when the plugin cannot write, or `not_found`.
#[tauri::command(async)]
pub fn copy_task(
    id: String,
    include_body: bool,
    app: AppHandle,
    window: WebviewWindow,
    app_state: State<'_, AppState>,
) -> Result<String, ServiceError> {
    let text = lock(&app_state, &window).copy_payload(&id, include_body)?;
    app.clipboard()
        .write_text(text.clone())
        .map_err(|error| ServiceError::new("clipboard", error.to_string()))?;
    Ok(text)
}

/// Copy arbitrary text (used for draft recovery) to the native clipboard.
///
/// # Errors
///
/// Returns `clipboard` when the plugin cannot write.
#[tauri::command(async)]
pub fn copy_text(text: String, app: AppHandle) -> Result<(), ServiceError> {
    app.clipboard()
        .write_text(text)
        .map_err(|error| ServiceError::new("clipboard", error.to_string()))
}

/// Open an external URL after validating its scheme.
///
/// # Errors
///
/// Returns `unsupported_scheme` unless the URL is http, https, or mailto.
#[tauri::command(async)]
pub fn open_external(url: String, app: AppHandle) -> Result<(), ServiceError> {
    let trimmed = url.trim();
    if !is_allowed_external_url(trimmed) {
        return Err(ServiceError::new(
            "unsupported_scheme",
            "only http, https, and mailto links can be opened",
        ));
    }
    app.opener()
        .open_url(trimmed, None::<&str>)
        .map_err(|error| ServiceError::new("opener", error.to_string()))
}

/// Hide the main window. Closing the window hides the app to the menu bar
/// icon instead of quitting; the webview stays alive so drafts survive.
///
/// Confirmation for a dirty draft happens in the fail-closed in-app modal, so
/// there is no native message-dialog command here: the dialog plugin is only
/// used for the folder picker, whose cancellation returns no path and never
/// authorizes anything.
///
/// # Errors
///
/// This currently cannot fail; the `Result` keeps the IPC contract uniform.
#[tauri::command]
pub fn close_main_window(app: AppHandle) -> Result<(), ServiceError> {
    menubar::hide_main(&app);
    Ok(())
}

/// Hide the popover and show the main window (the popover's Open TT).
///
/// # Errors
///
/// This currently cannot fail; the `Result` keeps the IPC contract uniform.
#[tauri::command]
pub fn open_desktop(app: AppHandle) -> Result<(), ServiceError> {
    menubar::hide(&app);
    menubar::show_main(&app);
    Ok(())
}

/// Quit the whole app (the popover's Quit), main window included.
///
/// # Errors
///
/// This currently cannot fail; the `Result` keeps the IPC contract uniform.
#[tauri::command]
pub fn quit_app(app: AppHandle) -> Result<(), ServiceError> {
    app.exit(0);
    Ok(())
}

/// Hide the menu bar popover (Escape in the popover).
///
/// # Errors
///
/// This currently cannot fail; the `Result` keeps the IPC contract uniform.
#[tauri::command]
pub fn hide_menubar(app: AppHandle) -> Result<(), ServiceError> {
    menubar::hide(&app);
    Ok(())
}

/// Show or clear the tray icon's attention dot. The popover computes this from
/// its own Project after every snapshot.
///
/// # Errors
///
/// This currently cannot fail; the `Result` keeps the IPC contract uniform.
#[tauri::command]
pub fn set_tray_attention(attention: bool, app: AppHandle) -> Result<(), ServiceError> {
    menubar::set_tray_attention(&app, attention);
    Ok(())
}

/// Hide the popover, show and focus the main window, and ask it to open a
/// Project and select a Task. The main window's own dirty-draft guards apply:
/// this only requests the navigation.
///
/// # Errors
///
/// Returns `window` when the focus request cannot be delivered.
#[tauri::command]
pub fn open_in_desktop(
    project_slug: String,
    task_id: String,
    app: AppHandle,
) -> Result<(), ServiceError> {
    menubar::hide(&app);
    menubar::show_main(&app);
    app.emit_to(
        menubar::MAIN_LABEL,
        menubar::FOCUS_TASK_EVENT,
        serde_json::json!({ "projectSlug": project_slug, "taskId": task_id }),
    )
    .map_err(|error| ServiceError::new("window", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::is_allowed_external_url;

    #[test]
    fn external_urls_allow_only_http_https_and_mailto() {
        assert!(is_allowed_external_url("https://example.com/a?b=c#d"));
        assert!(is_allowed_external_url("http://example.com"));
        assert!(is_allowed_external_url("mailto:me@example.com"));
        assert!(is_allowed_external_url("HTTPS://example.com"));
        assert!(is_allowed_external_url("  https://example.com  "));

        assert!(!is_allowed_external_url("javascript:alert(1)"));
        assert!(!is_allowed_external_url("file:///etc/passwd"));
        assert!(!is_allowed_external_url("tttask:abc1234567"));
        assert!(!is_allowed_external_url("no-scheme"));
        assert!(!is_allowed_external_url("_ttp://example.com"));
    }
}
