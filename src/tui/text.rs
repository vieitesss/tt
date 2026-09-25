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

/// Remove the word before the end of `text`, as `Ctrl-W` does: first any
/// trailing whitespace, then the run of non-whitespace before it. This is a
/// pure buffer edit shared by every prompt and picker query so the shortcut
/// means the same thing everywhere.
pub(crate) fn pop_word(text: &mut String) {
    while text.chars().last().is_some_and(char::is_whitespace) {
        text.pop();
    }
    while text.chars().last().is_some_and(|c| !c.is_whitespace()) {
        text.pop();
    }
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
    fn pop_word_removes_whitespace_and_the_word_before_it() {
        let mut text = "add three words".to_owned();
        pop_word(&mut text);
        assert_eq!(text, "add three ");
        pop_word(&mut text);
        assert_eq!(text, "add ");
        pop_word(&mut text);
        assert_eq!(text, "");
        pop_word(&mut text);
        assert_eq!(text, "", "an empty buffer is a no-op");
    }

    #[test]
    fn pop_word_consumes_trailing_whitespace_first() {
        let mut text = "trailing spaces   ".to_owned();
        pop_word(&mut text);
        assert_eq!(text, "trailing ");
        let mut whitespace = "   ".to_owned();
        pop_word(&mut whitespace);
        assert_eq!(whitespace, "");
    }

    #[test]
    fn width_counts_wide_characters_as_two_cells() {
        assert_eq!(text_width("日本"), 4);
        assert_eq!(truncate_to_width("日本", 3), "日");
    }
}
