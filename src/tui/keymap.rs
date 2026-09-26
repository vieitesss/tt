//! The complete keymap reference: content and pure geometry.
//!
//! This is the single source of truth for the keymap overlay. It owns the
//! group table (titles plus `(key, label)` rows) and the pure geometry
//! derived from it: how many columns a width gets, how tall the packed
//! columns are, how wide the packed content is, and the entire popup
//! geometry (popup size, visible body rows, scroll clamp) for a middle-area
//! size. Both [`super::app`] (scroll clamping) and [`super::ui`] (rendering)
//! obtain every number from [`keymap_geometry`]; it depends on neither, so
//! the heights can never drift from the groups they measure. Terminal-free:
//! no rendering, no crossterm I/O.

use super::text::text_width;

/// One group in the complete keymap reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeymapGroup {
    pub(crate) title: &'static str,
    pub(crate) rows: &'static [(&'static str, &'static str)],
}

/// The single definition of the keymap content. Styling lives in
/// [`super::ui`]; geometry below derives group heights from this table.
pub(crate) const KEYMAP_GROUPS: &[KeymapGroup] = &[
    KeymapGroup {
        title: "Movement",
        rows: &[
            ("j / k", "move selection (also up/down arrows)"),
            ("gg / G", "first / last"),
            ("h / l", "fold / unfold (also left/right)"),
            ("Tab", "mark / unmark"),
            ("Esc", "clear selection"),
        ],
    },
    KeymapGroup {
        title: "Tasks",
        rows: &[
            ("a", "add child"),
            ("A", "add task at top"),
            ("N", "quick capture"),
            ("x", "cycle state"),
            ("!", "set priority"),
            ("t", "edit tags"),
            ("J / K", "move among siblings"),
            ("r", "rename"),
            ("y", "copy task metadata"),
            ("e / Enter", "edit in $EDITOR"),
            ("m", "move marked subtrees within/across Projects"),
            ("d", "delete marked"),
            ("L", "add link"),
            ("f", "filter list"),
        ],
    },
    KeymapGroup {
        title: "Jump",
        rows: &[
            ("/", "search titles"),
            ("o", "follow links"),
            ("p", "switch project"),
            ("P", "register project"),
        ],
    },
    KeymapGroup {
        title: "Vault",
        rows: &[("g ?", "store issues"), ("?", "this keymap"), ("q", "quit")],
    },
    KeymapGroup {
        title: "Pickers",
        rows: &[
            ("up / down", "move highlight"),
            ("ctrl-n / p", "move highlight"),
            ("Enter", "commit"),
            ("Esc", "cancel"),
        ],
    },
    KeymapGroup {
        title: "Modals",
        rows: &[
            ("left / right", "choose button"),
            ("y / n", "confirm / cancel delete"),
            ("j / k", "move through issues"),
            ("e / Enter", "edit the issue"),
        ],
    },
];

/// Middle-area width at which the overlay switches from two to three columns.
pub(crate) const KEYMAP_WIDE_WIDTH: u16 = 120;

/// Number of keymap columns for a middle-area width.
pub(crate) fn keymap_columns(width: u16) -> usize {
    if width >= KEYMAP_WIDE_WIDTH {
        3
    } else {
        2
    }
}

/// Rows a group occupies: its title plus one row per binding.
fn group_height(group: &KeymapGroup) -> usize {
    group.rows.len() + 1
}

/// Greedy packing plan: group indices per column, each group kept intact.
/// Shared by the content-height geometry and the renderer's line builder so
/// the two can never disagree about which group lands in which column.
pub(crate) fn packed_group_indices(columns: usize) -> Vec<Vec<usize>> {
    let columns = columns.max(1);
    let mut packed: Vec<Vec<usize>> = vec![Vec::new(); columns];
    let mut heights = vec![0usize; columns];
    for (index, group) in KEYMAP_GROUPS.iter().enumerate() {
        let shortest = heights
            .iter()
            .enumerate()
            .min_by_key(|(_, height)| **height)
            .map_or(0, |(index, _)| index);
        // A blank separator row between groups in the same column.
        if !packed[shortest].is_empty() {
            heights[shortest] += 1;
        }
        heights[shortest] += group_height(group);
        packed[shortest].push(index);
    }
    packed
}

/// Number of keymap rows after greedily packing groups into columns.
pub(crate) fn keymap_content_lines(columns: usize) -> usize {
    packed_group_indices(columns)
        .iter()
        .map(|column| {
            let mut height = 0usize;
            for (position, group) in column.iter().enumerate() {
                if position > 0 {
                    height += 1;
                }
                height += group_height(&KEYMAP_GROUPS[*group]);
            }
            height
        })
        .max()
        .unwrap_or(0)
}

/// Display width of the packed keymap content for a column count, measured
/// exactly like the renderer's line builder: each column is as wide as its
/// widest row (titles or `2 + key_width + 2 + label`), columns are joined by
/// 4-cell gaps, and trailing pad fills every used column to its width. The
/// renderer must use this instead of measuring its own lines, so the popup
/// width can never drift from the content it frames.
pub(crate) fn keymap_content_width(columns: usize) -> usize {
    let columns = columns.max(1);
    let key_widths: Vec<usize> = KEYMAP_GROUPS
        .iter()
        .map(|group| {
            group
                .rows
                .iter()
                .map(|(key, _)| text_width(key))
                .max()
                .unwrap_or_default()
        })
        .collect();
    let plan = packed_group_indices(columns);
    // Per-column rows as (display width, is-content). Separator blanks and
    // missing rows are non-content, matching the renderer's blank handling.
    let mut packed: Vec<Vec<(usize, bool)>> = vec![Vec::new(); columns];
    for (column, indices) in plan.iter().enumerate() {
        for (position, group_index) in indices.iter().enumerate() {
            if position > 0 {
                packed[column].push((0, false));
            }
            let group = &KEYMAP_GROUPS[*group_index];
            packed[column].push((text_width(group.title), true));
            let key_width = key_widths[*group_index];
            for (_, label) in group.rows {
                packed[column].push((2 + key_width + 2 + text_width(label), true));
            }
        }
    }
    let widths: Vec<usize> = packed
        .iter()
        .map(|column| {
            column
                .iter()
                .map(|(width, _)| *width)
                .max()
                .unwrap_or_default()
        })
        .collect();
    let height = packed.iter().map(Vec::len).max().unwrap_or_default();
    let mut content_width = 0usize;
    for row in 0..height {
        let mut last: Option<usize> = None;
        for (column, rows) in packed.iter().enumerate() {
            if rows.get(row).map(|(_, content)| *content).unwrap_or(false) {
                last = Some(column);
            }
        }
        if let Some(last) = last {
            let mut width = 0usize;
            for (column, column_width) in widths.iter().enumerate().take(last + 1) {
                if column > 0 {
                    width += 4;
                }
                width += *column_width;
            }
            content_width = content_width.max(width);
        }
    }
    content_width
}

/// Everything both callers need for one middle-area size: the popup size,
/// the visible body rows, and the scroll clamp. The body is the bordered
/// popup minus its two border rows minus the one footer row
/// (`popup_height.saturating_sub(3)`), exactly the rectangles the renderer
/// draws; `max_scroll` is what the renderer honours, and `scroll` is the
/// requested offset clamped to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeymapGeometry {
    /// Popup width, including borders.
    pub(crate) popup_width: u16,
    /// Popup height, including borders.
    pub(crate) popup_height: u16,
    /// Visible content rows in the popup body.
    pub(crate) body_height: usize,
    /// Requested scroll clamped to `max_scroll`.
    pub(crate) scroll: usize,
    /// Largest scroll the renderer honours.
    pub(crate) max_scroll: usize,
}

/// The single owner of the keymap popup geometry. Given the available
/// middle area, the measured content, and a requested scroll, returns the
/// popup size, the body rows the renderer draws, and the clamped scroll.
/// Both [`super::app`] (clamping) and [`super::ui`] (drawing) must obtain
/// every number from here and recompute none of them.
pub(crate) fn keymap_geometry(
    area_width: u16,
    area_height: u16,
    content_width: usize,
    content_lines: usize,
    scroll: usize,
) -> KeymapGeometry {
    let desired_width = content_width.saturating_add(4).min(u16::MAX as usize) as u16;
    let popup_width = desired_width
        .min(area_width.saturating_sub(2))
        .max(10.min(area_width));
    let mut popup_height =
        (content_lines.saturating_add(3)).min(usize::from(area_height.saturating_sub(2))) as u16;
    if popup_height < 6 {
        popup_height = area_height;
    }
    let body_height = usize::from(popup_height.saturating_sub(3));
    let max_scroll = content_lines.saturating_sub(body_height);
    let clamped = scroll.min(max_scroll);
    KeymapGeometry {
        popup_width,
        popup_height,
        body_height,
        scroll: clamped,
        max_scroll,
    }
}
