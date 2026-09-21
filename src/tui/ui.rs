//! Ratatui rendering for the list view and its persistent preview.
//!
//! The list is the only main view: one indented row per task in pre-order and
//! a preview of the selected task on the right, with three footer rows (two
//! key-hint rows, then project context), a one-row header (issue badge) on
//! top, and a transient toast popup for action feedback.

use chrono::NaiveDate;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;
use tt::{path_display, Priority, Task, TaskId, TaskState, VaultIssue};

use super::app::{App, InputMode};
use super::launch::Launch;
use super::list::ListRow;
use super::markdown;

/// Maximum matches shown in the search popup.
const SEARCH_POPUP_MATCHES: usize = 5;

/// Areas of the main screen: the header row, the list and preview panes, the
/// middle area (both panes; the issues overlay and the toast cover it), and
/// the three footer rows (two hint rows, then project context).
#[derive(Debug)]
pub(crate) struct LayoutAreas {
    pub(crate) header: Rect,
    pub(crate) list: Rect,
    pub(crate) preview: Rect,
    pub(crate) middle: Rect,
    /// Two footer rows: per-mode key hints or the live input prompt on the
    /// first row, the second hint row beneath it.
    pub(crate) hint: Rect,
    /// Third footer row: project context and sticky status.
    pub(crate) context: Rect,
}

/// Split `area` into the main screen panes. One definition, so the list pane
/// the event loop reports to the app always matches the pane `render` draws.
pub(crate) fn layout(area: Rect) -> LayoutAreas {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(area);
    // The preview is the majority pane: it carries the title and body, while
    // the list stays a scannable column. A two-column spacer keeps the panes
    // visually separate; the spacer is never painted.
    let panes = Layout::horizontal([
        Constraint::Percentage(40),
        Constraint::Length(2),
        Constraint::Min(1),
    ])
    .split(chunks[1]);
    let footer = Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).split(chunks[2]);
    LayoutAreas {
        header: chunks[0],
        list: panes[0],
        preview: panes[2],
        middle: chunks[1],
        hint: footer[0],
        context: footer[1],
    }
}

/// Draw the whole screen: the header, the task list, the preview of the
/// selected task, and the status/input line. Pure with respect to the app:
/// scroll clamping happens before the draw, from the panes measured by
/// [`layout`].
pub(crate) fn render(frame: &mut Frame<'_>, app: &App) {
    let areas = layout(frame.area());

    render_header(frame, areas.header, app);
    render_list(frame, areas.list, app);
    render_search_popup(frame, areas.list, app);
    render_project_popup(frame, areas.list, app);
    render_link_popup(frame, areas.list, app);
    render_move_popup(frame, areas.list, app);
    render_priority_popup(frame, areas.list, app);
    render_tag_popup(frame, areas.list, app);
    render_filter_popup(frame, areas.list, app);
    render_create_link_popup(frame, areas.list, app);
    render_preview(frame, areas.preview, app);
    render_register_popup(frame, areas.middle, app);
    render_hint(frame, areas.hint, app);
    render_context(frame, areas.context, app);

    // Drawn after the panes but before the modal overlay, so it overlays the
    // list and preview, never the footer, and never covers a modal.
    render_toast(frame, areas.middle, app);
    render_confirm_delete(frame, areas.middle, app);

    if app.issues_open {
        render_issues_overlay(frame, areas.middle, app);
    }

    if let Some(prefix) = app.prompt_prefix() {
        let column = areas
            .hint
            .x
            .saturating_add((prefix.chars().count() + app.input_buffer_len()) as u16);
        let row = areas.hint.y;
        frame.set_cursor_position((column.min(areas.hint.right().saturating_sub(1)), row));
    }
}

/// One-row header: the shortened project path and the persistent vault-issue
/// badge. The invisible store folder is never shown; the `--vault` escape
/// hatch falls back to the app name.
fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let badge = app.issue_badge();
    let mut left = area;
    let mut right = None;
    if let Some(text) = &badge {
        let width = text.chars().count() as u16 + 1;
        if area.width > width {
            let chunks =
                Layout::horizontal([Constraint::Min(1), Constraint::Length(width)]).split(area);
            left = chunks[0];
            right = Some((chunks[1], format!(" {text}")));
        }
    }

    let label = match &app.project {
        Some(project) => Span::styled(
            path_display::shorten(&project.path, &app.config.path_display),
            Style::default(),
        ),
        None => Span::styled("tt", Style::default().fg(Color::DarkGray)),
    };
    frame.render_widget(Paragraph::new(label), left);
    if let Some((badge_area, text)) = right {
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::Yellow))
                .alignment(Alignment::Right),
            badge_area,
        );
    }
}

/// The task list: one indented row per task in pre-order, with a selection
/// gutter, tree guides, a state glyph, the title, and right-aligned metadata
/// when present.
fn render_list(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if app.list.is_empty() {
        frame.render_widget(
            Paragraph::new("(no tasks — press a to add)")
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let selected = app.selected.clone();
    let scroll = app.list_scroll;
    let rows: Vec<ListRow> = app.list.rows().to_vec();
    for (offset, row) in rows
        .iter()
        .skip(scroll)
        .take(area.height as usize)
        .enumerate()
    {
        let line = row_line(
            app,
            row,
            selected.as_ref() == Some(&row.id),
            app.marked.contains(&row.id),
            area.width,
        );
        let position = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        frame.render_widget(Paragraph::new(line), position);
    }
}

/// State glyph and color, shared by list rows and the preview metadata.
fn state_glyph(state: Option<TaskState>) -> (&'static str, Style) {
    match state {
        Some(TaskState::Done) => ("●", Style::default().fg(Color::Green)),
        Some(TaskState::Cancelled) | None => ("—", Style::default().fg(Color::DarkGray)),
        Some(TaskState::Open) => ("○", Style::default().fg(Color::DarkGray)),
    }
}

/// Priority marker: colored background, true-color white foreground.
///
/// ANSI `Color::White` follows the terminal palette and can read as black
/// under reverse-video selection; RGB white does not. `sub_modifier` clears
/// reverse inherited from the selected row.
fn priority_glyph(priority: Priority) -> (&'static str, Style) {
    let (text, bg) = match priority {
        Priority::High => ("!", Color::Red),
        Priority::Med => ("~", Color::Yellow),
        Priority::Low => ("↓", Color::DarkGray),
    };
    (
        text,
        Style::default()
            .fg(Color::Rgb(255, 255, 255))
            .bg(bg)
            .remove_modifier(Modifier::all()),
    )
}

/// One task row: selection gutter, guides, state glyph, title, and
/// right-aligned metadata.
///
/// Every row opens with a reserved two-column selection gutter: `▪ ` when the
/// row is marked, two spaces otherwise, so guides and titles never shift.
/// Marked rows get a Yellow background across the whole row; the cursor row is
/// reversed on top of it, so the state glyph colors and the fold column stay
/// readable.
fn row_line(app: &App, row: &ListRow, selected: bool, marked: bool, width: u16) -> Line<'static> {
    let mut selection = Style::default();
    if selected {
        selection = selection.add_modifier(Modifier::REVERSED);
    }
    if marked {
        selection = selection.bg(Color::Yellow);
    }

    let task = app.vault.get(&row.id);
    let (glyph, glyph_style) = state_glyph(task.map(|task| task.state));

    let title = task.map_or_else(
        || format!("{} (missing)", row.id),
        |task| task.title.clone(),
    );
    let title_style = title_style(task, app.today);

    let meta = row_meta(app, &row.id);
    let meta_width: usize = meta.iter().map(|span| span.content.chars().count()).sum();
    // A reserved two-column fold marker keeps every title aligned whether or
    // not the row is a parent.
    let fold = if row.folded {
        "▸ "
    } else if row.has_children {
        "▾ "
    } else {
        "  "
    };
    // The selection gutter is reserved on every row, marker or not, so
    // marking a row never shifts its guides or title.
    let gutter = if marked { "▪ " } else { "  " };
    let prefix = format!("{gutter}{}{}{} ", row.guides(), fold, glyph);
    let prefix_width = prefix.chars().count();
    let reserved = meta_width + usize::from(meta_width > 0);
    let title_width = (width as usize)
        .saturating_sub(prefix_width)
        .saturating_sub(reserved);
    let title = truncate_title(&title, title_width);
    let used = prefix_width + title.chars().count() + meta_width;
    let pad = (width as usize).saturating_sub(used);

    let mut spans = vec![
        Span::styled(prefix, glyph_style.patch(selection)),
        Span::styled(title, title_style.patch(selection)),
    ];
    if meta_width > 0 {
        spans.push(Span::styled(" ".repeat(pad), selection));
        spans.extend(meta.into_iter().map(|span| {
            // Badges set their own background; do not inherit the row's
            // reverse-video selection or their fg/bg will invert.
            let style = if span.style.bg.is_some() {
                span.style
            } else {
                span.style.patch(selection)
            };
            Span::styled(span.content, style)
        }));
    } else if selected || marked {
        // Fill the rest of the row so the highlight spans it.
        spans.push(Span::styled(" ".repeat(pad), selection));
    }
    Line::from(spans)
}

/// Right-aligned metadata spans for one row, in display order: relative due,
/// priority marker, and open rollup for parents. Links are deliberately
/// absent: outgoing links are visible in the body and incoming ones in the
/// Preview's backlink list.
fn row_meta(app: &App, id: &TaskId) -> Vec<Span<'static>> {
    let Some(task) = app.vault.get(id) else {
        return Vec::new();
    };
    let mut spans: Vec<Span<'static>> = Vec::new();

    if let Some(due) = task.due {
        let today = app.today;
        let (text, style) = if due < today {
            ("overdue".to_owned(), Style::default().fg(Color::Red))
        } else if due == today {
            ("today".to_owned(), Style::default().fg(Color::Red))
        } else {
            (
                format!("{}d", (due - today).num_days()),
                Style::default().fg(Color::DarkGray),
            )
        };
        push_meta(&mut spans, Span::styled(text, style));
    }

    if let Some(priority) = task.priority {
        let (text, style) = priority_glyph(priority);
        push_meta(&mut spans, Span::styled(text, style));
    }

    if !app.vault.children(id).is_empty() {
        let (done, total) = app.vault.rollup(id);
        if total > 0 {
            push_meta(
                &mut spans,
                Span::styled(
                    format!("{done}/{total}"),
                    Style::default().fg(Color::DarkGray),
                ),
            );
        }
    }

    spans
}

/// Append a metadata span, separating it from earlier ones with a space.
fn push_meta(spans: &mut Vec<Span<'static>>, span: Span<'static>) {
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans.push(span);
}

/// Title style: cancelled titles are dim and struck through, overdue titles
/// turn red.
fn title_style(task: Option<&Task>, today: NaiveDate) -> Style {
    let Some(task) = task else {
        return Style::default().fg(Color::Red);
    };
    let mut style = Style::default();
    if task.state == TaskState::Cancelled {
        style = style
            .add_modifier(Modifier::CROSSED_OUT)
            .add_modifier(Modifier::DIM);
    }
    if task.due.is_some_and(|due| due < today) {
        style = style.fg(Color::Red);
    }
    style
}

/// Truncate a title to `max_chars`, appending `…` when it does not fit.
fn truncate_title(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

/// Live title-search matches: a small popup at the bottom of the list pane.
fn render_search_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_search() {
        return;
    }
    let entries: Vec<String> = app
        .search_matches()
        .iter()
        .map(|id| resolve_title(app, id))
        .collect();
    render_picker_popup(frame, area, "matches", &entries, picker.highlight);
}

/// Project picker for `p`: slug plus shortened project path.
fn render_project_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_project() {
        return;
    }
    let entries: Vec<String> = app
        .project_matches()
        .iter()
        .map(|project| {
            format!(
                "{}  {}",
                project.slug,
                path_display::shorten(&project.path, &app.config.path_display)
            )
        })
        .collect();
    render_picker_popup(frame, area, "projects", &entries, picker.highlight);
}

/// Link picker for `o`: resolved titles (or ids) of the selection's links.
fn render_link_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_link() {
        return;
    }
    let entries: Vec<String> = app
        .link_matches()
        .iter()
        .map(|id| resolve_title(app, id))
        .collect();
    render_picker_popup(frame, area, "links", &entries, picker.highlight);
}

/// Move picker for `m`: the synthetic `⌂ root` entry plus candidate titles.
fn render_move_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_move() {
        return;
    }
    let entries: Vec<String> = app
        .move_matches()
        .iter()
        .map(|target| match target {
            None => "⌂ root".to_owned(),
            Some(id) => resolve_title(app, id),
        })
        .collect();
    render_picker_popup(frame, area, "move under…", &entries, picker.highlight);
}

/// Priority picker for `!`: high, med, low, and none.
fn render_priority_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_priority() {
        return;
    }
    let entries: Vec<String> = app
        .priority_matches()
        .into_iter()
        .map(|priority| priority.map_or_else(|| "none".to_owned(), |value| value.to_string()))
        .collect();
    render_picker_popup(frame, area, "priority", &entries, picker.highlight);
}

/// Tag picker for `t`: unique tags already present in the vault.
fn render_tag_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_tags() {
        return;
    }
    let entries = app.tag_matches();
    render_picker_popup(frame, area, "tags", &entries, picker.highlight);
}

/// Session-only filter picker for `f`.
fn render_filter_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_filter() {
        return;
    }
    let entries: Vec<String> = app
        .filter_matches()
        .into_iter()
        .map(|choice| choice.label())
        .collect();
    render_picker_popup(frame, area, "filter", &entries, picker.highlight);
}

/// Link-creation picker for `L`: every task except the source.
fn render_create_link_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_create_link() {
        return;
    }
    let entries: Vec<String> = app
        .create_link_matches()
        .iter()
        .map(|id| resolve_title(app, id))
        .collect();
    render_picker_popup(frame, area, "link to…", &entries, picker.highlight);
}

/// A picker popup listing at most [`SEARCH_POPUP_MATCHES`] entries, scrolled
/// to keep the highlighted one visible.
fn render_picker_popup(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    entries: &[String],
    highlight: usize,
) {
    if entries.is_empty() || area.height < 3 || area.width < 4 {
        return;
    }

    let visible = entries.len().min(SEARCH_POPUP_MATCHES);
    let height = (visible as u16 + 2).min(area.height);
    let popup = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(height),
        area.width.min(40),
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let start = highlight
        .saturating_sub(SEARCH_POPUP_MATCHES.saturating_sub(1))
        .min(entries.len().saturating_sub(SEARCH_POPUP_MATCHES));
    let lines: Vec<Line<'static>> = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, label)| {
            let style = if index == highlight {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                truncate_title(label, inner.width as usize),
                style,
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `d` confirmation: the question, including how many descendants die with
/// the selection, and the Delete/Cancel buttons (Cancel highlighted by
/// default), over the middle band. The footer stays visible below it.
fn render_confirm_delete(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let lines = app.confirm_delete_lines();
    if lines.is_empty() {
        return;
    }
    let width = area.width.min(72);
    if width == 0 {
        return;
    }
    let inner_width = width.saturating_sub(2) as usize;

    // Conservative wrapped-height estimate: text rows, a blank row, the
    // button row, and the two border rows.
    let question_rows: usize = lines
        .iter()
        .map(|line| wrapped_rows(line, inner_width))
        .sum::<usize>()
        + 4;
    let height = question_rows.min(area.height as usize) as u16;
    if height == 0 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title("delete")
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let text: Vec<Line<'static>> = lines.into_iter().map(Line::from).collect();
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), chunks[0]);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            button_row(app.confirm_delete_buttons(), app.confirm_delete_button()),
        ]),
        chunks[1],
    );
}

/// The pre-App launch modal: a centered box with the question, any error, and
/// the highlighted button row. The buttons live in their own bottom sub-area,
/// so they stay visible even when a long question is clipped. The project
/// picker and empty-registry path prompt replace the question when open.
pub(crate) fn render_launch(frame: &mut Frame<'_>, launch: &Launch) {
    if let Some((query, highlight, projects)) = launch.project_pick() {
        render_launch_picker(frame, launch, query, highlight, &projects);
        return;
    }
    if let Some(input) = launch.path_input() {
        render_launch_path(frame, input, launch.overlay_error());
        return;
    }
    let lines = launch.lines();
    if lines.is_empty() {
        return;
    }
    let buttons = launch.buttons();
    let area = frame.area();
    let width = area.width.min(72);
    if width == 0 {
        return;
    }
    let inner_width = width.saturating_sub(2) as usize;

    // Conservative wrapped-height estimate (overestimating only makes the
    // modal taller, never hides the buttons).
    let question_rows = lines
        .iter()
        .map(|line| wrapped_rows(line, inner_width))
        .sum::<usize>()
        + 1;
    let button_rows = if buttons.is_empty() { 0 } else { 2 };
    let height = (question_rows + button_rows + 2).min(area.height as usize) as u16;
    if height == 0 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title("tt")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let text: Vec<Line<'static>> = lines.into_iter().map(Line::from).collect();
    if buttons.is_empty() {
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        return;
    }

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), chunks[0]);

    let row = if launch.key_box_buttons() {
        key_box_row(chunks[1].width as usize, launch.button_index())
    } else {
        button_row(buttons, launch.button_index())
    };
    frame.render_widget(Paragraph::new(vec![Line::from(""), row]), chunks[1]);
}

/// Unregistered launch buttons: `[y] Yes  [p] Projects` on the left, `[n] Cancel`
/// on the right. Only the `[y]`/`[p]`/`[n]` box is reversed when highlighted.
fn key_box_row(width: usize, highlighted: usize) -> Line<'static> {
    fn key_span(key: char, highlighted: bool) -> Span<'static> {
        let style = if highlighted {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        Span::styled(format!("[{key}]"), style)
    }

    let left: Vec<Span<'static>> = vec![
        key_span('y', highlighted == 0),
        Span::raw(" Yes"),
        Span::raw("  "),
        key_span('p', highlighted == 1),
        Span::raw(" Projects"),
    ];
    let right: Vec<Span<'static>> = vec![key_span('n', highlighted == 2), Span::raw(" Cancel")];
    let left_width = left
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>();
    let right_width = right
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>();
    let gap = width
        .saturating_sub(left_width)
        .saturating_sub(right_width)
        .max(1);

    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// `--projects` / launch Projects overlay: query line plus matching slugs.
fn render_launch_picker(
    frame: &mut Frame<'_>,
    launch: &Launch,
    query: &str,
    highlight: usize,
    projects: &[tt::Project],
) {
    let area = frame.area();
    let width = area.width.min(72);
    if width < 8 || area.height < 5 {
        return;
    }
    let inner_width = width.saturating_sub(2) as usize;
    let visible = projects.len().clamp(1, 12);
    let extra = usize::from(launch.overlay_error().is_some());
    let height = (visible + 3 + extra + 2).min(area.height as usize) as u16;
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title("projects")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let prompt = super::picker::PickerKind::Project.prompt();
    let mut lines: Vec<Line<'static>> = vec![Line::from(format!("{prompt}{query}"))];
    if let Some(error) = launch.overlay_error() {
        lines.push(Line::from(format!("error: {error}")));
    }
    if projects.is_empty() {
        lines.push(Line::from("(no matching projects)"));
    } else {
        let start = highlight
            .saturating_sub(visible.saturating_sub(1))
            .min(projects.len().saturating_sub(visible));
        for (index, project) in projects.iter().enumerate().skip(start).take(visible) {
            let label = format!(
                "{}  {}",
                project.slug,
                path_display::shorten(&project.path, launch.path_display())
            );
            let style = if index == highlight {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                truncate_title(&label, inner_width),
                style,
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let column = inner
        .x
        .saturating_add(prompt.len() as u16)
        .saturating_add(query.len() as u16);
    frame.set_cursor_position((column.min(inner.right().saturating_sub(1)), inner.y));
}

/// Empty-registry register-path prompt at launch.
fn render_launch_path(frame: &mut Frame<'_>, input: &str, error: Option<&str>) {
    let area = frame.area();
    let width = area.width.min(70);
    let extra = u16::from(error.is_some());
    let height = (5 + extra).min(area.height);
    if width < 4 || height < 3 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title("new project")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = vec![
        Line::from("Register a project directory:"),
        Line::from(format!("> {input}")),
    ];
    if let Some(error) = error {
        lines.push(Line::from(format!("error: {error}")));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    let column = inner
        .x
        .saturating_add(2)
        .saturating_add(input.chars().count() as u16);
    frame.set_cursor_position((
        column.min(inner.right().saturating_sub(1)),
        inner.y.saturating_add(1),
    ));
}

/// One row of `[ label ]` buttons with the highlighted one reversed.
fn button_row(buttons: &[&'static str], highlighted: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, label) in buttons.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let style = if index == highlighted {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        spans.push(Span::styled(format!("[ {label} ]"), style));
    }
    Line::from(spans)
}

/// Conservative row count for `line` under ratatui's `Wrap { trim: false }`.
/// Long words hard-break at the width, so character count divided by width is
/// the common case; the caller adds a safety row.
fn wrapped_rows(line: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    line.chars().count().div_ceil(width).max(1)
}

/// Persistent preview of the selected task: a dim metadata line, the title in
/// bold accent, then the body. Outgoing links stay inline in the body; tasks
/// linking back are listed by live title after it. Link navigation stays on
/// `o`; children are not listed.
fn render_preview(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(id) = app.selected.as_ref() else {
        frame.render_widget(
            Paragraph::new("no task selected").style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    };
    let Some(task) = app.vault.get(id) else {
        frame.render_widget(
            Paragraph::new(format!("task not found: {id}")).style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    let metadata = preview_metadata(app, task);
    if !metadata.is_empty() {
        lines.push(Line::from(metadata));
    }
    // Terminals have no font size; bold plus an accent color is the only
    // honest way to make the title prominent.
    lines.push(Line::from(Span::styled(
        task.title.clone(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));

    // The body is edited externally ($EDITOR), never in the preview. It is
    // parsed from the in-memory task; an empty body gets a dim placeholder so
    // the pane never looks like it failed to render.
    if task.body.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "[empty body]",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )));
    } else {
        lines.push(Line::from(""));
        lines.extend(markdown::preview_body_lines(&task.body, |id| {
            TaskId::parse(id)
                .ok()
                .and_then(|id| app.vault.get(&id).map(|task| task.title.clone()))
        }));
    }

    // Backlinks close the preview as `↩ Title` lines in the linking task's
    // live title, ordered by title. The pane does not scroll, so a list taller
    // than the pane clips at its bottom edge; that is accepted here.
    let backlinks = preview_backlinks(app, task);
    if !backlinks.is_empty() {
        lines.push(Line::from(""));
        lines.extend(backlinks);
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// Backlink lines for the preview: one dim `↩ Title` per task linking to
/// `task`, in lowercased-title order (id order breaks ties) with the title
/// resolved live from the vault.
fn preview_backlinks(app: &App, task: &Task) -> Vec<Line<'static>> {
    let mut ids: Vec<TaskId> = app.vault.backlinks(&task.id).to_vec();
    ids.sort_by_key(|id| {
        app.vault
            .get(id)
            .map(|task| task.title.to_lowercase())
            .unwrap_or_default()
    });
    let dim = Style::default().fg(Color::DarkGray);
    ids.into_iter()
        .map(|id| Line::from(Span::styled(format!("↩ {}", resolve_title(app, &id)), dim)))
        .collect()
}

/// Compact, dimmed metadata for the preview: due, priority, and tags. Link
/// counts are deliberately absent — outgoing links are visible inline in the
/// body and incoming ones are listed by title below it. The list glyph already
/// shows the state, so it is deliberately absent here too. Fields with no
/// value are omitted, so a bare task renders no metadata line at all.
fn preview_metadata(app: &App, task: &Task) -> Vec<Span<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans: Vec<Span<'static>> = Vec::new();

    if let Some(due) = task.due {
        let style = if due < app.today {
            Style::default().fg(Color::Red)
        } else {
            dim
        };
        push_meta(&mut spans, Span::styled(format!("due {due}"), style));
    }

    if let Some(priority) = task.priority {
        let (marker, style) = priority_glyph(priority);
        push_meta(
            &mut spans,
            Span::styled(format!("{marker} {}", priority.as_str()), style),
        );
    }

    for tag in &task.tags {
        push_meta(&mut spans, Span::styled(format!("#{tag}"), dim));
    }

    spans
}

/// Title of a task, or `id (missing)` for a dangling reference.
fn resolve_title(app: &App, id: &TaskId) -> String {
    app.vault
        .get(id)
        .map_or_else(|| format!("{id} (missing)"), |task| task.title.clone())
}

/// First two footer rows: the live input prompt on the top row while typing,
/// the per-mode key hints otherwise; the second hint row stays empty while a
/// prompt owns the footer.
fn render_hint(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let style = match app.mode {
        InputMode::Navigate => Style::default().fg(Color::DarkGray),
        InputMode::Add { .. }
        | InputMode::Capture
        | InputMode::Rename { .. }
        | InputMode::Tag
        | InputMode::Pick(_)
        | InputMode::RegisterPath
        | InputMode::ConfirmDelete { .. } => Style::default().fg(Color::Cyan),
    };

    let [first, second] = app.status_lines();
    frame.render_widget(
        Paragraph::new(vec![Line::from(first), Line::from(second)]).style(style),
        area,
    );
}

/// `P` path prompt: a small centered box with the question and the typed path.
/// The input buffer lives here, so the cursor is placed inside the popup.
fn render_register_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if !matches!(app.mode, InputMode::RegisterPath) {
        return;
    }
    let width = area.width.min(70);
    let height = 5.min(area.height);
    if width < 4 || height < 3 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title("new project")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let lines = vec![
        Line::from("Register a project directory:"),
        Line::from(format!("> {}", app.input)),
    ];
    frame.render_widget(Paragraph::new(lines), inner);

    let column = inner
        .x
        .saturating_add(2)
        .saturating_add(app.input_buffer_len() as u16);
    let row = inner.y.saturating_add(1);
    frame.set_cursor_position((
        column.min(inner.right().saturating_sub(1)),
        row.min(inner.bottom().saturating_sub(1)),
    ));
}

/// Third footer row: project context and the sticky external-change flag.
fn render_context(frame: &mut Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(app.context_text()).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

/// Transient action feedback as a non-blocking toast at the bottom center of
/// the middle band. It never takes focus; the message lives in the app state
/// and expires on tick, so keys pass straight through.
fn render_toast(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(toast) = &app.toast else {
        return;
    };
    let width = (toast.text.chars().count() as u16 + 4).min(area.width);
    let height = 3.min(area.height);
    if width < 4 || height < 3 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height);
    let popup = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::bordered().border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(truncate_title(&toast.text, inner.width as usize))
            .style(Style::default().fg(Color::Cyan)),
        inner,
    );
}

/// Modal overlay listing every issue from the most recent scan: relative
/// path, kind, and detail. The highlighted issue is reversed, the content
/// scrolls to keep it visible, and `e`/Enter opens it in `$EDITOR`.
fn render_issues_overlay(frame: &mut Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(Clear, area);

    let title = if app.vault_issues.is_empty() {
        "vault issues".to_owned()
    } else {
        format!(
            "vault issues ({}) · j/k move · e/enter edit",
            app.vault_issues.len()
        )
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(Color::Yellow));

    let lines: Vec<Line<'static>> = if app.vault_issues.is_empty() {
        vec![Line::from("no vault issues")]
    } else {
        let mut lines = Vec::new();
        for (index, issue) in app.vault_issues.iter().enumerate() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(issue_lines(app, issue, index == app.issues_cursor));
        }
        lines
    };

    // Render the block and its content separately so text can never paint over
    // the border at narrow widths.
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let scroll = issues_scroll(app, inner.width as usize, inner.height as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .style(Style::default()),
        inner,
    );
}

/// Scroll offset (in wrapped rows) that keeps the highlighted issue visible.
fn issues_scroll(app: &App, width: usize, height: usize) -> u16 {
    if app.vault_issues.is_empty() || width == 0 || height == 0 {
        return 0;
    }
    let mut row = 0usize;
    for (index, issue) in app.vault_issues.iter().enumerate() {
        if index > 0 {
            row += 1;
        }
        if index == app.issues_cursor {
            break;
        }
        for line in issue_lines(app, issue, false) {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            row += wrapped_rows(&text, width);
        }
    }
    let start = row.min(u16::MAX as usize) as u16;
    let visible = height.min(u16::MAX as usize) as u16;
    start.saturating_sub(visible.saturating_sub(1))
}

/// Two lines for one issue: file (with kind) and its wrapped detail. The
/// highlighted issue is reversed.
fn issue_lines(app: &App, issue: &VaultIssue, selected: bool) -> Vec<Line<'static>> {
    let path = issue.path.strip_prefix(app.vault.root()).map_or_else(
        |_| issue.path.display().to_string(),
        |path| path.display().to_string(),
    );
    let path_style = if selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let kind_style = if selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::REVERSED)
    } else {
        Style::default().fg(Color::Yellow)
    };
    vec![
        Line::from(vec![
            Span::styled(path, path_style),
            Span::styled(format!("  [{}]", issue.kind.as_str()), kind_style),
        ]),
        Line::from(issue.detail.clone()),
    ]
}
