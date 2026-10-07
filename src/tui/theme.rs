//! Named colour roles for the TUI: the one place to change a colour.

use ratatui::style::Color;

/// Secondary text, inactive glyphs, and block furniture.
pub(crate) const DIM: Color = Color::DarkGray;
/// Interactive chrome: popup borders, key names, links.
pub(crate) const ACCENT: Color = Color::Cyan;
/// Attention that is not an error: badges, marks, keymap and issue overlays.
pub(crate) const WARNING: Color = Color::Yellow;
/// Errors, overdue dates, destructive confirmation, high priority.
pub(crate) const DANGER: Color = Color::Red;
/// Completed tasks.
pub(crate) const DONE: Color = Color::Green;
/// Text on a coloured background. True-color white on purpose: ANSI
/// `Color::White` follows the terminal palette and can read as black under
/// reverse video.
pub(crate) const ON_COLOR: Color = Color::Rgb(255, 255, 255);
