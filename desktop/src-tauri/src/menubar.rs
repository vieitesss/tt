//! Menu bar (tray) icon and the popover window it opens.
//!
//! Facts this module encodes (see `docs/design/macos-client.md`):
//!
//! - The popover is a second webview window (`menubar.html`) with its own
//!   Project (its own [`crate::AppState`] slot) and native popover material.
//! - A click on the tray (left or right) toggles it, anchored under the tray icon's
//!   *current* rect and clamped to the monitor that icon is on; losing focus,
//!   Escape, or the global shortcut hides it. Hiding never destroys it, so
//!   drafts survive.
//! - The tray icon is a monochrome template icon that gains a dot while the
//!   popover's Project has an open Task due today or earlier.
//!
//! Coordinate spaces, because two of them meet here. Measured on a 2× display
//! whose `CGDisplayBounds` is 1470×956 points with a 2940×1912 backing store:
//!
//! - the tray rect is *backing pixels* (points × backing scale), top-left origin;
//! - `Monitor::position()` and `size()` land in that same space (macOS reports
//!   `CGDisplayPixelsWide` as the point width in a HiDPI mode, and tao scales it
//!   back up), so monitor bounds and tray rects compare directly;
//! - `monitor_from_point`, `set_position` (logical) and everything the popover
//!   is placed by work in points, so the scale factor is applied there.

use std::sync::Mutex;
use std::time::Instant;

use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::utils::config::WindowEffectsConfig;
use tauri::utils::{WindowEffect, WindowEffectState};
use tauri::{
    AppHandle, Emitter, LogicalPosition, Manager, Monitor, Rect, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

/// Window label of the popover; also the key of its [`crate::AppState`] slot.
pub const MENUBAR_LABEL: &str = "menubar";
/// Window label of the main window.
pub const MAIN_LABEL: &str = "main";
/// Tray icon id, so the attention dot can be swapped in.
pub const TRAY_ID: &str = "tt-tray";
/// Emitted to the popover with `"shown"`/`"hidden"` when it becomes visible or
/// hidden. A shown popover refreshes immediately instead of waiting a tick.
pub const VISIBILITY_EVENT: &str = "tt://menubar-visibility";
/// Emitted to the main window to open a Project and select a Task.
pub const FOCUS_TASK_EVENT: &str = "tt://focus-task";
/// Popover size in logical pixels (the spec's 360×480).
pub const POPOVER_SIZE: (f64, f64) = (360.0, 480.0);
/// A tray click this soon after the popover was hidden by losing focus is the
/// other half of the click that hid it; reopening would make the toggle look
/// broken.
pub const FOCUS_LOSS_GRACE_MS: u64 = 300;
/// Height of the menu bar band, used only to park the popover when the tray
/// has never reported a rect.
const MENU_BAR_HEIGHT: f64 = 24.0;
/// Width of the synthetic anchor used for that same fallback.
const FALLBACK_ANCHOR_WIDTH: f64 = 36.0;

const SHOWN: &str = "shown";
const HIDDEN: &str = "hidden";

/// A rectangle in one coordinate space (physical pixels for the callers here).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectPx {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl RectPx {
    /// Convert the physical or logical `tauri::Rect` the tray reports. The
    /// tray always reports physical pixels, so no scale factor is applied.
    fn from_tauri(rect: &Rect) -> Self {
        let position = rect.position.to_physical::<f64>(1.0);
        let size = rect.size.to_physical::<f64>(1.0);
        Self {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        }
    }

    fn centre(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// The same rect expressed in logical pixels (points).
    fn to_logical(self, scale: f64) -> Self {
        Self {
            x: self.x / scale,
            y: self.y / scale,
            width: self.width / scale,
            height: self.height / scale,
        }
    }
}

/// Whether a point is inside a rectangle, edges included.
fn contains(bounds: RectPx, x: f64, y: f64) -> bool {
    x >= bounds.x && x <= bounds.x + bounds.width && y >= bounds.y && y <= bounds.y + bounds.height
}

/// The monitor whose rect contains the icon's centre. Monitors and icon must be
/// in the same (physical) space; `None` means the icon is off every display.
pub fn monitor_index_at(icon: RectPx, monitors: &[RectPx]) -> Option<usize> {
    let (x, y) = icon.centre();
    monitors.iter().position(|bounds| contains(*bounds, x, y))
}

/// Whether a tray click should be swallowed instead of reopening the popover.
/// `ms_since_focus_loss` is `None` when no focus loss hid it recently.
pub fn suppresses_reopen(ms_since_focus_loss: Option<u64>) -> bool {
    ms_since_focus_loss.is_some_and(|ms| ms <= FOCUS_LOSS_GRACE_MS)
}

/// Where to put the popover: centered under the tray icon, then clamped inside
/// the monitor so it never hangs off an edge or over the menu bar. All three
/// inputs must be in the same coordinate space (logical pixels at the call
/// site, matching `WebviewWindow::set_position`).
pub fn anchored_position(icon: RectPx, popover: (f64, f64), monitor: RectPx) -> (f64, f64) {
    let centered = icon.x + (icon.width - popover.0) / 2.0;
    let below = icon.y + icon.height;
    // A monitor smaller than the popover cannot fit it; pin to the origin
    // instead of producing an inverted clamp range.
    let max_x = (monitor.x + monitor.width - popover.0).max(monitor.x);
    let max_y = (monitor.y + monitor.height - popover.1).max(monitor.y);
    (
        centered.clamp(monitor.x, max_x),
        below.clamp(monitor.y, max_y),
    )
}

/// Where to anchor when no tray rect is known (the shortcut can be pressed
/// before the tray has ever reported one): a synthetic icon at the top-right of
/// the monitor, in the menu bar band. Never used once a rect exists, so the
/// popover is never placed from stale coordinates.
pub fn default_anchor(monitor: RectPx) -> (f64, f64) {
    let icon = RectPx {
        x: monitor.x + monitor.width - FALLBACK_ANCHOR_WIDTH,
        y: monitor.y,
        width: FALLBACK_ANCHOR_WIDTH,
        height: MENU_BAR_HEIGHT,
    };
    anchored_position(icon, POPOVER_SIZE, monitor)
}

/// The menu bar icon, the last tray rect a click reported, and when a focus
/// loss last hid the popover.
pub struct Menubar {
    last_rect: Mutex<Option<Rect>>,
    focus_loss_at: Mutex<Option<Instant>>,
    plain: Image<'static>,
    attention: Image<'static>,
}

impl Menubar {
    fn last_rect(&self) -> Option<Rect> {
        *self
            .last_rect
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn remember_rect(&self, rect: Rect) {
        let mut last = self
            .last_rect
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *last = Some(rect);
    }

    /// Milliseconds since a focus loss hid the popover, consumed on read so a
    /// stale flag cannot suppress a later, unrelated click.
    fn take_focus_loss_age(&self) -> Option<u64> {
        let mut last = self
            .focus_loss_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        last.take().map(|at| at.elapsed().as_millis() as u64)
    }

    fn note_focus_loss(&self) {
        let mut last = self
            .focus_loss_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *last = Some(Instant::now());
    }
}

/// Create the tray icon and the (hidden) popover window. Called once from the
/// Tauri setup hook.
///
/// # Errors
///
/// Returns the native error when the tray icon, its menu, or the popover
/// window cannot be created.
pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Committed PNGs, embedded as raw pixels by `include_image!` (no runtime
    // image decoding feature needed). tray-icon scales whatever it is given to
    // 18 pt tall (tray-icon-0.24.2 src/platform_impl/macos/mod.rs:296), so the
    // 44 px source is the one to ship: on a 2× display it lands on the Retina
    // grid instead of being upscaled from 22 px.
    let plain = tauri::include_image!("icons/tray-icon@2x.png");
    let attention = tauri::include_image!("icons/tray-icon-attention@2x.png");
    app.manage(Menubar {
        last_rect: Mutex::new(None),
        focus_loss_at: Mutex::new(None),
        plain: plain.clone(),
        attention: attention.clone(),
    });

    // No native tray menu: every click on the icon opens the popover, whose
    // footer carries Open TT and Quit.
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(plain)
        .icon_as_template(true)
        .tooltip("TT")
        .on_tray_icon_event(|tray, event| {
            // tray-icon reports left clicks on release and right clicks on
            // press (it has no right-release handler); both toggle.
            if let TrayIconEvent::Click {
                button,
                button_state,
                rect,
                ..
            } = event
            {
                let toggles = matches!(
                    (button, button_state),
                    (MouseButton::Left, MouseButtonState::Up)
                        | (MouseButton::Right, MouseButtonState::Down)
                );
                if toggles {
                    toggle(tray.app_handle(), Some(rect));
                }
            }
        })
        .build(app)?;

    WebviewWindowBuilder::new(app, MENUBAR_LABEL, WebviewUrl::App("menubar.html".into()))
        .title("TT menu bar")
        .inner_size(POPOVER_SIZE.0, POPOVER_SIZE.1)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .effects(WindowEffectsConfig {
            effects: vec![WindowEffect::Popover],
            state: Some(WindowEffectState::Active),
            radius: Some(12.0),
            color: None,
        })
        .build()?;

    Ok(())
}

/// Hide the popover. Its webview stays alive, so its drafts survive.
pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MENUBAR_LABEL) {
        let _ = window.hide();
        let _ = app.emit_to(MENUBAR_LABEL, VISIBILITY_EVENT, HIDDEN);
    }
}

/// Hide the popover because it stopped being the focused window, remembering
/// when: the click that caused this is usually the one that would toggle it
/// straight back open (see [`suppresses_reopen`]).
pub fn hide_after_focus_loss<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<Menubar>() {
        state.note_focus_loss();
    }
    hide(app);
}

/// Show the popover anchored under the tray icon, or hide it when visible.
/// `rect` is the tray icon rect from a click; `None` (global shortcut) keeps
/// whatever the tray reports now.
pub fn toggle<R: Runtime>(app: &AppHandle<R>, rect: Option<Rect>) {
    let Some(window) = app.get_webview_window(MENUBAR_LABEL) else {
        return;
    };
    let state = app.try_state::<Menubar>();
    if let (Some(rect), Some(state)) = (rect.as_ref(), state.as_ref()) {
        state.remember_rect(*rect);
    }
    if window.is_visible().unwrap_or(false) {
        hide(app);
        return;
    }
    // A tray click right after the popover hid itself on focus loss is the
    // second half of that same click: stay closed.
    if rect.is_some() {
        let age = state.as_ref().and_then(|state| state.take_focus_loss_age());
        if suppresses_reopen(age) {
            return;
        }
    }
    position(&window, app);
    // Activate first: an Accessory app with no visible window is not the active
    // app, and an inactive app cannot own a key window.
    activate(app);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit_to(MENUBAR_LABEL, VISIBILITY_EVENT, SHOWN);
}

/// Make the app itself active, which is what lets the popover become key and
/// receive typing while it runs as an Accessory (main window hidden, no Dock
/// icon).
///
/// `set_focus` alone is not enough: tao calls the deprecated
/// `activateIgnoringOtherApps:`, which the system ignores, so a programmatic
/// show leaves the popover visible but permanently non-key. `NSApplication`
/// has the supported `-activate` since macOS 14 and the bundle's minimum
/// system version is 26, so it is called directly.
///
/// Safe by construction and still `unsafe_code = "forbid"`: `MainThreadMarker`
/// is only produced on the main thread, and `sharedApplication`/`activate` are
/// safe bindings.
#[cfg(target_os = "macos")]
fn activate<R: Runtime>(_app: &AppHandle<R>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    // Only the main thread may touch the application object. Every show path
    // runs there: tray events come off the event loop, and the global shortcut
    // handler dispatches with `run_on_main_thread`.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm).activate();
}

/// Activation is a macOS concept; elsewhere the window's own focus is all there
/// is.
#[cfg(not(target_os = "macos"))]
fn activate<R: Runtime>(_app: &AppHandle<R>) {}

/// Hide the main window and stop showing a Dock icon.
pub fn hide_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.hide();
        set_activation(app, false);
    }
}

/// Show and focus the main window, restoring the Dock icon. Used by the tray
/// menu, Dock reopen, and "Open in TT".
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        set_activation(app, true);
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Swap the tray icon between plain and with-dot.
pub fn set_tray_attention<R: Runtime>(app: &AppHandle<R>, attention: bool) {
    let Some(state) = app.try_state::<Menubar>() else {
        return;
    };
    let icon = if attention {
        state.attention.clone()
    } else {
        state.plain.clone()
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // `set_icon` resets the template flag; keep it so macOS inverts the
        // monochrome icon for the current menu bar appearance.
        let _ = tray.set_icon_with_as_template(Some(icon), true);
    }
}

/// Regular (Dock icon) while the main window is visible, Accessory when only
/// the menu bar icon is.
#[cfg(target_os = "macos")]
fn set_activation<R: Runtime>(app: &AppHandle<R>, regular: bool) {
    let policy = if regular {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.set_activation_policy(policy);
}

/// The Dock/activation policy is a macOS concept; elsewhere the main window is
/// simply hidden.
#[cfg(not(target_os = "macos"))]
fn set_activation<R: Runtime>(_app: &AppHandle<R>, _regular: bool) {}

/// Physical bounds of a monitor, in the same space as the tray rect: tao
/// returns `position()` and `size()` as points × backing scale, which is
/// exactly what `tray-icon` reports, so nothing has to be converted.
fn physical_bounds(monitor: &Monitor) -> RectPx {
    RectPx {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        width: f64::from(monitor.size().width),
        height: f64::from(monitor.size().height),
    }
}

/// The monitor the tray icon is actually on. Prefers Tauri's
/// `monitor_from_point` (which works in CG points, so the icon centre is
/// converted with the scale of the monitor that contains it) and falls back to
/// that same physical containment match, then to nothing at all.
fn monitor_for<R: Runtime>(app: &AppHandle<R>, icon: RectPx) -> Option<Monitor> {
    let monitors = app.available_monitors().unwrap_or_default();
    let scales: Vec<f64> = monitors
        .iter()
        .map(|monitor| monitor.scale_factor())
        .collect();
    let bounds: Vec<RectPx> = monitors.iter().map(physical_bounds).collect();
    let index = monitor_index_at(icon, &bounds);
    let scale = index
        .and_then(|index| scales.get(index).copied())
        .filter(|scale| *scale > 0.0)
        .unwrap_or(1.0);
    let (x, y) = icon.centre();
    if let Ok(Some(monitor)) = app.monitor_from_point(x / scale, y / scale) {
        return Some(monitor);
    }
    index.and_then(|index| monitors.get(index).cloned())
}

/// The tray icon's current rect, falling back to the last click's rect. Both
/// are physical pixels as reported by `tray-icon`.
fn tray_rect<R: Runtime>(app: &AppHandle<R>) -> Option<RectPx> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(Some(rect)) = tray.rect() {
            return Some(RectPx::from_tauri(&rect));
        }
    }
    app.try_state::<Menubar>()
        .and_then(|state| state.last_rect())
        .map(|rect| RectPx::from_tauri(&rect))
}

/// Place the popover under the tray icon's current position on the monitor
/// that icon is on; with no tray rect at all, park it at the top-right of the
/// primary monitor rather than anywhere stale or undefined. The primary
/// monitor is also the last resort when the tray icon cannot be placed on any
/// display, so a show never silently keeps a stale position.
fn position<R: Runtime>(window: &WebviewWindow<R>, app: &AppHandle<R>) {
    let icon = tray_rect(app);
    let monitor = icon
        .and_then(|icon| monitor_for(app, icon))
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let bounds = physical_bounds(&monitor).to_logical(scale);
    let (x, y) = match icon {
        Some(icon) => anchored_position(icon.to_logical(scale), POPOVER_SIZE, bounds),
        None => default_anchor(bounds),
    };
    let _ = window.set_position(LogicalPosition::new(x, y));
}

#[cfg(test)]
mod tests {
    use super::{
        anchored_position, default_anchor, monitor_index_at, suppresses_reopen, RectPx,
        FOCUS_LOSS_GRACE_MS, POPOVER_SIZE,
    };

    fn rect(x: f64, y: f64, width: f64, height: f64) -> RectPx {
        RectPx {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn popover_centers_under_the_icon_when_there_is_room() {
        // A 1000×800 monitor starting at the origin; icon 200..224 at the top.
        let icon = rect(200.0, 0.0, 24.0, 24.0);
        let monitor = rect(0.0, 0.0, 1000.0, 800.0);
        assert_eq!(
            anchored_position(icon, (360.0, 480.0), monitor),
            // 200 + (24 - 360) / 2 = 32, and the popover hangs below the icon.
            (32.0, 24.0)
        );
    }

    #[test]
    fn popover_clamps_inside_the_monitor() {
        let monitor = rect(0.0, 0.0, 1000.0, 800.0);
        // A tray icon at the far right would push the popover off-screen.
        let icon = rect(980.0, 0.0, 24.0, 24.0);
        assert_eq!(
            anchored_position(icon, (360.0, 480.0), monitor),
            (640.0, 24.0)
        );
        // Same for an icon on the left edge of a second monitor.
        let icon = rect(1000.0, 0.0, 24.0, 24.0);
        assert_eq!(
            anchored_position(icon, (360.0, 480.0), monitor),
            (640.0, 24.0)
        );
    }

    #[test]
    fn popover_clamps_to_the_bottom_and_to_a_secondary_monitor() {
        // Secondary monitor above/left of the primary one (negative origin).
        let monitor = rect(-1280.0, -200.0, 1280.0, 1024.0);
        let icon = rect(-1200.0, -200.0, 24.0, 24.0);
        // Centering would put the left edge at -1368, outside the monitor, so
        // the popover pins to the monitor's left edge instead.
        assert_eq!(
            anchored_position(icon, (360.0, 480.0), monitor),
            (-1280.0, -176.0)
        );
        // Icon at the very bottom: the popover must not overflow.
        let icon = rect(-1200.0, 700.0, 24.0, 24.0);
        assert_eq!(
            anchored_position(icon, (360.0, 480.0), monitor),
            (-1280.0, 344.0)
        );
    }

    #[test]
    fn popover_pins_to_a_monitor_smaller_than_itself() {
        let monitor = rect(0.0, 0.0, 300.0, 300.0);
        let icon = rect(0.0, 0.0, 24.0, 24.0);
        assert_eq!(anchored_position(icon, (360.0, 480.0), monitor), (0.0, 0.0));
    }

    #[test]
    fn the_monitor_under_the_icon_wins_over_the_first_one() {
        // Primary 1920 wide, secondary to its right.
        let primary = rect(0.0, 0.0, 1920.0, 1080.0);
        let secondary = rect(1920.0, 0.0, 1440.0, 900.0);
        let monitors = [primary, secondary];
        // Icon in the secondary monitor's menu bar, x = 1920 + 1400.
        let icon = rect(3320.0, 0.0, 36.0, 24.0);
        assert_eq!(monitor_index_at(icon, &monitors), Some(1));
        // Icon on the primary.
        let icon = rect(1500.0, 0.0, 36.0, 24.0);
        assert_eq!(monitor_index_at(icon, &monitors), Some(0));
        // Icon on the second monitor's left edge and its right edge are both
        // inside (edges included).
        assert_eq!(
            monitor_index_at(rect(1920.0, 0.0, 36.0, 24.0), &monitors),
            Some(1)
        );
        assert_eq!(
            monitor_index_at(rect(3324.0, 0.0, 36.0, 24.0), &monitors),
            Some(1)
        );
    }

    #[test]
    fn an_icon_off_every_display_matches_no_monitor() {
        let monitors = [rect(0.0, 0.0, 1920.0, 1080.0)];
        assert_eq!(
            monitor_index_at(rect(3000.0, 0.0, 36.0, 24.0), &monitors),
            None
        );
        assert_eq!(monitor_index_at(rect(0.0, 0.0, 0.0, 0.0), &[]), None);
    }

    #[test]
    fn a_tray_click_right_after_a_focus_loss_does_not_reopen() {
        assert!(suppresses_reopen(Some(0)));
        assert!(suppresses_reopen(Some(FOCUS_LOSS_GRACE_MS)));
        assert!(!suppresses_reopen(Some(FOCUS_LOSS_GRACE_MS + 1)));
        assert!(!suppresses_reopen(Some(5_000)));
        // Nothing hid it: a normal click opens the popover.
        assert!(!suppresses_reopen(None));
    }

    #[test]
    fn without_a_tray_rect_the_popover_parks_at_the_top_right() {
        let monitor = rect(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(default_anchor(monitor), (1560.0, 24.0));
        // Off the primary monitor's origin it stays inside that monitor.
        let monitor = rect(1920.0, 0.0, 1440.0, 900.0);
        assert_eq!(default_anchor(monitor), (3000.0, 24.0));
        // A monitor narrower than the popover cannot fit it; it pins left.
        let monitor = rect(0.0, 0.0, 300.0, 300.0);
        assert_eq!(default_anchor(monitor), (0.0, 0.0));
        assert_eq!(POPOVER_SIZE, (360.0, 480.0));
    }
}
