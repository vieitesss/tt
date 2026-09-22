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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    let metadata: Vec<RowMeta> = rows.iter().map(|row| row_meta(app, &row.id)).collect();
    let columns = ListColumns::measure(&rows, &metadata, area.width);
    for (offset, (row, meta)) in rows
        .iter()
        .zip(&metadata)
        .skip(scroll)
        .take(area.height as usize)
        .enumerate()
    {
        let line = row_line(
            app,
            row,
            meta,
            selected.as_ref() == Some(&row.id),
            app.marked.contains(&row.id),
            &columns,
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

/// Widths shared by every row in one rendering of the List.
///
/// Tree indentation is per-row: deeper rows keep their full guide string, so
/// the fold/state/title columns shift one indent step further right per depth
/// level and titles get progressively narrower. Only the metadata columns
/// (due/priority/rollup) and the guide clipping budget are shared, keeping
/// right-aligned metadata stable across depths.
struct ListColumns {
    width: usize,
    /// Maximum guide cells a row may use while keeping the minimum title
    /// and the admitted metadata columns. Rows truncate their guides to this
    /// budget, so at sane widths every row keeps its full guides and only
    /// extreme narrowness falls back to clipped (aligned) guides.
    guides_budget: usize,
    metadata_width: usize,
    due: usize,
    priority: usize,
    rollup: usize,
}

impl ListColumns {
    /// Measure the shared columns once so right-aligned metadata never shifts
    /// between rows. Metadata columns reserve their width even when a
    /// particular task has no value; admission is judged against the deepest
    /// row so metadata still fits after indentation. The title width is
    /// derived per row in [`row_line`], narrowing with depth.
    fn measure(rows: &[ListRow], metadata: &[RowMeta], width: u16) -> Self {
        let measured_guides = rows
            .iter()
            .map(|row| text_width(&row.guides()))
            .max()
            .unwrap_or_default();
        let measured_due = metadata
            .iter()
            .filter_map(|meta| meta.due.as_ref())
            .map(span_width)
            .max()
            .unwrap_or_default();
        let measured_priority = metadata
            .iter()
            .filter_map(|meta| meta.priority.as_ref())
            .map(span_width)
            .max()
            .unwrap_or_default();
        let measured_rollup = metadata
            .iter()
            .filter_map(|meta| meta.rollup.as_ref())
            .map(span_width)
            .max()
            .unwrap_or_default();
        // Gutter (2), fold marker (2), and state glyph plus space (2) are
        // fixed. At narrow widths, retain a small title before admitting
        // metadata columns; the columns that still fit remain shared by every
        // row. Guides are per-row (deeper rows indent further); only when the
        // deepest row would not fit do guides clip to a shared budget.
        let width = width as usize;
        let minimum_title = width.saturating_sub(6).min(4);
        let max_guides = measured_guides.min(width.saturating_sub(6 + minimum_title));
        let metadata_space = width.saturating_sub(6 + max_guides + minimum_title);
        let measured_metadata_width = [measured_due, measured_priority, measured_rollup]
            .into_iter()
            .filter(|column| *column > 0)
            .map(|column| column + 1)
            .sum::<usize>();
        let (due, priority, rollup) = if measured_metadata_width <= metadata_space {
            (measured_due, measured_priority, measured_rollup)
        } else {
            let mut remaining = metadata_space;
            let mut fit = |column: usize| {
                let needed = column + usize::from(column > 0);
                if column > 0 && needed <= remaining {
                    remaining -= needed;
                    column
                } else {
                    0
                }
            };
            (
                fit(measured_due),
                fit(measured_priority),
                fit(measured_rollup),
            )
        };
        let metadata_width = [due, priority, rollup]
            .into_iter()
            .filter(|column| *column > 0)
            .map(|column| column + 1)
            .sum::<usize>();
        let guides_budget = width
            .saturating_sub(6 + minimum_title)
            .saturating_sub(metadata_width);
        Self {
            width,
            guides_budget,
            metadata_width,
            due,
            priority,
            rollup,
        }
    }
}

#[derive(Default)]
struct RowMeta {
    due: Option<Span<'static>>,
    priority: Option<Span<'static>>,
    rollup: Option<Span<'static>>,
}

/// One task row: fixed-width selection gutter, per-depth tree guides, fold,
/// state, title, relative-due, priority, and rollup columns.
///
/// Every row opens with a reserved two-column selection gutter: `▪ ` when the
/// row is marked, two spaces otherwise. Guides are not padded to a shared
/// width: each row draws its own guide string, so a child's connector starts
/// at its parent's content start and the child's content sits one indent step
/// further right, with `│` continuation bars lining up in the ancestor
/// columns. Titles narrow with depth; metadata stays right-aligned. Marked
/// rows get a Yellow background across the whole row; the cursor row is
/// reversed on top of it, so the state glyph colors and fold column stay
/// readable.
fn row_line(
    app: &App,
    row: &ListRow,
    meta: &RowMeta,
    selected: bool,
    marked: bool,
    columns: &ListColumns,
) -> Line<'static> {
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
    let fold = if row.folded {
        "▸ "
    } else if row.has_children {
        "▾ "
    } else {
        "  "
    };
    let gutter = if marked { "▪ " } else { "  " };
    let guides = truncate_to_width(&row.guides(), columns.guides_budget);
    let prefix = format!("{gutter}{guides}{fold}{glyph} ");
    let title_width = columns
        .width
        .saturating_sub(text_width(&prefix))
        .saturating_sub(columns.metadata_width);
    let title = truncate_title(&title, title_width);

    let mut spans = vec![
        Span::styled(prefix, glyph_style.patch(selection)),
        Span::styled(title.clone(), title_style.patch(selection)),
        Span::styled(
            " ".repeat(title_width.saturating_sub(text_width(&title))),
            selection,
        ),
    ];
    push_row_column(&mut spans, meta.due.as_ref(), columns.due, selection);
    push_row_column(
        &mut spans,
        meta.priority.as_ref(),
        columns.priority,
        selection,
    );
    push_row_column(&mut spans, meta.rollup.as_ref(), columns.rollup, selection);
    Line::from(spans)
}

/// Append one fixed-width metadata column, including its leading separator.
fn push_row_column(
    spans: &mut Vec<Span<'static>>,
    value: Option<&Span<'static>>,
    width: usize,
    selection: Style,
) {
    if width == 0 {
        return;
    }
    spans.push(Span::styled(" ", selection));
    let value_width = value.map_or(0, span_width);
    if let Some(value) = value {
        // Badges set their own background; do not inherit the row's
        // reverse-video selection or their fg/bg will invert.
        let style = if value.style.bg.is_some() {
            value.style
        } else {
            value.style.patch(selection)
        };
        spans.push(Span::styled(value.content.clone(), style));
    }
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(value_width)),
        selection,
    ));
}

fn span_width(span: &Span<'_>) -> usize {
    text_width(span.content.as_ref())
}

/// Metadata for one List row, split into independently aligned columns.
fn row_meta(app: &App, id: &TaskId) -> RowMeta {
    let Some(task) = app.vault.get(id) else {
        return RowMeta::default();
    };
    let due = task.due.map(|due| {
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
        Span::styled(text, style)
    });
    let priority = task.priority.map(|priority| {
        let (text, style) = priority_glyph(priority);
        Span::styled(text, style)
    });
    let rollup = if app.vault.children(id).is_empty() {
        None
    } else {
        let (done, total) = app.vault.rollup(id);
        (total > 0).then(|| {
            Span::styled(
                format!("{done}/{total}"),
                Style::default().fg(Color::DarkGray),
            )
        })
    };
    RowMeta {
        due,
        priority,
        rollup,
    }
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

fn text_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Keep the longest prefix of `text` that fits in `max_width` terminal cells.
fn truncate_to_width(text: &str, max_width: usize) -> String {
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

/// Truncate text to `max_width` terminal cells, appending `…` when it does not fit.
fn truncate_title(text: &str, max_width: usize) -> String {
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

const PROJECT_PATH_MIN_WIDTH: usize = 8;

/// Shared slug-column width for a project picker row. Long slugs yield space
/// to a useful path suffix instead of pushing the entire path off-screen.
fn project_slug_width<'a>(slugs: impl Iterator<Item = &'a str>, row_width: usize) -> usize {
    let measured = slugs.map(text_width).max().unwrap_or_default();
    let path_width = PROJECT_PATH_MIN_WIDTH.min(row_width.saturating_sub(2));
    measured.min(row_width.saturating_sub(path_width + 2))
}

fn project_entry(slug: &str, path: &str, slug_width: usize, row_width: usize) -> String {
    if slug_width == 0 {
        return truncate_title(path, row_width);
    }
    let slug = truncate_title(slug, slug_width);
    let padding = " ".repeat(slug_width.saturating_sub(text_width(&slug)));
    truncate_title(&format!("{slug}{padding}  {path}"), row_width)
}

/// Project picker for `p`: slug plus shortened project path.
fn render_project_popup(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(picker) = app.picker() else {
        return;
    };
    if !picker.is_project() {
        return;
    }
    let projects = app.project_matches();
    let row_width = area.width.min(40).saturating_sub(2) as usize;
    let slug_width = project_slug_width(
        projects.iter().map(|project| project.slug.as_str()),
        row_width,
    );
    let entries: Vec<String> = projects
        .iter()
        .map(|project| {
            project_entry(
                &project.slug,
                &path_display::shorten(&project.path, &app.config.path_display),
                slug_width,
                row_width,
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
        let slug_width = project_slug_width(
            projects.iter().map(|project| project.slug.as_str()),
            inner_width,
        );
        for (index, project) in projects.iter().enumerate().skip(start).take(visible) {
            let label = project_entry(
                &project.slug,
                &path_display::shorten(&project.path, launch.path_display()),
                slug_width,
                inner_width,
            );
            let style = if index == highlight {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(label, style)));
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
    text_width(line).div_ceil(width).max(1)
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

    // Render the block and its content separately so text can never paint over
    // the border at narrow widths. The path column is measured once across
    // every issue and capped so the kind column still degrades cleanly.
    let inner = block.inner(area);
    let max_kind_width = app
        .vault_issues
        .iter()
        .map(|issue| text_width(issue.kind.as_str()) + 2)
        .max()
        .unwrap_or_default();
    let path_width = app
        .vault_issues
        .iter()
        .map(|issue| text_width(&issue_path(app, issue)))
        .max()
        .unwrap_or_default()
        .min(
            (inner.width as usize)
                .saturating_sub(2)
                .saturating_sub(max_kind_width),
        );
    let lines: Vec<Line<'static>> = if app.vault_issues.is_empty() {
        vec![Line::from("no vault issues")]
    } else {
        let mut lines = Vec::new();
        for (index, issue) in app.vault_issues.iter().enumerate() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(issue_lines(
                app,
                issue,
                index == app.issues_cursor,
                path_width,
            ));
        }
        lines
    };

    frame.render_widget(block, area);
    let scroll = issues_scroll(app, inner.width as usize, inner.height as usize, path_width);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .style(Style::default()),
        inner,
    );
}

/// Scroll offset (in wrapped rows) that keeps the highlighted issue visible.
fn issues_scroll(app: &App, width: usize, height: usize, path_width: usize) -> u16 {
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
        for line in issue_lines(app, issue, false, path_width) {
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
fn issue_path(app: &App, issue: &VaultIssue) -> String {
    issue.path.strip_prefix(app.vault.root()).map_or_else(
        |_| issue.path.display().to_string(),
        |path| path.display().to_string(),
    )
}

fn issue_lines(
    app: &App,
    issue: &VaultIssue,
    selected: bool,
    path_width: usize,
) -> Vec<Line<'static>> {
    let path = truncate_title(&issue_path(app, issue), path_width);
    let path_padding = " ".repeat(path_width.saturating_sub(text_width(&path)));
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
            Span::styled(path_padding, path_style),
            Span::styled(format!("  [{}]", issue.kind.as_str()), kind_style),
        ]),
        Line::from(issue.detail.clone()),
    ]
}
