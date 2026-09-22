//! Terminal-cell text measurement and truncation.
//!
//! Every pane that must fit a string into a fixed number of display cells —
//! list rows, picker popups, the keymap overlay, toasts, the issues table —
//! measures with the same functions, so a string's width can never be
//! computed one way and truncated another. Pure: no rendering, no crossterm.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Display-cell width of one string.
pub(crate) fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Keep the longest prefix of `text` that fits in `max_width` terminal cells.
pub(crate) fn truncate_to_width(text: &str, max_width: usize) -> String {
    let mut width = 0;
    text.chars()
        .take_while(|character| {
            let character_width = UnicodeWidthChar::width(*character).unwrap_or_default();
            if width + character_width > max_width {
                return false;
            }
            width += character_width;
            true
        })
        .collect()
}

/// Truncate text to `max_width` terminal cells, appending `…` when it does not
/// fit.
pub(crate) fn truncate_title(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text_width(text) <= max_width {
        return text.to_owned();
    }
    let mut truncated = truncate_to_width(text, max_width.saturating_sub(1));
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_cells_and_appends_an_ellipsis() {
        assert_eq!(truncate_title("abcdef", 4), "abc…");
        assert_eq!(truncate_title("abc", 4), "abc");
        assert_eq!(truncate_title("abc", 0), "");
    }

    #[test]
    fn width_counts_wide_characters_as_two_cells() {
        assert_eq!(text_width("日本"), 4);
        assert_eq!(truncate_to_width("日本", 3), "日");
    }
}
