//! TUI tests: drive [`App`] directly and render to a `TestBackend`.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;
use tt::resolve::Resolution;
use tt::{
    Config, NewTask, PathDisplay, PathDisplayStyle, Priority, Project, Task, TaskId, TaskState,
    Vault,
};

use super::app::{
    ensure_selection_visible, expand_tilde_with_home, App, FooterLine, InputMode, TOAST_TICKS,
};
use super::keymap::{keymap_columns, keymap_content_lines, keymap_content_width, keymap_geometry};
use super::launch::Launch;
use super::picker::{FilterCriterion, PickerKind};
use super::ui::{layout, render, render_launch};

fn setup() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().expect("temp dir");
    let vault = Vault::open(dir.path()).expect("open vault");
    (dir, App::new(vault, Config::default(), None, None))
}

/// A vault seeded with broken-frontmatter files, which `Vault` skips and
/// reports. (Files without any frontmatter are ordinary notes and produce no
/// issues.)
fn setup_with_issues(names: &[&str]) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().expect("temp dir");
    for name in names {
        fs::write(
            dir.path().join(name),
            "---\nid: broken0001\nstate: open\n---\nmissing title\n",
        )
        .expect("write malformed file");
    }
    let vault = Vault::open(dir.path()).expect("open vault");
    (dir, App::new(vault, Config::default(), None, None))
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::from(code)
}

fn footer_text(line: &FooterLine) -> String {
    match line {
        FooterLine::Text(text) => text.clone(),
        FooterLine::Hints(hints) => hints
            .iter()
            .map(|hint| format!("{} {}", hint.key, hint.label))
            .collect::<Vec<_>>()
            .join("   "),
    }
}

fn parse_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("valid id")
}

/// Test-only view of the shared clamp: the maximum scroll [`keymap_geometry`]
/// honours for a middle-area size. Delegates to the single geometry
/// derivation, so the test and the app can never drift.
fn keymap_max_scroll(width: u16, height: u16) -> usize {
    let columns = keymap_columns(width);
    keymap_geometry(
        width,
        height,
        keymap_content_width(columns),
        keymap_content_lines(columns),
        0,
    )
    .max_scroll
}

fn project(path: &str, slug: &str) -> Project {
    Project {
        path: PathBuf::from(path),
        slug: slug.to_owned(),
        never_ask_nested: false,
    }
}

fn add_task(app: &mut App, title: &str, parent: Option<&TaskId>) -> TaskId {
    app.vault
        .add(NewTask {
            parent: parent.cloned(),
            ..NewTask::new(title)
        })
        .expect("add task")
        .id
}

/// Render the app and return the terminal buffer row by row.
fn render_lines(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let areas = layout(Rect::new(0, 0, width, height));
    app.set_list_viewport(areas.list.width, areas.list.height);
    app.set_middle_viewport(areas.middle.width, areas.middle.height);
    terminal.draw(|frame| render(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer();

    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| {
                    buffer
                        .cell((x, y))
                        .map_or(' ', |cell| cell.symbol().chars().next().unwrap_or(' '))
                })
                .collect()
        })
        .collect()
}

/// Render the app and return each cell's symbol and style.
fn render_cells(app: &mut App, width: u16, height: u16) -> Vec<Vec<(char, Style)>> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let areas = layout(Rect::new(0, 0, width, height));
    app.set_list_viewport(areas.list.width, areas.list.height);
    app.set_middle_viewport(areas.middle.width, areas.middle.height);
    terminal.draw(|frame| render(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer();

    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| {
                    let cell = buffer.cell((x, y)).expect("cell");
                    (cell.symbol().chars().next().unwrap_or(' '), cell.style())
                })
                .collect()
        })
        .collect()
}

/// Style of the first cell of the first rendering of `needle`.
fn style_at_text(cells: &[Vec<(char, Style)>], needle: &str) -> Style {
    for row in cells {
        let text: String = row.iter().map(|(symbol, _)| *symbol).collect();
        if let Some(byte_index) = text.find(needle) {
            let char_index = text[..byte_index].chars().count();
            return row[char_index].1;
        }
    }
    panic!("{needle:?} not rendered");
}

/// Style of the first cell of the first rendering of `needle` inside `area`.
/// Pane-local, so a preview assertion cannot pick up a list-pane match.
fn style_at_text_in(cells: &[Vec<(char, Style)>], needle: &str, area: Rect) -> Style {
    let start = area.x as usize;
    let end = (area.x + area.width) as usize;
    for row in cells
        .iter()
        .skip(area.y as usize)
        .take(area.height as usize)
    {
        let segment: String = row[start.min(row.len())..end.min(row.len())]
            .iter()
            .map(|(symbol, _)| *symbol)
            .collect();
        if let Some(byte_index) = segment.find(needle) {
            let char_index = segment[..byte_index].chars().count();
            return row[start + char_index].1;
        }
    }
    panic!("{needle:?} not rendered inside {area:?}");
}

/// The text of `lines` inside `area`, one string per pane row.
fn pane_rows(lines: &[String], area: Rect) -> Vec<String> {
    let start_x = area.x as usize;
    let width = area.width as usize;
    lines
        .iter()
        .skip(area.y as usize)
        .take(area.height as usize)
        .map(|line| line.chars().skip(start_x).take(width).collect())
        .collect()
}

/// Character column where `needle` starts in `row`.
fn column(row: &str, needle: &str) -> usize {
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    row[..byte].chars().count()
}

#[test]
fn list_starts_with_the_first_root_selected() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "Second", None);
    add_task(&mut app, "First", None);
    app.refresh();

    assert_eq!(app.selected_id(), Some(first));
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("First"), "list should render nodes: {text}");
    assert!(
        !text.contains('╭') && !text.contains('╔'),
        "map boxes must be gone: {text}"
    );
}

#[test]
fn j_k_and_arrows_move_in_pre_order() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    let grandchild = add_task(&mut app, "Grandchild", Some(&child));
    let second = add_task(&mut app, "Second root", None);
    app.refresh();

    assert_eq!(
        app.selected_id(),
        Some(parent.clone()),
        "the first row starts selected"
    );
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(
        app.selected_id(),
        Some(child.clone()),
        "j goes to the child, not spatially right"
    );
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.selected_id(), Some(grandchild.clone()));
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.selected_id(), Some(child.clone()));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.selected_id(), Some(parent.clone()));

    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected_id(), Some(second.clone()), "j crosses roots");
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected_id(), Some(second), "clamped at the bottom");
    for _ in 0..8 {
        app.handle_key(key(KeyCode::Char('k')));
    }
    assert_eq!(app.selected_id(), Some(parent), "clamped at the top");
}

#[test]
fn uppercase_j_and_k_move_only_the_cursor_task_and_keep_it_selected() {
    let (_dir, mut app) = setup();
    let alpha = add_task(&mut app, "Alpha", None);
    let beta = add_task(&mut app, "Beta", None);
    let gamma = add_task(&mut app, "Gamma", None);
    app.refresh();
    app.marked.insert(gamma.clone());
    app.selected = Some(alpha.clone());

    app.handle_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.vault.roots(),
        &[beta.clone(), alpha.clone(), gamma.clone()]
    );
    assert_eq!(app.selected_id(), Some(alpha.clone()));
    assert_eq!(
        app.marked,
        std::collections::BTreeSet::from([gamma.clone()])
    );

    app.handle_key(key(KeyCode::Char('K')));
    assert_eq!(app.vault.roots(), &[alpha.clone(), beta, gamma]);
    assert_eq!(app.selected_id(), Some(alpha));
}

#[test]
fn rank_keys_toast_without_writing_in_a_filter_or_at_a_bound() {
    let (dir, mut app) = setup();
    let alpha = add_task(&mut app, "Alpha", None);
    add_task(&mut app, "Beta", None);
    app.refresh();
    let path = dir.path().join(format!("{alpha}.md"));
    let before = fs::read_to_string(&path).expect("read before");

    app.active_filter = Some(FilterCriterion::State(TaskState::Open));
    app.refresh();
    app.handle_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("clear the filter to change rank")
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read after filter"),
        before
    );

    app.active_filter = None;
    app.refresh();
    app.selected = Some(alpha);
    app.handle_key(key(KeyCode::Char('K')));
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("already first among siblings")
    );
    assert_eq!(fs::read_to_string(&path).expect("read after bound"), before);
}

#[test]
fn rank_key_toasts_for_a_task_without_siblings() {
    let (dir, mut app) = setup();
    let only = add_task(&mut app, "Only", None);
    app.refresh();
    let path = dir.path().join(format!("{only}.md"));
    let before = fs::read_to_string(&path).expect("read before");

    app.handle_key(key(KeyCode::Char('J')));

    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("task has no siblings")
    );
    assert_eq!(fs::read_to_string(path).expect("read after"), before);
}

#[test]
fn gg_and_shift_g_jump_to_the_ends() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "Alpha", None);
    add_task(&mut app, "Beta", None);
    let last = add_task(&mut app, "Gamma", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('G')));
    assert_eq!(app.selected_id(), Some(last.clone()), "G goes to the end");
    app.handle_key(key(KeyCode::Char('g')));
    assert_eq!(
        app.selected_id(),
        Some(last),
        "a single g waits for the chord"
    );
    app.handle_key(key(KeyCode::Char('g')));
    assert_eq!(app.selected_id(), Some(first), "gg goes to the top");
}

/// `Parent > [Child > Grandchild, Sibling]`; returns the ids.
fn fold_fixture(app: &mut App) -> (TaskId, TaskId, TaskId, TaskId) {
    let parent = add_task(app, "Parent", None);
    let child = add_task(app, "Child", Some(&parent));
    let grandchild = add_task(app, "Grandchild", Some(&child));
    let sibling = add_task(app, "Sibling", Some(&parent));
    app.refresh();
    (parent, child, grandchild, sibling)
}

#[test]
fn h_collapses_the_selected_parent_and_hides_its_subtree() {
    let (_dir, mut app) = setup();
    let (parent, child, grandchild, sibling) = fold_fixture(&mut app);

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.collapsed.contains(&parent));
    assert_eq!(
        app.selected_id(),
        Some(parent.clone()),
        "stays on the parent"
    );
    assert!(app.list.index_of(&child).is_none(), "child hidden");
    assert!(
        app.list.index_of(&grandchild).is_none(),
        "grandchild hidden"
    );
    assert!(app.list.index_of(&sibling).is_none(), "sibling hidden");
    assert_eq!(app.list.len(), 1, "only the parent row remains");
}

#[test]
fn h_on_a_collapsed_or_childless_row_jumps_to_its_parent() {
    let (_dir, mut app) = setup();
    let (parent, _child, _grandchild, _sibling) = fold_fixture(&mut app);

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    app.handle_key(key(KeyCode::Char('h')));
    assert_eq!(
        app.selected_id(),
        Some(parent.clone()),
        "a root has no parent to jump to"
    );

    // A childless row climbs to its parent even without any fold.
    let leaf = add_task(&mut app, "Leaf", Some(&parent));
    app.refresh();
    app.selected = Some(leaf.clone());
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.selected_id(), Some(parent));
}

#[test]
fn l_expands_only_a_collapsed_parent() {
    let (_dir, mut app) = setup();
    let (parent, child, grandchild, _sibling) = fold_fixture(&mut app);

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('l')));
    assert!(app.collapsed.is_empty(), "expanded parents ignore l");

    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.collapsed.contains(&parent));
    assert!(app.list.index_of(&child).is_none());
    app.handle_key(key(KeyCode::Right));
    assert!(!app.collapsed.contains(&parent), "l expands the parent");
    assert!(app.list.index_of(&child).is_some());
    assert!(app.list.index_of(&grandchild).is_some());

    // A childless row ignores l.
    let leaf = add_task(&mut app, "Leaf", None);
    app.refresh();
    app.selected = Some(leaf);
    app.handle_key(key(KeyCode::Char('l')));
    assert!(app.collapsed.is_empty());
}

#[test]
fn refresh_unfolds_ancestors_when_the_selection_is_hidden() {
    let (_dir, mut app) = setup();
    let (parent, child, _grandchild, _sibling) = fold_fixture(&mut app);
    app.selected = Some(child.clone());
    app.collapsed.insert(parent.clone());

    app.refresh();
    assert!(
        app.list.index_of(&child).is_some(),
        "the hidden selection is revealed"
    );
    assert_eq!(app.selected_id(), Some(child));
    assert!(!app.collapsed.contains(&parent), "ancestors unfolded");
}

#[test]
fn collapsing_clamps_the_scroll_to_the_shorter_list() {
    let (_dir, mut app) = setup();
    for index in 0..10 {
        add_task(&mut app, &format!("Root {index:02}"), None);
    }
    let parent = add_task(&mut app, "Parent", None);
    for index in 0..12 {
        add_task(&mut app, &format!("Child {index:02}"), Some(&parent));
    }
    app.refresh();

    app.handle_key(key(KeyCode::Char('G')));
    render_lines(&mut app, 60, 8);
    assert!(app.list_scroll > 0, "the long list scrolled");

    app.handle_key(key(KeyCode::Char('/')));
    for character in "Parent".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_id(), Some(parent.clone()));
    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.collapsed.contains(&parent));

    let viewport = app.list_viewport.1 as usize;
    assert!(
        app.list_scroll <= app.list.len().saturating_sub(viewport),
        "scroll {} clamps to the shorter list",
        app.list_scroll
    );
}

#[test]
fn search_unfolds_ancestors_to_reveal_a_hidden_task() {
    let (_dir, mut app) = setup();
    let (parent, _child, grandchild, _sibling) = fold_fixture(&mut app);

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    assert!(
        app.list.index_of(&grandchild).is_none(),
        "hidden by the fold"
    );

    app.handle_key(key(KeyCode::Char('/')));
    for character in "Grandchild".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    assert_eq!(
        app.search_matches(),
        vec![grandchild.clone()],
        "search sees folded rows"
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_id(), Some(grandchild.clone()));
    assert!(!app.collapsed.contains(&parent), "ancestors unfolded");
    assert!(
        app.list.index_of(&grandchild).is_some(),
        "the target is visible"
    );
}

#[test]
fn link_jump_unfolds_a_hidden_target() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let hidden = add_task(&mut app, "Hidden target", Some(&parent));
    let source = app
        .vault
        .add(NewTask {
            body: format!("[[{hidden}]]"),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.refresh();

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.list.index_of(&hidden).is_none());

    app.selected = Some(source);
    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Link { .. })));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.selected_id(),
        Some(hidden.clone()),
        "the hidden link target is selected"
    );
    assert!(!app.collapsed.contains(&parent), "ancestors unfolded");
    assert!(app.list.index_of(&hidden).is_some());
    assert!(app.toast.is_none(), "no 'not in this project' error");
    assert_eq!(app.mode, InputMode::Navigate);
}

#[test]
fn adding_a_child_under_a_collapsed_parent_expands_it() {
    let (_dir, mut app) = setup();
    let (parent, _child, _grandchild, _sibling) = fold_fixture(&mut app);

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.collapsed.contains(&parent));

    app.handle_key(key(KeyCode::Char('a')));
    for character in "New child".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    let created = app
        .vault
        .tasks()
        .find(|task| task.title == "New child")
        .expect("created task");
    assert_eq!(app.selected_id(), Some(created.id.clone()));
    assert!(!app.collapsed.contains(&parent), "the parent expands");
    assert!(
        app.list.index_of(&created.id).is_some(),
        "the new task is visible"
    );
}

#[test]
fn priority_picker_sets_high_and_none_clears_it() {
    let (_dir, mut app) = setup();
    let task = add_task(&mut app, "Prioritize me", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('!')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Priority)));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.vault.get(&task).expect("task").priority,
        Some(Priority::High)
    );

    app.handle_key(key(KeyCode::Char('!')));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.vault.get(&task).expect("task").priority, None);
}

#[test]
fn tag_picker_toggles_an_existing_tag_and_adds_a_new_query() {
    let (_dir, mut app) = setup();
    let task = app
        .vault
        .add(NewTask {
            tags: vec!["home".to_owned()],
            ..NewTask::new("Tag me")
        })
        .expect("add")
        .id;
    app.vault
        .add(NewTask {
            tags: vec!["work".to_owned()],
            ..NewTask::new("Tag source")
        })
        .expect("add");
    app.refresh();

    app.handle_key(key(KeyCode::Char('t')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Tags)));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.vault.get(&task).expect("task").tags.is_empty());
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("tag #home removed")
    );

    app.handle_key(key(KeyCode::Char('t')));
    for character in "#new".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.vault.get(&task).expect("task").tags,
        vec!["new".to_owned()]
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("tag #new added")
    );
}

#[test]
fn tag_key_uses_a_prompt_when_the_vault_has_no_tags() {
    let (_dir, mut app) = setup();
    let task = add_task(&mut app, "First tag", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('t')));
    assert_eq!(app.mode, InputMode::Tag);
    assert_eq!(app.prompt_prefix().as_deref(), Some("tag: "));
    for character in " #work ".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(
        app.vault.get(&task).expect("task").tags,
        vec!["work".to_owned()]
    );
}

#[test]
fn priority_picker_applies_to_every_marked_task() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();

    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('!')));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.vault.get(&first).expect("first").priority,
        Some(Priority::High)
    );
    assert_eq!(
        app.vault.get(&second).expect("second").priority,
        Some(Priority::High)
    );
    assert!(app.marked.is_empty(), "a bulk action clears the marks");
}

#[test]
fn x_still_toggles_a_collapsed_parent() {
    let (_dir, mut app) = setup();
    let (parent, _child, _grandchild, _sibling) = fold_fixture(&mut app);

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    app.handle_key(key(KeyCode::Char('x')));

    assert_eq!(
        app.vault.get(&parent).expect("parent").state,
        TaskState::Done
    );
    assert!(
        app.collapsed.contains(&parent),
        "the fold survives the change"
    );
    assert_eq!(app.list.index_of(&parent), Some(0));
}

#[test]
fn rollup_counts_collapsed_children() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    add_task(&mut app, "Sibling", Some(&parent));
    app.vault.set_state(&child, TaskState::Done).expect("done");
    app.refresh();

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.collapsed.contains(&parent));
    assert!(app.list.index_of(&child).is_none(), "child hidden");

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(
        text.contains("1/2"),
        "rollup still counts hidden children: {text}"
    );
}

#[test]
fn list_shows_fold_markers() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Alpha", None);
    let parent = add_task(&mut app, "Parent", None);
    add_task(&mut app, "Parent child", Some(&parent));
    app.refresh();

    let text = render_lines(&mut app, 80, 20).join("\n");
    assert!(text.contains('▾'), "expanded parents are marked: {text}");

    app.selected = Some(parent);
    app.handle_key(key(KeyCode::Char('h')));
    let text = render_lines(&mut app, 80, 20).join("\n");
    assert!(text.contains('▸'), "collapsed parents are marked: {text}");
}

#[test]
fn fold_markers_keep_sibling_titles_aligned() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Alpha", None);
    let parent = add_task(&mut app, "Parent", None);
    add_task(&mut app, "Parent child", Some(&parent));
    add_task(&mut app, "Childless", Some(&parent));
    app.refresh();

    let lines = render_lines(&mut app, 80, 20);
    let column = |needle: &str| {
        lines.iter().find_map(|row| {
            let list: String = row.chars().take(44).collect();
            list.find(needle).map(|byte| list[..byte].chars().count())
        })
    };
    assert_eq!(
        column("Parent child"),
        column("Childless"),
        "a fold marker must not shift sibling titles"
    );
}

#[test]
fn selection_stays_visible_when_moving_past_the_viewport() {
    let (_dir, mut app) = setup();
    for index in 0..20 {
        add_task(&mut app, &format!("Task {index:02}"), None);
    }
    app.refresh();

    app.handle_key(key(KeyCode::Char('G')));
    let text = render_lines(&mut app, 60, 8).join("\n");
    assert!(
        text.contains("Task 19"),
        "the selected row must be on screen: {text}"
    );
    assert!(app.list_scroll > 0, "the list scrolls to follow selection");

    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('g')));
    let text = render_lines(&mut app, 60, 8).join("\n");
    assert!(text.contains("Task 00"), "gg scrolls back: {text}");
}

#[test]
fn rows_render_tree_guides_and_state_glyphs() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let done = add_task(&mut app, "Done child", Some(&parent));
    let cancelled = add_task(&mut app, "Cancelled child", Some(&parent));
    app.vault.set_state(&done, TaskState::Done).expect("done");
    app.vault
        .set_state(&cancelled, TaskState::Cancelled)
        .expect("cancel");
    app.refresh();

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(
        text.contains('├') && text.contains('└'),
        "tree guides: {text}"
    );
    assert!(text.contains('○'), "open glyph: {text}");
    assert!(text.contains('●'), "done glyph: {text}");
    assert!(text.contains('—'), "cancelled glyph: {text}");
    assert!(
        !text.contains('╭') && !text.contains('╔') && !text.contains('╚'),
        "map boxes must be gone: {text}"
    );
}

#[test]
fn header_has_no_project_tab_strip_and_shows_the_badge() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md"]);
    add_task(&mut app, "Alpha", None);
    add_task(&mut app, "Beta", None);
    app.refresh();

    let lines = render_lines(&mut app, 80, 20);
    let header = &lines[0];
    assert!(header.contains("⚠ 1 issue"), "badge in header: {header:?}");
    assert!(
        !header.contains("Alpha"),
        "no project tab strip: {header:?}"
    );
    assert!(!header.contains('│'), "no tab separators: {header:?}");
}

#[test]
fn header_shows_the_shortened_project_path() {
    let (_dir, mut app) = setup();
    app.project = Some(project("/work/alpha/beta", "beta"));
    add_task(&mut app, "Only", None);
    app.refresh();

    let lines = render_lines(&mut app, 100, 20);
    assert!(lines[0].contains("/w/a/beta"), "header: {:?}", lines[0]);
    assert!(
        !lines[0].contains("Only"),
        "the header must not list tasks: {:?}",
        lines[0]
    );
}

#[test]
fn header_honors_the_tail_path_style() {
    let (_dir, mut app) = setup();
    app.project = Some(project("/work/alpha/beta/gamma", "gamma"));
    app.config.path_display = PathDisplay {
        style: PathDisplayStyle::Tail,
        tail: 2,
    };
    add_task(&mut app, "Only", None);
    app.refresh();

    let lines = render_lines(&mut app, 100, 20);
    assert!(
        lines[0].contains("/w/a/beta/gamma"),
        "header: {:?}",
        lines[0]
    );
}

#[test]
fn header_shows_the_path_and_issue_badge_together() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md"]);
    app.project = Some(project("/work/alpha/beta", "beta"));
    add_task(&mut app, "Only", None);
    app.refresh();

    let lines = render_lines(&mut app, 100, 20);
    assert!(lines[0].contains("/w/a/beta"), "header: {:?}", lines[0]);
    assert!(lines[0].contains("⚠ 1 issue"), "badge: {:?}", lines[0]);
}

#[test]
fn action_feedback_renders_as_a_toast_and_the_badge_stays_in_the_header() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md"]);
    app.project = Some(project("/work/alpha/beta", "beta"));
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('x')));

    let lines = render_lines(&mut app, 100, 20);
    let text = lines.join("\n");
    assert!(lines[0].contains("⚠ 1 issue"), "badge in header: {text}");
    assert!(text.contains("done Only"), "toast feedback: {text}");
    let footer = lines.last().expect("footer row");
    assert!(
        !footer.contains("done Only"),
        "feedback moved to the toast, not the footer: {footer:?}"
    );
    assert!(
        !footer.contains('⚠'),
        "the badge belongs in the header: {footer:?}"
    );
}

#[test]
fn toast_replaces_the_previous_message_and_expires_on_ticks() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Toggle me", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("done Toggle me")
    );

    app.handle_key(key(KeyCode::Char('x')));
    let toast = app.toast.as_ref().expect("replacement toast");
    assert_eq!(toast.text, "open Toggle me");
    assert_eq!(toast.ticks_left, TOAST_TICKS, "the countdown resets");

    for _ in 0..TOAST_TICKS - 1 {
        app.on_tick();
    }
    assert!(app.toast.is_some(), "still visible before the last tick");
    app.on_tick();
    assert!(
        app.toast.is_none(),
        "expires after exactly TOAST_TICKS ticks"
    );
    assert_eq!(app.vault.get(&id).expect("task").state, TaskState::Open);
}

#[test]
fn toast_never_intercepts_keys() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();
    assert_eq!(app.selected_id(), Some(first));

    app.handle_key(key(KeyCode::Char('x')));
    assert!(app.toast.is_some(), "an action toast is visible");

    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected_id(), Some(second), "j still moves the list");
    assert!(app.toast.is_some(), "moving did not dismiss the toast");

    app.handle_key(key(KeyCode::Esc));
    assert!(app.toast.is_none(), "esc dismisses as a side effect");
}

#[test]
fn footer_is_three_rows_of_hints_and_context() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.project = Some(project("/work/alpha/beta", "beta"));
    app.refresh();

    let lines = render_lines(&mut app, 100, 20);
    let spacer = &lines[lines.len() - 3];
    let hint = &lines[lines.len() - 2];
    let context = &lines[lines.len() - 1];
    assert!(spacer.trim().is_empty(), "spacer row: {spacer:?}");
    assert!(hint.contains("j/k move"), "navigation hint row: {hint:?}");
    assert!(context.contains("beta"), "context row: {context:?}");
    assert!(context.contains("1 task"), "context row: {context:?}");
    assert!(
        !hint.contains("1 task"),
        "hint and context are separate rows"
    );

    let areas = layout(Rect::new(0, 0, 100, 20));
    assert_eq!(areas.spacer.height, 1);
    assert_eq!(areas.hint.height, 1);
    assert_eq!(areas.context.height, 1);
    assert_eq!(areas.middle.height, 16, "header 1 + footer 3");
}

#[test]
fn selected_row_is_reversed() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Yak selected", None);
    app.refresh();

    let cells = render_cells(&mut app, 60, 12);
    let title = style_at_text(&cells, "Yak selected");
    assert!(
        title.add_modifier.contains(Modifier::REVERSED),
        "selected title must be reversed: {title:?}"
    );
}

#[test]
fn tab_toggles_the_cursor_row_mark() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    add_task(&mut app, "Second", None);
    app.refresh();

    app.handle_key(key(KeyCode::Tab));
    assert!(app.marked.contains(&first), "tab marks the cursor row");

    app.handle_key(key(KeyCode::Tab));
    assert!(app.marked.is_empty(), "tab unmarks the cursor row");
}

#[test]
fn marked_rows_get_a_yellow_gutter_background_and_keep_titles_aligned() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();

    // Mark `First`, then move the cursor to `Second`: one row is marked
    // without being the cursor, the other is the cursor without a mark.
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('j')));
    assert!(app.marked.contains(&first), "tab marks the cursor row");

    let width = 60;
    let lines = render_lines(&mut app, width, 12);
    let cells = render_cells(&mut app, width, 12);
    let areas = layout(Rect::new(0, 0, width, 12));
    let list_rows = pane_rows(&lines, areas.list);
    let first_row = list_rows
        .iter()
        .position(|row| row.contains("First"))
        .expect("first row");
    let second_row = list_rows
        .iter()
        .position(|row| row.contains("Second"))
        .expect("second row");

    assert!(
        list_rows[first_row].starts_with("▪ "),
        "a marked row opens with the gutter marker: {:?}",
        list_rows[first_row]
    );
    assert!(
        list_rows[second_row].starts_with("  ") && !list_rows[second_row].contains('▪'),
        "an unmarked row keeps the reserved blank gutter: {:?}",
        list_rows[second_row]
    );
    let column = |row: &str, needle: &str| {
        let byte = row.find(needle).expect("needle");
        row[..byte].chars().count()
    };
    assert_eq!(
        column(&list_rows[first_row], "First"),
        column(&list_rows[second_row], "Second"),
        "the gutter must not shift titles between marked and unmarked rows"
    );

    assert_eq!(
        app.selected_id(),
        Some(second.clone()),
        "the cursor sits on the unmarked row"
    );
    assert!(
        !app.marked.contains(&second),
        "the cursor row itself is not marked"
    );

    let first_style = style_at_text_in(&cells, "First", areas.list);
    assert_eq!(
        first_style.bg,
        Some(Color::Yellow),
        "a marked non-cursor row keeps the mark background: {first_style:?}"
    );
    assert!(
        !first_style.add_modifier.contains(Modifier::REVERSED),
        "a marked row that is not the cursor is not reversed: {first_style:?}"
    );

    let second_style = style_at_text_in(&cells, "Second", areas.list);
    assert!(
        second_style.add_modifier.contains(Modifier::REVERSED),
        "the cursor row stays reversed: {second_style:?}"
    );
    assert_ne!(
        second_style.bg,
        Some(Color::Yellow),
        "the mark background belongs to marked rows only: {second_style:?}"
    );

    // The mark covers the whole row, including the gutter, the tree
    // guides/fold column and the trailing pad, not just the title span.
    let terminal_row = areas.list.y as usize + first_row;
    for (x, (symbol, style)) in cells[terminal_row]
        .iter()
        .enumerate()
        .take(areas.list.width as usize)
    {
        assert_eq!(
            style.bg,
            Some(Color::Yellow),
            "cell {x} ({symbol:?}) of the marked row must carry the mark background"
        );
    }
}

#[test]
fn a_marked_cursor_row_shows_the_gutter_marker_and_reversed_style() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    app.handle_key(key(KeyCode::Tab));

    let width = 60;
    let lines = render_lines(&mut app, width, 12);
    let cells = render_cells(&mut app, width, 12);
    let areas = layout(Rect::new(0, 0, width, 12));
    let list_rows = pane_rows(&lines, areas.list);
    assert!(
        list_rows[0].starts_with("▪ "),
        "the marked cursor row still shows the gutter marker: {:?}",
        list_rows[0]
    );

    let title = style_at_text_in(&cells, "Only", areas.list);
    assert!(
        title.add_modifier.contains(Modifier::REVERSED),
        "the marked cursor row stays reversed: {title:?}"
    );
    assert_eq!(
        title.bg,
        Some(Color::Yellow),
        "the mark background composes under the cursor reversal: {title:?}"
    );
}

#[test]
fn esc_clears_marks_first_and_a_second_esc_dismisses_the_toast() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Only", None);
    app.refresh();
    app.set_toast("keep me");
    app.handle_key(key(KeyCode::Tab));
    assert!(app.marked.contains(&id));

    app.handle_key(key(KeyCode::Esc));
    assert!(
        app.marked.is_empty(),
        "esc clears the multi-selection first"
    );
    assert!(
        app.toast.is_some(),
        "the first esc leaves the toast alone: {:?}",
        app.toast
    );

    app.handle_key(key(KeyCode::Esc));
    assert!(app.toast.is_none(), "a second esc dismisses the toast");
}

#[test]
fn selection_hints_replace_the_list_hints_while_marked() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    assert_eq!(
        footer_text(&app.status_line()),
        "j/k move   a add   x state   p projects   / find   ? keys"
    );

    app.handle_key(key(KeyCode::Tab));
    assert_eq!(
        footer_text(&app.status_line()),
        "tab un/select   esc clear   m move   d delete   x state   ! priority   t tags"
    );

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(
        footer_text(&app.status_line()),
        "j/k move   a add   x state   p projects   / find   ? keys"
    );
}

#[test]
fn marks_survive_folds_and_search() {
    let (_dir, mut app) = setup();
    let root = add_task(&mut app, "Root", None);
    let child = add_task(&mut app, "Child", Some(&root));
    app.refresh();

    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.selected_id(), Some(child.clone()));
    app.handle_key(key(KeyCode::Tab));
    assert!(app.marked.contains(&child));

    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.selected_id(), Some(root));
    app.handle_key(key(KeyCode::Char('h')));
    assert!(
        app.list.index_of(&child).is_none(),
        "the child is folded away"
    );
    assert!(app.marked.contains(&child), "a hidden row stays marked");

    app.handle_key(key(KeyCode::Char('/')));
    for character in "child".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Esc));
    assert!(app.marked.contains(&child), "search keeps marks");
}

#[test]
fn reload_prunes_marks_for_vanished_tasks() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();
    app.handle_key(key(KeyCode::Tab));
    assert!(app.marked.contains(&first));

    app.vault
        .delete(std::slice::from_ref(&first))
        .expect("delete");
    app.reload_now();

    assert!(app.marked.is_empty(), "the vanished mark is pruned");
    assert!(app.vault.get(&second).is_some());
}

#[test]
fn x_cycles_all_marked_tasks_and_clears_marks() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();

    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.marked.len(), 2);

    app.handle_key(key(KeyCode::Char('x')));

    assert_eq!(app.vault.get(&first).expect("first").state, TaskState::Done);
    assert_eq!(
        app.vault.get(&second).expect("second").state,
        TaskState::Done
    );
    assert!(app.marked.is_empty(), "a mutating action clears the marks");
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("cycled 2 tasks")
    );
}

#[test]
fn x_on_a_single_marked_task_reports_a_summary_and_clears_the_mark() {
    let (_dir, mut app) = setup();
    let only = add_task(&mut app, "Only", None);
    app.refresh();

    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('x')));

    assert_eq!(app.vault.get(&only).expect("task").state, TaskState::Done);
    assert!(app.marked.is_empty(), "the single mark is consumed");
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("cycled 1 task")
    );
}

#[test]
fn move_picker_offers_root_first_and_hides_the_moving_subtree() {
    let (_dir, mut app) = setup();
    let root = add_task(&mut app, "Root", None);
    let child = add_task(&mut app, "Child", Some(&root));
    let grandchild = add_task(&mut app, "Grandchild", Some(&child));
    let other = add_task(&mut app, "Other", None);
    app.refresh();

    app.selected = Some(root.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('m')));

    assert_eq!(
        app.picker_kind(),
        Some(&PickerKind::Move {
            moving: vec![root.clone()]
        })
    );
    assert_eq!(
        app.move_matches(),
        vec![None, Some(other.clone())],
        "root entry first, then the one task outside the moving subtree"
    );
    assert!(
        footer_text(&app.status_line()).contains("move under: "),
        "the prompt names the picker: {}",
        footer_text(&app.status_line())
    );

    let lines = render_lines(&mut app, 100, 20);
    let text = lines.join("\n");
    assert!(text.contains("move under…"), "popup title: {text}");
    assert!(text.contains("⌂ root"), "the root entry renders: {text}");

    // Folding a moving task must not re-admit its hidden descendants.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Char('h')));
    assert!(app.collapsed.contains(&root));
    app.handle_key(key(KeyCode::Char('m')));
    let matches = app.move_matches();
    assert!(
        !matches.contains(&Some(child.clone())) && !matches.contains(&Some(grandchild.clone())),
        "descendants stay excluded behind a fold: {matches:?}"
    );
}

#[test]
fn move_picker_filters_by_query_but_keeps_root_first() {
    let (_dir, mut app) = setup();
    let root = add_task(&mut app, "Root", None);
    let alpha = add_task(&mut app, "Alpha", Some(&root));
    let beta = add_task(&mut app, "Beta", Some(&root));
    app.refresh();

    app.selected = Some(beta.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('m')));
    for character in "alpha".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }

    assert_eq!(
        app.move_matches(),
        vec![None, Some(alpha)],
        "query filters tasks, and the root entry always stays first"
    );
}

#[test]
fn move_commit_reparents_marks_and_selects_the_first_moved_task() {
    let (_dir, mut app) = setup();
    let root = add_task(&mut app, "Root", None);
    let child = add_task(&mut app, "Child", Some(&root));
    let other = add_task(&mut app, "Other", None);
    app.refresh();

    app.selected = Some(root.clone());
    app.handle_key(key(KeyCode::Tab));
    app.selected = Some(child.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('m')));
    assert_eq!(
        app.move_matches(),
        vec![None, Some(other.clone())],
        "the moving set is excluded"
    );

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.vault.parent(&root), Some(&other));
    assert_eq!(app.vault.parent(&child), Some(&other));
    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(
        app.selected_id(),
        Some(root.clone()),
        "the first moved task is selected"
    );
    assert!(app.marked.is_empty(), "a successful move clears the marks");
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("moved 2 tasks")
    );
}

#[test]
fn move_to_root_clears_the_parent() {
    let (_dir, mut app) = setup();
    let root = add_task(&mut app, "Root", None);
    let child = add_task(&mut app, "Child", Some(&root));
    app.refresh();

    app.selected = Some(child.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('m')));
    assert_eq!(
        app.move_matches(),
        vec![None, Some(root.clone())],
        "the root entry comes first; the current parent is also a candidate"
    );

    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.vault.parent(&child), None);
    assert!(app.vault.roots().contains(&child));
    assert_eq!(app.selected_id(), Some(child.clone()));
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("moved 1 task")
    );
    assert!(app.marked.is_empty());
}

#[test]
fn move_with_a_single_task_offers_only_the_root_entry() {
    let (_dir, mut app) = setup();
    let only = add_task(&mut app, "Only", None);
    app.refresh();

    app.selected = Some(only.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('m')));
    assert_eq!(
        app.move_matches(),
        vec![None],
        "with every other task moving, only ⌂ root is left"
    );

    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.vault.parent(&only), None);
    assert!(app.marked.is_empty());
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("moved 1 task")
    );
}

#[test]
fn move_unfolds_the_new_parents_ancestors() {
    let (_dir, mut app) = setup();
    let alpha = add_task(&mut app, "Alpha", None);
    let beta = add_task(&mut app, "Beta", Some(&alpha));
    let gamma = add_task(&mut app, "Gamma", None);
    app.refresh();

    // Collapse Alpha, then move Gamma under Beta, inside the fold.
    app.collapsed.insert(alpha.clone());
    app.selected = Some(gamma.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('m')));
    // Candidates: root, Alpha, Beta (Gamma is excluded).
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.vault.parent(&gamma), Some(&beta));
    assert!(!app.collapsed.contains(&alpha), "the new ancestry unfolds");
    assert!(
        app.list.index_of(&gamma).is_some(),
        "the moved task is visible"
    );
    assert_eq!(app.selected_id(), Some(gamma));
}

#[test]
fn delete_confirmation_defaults_to_cancel_and_enter_cancels() {
    let (dir, mut app) = setup();
    let id = add_task(&mut app, "Doomed", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.mode, InputMode::ConfirmDelete { .. }));
    assert_eq!(
        app.confirm_delete_button(),
        1,
        "Cancel is highlighted by default"
    );
    assert_eq!(
        app.confirm_delete_lines(),
        vec!["Delete \"Doomed\"?".to_owned()]
    );

    let lines = render_lines(&mut app, 100, 20);
    let text = lines.join("\n");
    assert!(
        text.contains("Delete \"Doomed\"?"),
        "question renders: {text}"
    );
    assert!(
        text.contains("[ Delete ]") && text.contains("[ Cancel ]"),
        "both buttons render: {text}"
    );
    let hint = &lines[lines.len() - 2];
    assert!(hint.contains("enter confirm"), "confirm hints: {hint:?}");

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::Navigate);
    assert!(
        app.vault.get(&id).is_some(),
        "Enter on Cancel deletes nothing"
    );
    assert!(dir.path().join(format!("{id}.md")).exists());
}

#[test]
fn delete_confirmation_counts_descendants_excluding_marked_tasks() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    let _sibling = add_task(&mut app, "Sibling", Some(&parent));
    app.refresh();

    // Mark the parent and one child: the unmarked sibling is the only task
    // beyond the marked set, so the count excludes the marked child.
    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Tab));
    app.selected = Some(child.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('d')));

    assert_eq!(
        app.confirm_delete_lines(),
        vec!["Delete 2 tasks and their 1 descendant?".to_owned()]
    );
}

#[test]
fn delete_confirmation_omits_the_descendant_clause_for_a_leaf() {
    let (_dir, mut app) = setup();
    let leaf = add_task(&mut app, "Leaf", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(
        app.confirm_delete_lines(),
        vec!["Delete \"Leaf\"?".to_owned()]
    );

    app.handle_key(key(KeyCode::Esc));
    assert!(app.vault.get(&leaf).is_some());
}

#[test]
fn y_confirms_a_subtree_delete_and_reports_the_total() {
    let (dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    app.refresh();

    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(
        app.confirm_delete_lines(),
        vec!["Delete \"Parent\" and its 1 descendant?".to_owned()]
    );

    app.handle_key(key(KeyCode::Char('y')));

    assert_eq!(app.mode, InputMode::Navigate);
    assert!(app.vault.get(&parent).is_none());
    assert!(
        app.vault.get(&child).is_none(),
        "the descendant is deleted too, not reparented"
    );
    assert!(!dir.path().join(format!("{parent}.md")).exists());
    assert!(!dir.path().join(format!("{child}.md")).exists());
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("deleted 2 tasks")
    );
}

#[test]
fn marked_multi_delete_removes_whole_subtrees_and_clears_marks() {
    let (dir, mut app) = setup();
    let grandparent = add_task(&mut app, "Grandparent", None);
    let parent = add_task(&mut app, "Parent", Some(&grandparent));
    let child = add_task(&mut app, "Child", Some(&parent));
    let sibling = add_task(&mut app, "Sibling", Some(&parent));
    let bystander = add_task(&mut app, "Bystander", None);
    app.refresh();

    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Tab));
    app.selected = Some(child.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(
        app.confirm_delete_lines(),
        vec!["Delete 2 tasks and their 1 descendant?".to_owned()]
    );

    app.handle_key(key(KeyCode::Char('y')));

    assert_eq!(app.mode, InputMode::Navigate);
    assert!(app.vault.get(&parent).is_none());
    assert!(app.vault.get(&child).is_none());
    assert!(
        app.vault.get(&sibling).is_none(),
        "an unmarked descendant dies with the marked parent"
    );
    assert!(!dir.path().join(format!("{parent}.md")).exists());
    assert!(!dir.path().join(format!("{child}.md")).exists());
    assert!(!dir.path().join(format!("{sibling}.md")).exists());
    assert!(app.vault.get(&grandparent).is_some());
    assert!(app.vault.get(&bystander).is_some());
    assert!(
        app.marked.is_empty(),
        "a successful delete clears the marks"
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("deleted 3 tasks")
    );
}

#[test]
fn button_navigation_commits_and_d_confirms_directly() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(app.confirm_delete_button(), 1);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.confirm_delete_button(), 0, "left highlights Delete");
    app.handle_key(key(KeyCode::Enter));
    assert!(app.vault.get(&first).is_none());
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("deleted 1 task")
    );

    // The cursor fell back to the remaining task; `d` confirms directly even
    // though Cancel is the highlighted default.
    app.handle_key(key(KeyCode::Char('d')));
    assert_eq!(app.confirm_delete_button(), 1);
    app.handle_key(key(KeyCode::Char('d')));
    assert!(app.vault.get(&second).is_none());
    assert_eq!(app.mode, InputMode::Navigate);
}

#[test]
fn esc_and_n_cancel_the_delete_and_tab_moves_the_button() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Safe", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('d')));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.confirm_delete_button(), 0, "tab moves to Delete");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Navigate);
    assert!(app.vault.get(&id).is_some());

    app.handle_key(key(KeyCode::Char('d')));
    app.handle_key(key(KeyCode::Char('n')));
    assert_eq!(app.mode, InputMode::Navigate);
    assert!(app.vault.get(&id).is_some(), "n cancels without deleting");
}

#[test]
fn shift_l_appends_a_wikilink_to_an_empty_body() {
    let (dir, mut app) = setup();
    let target = add_task(&mut app, "Target", None);
    let source = add_task(&mut app, "Source", None);
    app.refresh();

    app.selected = Some(source.clone());
    app.handle_key(key(KeyCode::Char('L')));
    assert_eq!(
        app.picker_kind(),
        Some(&PickerKind::CreateLink {
            source: source.clone()
        })
    );
    assert_eq!(
        app.create_link_matches(),
        vec![target.clone()],
        "only the other task is offered"
    );
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("link to…"), "popup title: {text}");

    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.vault.get(&source).expect("source").body,
        format!("[[{target}.md|Target]]")
    );
    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("linked to Target")
    );
    let stored = Task::from_document(
        &fs::read_to_string(dir.path().join(format!("{source}.md"))).expect("read"),
    )
    .expect("parse");
    assert_eq!(stored.body, format!("[[{target}.md|Target]]"));
    assert_eq!(app.vault.links(&source), &[target.clone()][..]);
    assert_eq!(app.vault.backlinks(&target), &[source.clone()][..]);

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("Target"),
        "the body's wikilink resolves to its live title: {preview}"
    );
    assert!(
        !preview.contains('⇄'),
        "link counts are gone from the preview: {preview}"
    );
}

#[test]
fn shift_l_separates_the_link_from_an_existing_body() {
    let (_dir, mut app) = setup();
    let target = add_task(&mut app, "Target", None);
    let source = app
        .vault
        .add(NewTask {
            body: "first line\n".to_owned(),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.refresh();

    app.selected = Some(source.clone());
    app.handle_key(key(KeyCode::Char('L')));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.vault.get(&source).expect("source").body,
        format!("first line\n\n[[{target}.md|Target]]"),
        "a blank line separates the link and trailing newlines collapse"
    );
}

#[test]
fn shift_l_falls_back_to_the_bare_path_for_pipe_and_bracket_titles() {
    let (_dir, mut app) = setup();
    let bracket = add_task(&mut app, "Bracket ] title", None);
    let pipe = add_task(&mut app, "Pipe | title", None);
    let source = app
        .vault
        .add(NewTask::new("Source"))
        .expect("add source")
        .id;
    app.refresh();
    app.selected = Some(source.clone());

    app.handle_key(key(KeyCode::Char('L')));
    let matches = app.create_link_matches();
    let highlighted = matches
        .iter()
        .position(|id| id == &bracket)
        .expect("bracket candidate");
    for _ in 0..highlighted {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.vault.get(&source).expect("source").body,
        format!("[[{bracket}.md]]"),
        "a `]` title cannot carry an alias"
    );

    app.handle_key(key(KeyCode::Char('L')));
    let matches = app.create_link_matches();
    let highlighted = matches
        .iter()
        .position(|id| id == &pipe)
        .expect("pipe candidate");
    for _ in 0..highlighted {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.vault.get(&source).expect("source").body,
        format!("[[{bracket}.md]]\n\n[[{pipe}.md]]"),
        "a `|` title cannot carry an alias either"
    );
    assert_eq!(
        app.vault.links(&source),
        &[bracket.clone(), pipe.clone()][..],
        "both bare-path links still index by stem"
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("linked to Pipe | title"),
        "the toast still uses the live title"
    );
}

#[test]
fn shift_l_ignores_marks_folds_and_excludes_the_cursor_task() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    let other = add_task(&mut app, "Other", None);
    app.refresh();

    // Fold the parent so the child is hidden, mark both roots, and link from
    // "Other": the source is the cursor, not the marked set, and folded
    // tasks are still candidates.
    app.collapsed.insert(parent.clone());
    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Tab));
    app.selected = Some(other.clone());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Char('L')));

    assert_eq!(
        app.picker_kind(),
        Some(&PickerKind::CreateLink {
            source: other.clone()
        }),
        "L links from the cursor and ignores the marks"
    );
    let matches = app.create_link_matches();
    assert!(
        matches.contains(&parent) && matches.contains(&child),
        "even a folded task can be linked: {matches:?}"
    );
    assert!(!matches.contains(&other), "the source is excluded");
}

#[test]
fn shift_l_with_no_other_tasks_toasts_and_does_not_open() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('L')));

    assert_eq!(app.mode, InputMode::Navigate, "the picker does not open");
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("no tasks to link")
    );
}

#[test]
fn shift_l_leaves_o_and_search_intact() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Target", None);
    let source = add_task(&mut app, "Source", None);
    app.refresh();

    app.selected = Some(source);
    app.handle_key(key(KeyCode::Char('L')));
    app.handle_key(key(KeyCode::Enter));

    app.handle_key(key(KeyCode::Char('o')));
    assert!(
        matches!(app.picker_kind(), Some(PickerKind::Link { .. })),
        "o still jumps through links"
    );
    app.handle_key(key(KeyCode::Esc));

    app.handle_key(key(KeyCode::Char('/')));
    assert!(
        matches!(app.picker_kind(), Some(PickerKind::Search { .. })),
        "/ still searches"
    );
    app.handle_key(key(KeyCode::Esc));
}

#[test]
fn r_opens_a_prefilled_rename_prompt_with_the_cursor_at_the_end() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Old title", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('r')));

    assert!(
        matches!(app.mode, InputMode::Rename { id: ref target } if target == &id),
        "r opens the rename prompt on the cursor task"
    );
    assert_eq!(app.input, "Old title", "the buffer is prefilled");
    assert_eq!(footer_text(&app.status_line()), "rename to: Old title");

    // The rendered cursor sits after the prompt prefix and the prefilled
    // title, i.e. at the end of the buffer.
    let area = layout(Rect::new(0, 0, 80, 20));
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    app.set_list_viewport(area.list.width, area.list.height);
    terminal.draw(|frame| render(frame, &app)).expect("draw");
    let prefix = "rename to: ";
    assert_eq!(
        terminal.backend().cursor_position(),
        Position::new(
            area.hint.x + (prefix.chars().count() + app.input_buffer_len()) as u16,
            area.hint.y,
        ),
        "cursor at the end of the prefilled buffer"
    );
}

#[test]
fn r_renames_and_cascades_mirror_aliases_in_the_same_action() {
    let (dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Old title"))
        .expect("add target");
    let mirror = app
        .vault
        .add(NewTask {
            body: format!("see [[{}.md|Old title]]", target.id),
            ..NewTask::new("Mirror")
        })
        .expect("add mirror");
    let contextual = app
        .vault
        .add(NewTask {
            body: format!("see [[{}.md|my words]] and [[{}.md]]", target.id, target.id),
            ..NewTask::new("Contextual")
        })
        .expect("add contextual");
    app.refresh();

    // Mark the mirror, then put the cursor on the target: `r` ignores the
    // marks and the marks must survive the rename.
    app.selected = Some(mirror.id.clone());
    app.handle_key(key(KeyCode::Tab));
    app.selected = Some(target.id.clone());

    app.handle_key(key(KeyCode::Char('r')));
    while !app.input.is_empty() {
        app.handle_key(key(KeyCode::Backspace));
    }
    for character in "New title".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    // The target's frontmatter and the mirror alias changed in the same
    // commit; the contextual alias and the bare link are untouched. Exact
    // file bytes, not just the parsed cache.
    let mut renamed = target.clone();
    renamed.title = "New title".to_owned();
    assert_eq!(
        fs::read_to_string(dir.path().join(format!("{}.md", target.id))).expect("read target"),
        renamed.to_document(),
        "title rewritten on disk"
    );
    let mut mirror_after = mirror.clone();
    mirror_after.body = format!("see [[{}.md|New title]]", target.id);
    assert_eq!(
        fs::read_to_string(dir.path().join(format!("{}.md", mirror.id))).expect("read mirror"),
        mirror_after.to_document(),
        "mirror alias rewritten in the same action"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(format!("{}.md", contextual.id))).expect("read"),
        contextual.to_document(),
        "contextual aliases and bare links are never rewritten"
    );
    assert_eq!(
        app.vault.get(&target.id).expect("target").title,
        "New title"
    );
    assert_eq!(
        app.vault.get(&mirror.id).expect("mirror").body,
        format!("see [[{}.md|New title]]", target.id)
    );
    assert!(app.marked.contains(&mirror.id), "marks survive a rename");
    assert_eq!(
        app.selected_id(),
        Some(target.id.clone()),
        "the cursor stays on the renamed task"
    );
    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("renamed to New title · 1 link updated")
    );
}

#[test]
fn r_without_mirrors_toasts_the_plain_rename() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Old title", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('r')));
    while !app.input.is_empty() {
        app.handle_key(key(KeyCode::Backspace));
    }
    for character in "New title".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("renamed to New title")
    );
}

#[test]
fn esc_cancels_rename_with_no_writes() {
    let (dir, mut app) = setup();
    let id = add_task(&mut app, "Old title", None);
    app.refresh();
    let path = dir.path().join(format!("{id}.md"));
    let before = fs::read_to_string(&path).expect("read");

    app.handle_key(key(KeyCode::Char('r')));
    for character in " changed".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Esc));

    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(app.input, "");
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        before,
        "Esc cancels with zero writes"
    );
    assert_eq!(app.vault.get(&id).expect("task").title, "Old title");
    assert!(app.toast.is_none(), "Esc cancels quietly");
}

#[test]
fn a_blank_rename_title_is_rejected_like_add() {
    let (dir, mut app) = setup();
    let id = add_task(&mut app, "Old title", None);
    app.refresh();
    let path = dir.path().join(format!("{id}.md"));
    let before = fs::read_to_string(&path).expect("read");

    app.handle_key(key(KeyCode::Char('r')));
    while !app.input.is_empty() {
        app.handle_key(key(KeyCode::Backspace));
    }
    for character in "  ".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        before,
        "a blank title writes nothing"
    );
    assert_eq!(app.vault.get(&id).expect("task").title, "Old title");
}

#[test]
fn cancelled_rows_are_struck_through() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Washed out", None);
    app.vault
        .set_state(&id, TaskState::Cancelled)
        .expect("cancel");
    app.refresh();

    let cells = render_cells(&mut app, 60, 12);
    let title = style_at_text(&cells, "Washed out");
    assert!(
        title.add_modifier.contains(Modifier::CROSSED_OUT),
        "cancelled title must be struck through: {title:?}"
    );
    assert!(
        title.add_modifier.contains(Modifier::DIM),
        "cancelled title must be dim: {title:?}"
    );
}

#[test]
fn list_rows_indent_by_depth_and_share_metadata_columns() {
    let (_dir, mut app) = setup();
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    app.today = today;
    let all_fields = app
        .vault
        .add(NewTask {
            due: Some(today),
            priority: Some(Priority::High),
            ..NewTask::new("All fields")
        })
        .expect("add")
        .id;
    add_task(&mut app, "Nested bare", Some(&all_fields));
    app.vault
        .add(NewTask {
            due: Some(today),
            ..NewTask::new("Wide 猫")
        })
        .expect("add wide");
    app.vault
        .add(NewTask {
            due: NaiveDate::from_ymd_opt(2020, 1, 1),
            ..NewTask::new("Due rollup")
        })
        .expect("add due parent");
    let priority_rollup = app
        .vault
        .add(NewTask {
            priority: Some(Priority::Low),
            ..NewTask::new("Priority rollup")
        })
        .expect("add priority parent")
        .id;
    add_task(&mut app, "Priority child", Some(&priority_rollup));
    let due_rollup = app
        .vault
        .tasks()
        .find(|task| task.title == "Due rollup")
        .expect("due parent")
        .id
        .clone();
    add_task(&mut app, "Due child", Some(&due_rollup));
    app.refresh();

    let width = 120;
    let areas = layout(Rect::new(0, 0, width, 20));
    let rows = pane_rows(&render_lines(&mut app, width, 20), areas.list);
    let row = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row containing {needle:?}: {rows:?}"))
    };

    let fold_columns =
        ["All fields", "Due rollup", "Priority rollup"].map(|title| column(row(title), "▾"));
    assert!(
        fold_columns
            .windows(2)
            .all(|columns| columns[0] == columns[1]),
        "fold markers share one fixed column: {rows:?}"
    );
    assert_eq!(
        column(row("Nested bare"), "└"),
        column(row("Priority child"), "└"),
        "same-depth connectors share one indent column"
    );
    // Tree-view indentation: a child's connector starts at its parent's
    // content start, and the child's content sits one step further right.
    assert_eq!(
        column(row("Nested bare"), "└"),
        column(row("All fields"), "▾"),
        "a child's connector starts at its parent's content start: {rows:?}"
    );
    for (parent, child) in [
        ("All fields", "Nested bare"),
        ("Priority rollup", "Priority child"),
        ("Due rollup", "Due child"),
    ] {
        assert!(
            column(row(child), "○") > column(row(parent), "○"),
            "the depth-1 state glyph indents past its parent: {rows:?}"
        );
        assert!(
            column(row(child), child) > column(row(parent), parent),
            "the depth-1 title indents past its parent: {rows:?}"
        );
    }
    assert_eq!(
        column(row("All fields"), "today"),
        column(row("Due rollup"), "overdue"),
        "due values share one fixed column"
    );
    assert_eq!(
        column(row("All fields"), "today"),
        column(row("Wide 猫"), "today"),
        "a wide title does not shift the due column"
    );
    assert_eq!(
        column(row("All fields"), "!"),
        column(row("Priority rollup"), "↓"),
        "priority stays aligned when due is missing"
    );
    assert_eq!(
        column(row("All fields"), "0/1"),
        column(row("Due rollup"), "0/1"),
        "rollup stays aligned when priority is missing"
    );
}

#[test]
fn deeper_rows_indent_further_right_and_continuation_bars_line_up() {
    let (_dir, mut app) = setup();
    let root = add_task(&mut app, "Root", None);
    let first = add_task(&mut app, "First child", Some(&root));
    let grandchild = add_task(&mut app, "Grandchild", Some(&first));
    let last = add_task(&mut app, "Last child", Some(&root));
    app.refresh();

    let width = 100;
    let areas = layout(Rect::new(0, 0, width, 20));
    let rows = pane_rows(&render_lines(&mut app, width, 20), areas.list);
    let row = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row containing {needle:?}: {rows:?}"))
    };

    // Content indents one step per depth level.
    assert!(
        column(row("First child"), "○") > column(row("Root"), "○"),
        "depth-1 content starts further right than depth-0: {rows:?}"
    );
    assert!(
        column(row("Grandchild"), "○") > column(row("First child"), "○"),
        "depth-2 content starts further right than depth-1: {rows:?}"
    );
    assert!(
        column(row("Grandchild"), "Grandchild") > column(row("First child"), "First child"),
        "depth-2 titles start further right than depth-1: {rows:?}"
    );
    // A child's connector starts at its parent's content start.
    assert_eq!(
        column(row("First child"), "├"),
        column(row("Root"), "▾"),
        "the depth-1 connector starts at the root's content start: {rows:?}"
    );
    assert_eq!(
        column(row("Grandchild"), "└"),
        column(row("First child"), "▾"),
        "the depth-2 connector starts at its parent's content start: {rows:?}"
    );
    // Continuation bars line up column-by-column for non-last ancestors.
    assert_eq!(
        column(row("Grandchild"), "│"),
        column(row("First child"), "├"),
        "the continuation bar lines up under the non-last parent: {rows:?}"
    );
    // Same-depth siblings share one indent column.
    assert_eq!(
        column(row("First child"), "First child"),
        column(row("Last child"), "Last child"),
        "sibling titles stay aligned: {rows:?}"
    );
    assert!(app.vault.get(&grandchild).is_some() && app.vault.get(&last).is_some());
}

#[test]
fn narrow_list_indents_the_child_and_clips_predictably() {
    let (_dir, mut app) = setup();
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    app.today = today;
    let parent = app
        .vault
        .add(NewTask {
            due: Some(today),
            priority: Some(Priority::High),
            ..NewTask::new("All metadata")
        })
        .expect("add")
        .id;
    add_task(&mut app, "Nested title", Some(&parent));
    app.refresh();

    let width = 40;
    let areas = layout(Rect::new(0, 0, width, 12));
    let rows = pane_rows(&render_lines(&mut app, width, 12), areas.list);
    let parent = rows.iter().find(|row| row.contains("All")).expect("parent");
    let child = rows.iter().find(|row| row.contains("Nes")).expect("child");
    assert!(
        column(child, "Nes") > column(parent, "All"),
        "the nested title indents past its parent even at narrow widths: {rows:?}"
    );
    assert_eq!(
        column(child, "└"),
        column(parent, "▾"),
        "the child connector starts at the parent's content start: {rows:?}"
    );
    for row in [parent, child] {
        assert!(
            row.chars().count() <= areas.list.width as usize,
            "rows clip to the list pane instead of overflowing: {rows:?}"
        );
    }
}

#[test]
fn overdue_and_today_due_meta_render_red() {
    let (_dir, mut app) = setup();
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    app.today = today;
    app.vault
        .add(NewTask {
            due: NaiveDate::from_ymd_opt(2020, 1, 1),
            ..NewTask::new("Zebra urgency")
        })
        .expect("add");
    app.refresh();

    let cells = render_cells(&mut app, 60, 12);
    let overdue = style_at_text(&cells, "overdue");
    assert_eq!(overdue.fg, Some(Color::Red), "{overdue:?}");

    app.vault
        .add(NewTask {
            due: Some(today),
            ..NewTask::new("Today task")
        })
        .expect("add");
    app.refresh();
    let cells = render_cells(&mut app, 60, 12);
    let today_style = style_at_text(&cells, "today");
    assert_eq!(today_style.fg, Some(Color::Red), "{today_style:?}");
}

#[test]
fn priority_and_rollup_render_without_link_marks() {
    let (_dir, mut app) = setup();
    let parent = app
        .vault
        .add(NewTask {
            priority: Some(Priority::High),
            ..NewTask::new("Parent high")
        })
        .expect("add")
        .id;
    let child = add_task(&mut app, "Child one", Some(&parent));
    add_task(&mut app, "Child two", Some(&parent));
    app.vault.set_state(&child, TaskState::Done).expect("done");
    let linked = app.vault.add(NewTask::new("Linked")).expect("add").id;
    app.vault
        .add(NewTask {
            body: format!("[[{linked}]]"),
            ..NewTask::new("Source")
        })
        .expect("add");
    app.refresh();
    app.selected = Some(parent.clone());

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let list = pane_rows(&lines, areas.list).join("\n");
    assert!(list.contains('!'), "high priority marker: {list}");
    assert!(list.contains("1/2"), "open rollup: {list}");
    assert!(!list.contains('⇄'), "list rows carry no link mark: {list}");

    let cells = render_cells(&mut app, 120, 20);
    let list_priority = style_at_text_in(&cells, "!", areas.list);
    assert_eq!(list_priority.bg, Some(Color::Red), "{list_priority:?}");
    assert_eq!(
        list_priority.fg,
        Some(Color::Rgb(255, 255, 255)),
        "{list_priority:?}"
    );
    assert!(
        !list_priority.add_modifier.contains(Modifier::REVERSED),
        "the selected row must not invert the priority contrast: {list_priority:?}"
    );
    let preview_priority = style_at_text_in(&cells, "! high", areas.preview);
    assert_eq!(
        preview_priority.bg,
        Some(Color::Red),
        "{preview_priority:?}"
    );
    assert_eq!(
        preview_priority.fg,
        Some(Color::Rgb(255, 255, 255)),
        "{preview_priority:?}"
    );
}

#[test]
fn state_filter_flattens_matching_parent_and_child() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    let done = add_task(&mut app, "Done", Some(&parent));
    app.vault.set_state(&done, TaskState::Done).expect("done");
    app.refresh();

    app.handle_key(key(KeyCode::Char('f')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Filter)));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.list.len(), 2);
    for id in [&parent, &child] {
        let row = &app.list.rows()[app.list.index_of(id).expect("matching row")];
        assert_eq!(row.depth, 0, "filtered rows are all roots");
        assert!(!row.has_children, "filtered rows have no tree markers");
        assert!(row.guides().is_empty());
    }
    assert!(app.list.index_of(&done).is_none());
    assert!(app.context_text().contains("[filter open]"));
}

#[test]
fn clearing_filter_restores_tree_without_writing_files() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    app.refresh();
    app.selected = Some(child.clone());
    let parent_path = app.vault.root().join(format!("{parent}.md"));
    let child_path = app.vault.root().join(format!("{child}.md"));
    let before = (
        fs::read(&parent_path).expect("read parent"),
        fs::read(&child_path).expect("read child"),
    );

    app.handle_key(key(KeyCode::Char('f')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_id(), Some(child.clone()));
    assert_eq!(app.list.rows()[app.list.index_of(&child).unwrap()].depth, 0);

    app.handle_key(key(KeyCode::Char('f')));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.active_filter.is_none());
    assert_eq!(app.selected_id(), Some(child.clone()));
    let child_row = &app.list.rows()[app.list.index_of(&child).expect("child row")];
    assert_eq!(child_row.depth, 1, "the tree is restored");
    assert!(app.list.rows()[app.list.index_of(&parent).unwrap()].has_children);
    assert_eq!(
        before,
        (
            fs::read(parent_path).expect("read parent"),
            fs::read(child_path).expect("read child"),
        ),
        "filtering is entirely in memory"
    );
}

#[test]
fn tag_filter_flattens_matching_rows() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    let other = add_task(&mut app, "Other", None);
    app.vault
        .set_tags(&parent, vec!["work".to_owned()])
        .expect("tag parent");
    app.vault
        .set_tags(&child, vec!["work".to_owned()])
        .expect("tag child");
    app.refresh();

    app.handle_key(key(KeyCode::Char('f')));
    for character in "#work".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.list.len(), 2);
    assert!(app.list.index_of(&other).is_none());
    assert!(app.list.rows().iter().all(|row| row.depth == 0));
    assert!(app.context_text().contains("[filter #work]"));
}

#[test]
fn priority_filter_flattens_matching_rows() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    let other = add_task(&mut app, "Other", None);
    app.vault
        .set_priority(&parent, Some(Priority::High))
        .expect("prioritize parent");
    app.vault
        .set_priority(&child, Some(Priority::High))
        .expect("prioritize child");
    app.refresh();

    app.handle_key(key(KeyCode::Char('f')));
    for character in "high".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.list.len(), 2);
    assert!(app.list.index_of(&other).is_none());
    assert!(app.list.rows().iter().all(|row| row.depth == 0));
    assert!(app.context_text().contains("[filter high]"));
}

#[test]
fn filtered_search_only_matches_visible_rows_and_folds_are_no_ops() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Needle parent", None);
    let child = add_task(&mut app, "Needle child", Some(&parent));
    let hidden = add_task(&mut app, "Needle done", None);
    app.vault
        .set_state(&hidden, TaskState::Done)
        .expect("mark done");
    app.refresh();

    app.handle_key(key(KeyCode::Char('f')));
    app.handle_key(key(KeyCode::Enter));
    app.selected = Some(parent.clone());
    app.handle_key(key(KeyCode::Char('h')));
    app.handle_key(key(KeyCode::Char('l')));
    assert_eq!(app.selected_id(), Some(parent));
    assert_eq!(app.list.len(), 2, "fold keys do not change a flat filter");
    assert!(app.list.index_of(&child).is_some());

    app.handle_key(key(KeyCode::Char('/')));
    for character in "needle".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    let matches = app.search_matches();
    assert_eq!(matches.len(), 2);
    assert!(
        !matches.contains(&hidden),
        "hidden filtered rows do not match"
    );
}

#[test]
fn a_new_nonmatching_task_stays_hidden_by_the_filter() {
    let (_dir, mut app) = setup();
    let visible = add_task(&mut app, "Already done", None);
    app.vault
        .set_state(&visible, TaskState::Done)
        .expect("mark done");
    app.refresh();

    app.handle_key(key(KeyCode::Char('f')));
    for character in "done".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('A')));
    for character in "New open task".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    let created = app
        .vault
        .tasks()
        .find(|task| task.title == "New open task")
        .expect("created task");
    assert!(app.list.index_of(&created.id).is_none());
    assert_eq!(app.selected_id(), Some(visible));
}

#[test]
fn search_matches_titles_case_insensitively() {
    let (_dir, mut app) = setup();
    let food = add_task(&mut app, "Food", None);
    let foo = add_task(&mut app, "FOO bar", None);
    add_task(&mut app, "Unrelated", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('/')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Search { .. })));
    for character in "foo".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }

    let matches = app.search_matches();
    assert_eq!(matches.len(), 2, "only matching titles: {matches:?}");
    assert!(matches.contains(&food) && matches.contains(&foo));

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("matches"), "popup: {text}");
    assert!(text.contains("Food") && text.contains("FOO bar"), "{text}");
}

#[test]
fn search_enter_selects_the_first_match() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "Foo one", None);
    add_task(&mut app, "Foo two", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('/')));
    for character in "foo".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(app.selected_id(), Some(first));
}

#[test]
fn search_arrows_move_the_highlight_before_enter() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Foo one", None);
    let second = add_task(&mut app, "Foo two", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('/')));
    for character in "foo".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.selected_id(), Some(second));
}

#[test]
fn search_highlight_uses_arrows_and_ctrl_aliases() {
    let (_dir, mut app) = setup();
    let first = add_task(&mut app, "Foo one", None);
    let second = add_task(&mut app, "Foo two", None);
    app.refresh();

    // Up and down arrows move the highlight.
    app.handle_key(key(KeyCode::Char('/')));
    for character in "foo".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.picker_highlight(), Some(0));

    // ctrl-n is a down alias: it highlights the second match.
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(app.picker_highlight(), Some(1));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_id(), Some(second.clone()));

    // ctrl-p is an up alias: down to the second match, ctrl-p back to the
    // first, Enter selects the first.
    app.selected = Some(second);
    app.handle_key(key(KeyCode::Char('/')));
    for character in "foo".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Down));
    app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert_eq!(app.picker_highlight(), Some(0));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_id(), Some(first));
}

#[test]
fn search_types_j_and_k_into_the_query() {
    let (_dir, mut app) = setup();
    let target = add_task(&mut app, "JK notes", None);
    add_task(&mut app, "Other", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('/')));
    app.handle_key(key(KeyCode::Char('j')));
    app.handle_key(key(KeyCode::Char('k')));

    assert_eq!(app.input, "jk", "j and k are part of the query");
    assert_eq!(app.search_matches(), vec![target]);
}

#[test]
fn search_hint_mentions_arrow_selection() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Foo", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('/')));
    let status = footer_text(&app.status_line());
    assert!(
        status.contains("↓") && status.contains("select"),
        "search hint: {status}"
    );
}

#[test]
fn search_esc_restores_the_previous_selection_without_writing() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Foo one", None);
    let second = add_task(&mut app, "Foo two", None);
    app.refresh();
    app.selected = Some(second.clone());

    app.handle_key(key(KeyCode::Char('/')));
    for character in "foo".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Esc));

    assert_eq!(app.mode, InputMode::Navigate);
    assert!(app.input.is_empty());
    assert_eq!(app.selected_id(), Some(second));
    assert_eq!(app.vault.len(), 2, "search must not change the vault");
}

#[test]
fn search_with_no_matches_stays_open_with_a_status() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Foo", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('/')));
    for character in "zzz".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    assert!(app.search_matches().is_empty());
    app.handle_key(key(KeyCode::Enter));

    assert!(
        matches!(app.picker_kind(), Some(PickerKind::Search { .. })),
        "stays open"
    );
    assert!(
        footer_text(&app.status_line()).contains("no matches"),
        "{}",
        footer_text(&app.status_line())
    );
}

#[test]
fn hints_describe_the_list_keymap() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    let hints = footer_text(&app.status_line());
    assert_eq!(
        hints,
        "j/k move   a add   x state   p projects   / find   ? keys"
    );
    assert_eq!(
        hints.matches("   ").count(),
        5,
        "three spaces separate hints"
    );
    assert!(!hints.contains('·'), "hint rows have no separators");

    let lines = render_lines(&mut app, 80, 20);
    let spacer = &lines[lines.len() - 3];
    let hint = &lines[lines.len() - 2];
    let context = &lines[lines.len() - 1];
    assert!(
        spacer.trim().is_empty(),
        "spacer row above hints: {spacer:?}"
    );
    assert!(hint.contains(&hints), "hint row: {hint:?}");
    assert!(context.contains("vault"), "context row: {context:?}");
}

#[test]
fn q_and_ctrl_c_request_quit() {
    let (_dir, mut app) = setup();
    assert!(!app.should_quit());

    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.should_quit());

    let mut app = setup().1;
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit());
}

#[test]
fn x_toggles_done_on_disk_and_leaves_cancelled_alone() {
    let (dir, mut app) = setup();
    let id = add_task(&mut app, "Toggle me", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.vault.get(&id).expect("task").state, TaskState::Done);
    let stored = Task::from_document(
        &fs::read_to_string(dir.path().join(format!("{id}.md"))).expect("read"),
    )
    .expect("parse");
    assert_eq!(stored.state, TaskState::Done);

    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.vault.get(&id).expect("task").state, TaskState::Open);

    app.vault
        .set_state(&id, TaskState::Cancelled)
        .expect("cancel");
    app.refresh();
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(
        app.vault.get(&id).expect("task").state,
        TaskState::Cancelled
    );
}

#[test]
fn a_opens_a_prompt_and_commits_a_child_on_disk() {
    let (dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('a')));
    assert!(matches!(app.mode, InputMode::Add { parent: Some(ref id), .. } if id == &parent));

    for character in "New child".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    assert_eq!(
        footer_text(&app.status_line()),
        "new task under Parent: New child"
    );

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::Navigate);

    let created = app
        .vault
        .tasks()
        .find(|task| task.title == "New child")
        .expect("created task");
    assert_eq!(created.parent, Some(parent.clone()));
    assert_eq!(
        app.selected_id(),
        Some(created.id.clone()),
        "the new task becomes the selection"
    );

    let stored = Task::from_document(
        &fs::read_to_string(dir.path().join(format!("{}.md", created.id))).expect("read"),
    )
    .expect("parse");
    assert_eq!(stored.parent, Some(parent));
}

#[test]
fn shift_a_opens_a_prompt_for_a_sibling() {
    let (_dir, mut app) = setup();
    let parent = add_task(&mut app, "Parent", None);
    let child = add_task(&mut app, "Child", Some(&parent));
    app.refresh();
    app.selected = Some(child.clone());
    assert_eq!(app.selected_id(), Some(child.clone()));

    app.handle_key(key(KeyCode::Char('A')));
    assert!(matches!(app.mode, InputMode::Add { parent: Some(ref id), .. } if id == &parent));

    for character in "Sibling".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    let created = app
        .vault
        .tasks()
        .find(|task| task.title == "Sibling")
        .expect("created task");
    assert_eq!(created.parent, Some(parent));
}

#[test]
fn shift_a_inserts_the_new_task_after_the_cursor_sibling() {
    let (_dir, mut app) = setup();
    let alpha = add_task(&mut app, "Alpha", None);
    let beta = add_task(&mut app, "Beta", None);
    let gamma = add_task(&mut app, "Gamma", None);
    app.refresh();
    app.selected = Some(alpha.clone());

    app.handle_key(key(KeyCode::Char('A')));
    for character in "After alpha".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    let created = app
        .vault
        .tasks()
        .find(|task| task.title == "After alpha")
        .expect("created task");
    assert_eq!(app.vault.roots(), &[alpha, created.id.clone(), beta, gamma]);
    assert_eq!(app.selected_id(), Some(created.id.clone()));
}

#[test]
fn quick_capture_uses_the_configured_target() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut vault = Vault::open(dir.path()).expect("open vault");
    let target = vault.add(NewTask::new("Target")).expect("add").id;
    let mut config = Config::default();
    config.capture_target = Some(target.clone());
    let mut app = App::new(vault, config, None, None);

    app.handle_key(key(KeyCode::Char('N')));
    assert_eq!(app.mode, InputMode::Capture);
    for character in "Captured".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    let created = app
        .vault
        .tasks()
        .find(|task| task.title == "Captured")
        .expect("created task");
    assert_eq!(created.parent, Some(target));
}

#[test]
fn e_and_enter_request_an_external_edit() {
    let (dir, mut app) = setup();
    let id = add_task(&mut app, "Editable", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('e')));
    let path = app.take_pending_edit().expect("pending edit after e");
    assert_eq!(path, dir.path().join(format!("{id}.md")));
    assert!(
        app.take_pending_edit().is_none(),
        "the request is consumed once"
    );

    app.handle_key(key(KeyCode::Enter));
    let path = app.take_pending_edit().expect("pending edit after enter");
    assert_eq!(path, dir.path().join(format!("{id}.md")));
}

#[test]
fn editor_reload_keeps_the_selection_and_picks_up_changes() {
    let (dir, mut app) = setup();
    let id = add_task(&mut app, "Editable", None);
    app.refresh();

    // Simulate the editor writing the file while the TUI is suspended.
    let path = dir.path().join(format!("{id}.md"));
    let mut edited = Task::from_document(&fs::read_to_string(&path).expect("read")).expect("parse");
    edited.title = "Edited externally".to_owned();
    edited.body = "new body".to_owned();
    fs::write(&path, edited.to_document()).expect("external write");

    app.reload_now();
    assert_eq!(
        app.selected_id(),
        Some(id.clone()),
        "the selection survives the editor round-trip"
    );
    assert_eq!(app.vault.get(&id).expect("task").title, "Edited externally");
    assert_eq!(app.vault.get(&id).expect("task").body, "new body");
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("Edited externally"), "{text}");
}

#[test]
fn watcher_reloads_when_idle_and_warns_while_editing() {
    let (dir, mut app) = setup();
    let external = parse_id("external01");
    fs::write(
        dir.path().join("external01.md"),
        Task::new(external.clone(), "External").to_document(),
    )
    .expect("external write");

    let mut reloaded = false;
    for _ in 0..40 {
        app.on_tick();
        if app.vault.get(&external).is_some() {
            reloaded = true;
            break;
        }
        sleep(Duration::from_millis(100));
    }
    assert!(reloaded, "watcher should reload while navigating");

    app.handle_key(key(KeyCode::Char('a')));
    for character in "draft".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }

    let second = parse_id("external02");
    fs::write(
        dir.path().join("external02.md"),
        Task::new(second.clone(), "External two").to_document(),
    )
    .expect("external write");

    let mut warned = false;
    for _ in 0..40 {
        app.on_tick();
        if app.external_change_pending {
            warned = true;
            break;
        }
        sleep(Duration::from_millis(100));
    }
    assert!(warned, "editing should warn about external changes");
    assert!(
        app.context_text().contains("external change"),
        "the sticky warning lives on the context row"
    );
    assert!(
        app.vault.get(&second).is_none(),
        "must not reload while an edit buffer is open"
    );
    assert_eq!(app.input, "draft", "the edit buffer must be preserved");

    app.handle_key(key(KeyCode::Esc));
    assert!(
        app.vault.get(&second).is_some(),
        "the pending change should be applied after cancelling"
    );
}

#[test]
fn watcher_reload_preserves_the_selected_task() {
    let (dir, mut app) = setup();
    add_task(&mut app, "First", None);
    let second = add_task(&mut app, "Second", None);
    app.refresh();
    app.selected = Some(second.clone());

    let external = parse_id("external01");
    fs::write(
        dir.path().join("external01.md"),
        Task::new(external.clone(), "External").to_document(),
    )
    .expect("external write");

    let mut reloaded = false;
    for _ in 0..40 {
        app.on_tick();
        if app.vault.get(&external).is_some() {
            reloaded = true;
            break;
        }
        sleep(Duration::from_millis(100));
    }
    assert!(reloaded, "watcher should reload while idle");
    assert_eq!(
        app.selected_id(),
        Some(second),
        "a reload must not steal the selection"
    );
}

#[test]
fn watcher_pauses_during_search() {
    let (dir, mut app) = setup();
    app.handle_key(key(KeyCode::Char('/')));
    for character in "draft".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }

    let external = parse_id("external01");
    fs::write(
        dir.path().join("external01.md"),
        Task::new(external.clone(), "External").to_document(),
    )
    .expect("external write");

    let mut warned = false;
    for _ in 0..40 {
        app.on_tick();
        if app.external_change_pending {
            warned = true;
            break;
        }
        sleep(Duration::from_millis(100));
    }
    assert!(warned, "search is an unsaved buffer");
    assert!(
        app.context_text().contains("external change"),
        "the sticky warning lives on the context row"
    );
    assert!(
        app.vault.get(&external).is_none(),
        "must not reload during search"
    );
    assert_eq!(app.input, "draft");

    app.handle_key(key(KeyCode::Esc));
    assert!(
        app.vault.get(&external).is_some(),
        "the pending change is applied after cancelling search"
    );
}

#[test]
fn reload_keeps_the_scroll_and_selection() {
    let (_dir, mut app) = setup();
    for index in 0..20 {
        add_task(&mut app, &format!("Task {index:02}"), None);
    }
    app.refresh();

    app.handle_key(key(KeyCode::Char('G')));
    render_lines(&mut app, 60, 8);
    let selected = app.selected_id().expect("selected");
    let scroll = app.list_scroll;
    assert!(scroll > 0, "the list scrolled");

    app.reload_now();
    assert_eq!(app.selected_id(), Some(selected), "selection survives");
    assert_eq!(app.list_scroll, scroll, "scroll survives a reload");
}

#[test]
fn preview_follows_the_selected_task_without_enter() {
    let (_dir, mut app) = setup();
    app.vault
        .add(NewTask {
            body: "first body".to_owned(),
            ..NewTask::new("First")
        })
        .expect("add first");
    let second = app
        .vault
        .add(NewTask {
            body: "second body".to_owned(),
            ..NewTask::new("Second")
        })
        .expect("add second")
        .id;
    app.refresh();

    let text = render_lines(&mut app, 120, 20).join("\n");
    assert!(
        text.contains("first body"),
        "preview should show it: {text}"
    );
    assert!(!text.contains("second body"), "only one preview: {text}");

    app.selected = Some(second);
    let text = render_lines(&mut app, 120, 20).join("\n");
    assert!(
        text.contains("second body"),
        "preview should follow: {text}"
    );
    assert!(!text.contains("first body"), "old preview is gone: {text}");
}

#[test]
fn preview_shows_body_and_backlinks_without_related_sections() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Linked target"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("see [[{target}]]"),
            ..NewTask::new("Detail source")
        })
        .expect("add source")
        .id;
    app.vault
        .add(NewTask {
            body: format!("refers to [[{source}]]"),
            ..NewTask::new("Backlink source")
        })
        .expect("add backlink source");
    app.vault
        .add(NewTask {
            parent: Some(source.clone()),
            ..NewTask::new("Child task")
        })
        .expect("add child");
    app.refresh();

    app.selected = Some(source);
    let lines = render_lines(&mut app, 120, 20);
    let areas = layout(Rect::new(0, 0, 120, 20));
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("Linked target"),
        "the body's wikilink resolves to its title: {preview}"
    );
    assert!(
        preview.contains("↩ Backlink source"),
        "incoming links are listed by live title: {preview}"
    );
    assert!(
        !preview.contains('⇄') && !preview.contains("↩1"),
        "link counts are gone from the preview: {preview}"
    );
    for gone in [
        "children (",
        "links (",
        "backlinks (",
        "id: ",
        "state: ",
        "parent: ",
        "see [[",
    ] {
        assert!(
            !preview.contains(gone),
            "related section {gone:?} remains: {preview}"
        );
    }
}

#[test]
fn preview_resolves_backlink_titles_live() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Target"))
        .expect("add target")
        .id;
    let linker = app
        .vault
        .add(NewTask {
            body: format!("[[{target}]]"),
            ..NewTask::new("Old title")
        })
        .expect("add linker")
        .id;
    app.refresh();
    app.selected = Some(target);

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("↩ Old title"),
        "the backlink uses the linker's live title: {preview}"
    );

    app.vault.set_title(&linker, "New title").expect("rename");
    app.refresh();
    let lines = render_lines(&mut app, 120, 20);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("↩ New title") && !preview.contains("Old title"),
        "a rename shows through the live title: {preview}"
    );
}

#[test]
fn preview_orders_backlinks_by_title() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Target"))
        .expect("add target")
        .id;
    for title in ["Zebra", "apple", "Mango"] {
        app.vault
            .add(NewTask {
                body: format!("[[{target}]]"),
                ..NewTask::new(title)
            })
            .expect("add backlink");
    }
    app.refresh();
    app.selected = Some(target);

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    let apple = preview.find("↩ apple").expect("apple backlink");
    let mango = preview.find("↩ Mango").expect("Mango backlink");
    let zebra = preview.find("↩ Zebra").expect("Zebra backlink");
    assert!(
        apple < mango && mango < zebra,
        "backlinks are alphabetical by lowercased title: {preview}"
    );
}

#[test]
fn preview_without_backlinks_renders_nothing_extra() {
    let (_dir, mut app) = setup();
    let id = app
        .vault
        .add(NewTask {
            body: "plain body".to_owned(),
            ..NewTask::new("Lonely")
        })
        .expect("add")
        .id;
    app.refresh();
    app.selected = Some(id);

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("plain body"),
        "the body still renders: {preview}"
    );
    assert!(
        !preview.contains('↩'),
        "no backlink glyph without backlinks: {preview}"
    );
}

#[test]
fn preview_metadata_line_omits_link_counts() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Target"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("[[{target}]]"),
            tags: vec!["work".to_owned()],
            due: NaiveDate::from_ymd_opt(2026, 6, 15),
            priority: Some(Priority::High),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.vault
        .add(NewTask {
            body: format!("[[{source}]]"),
            ..NewTask::new("Backlink source")
        })
        .expect("add backlink source");
    app.refresh();
    app.selected = Some(source);

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let rows = pane_rows(&lines, areas.preview);
    assert!(
        rows[0].contains("due 2026-06-15")
            && rows[0].contains('!')
            && rows[0].contains("high")
            && rows[0].contains("#work"),
        "due, priority, and tags stay in the metadata line: {rows:?}"
    );
    assert!(
        !rows[0].contains('⇄') && !rows[0].contains('↩'),
        "the metadata line has no link counts: {rows:?}"
    );
}

#[test]
fn preview_renders_unknown_wikilinks_as_raw_ids() {
    let (_dir, mut app) = setup();
    app.vault
        .add(NewTask {
            body: "[[dangling01]]".to_owned(),
            ..NewTask::new("With dangling")
        })
        .expect("add");
    app.refresh();

    let lines = render_lines(&mut app, 120, 20);
    let areas = layout(Rect::new(0, 0, 120, 20));
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(preview.contains("dangling01"), "raw id renders: {preview}");
    assert!(
        !preview.contains("[[dangling01]]") && !preview.contains("dangling01 (missing)"),
        "the preview strips brackets and never marks missing: {preview}"
    );
}

#[test]
fn preview_metadata_precedes_a_bold_accent_title() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Linked target"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("[[{target}]]"),
            tags: vec!["work".to_owned()],
            due: NaiveDate::from_ymd_opt(2026, 6, 15),
            priority: Some(Priority::High),
            ..NewTask::new("Prominent source")
        })
        .expect("add source")
        .id;
    app.refresh();
    app.selected = Some(source);

    let areas = layout(Rect::new(0, 0, 120, 20));
    assert!(
        areas.preview.width > areas.list.width,
        "the preview is the majority pane: {areas:?}"
    );
    assert!(
        areas.preview.x >= areas.list.x + areas.list.width + 2,
        "a two-column gap separates the panes: {areas:?}"
    );

    let lines = render_lines(&mut app, 120, 20);
    let rows = pane_rows(&lines, areas.preview);
    assert!(
        rows[0].contains("#work") && rows[0].contains("due 2026-06-15") && rows[0].contains('!'),
        "metadata is the first preview line: {rows:?}"
    );
    assert!(
        !rows[0].contains('⇄') && !rows[0].contains('↩'),
        "the metadata line carries no link counts: {rows:?}"
    );
    assert!(
        !rows[0].contains("open") && !rows[0].contains('○'),
        "the state lives in the list glyph, not the preview metadata: {rows:?}"
    );
    assert!(
        rows[1].contains("Prominent source"),
        "the title follows the metadata: {rows:?}"
    );

    let cells = render_cells(&mut app, 120, 20);
    let title_style = style_at_text_in(&cells, "Prominent source", areas.preview);
    assert!(
        title_style.add_modifier.contains(Modifier::BOLD),
        "the preview title is bold: {title_style:?}"
    );
    assert_eq!(
        title_style.fg,
        Some(Color::Cyan),
        "the preview title uses the accent color: {title_style:?}"
    );
    let meta_style = style_at_text_in(&cells, "#work", areas.preview);
    assert_eq!(
        meta_style.fg,
        Some(Color::DarkGray),
        "metadata is dimmed: {meta_style:?}"
    );
}

#[test]
fn preview_with_an_empty_body_shows_a_dim_placeholder() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Bare task", None);
    app.refresh();
    app.selected = Some(id);

    let areas = layout(Rect::new(0, 0, 120, 20));
    let lines = render_lines(&mut app, 120, 20);
    let rows = pane_rows(&lines, areas.preview);
    assert!(
        rows[0].contains("Bare task"),
        "a bare task has no metadata line: {rows:?}"
    );
    assert!(
        rows[1].contains("[empty body]"),
        "the placeholder follows the title: {rows:?}"
    );

    let cells = render_cells(&mut app, 120, 20);
    let style = style_at_text_in(&cells, "[empty body]", areas.preview);
    assert_eq!(
        style.fg,
        Some(Color::DarkGray),
        "the placeholder is dimmed: {style:?}"
    );
    assert!(
        style.add_modifier.contains(Modifier::DIM),
        "the placeholder is dimmed: {style:?}"
    );
}

#[test]
fn preview_renders_markdown_element_styles() {
    let (_dir, mut app) = setup();
    let body = "# Heading one\n\nplain **bold** and `code`\n\n- [ ] todo\n\n> quote\n\n---\n\n[link](https://example.com)";
    let id = app
        .vault
        .add(NewTask {
            body: body.to_owned(),
            ..NewTask::new("Rich")
        })
        .expect("add")
        .id;
    app.refresh();
    app.selected = Some(id);

    let areas = layout(Rect::new(0, 0, 120, 30));
    let cells = render_cells(&mut app, 120, 30);

    let heading = style_at_text_in(&cells, "Heading one", areas.preview);
    assert!(
        heading.add_modifier.contains(Modifier::BOLD),
        "heading is bold: {heading:?}"
    );
    let bold = style_at_text_in(&cells, "bold", areas.preview);
    assert!(
        bold.add_modifier.contains(Modifier::BOLD),
        "strong is bold: {bold:?}"
    );
    let code = style_at_text_in(&cells, "code", areas.preview);
    assert_eq!(code.bg, Some(Color::DarkGray), "code has a background");
    let link = style_at_text_in(&cells, "link", areas.preview);
    assert_eq!(link.fg, Some(Color::Cyan), "link text is accented");
    assert!(
        link.add_modifier.contains(Modifier::UNDERLINED),
        "link text is underlined: {link:?}"
    );

    let lines = render_lines(&mut app, 120, 30);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(preview.contains("- [ ] todo"), "task marker: {preview}");
    assert!(preview.contains("│ quote"), "blockquote prefix: {preview}");
    assert!(preview.contains("────"), "rule line: {preview}");
    assert!(
        !preview.contains("https://example.com"),
        "URLs do not render: {preview}"
    );
}

#[test]
fn preview_resolves_wikilinks_and_keeps_code_literal() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Linked target"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("see [[{target}]] and [[missing001]]\n\n```\n[[{target}]]\n```"),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.refresh();
    app.selected = Some(source);

    let areas = layout(Rect::new(0, 0, 120, 30));
    let lines = render_lines(&mut app, 120, 30);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("Linked target"),
        "in-vault wikilink resolves: {preview}"
    );
    assert!(
        preview.contains("missing001") && !preview.contains("missing001 (missing)"),
        "unknown ids stay raw without a missing marker: {preview}"
    );
    assert!(
        preview.contains(&format!("[[{target}]]")),
        "fenced wikilinks stay literal: {preview}"
    );

    let cells = render_cells(&mut app, 120, 30);
    let style = style_at_text_in(&cells, "Linked target", areas.preview);
    assert_eq!(style.fg, Some(Color::Cyan), "wikilinks look like links");
    assert!(
        style.add_modifier.contains(Modifier::UNDERLINED),
        "wikilinks look like links: {style:?}"
    );
}

#[test]
fn preview_resolves_rich_link_forms_and_never_shows_aliases() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Live title"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("see [[{target}.md|Stale alias]] and [[missing001.md|Ghost alias]]"),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.refresh();
    app.selected = Some(source);

    let areas = layout(Rect::new(0, 0, 120, 30));
    let lines = render_lines(&mut app, 120, 30);
    let preview = pane_rows(&lines, areas.preview).join("\n");
    assert!(
        preview.contains("Live title"),
        "the .md form resolves to the live title: {preview}"
    );
    assert!(
        !preview.contains("Stale alias"),
        "the alias is never displayed: {preview}"
    );
    assert!(
        preview.contains("missing001") && !preview.contains("missing001 (missing)"),
        "a dangling path+alias shows the raw stem without a marker: {preview}"
    );
    assert!(!preview.contains("Ghost alias"), "{preview}");
}

#[test]
fn question_mark_opens_and_closes_the_keymap() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md"]);

    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open, "? should open the keymap from the list");
    assert!(!app.issues_open);

    app.handle_key(key(KeyCode::Esc));
    assert!(!app.keymap_open, "Esc should close the keymap");
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.issues_open, "g? should open the issues overlay");
}

#[test]
fn keymap_scroll_is_modal_and_clamped() {
    let (_dir, mut app) = setup();
    let id = add_task(&mut app, "Only", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open);
    app.set_middle_viewport(80, 10);
    let selected_before = app.selected_id();
    app.handle_key(key(KeyCode::Char('j')));
    assert!(app.keymap_scroll > 0, "j scrolls the keymap");
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(
        app.vault.get(&id).expect("task").state,
        TaskState::Open,
        "task hotkeys are inert while the keymap is open"
    );
    assert_eq!(app.selected_id(), selected_before);

    for _ in 0..100 {
        app.handle_key(key(KeyCode::Char('j')));
    }
    let end = app.keymap_scroll;
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.keymap_scroll, end, "scroll clamps at the end");
    for _ in 0..100 {
        app.handle_key(key(KeyCode::Char('k')));
    }
    assert_eq!(app.keymap_scroll, 0, "scroll clamps at the beginning");

    app.handle_key(key(KeyCode::Char('?')));
    assert!(!app.keymap_open, "? closes the keymap");
    app.handle_key(key(KeyCode::Char('?')));
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.keymap_open, "Esc closes the keymap");
}

#[test]
fn y_key_copies_task_metadata_for_registered_project_and_reports_clipboard_failure() {
    let (dir, mut app) = setup();
    let workspace = dir.path().join("workspace");
    fs::create_dir(&workspace).expect("registered project directory");
    app.project = Some(project(
        workspace.to_str().expect("workspace path"),
        "workspace",
    ));
    let id = add_task(&mut app, "Copy this", None);
    app.refresh();

    let copied = std::cell::RefCell::new(String::new());
    app.handle_key_with(key(KeyCode::Char('y')), |text| {
        *copied.borrow_mut() = text.to_owned();
        Ok(())
    });

    let task_path = dir.path().join(id.file_name());
    let expected = format!(
        "Project path: {}\nTask file: {}\nID: {}\nTitle: Copy this\nState: open",
        workspace
            .canonicalize()
            .expect("absolute registered project path")
            .display(),
        task_path
            .canonicalize()
            .expect("absolute task path")
            .display(),
        id
    );
    assert_eq!(*copied.borrow(), expected);
    assert_eq!(
        app.toast.as_ref().expect("success toast").text,
        format!("copied task {id}")
    );

    app.handle_key_with(key(KeyCode::Char('y')), |_| {
        Err(std::io::Error::other("clipboard unavailable"))
    });
    assert_eq!(
        app.toast.as_ref().expect("failure toast").text,
        "clipboard error: clipboard unavailable"
    );
}

#[test]
fn keymap_overlay_shows_grouped_bindings_and_close_hint() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('?')));

    let text = render_lines(&mut app, 100, 40).join("\n");
    for expected in [
        "Keymap",
        "Movement",
        "j / k",
        "move selection (also up/down arrows)",
        "Tasks",
        "y",
        "copy task metadata",
        "Jump",
        "Vault",
        "g ?",
        "Pickers",
        "Modals",
        "esc or ? to close",
    ] {
        assert!(
            text.contains(expected),
            "keymap missing {expected:?}: {text}"
        );
    }
}

#[test]
fn keymap_overlay_uses_yellow_titles_cyan_keys_and_dim_labels() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open);

    let cells = render_cells(&mut app, 130, 30);
    let title = style_at_text(&cells, "Movement");
    assert_eq!(
        title.fg,
        Some(Color::Yellow),
        "group title is yellow: {title:?}"
    );
    assert!(
        title.add_modifier.contains(Modifier::BOLD),
        "group title is bold: {title:?}"
    );
    let binding = style_at_text(&cells, "gg / G");
    assert_eq!(
        binding.fg,
        Some(Color::Cyan),
        "binding key is cyan: {binding:?}"
    );
    let label = style_at_text(&cells, "first / last");
    assert_eq!(
        label.fg,
        Some(Color::DarkGray),
        "binding label is dim: {label:?}"
    );
}

#[test]
fn keymap_columns_follow_the_120_width_threshold() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open);

    let narrow = render_lines(&mut app, 100, 40);
    let row = narrow
        .iter()
        .find(|row| row.contains("Movement") && row.contains("Tasks"))
        .expect("two columns share a row below 120");
    assert!(
        !row.contains("Jump"),
        "Jump is not on the first row below 120: {row:?}"
    );

    let wide = render_lines(&mut app, 130, 40);
    assert!(
        wide.iter()
            .any(|row| row.contains("Movement") && row.contains("Tasks") && row.contains("Jump")),
        "three columns share a row at 120 or more: {wide:?}"
    );
}

#[test]
fn footer_hint_keys_are_cyan_and_labels_are_dim() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    let cells = render_cells(&mut app, 100, 20);
    let key = style_at_text(&cells, "j/k");
    assert_eq!(key.fg, Some(Color::Cyan), "hint key is cyan: {key:?}");
    let label = style_at_text(&cells, "move");
    assert_eq!(
        label.fg,
        Some(Color::DarkGray),
        "hint label is dim: {label:?}"
    );
}

#[test]
fn keymap_scroll_clamps_at_the_max_and_reclamps_on_resize() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open);

    // A short terminal so the keymap actually needs to scroll.
    app.set_middle_viewport(80, 10);
    let max = keymap_max_scroll(80, 10);
    assert!(max > 0, "the test size must require scrolling");
    for _ in 0..200 {
        app.handle_key(key(KeyCode::Char('j')));
    }
    assert_eq!(app.keymap_scroll, max, "scroll stops at the maximum");
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(
        app.keymap_scroll, max,
        "one more j does not move past the maximum"
    );

    // A taller viewport fits more rows, so the offset re-clamps down.
    app.set_middle_viewport(80, 40);
    assert_eq!(
        app.keymap_scroll,
        keymap_max_scroll(80, 40),
        "resize re-clamps the scroll"
    );
}

#[test]
fn keymap_scroll_max_matches_renderer_body_for_all_sizes() {
    // The single-geometry invariant: for every middle-area size, the app's
    // maximum scroll and the renderer's drawn body rows agree, so no scroll
    // position is unreachable or overshoots. Covers the tiny heights (1..=5,
    // including height 3 where the old visible-lines clamp returned 1 while
    // the renderer drew 0 rows) at both column counts.
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open);

    for width in [80u16, 130u16] {
        let columns = keymap_columns(width);
        let content_lines = keymap_content_lines(columns);
        let content_width = keymap_content_width(columns);
        assert!(
            content_lines > 0,
            "the keymap must need scrolling at {width}"
        );
        for height in 1u16..=40 {
            let geometry = keymap_geometry(width, height, content_width, content_lines, usize::MAX);
            // The renderer draws `body_height` rows; at maximum scroll the
            // remaining rows must be exactly the body, so nothing is hidden
            // or overshoots.
            assert_eq!(
                geometry.max_scroll,
                content_lines.saturating_sub(geometry.body_height),
                "max must match the drawn body at {width}x{height}"
            );
            assert_eq!(
                geometry.scroll, geometry.max_scroll,
                "saturating scroll must clamp to the max at {width}x{height}"
            );
            assert_eq!(
                content_lines.saturating_sub(geometry.max_scroll),
                content_lines.min(geometry.body_height),
                "drawn rows at max must be the body at {width}x{height}"
            );

            // The app reaches exactly the renderer's max through keys and
            // never exceeds it, including via resize re-clamping.
            app.set_middle_viewport(width, height);
            app.keymap_scroll = 0;
            for _ in 0..content_lines + 10 {
                app.handle_key(key(KeyCode::Char('j')));
            }
            assert_eq!(
                app.keymap_scroll, geometry.max_scroll,
                "app scroll must reach the renderer max at {width}x{height}"
            );
            app.handle_key(key(KeyCode::Char('j')));
            assert_eq!(
                app.keymap_scroll, geometry.max_scroll,
                "app scroll must not overshoot at {width}x{height}"
            );
            app.keymap_scroll = content_lines + 100;
            app.set_middle_viewport(width, height);
            assert_eq!(
                app.keymap_scroll, geometry.max_scroll,
                "resize must re-clamp an overshoot at {width}x{height}"
            );
        }
    }
}

#[test]
fn keymap_overlay_rendering_honours_shared_geometry_scroll_and_body() {
    // Regression guard: `render_keymap_overlay` in `ui.rs` must draw exactly
    // `geometry.body_height` content rows starting at `geometry.scroll`, where
    // the geometry is the shared `keymap_geometry` derivation. If the renderer
    // re-introduced its own popup/body sizing or ignored the scroll offset,
    // the first/last-line visibility below would be wrong and this test would
    // fail — unlike pure-geometry assertions, which compare `keymap_geometry`
    // against itself and pass either way.
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.keymap_open);

    // A terminal small enough that the keymap content cannot fit, so
    // scrolling is actually required. Verified, not assumed.
    const WIDTH: u16 = 80;
    const HEIGHT: u16 = 10;
    let middle = layout(Rect::new(0, 0, WIDTH, HEIGHT)).middle;
    let columns = keymap_columns(middle.width);
    let content_lines = keymap_content_lines(columns);
    let content_width = keymap_content_width(columns);
    let geometry = keymap_geometry(middle.width, middle.height, content_width, content_lines, 0);
    assert!(
        geometry.max_scroll > 0,
        "the test size must require scrolling: {middle:?} fits {content_lines} lines into {} body rows",
        geometry.body_height
    );
    assert!(
        geometry.body_height < content_lines,
        "the body must be shorter than the content: {geometry:?}"
    );
    let visible = content_lines.min(geometry.body_height);
    assert!(visible > 0, "the body must draw at least one row");

    // The popup body rectangle, mirroring the renderer: a centered popup,
    // a one-cell border inset, and exactly `body_height` drawn rows.
    let popup = Rect {
        x: middle.x + middle.width.saturating_sub(geometry.popup_width) / 2,
        y: middle.y + middle.height.saturating_sub(geometry.popup_height) / 2,
        width: geometry.popup_width.min(middle.width),
        height: geometry.popup_height.min(middle.height),
    };
    let body = Rect {
        x: popup.x.saturating_add(1),
        y: popup.y.saturating_add(1),
        width: popup.width.saturating_sub(2),
        height: (geometry.body_height as u16).min(popup.height.saturating_sub(2)),
    };
    assert_eq!(
        body.height as usize, visible,
        "the body draws exactly the shared body height"
    );
    let body_text = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .skip(body.y as usize)
            .take(body.height as usize)
            .map(|row| {
                row.chars()
                    .skip(body.x as usize)
                    .take(body.width as usize)
                    .collect()
            })
            .collect()
    };

    // Distinctive left-column needles for the first window (content rows
    // `0..visible`), the row just past it, the last window
    // (`max_scroll..max_scroll + visible`), and the row just before it. The
    // left column is never clipped by the popup width, unlike the right
    // column's tail, so these survive horizontal clipping. Update them if the
    // keymap table or the test size changes.
    let first_window = ["Movement", "move selection", "first / last"];
    let last_window = [
        "confirm / cancel delete",
        "move through issues",
        "edit the issue",
    ];
    assert_eq!(
        visible,
        first_window.len(),
        "the test size must show exactly the hardcoded first window; update the needles instead of assuming"
    );
    assert_eq!(
        visible,
        last_window.len(),
        "the test size must show exactly the hardcoded last window; update the needles instead of assuming"
    );

    // At scroll 0 the renderer honours `geometry.scroll`: the first content
    // rows are drawn, in order, and the last content line is nowhere on
    // screen. The row just past the body must also be absent, proving the
    // renderer drew no more than `body_height` rows.
    app.keymap_scroll = 0;
    let top = render_lines(&mut app, WIDTH, HEIGHT);
    let top_body = body_text(&top);
    assert_eq!(
        top_body.len(),
        visible,
        "the body draws min(content, body_height) rows at scroll 0"
    );
    for (row, needle) in top_body.iter().zip(first_window) {
        assert!(
            row.contains(needle),
            "body row must show {needle:?} at scroll 0: {row:?}"
        );
    }
    let top_screen = top.join("\n");
    assert!(
        !top_screen.contains("edit the issue"),
        "the last content line must be hidden at scroll 0"
    );
    assert!(
        !top_screen.contains("fold / unfold"),
        "the row just past the body must not be drawn at scroll 0"
    );

    // Drive to the maximum scroll through the same `j` path the app uses,
    // then render again: the last content rows are drawn, in order, and the
    // first content line has scrolled away. The row just before the window
    // must be absent, proving the renderer started exactly at the scroll.
    for _ in 0..content_lines + 10 {
        app.handle_key(key(KeyCode::Char('j')));
    }
    assert_eq!(
        app.keymap_scroll, geometry.max_scroll,
        "j must reach the shared maximum scroll"
    );
    let bottom = render_lines(&mut app, WIDTH, HEIGHT);
    let bottom_body = body_text(&bottom);
    assert_eq!(
        bottom_body.len(),
        visible,
        "the body draws min(content, body_height) rows at max scroll"
    );
    for (row, needle) in bottom_body.iter().zip(last_window) {
        assert!(
            row.contains(needle),
            "body row must show {needle:?} at max scroll: {row:?}"
        );
    }
    let bottom_screen = bottom.join("\n");
    assert!(
        bottom_screen.contains("edit the issue"),
        "the last content line must be visible at max scroll"
    );
    assert!(
        !bottom_screen.contains("Movement"),
        "the first content line must have scrolled away at max scroll"
    );
    assert!(
        !bottom_screen.contains("choose button"),
        "the row just before the last window must not be drawn at max scroll"
    );
}

#[test]
fn issue_overlay_kinds_share_a_fixed_column() {
    let (_dir, mut app) = setup_with_issues(&["x.md", "a-much-longer-name.md", "猫.md"]);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));

    let lines = render_lines(&mut app, 100, 20);
    let short = lines
        .iter()
        .find(|row| row.contains("x.md"))
        .expect("short");
    let long = lines
        .iter()
        .find(|row| row.contains("a-much-longer-name.md"))
        .expect("long");
    let wide = lines.iter().find(|row| row.contains('猫')).expect("wide");
    assert_eq!(
        column(short, "x.md"),
        column(long, "a-much-longer-name.md"),
        "issue paths share one left-aligned column: {lines:?}"
    );
    assert_eq!(
        column(wide, "猫"),
        column(long, "a-much-longer-name.md"),
        "wide issue paths share the path column: {lines:?}"
    );
    assert_eq!(
        column(short, "["),
        column(long, "["),
        "issue kinds share one left-aligned column: {lines:?}"
    );
    assert_eq!(
        column(wide, "["),
        column(long, "["),
        "a wide issue path does not shift the kind column: {lines:?}"
    );

    app.handle_key(key(KeyCode::Char('j')));
    let cells = render_cells(&mut app, 100, 20);
    let (row, text) = cells
        .iter()
        .map(|row| {
            let text: String = row.iter().map(|(symbol, _)| *symbol).collect();
            (row, text)
        })
        .find(|(_, text)| text.contains("x.md"))
        .expect("selected short issue");
    let padding_start = column(&text, "x.md") + "x.md".chars().count();
    let kind_start = column(&text, "[");
    assert!(
        row[padding_start..kind_start]
            .iter()
            .all(|(_, style)| style.add_modifier.contains(Modifier::REVERSED)),
        "selected issue keeps reverse video through reserved path padding"
    );
}

#[test]
fn issue_overlay_content_stays_inside_its_border_at_narrow_widths() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md", "SKILL.md"]);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.issues_open);

    let width = 18u16;
    let height = 10u16;
    let lines = render_lines(&mut app, width, height);

    // The overlay covers the middle band: header above, three-row footer below.
    let top_row = 1usize;
    let bottom_row = (height - 4) as usize;
    let top = &lines[top_row];
    assert!(top.starts_with('┌'), "top border: {top:?}");
    assert!(top.ends_with('┐'), "top border: {top:?}");
    let bottom = &lines[bottom_row];
    assert!(bottom.starts_with('└'), "bottom border: {bottom:?}");
    assert!(bottom.ends_with('┘'), "bottom border: {bottom:?}");

    for row in (top_row + 1)..bottom_row {
        let cells: Vec<char> = lines[row].chars().collect();
        assert_eq!(cells[0], '│', "row {row} left border: {lines:?}");
        assert_eq!(
            cells[width as usize - 1],
            '│',
            "row {row} right border: {lines:?}"
        );
    }
}

#[test]
fn action_feedback_survives_a_reload_and_the_issue_badge_renders() {
    let (_dir, mut app) = setup_with_issues(&["bad-one.md", "bad-two.md"]);
    assert_eq!(app.vault_issues.len(), 2);

    let id = add_task(&mut app, "Toggle me", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.vault.get(&id).expect("task").state, TaskState::Done);

    // A reload (watcher tick or settling a pending change) must not clobber
    // the action feedback with a cryptic issue count.
    app.reload_now();
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("done Toggle me")
    );
    assert_eq!(app.vault_issues.len(), 2);

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("done Toggle me"), "feedback lost: {text}");
    assert!(text.contains("⚠ 2 issues"), "badge missing: {text}");
}

#[test]
fn single_issue_badge_is_singular() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md"]);
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("⚠ 1 issue"), "badge missing: {text}");
    assert!(!text.contains("⚠ 1 issues"), "badge mispluralized: {text}");
}

#[test]
fn edit_return_warns_only_when_the_issue_count_rises() {
    let (dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    // An edit that introduces an id mismatch: the return toast teaches the
    // user that their edit broke something.
    fs::write(
        dir.path().join("wrongname.md"),
        "---\nid: abc1234567\ntitle: Broken\nstate: open\n---\n",
    )
    .expect("external write");
    app.reload_after_edit();
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("⚠ 1 issue — press g?")
    );

    // A second break pluralizes and reports the new total.
    fs::write(
        dir.path().join("othername.md"),
        "---\nid: def7654321\ntitle: Broken two\nstate: open\n---\n",
    )
    .expect("external write");
    app.reload_after_edit();
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("⚠ 2 issues — press g?")
    );

    // A flat count must not clobber existing action feedback.
    app.set_toast("done Only");
    app.reload_after_edit();
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("done Only")
    );
}

#[test]
fn plain_reload_never_warns_about_new_issues() {
    let (dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    fs::write(
        dir.path().join("wrongname.md"),
        "---\nid: abc1234567\ntitle: Broken\nstate: open\n---\n",
    )
    .expect("external write");
    app.reload_now();

    assert_eq!(app.vault_issues.len(), 1);
    assert!(
        app.toast.is_none(),
        "watcher reloads must stay quiet about issue rises"
    );
}

#[test]
fn reload_after_edit_syncs_mirror_aliases_and_is_idempotent() {
    let (dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Old title"))
        .expect("add target");
    let mirror = app
        .vault
        .add(NewTask {
            body: format!("see [[{}.md|Old title]]", target.id),
            ..NewTask::new("Mirror")
        })
        .expect("add mirror");
    let contextual = app
        .vault
        .add(NewTask {
            body: format!("see [[{}.md|my words]] and [[{}.md]]", target.id, target.id),
            ..NewTask::new("Contextual")
        })
        .expect("add contextual");
    app.refresh();

    let mut renamed = target.clone();
    renamed.title = "New title".to_owned();
    fs::write(
        dir.path().join(format!("{}.md", target.id)),
        renamed.to_document(),
    )
    .expect("external rename");

    app.reload_after_edit();

    assert_eq!(
        app.vault.get(&mirror.id).expect("mirror").body,
        format!("see [[{}.md|New title]]", target.id)
    );
    assert_eq!(
        app.vault.get(&contextual.id).expect("contextual").body,
        format!("see [[{}.md|my words]] and [[{}.md]]", target.id, target.id),
        "contextual aliases and bare links survive a TUI reload"
    );

    let mirror_path = dir.path().join(format!("{}.md", mirror.id));
    let after_first = fs::read_to_string(&mirror_path).expect("read");
    app.reload_after_edit();
    assert_eq!(
        fs::read_to_string(&mirror_path).expect("read"),
        after_first,
        "the post-cascade reload diffs clean and writes nothing"
    );
}

#[test]
fn watcher_reload_syncs_mirror_aliases() {
    let (dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Old title"))
        .expect("add target");
    let mirror = app
        .vault
        .add(NewTask {
            body: format!("[[{}.md|Old title]]", target.id),
            ..NewTask::new("Mirror")
        })
        .expect("add mirror");
    app.refresh();

    let mut renamed = target.clone();
    renamed.title = "New title".to_owned();
    fs::write(
        dir.path().join(format!("{}.md", target.id)),
        renamed.to_document(),
    )
    .expect("external rename");

    app.reload_now();

    assert_eq!(
        app.vault.get(&mirror.id).expect("mirror").body,
        format!("[[{}.md|New title]]", target.id)
    );
}

#[test]
fn app_start_never_syncs_stale_mirrors() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        dir.path().join("target0001.md"),
        "---\nid: target0001\ntitle: New title\nstate: open\n---\n",
    )
    .expect("write target");
    let source = "---\nid: source0001\ntitle: Source\nstate: open\n---\n\
                  see [[target0001.md|Old title]]\n";
    fs::write(dir.path().join("source0001.md"), source).expect("write source");

    let vault = Vault::open(dir.path()).expect("open vault");
    let _app = App::new(vault, Config::default(), None, None);

    assert_eq!(
        fs::read_to_string(dir.path().join("source0001.md")).expect("read"),
        source,
        "a cold TUI start has no prior index and never writes"
    );
}

#[test]
fn issue_overlay_shows_id_mismatch_and_edit_requests_the_file() {
    let (dir, mut app) = setup();
    fs::write(
        dir.path().join("wrongname.md"),
        "---\nid: abc1234567\ntitle: Broken\nstate: open\n---\n",
    )
    .expect("write");
    app.reload_now();
    assert_eq!(app.vault_issues.len(), 1);

    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("id mismatch"), "kind missing: {text}");
    assert!(text.contains("wrongname.md"), "path missing: {text}");
    assert!(
        text.contains("abc1234567"),
        "reason names the declared id: {text}"
    );

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.take_pending_edit(),
        Some(dir.path().join("wrongname.md"))
    );
}

#[test]
fn store_issues_open_on_g_then_question_mark_and_close_on_esc() {
    let (_dir, mut app) = setup_with_issues(&["CONTEXT.md", "SKILL.md"]);
    assert!(!app.issues_open);

    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.issues_open, "g? should open the overlay");

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("vault issues (2)"), "{text}");
    assert!(text.contains("CONTEXT.md"), "{text}");
    assert!(text.contains("SKILL.md"), "{text}");
    assert!(text.contains("malformed"), "issue kind missing: {text}");

    app.handle_key(key(KeyCode::Esc));
    assert!(!app.issues_open, "Esc should close the overlay");
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(
        !text.contains("vault issues (2)"),
        "overlay remains: {text}"
    );

    // While open, unrelated keys are ignored and `?` stays inert; only Esc
    // closes.
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.issues_open, "g? should reopen the overlay");
    let id = add_task(&mut app, "Still open", None);
    app.refresh();
    app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(
        app.vault.get(&id).expect("task").state,
        TaskState::Open,
        "keys other than q/esc must be inert while the overlay is open"
    );
    app.handle_key(key(KeyCode::Char('?')));
    assert!(
        app.issues_open,
        "? must stay inert while the issues overlay is open"
    );
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.issues_open, "Esc should close the overlay");
}

#[test]
fn issue_overlay_cursor_moves_and_clamps() {
    let (_dir, mut app) = setup_with_issues(&["bad-one.md", "bad-two.md", "bad-three.md"]);
    assert_eq!(app.vault_issues.len(), 3);

    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert_eq!(app.issues_cursor, 0, "opens on the first issue");
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.issues_cursor, 1);
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.issues_cursor, 2);
    app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(app.issues_cursor, 2, "clamped at the last issue");
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.issues_cursor, 1);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.issues_cursor, 0);
    app.handle_key(key(KeyCode::Char('k')));
    assert_eq!(app.issues_cursor, 0, "clamped at the first issue");

    // Reopening starts over from the first issue.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.issues_cursor, 0);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert_eq!(app.issues_cursor, 0);
}

#[test]
fn issue_overlay_edit_requests_the_highlighted_file() {
    let (_dir, mut app) = setup_with_issues(&["bad-one.md", "bad-two.md"]);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    app.handle_key(key(KeyCode::Char('j')));
    let expected = app.vault_issues[1].path.clone();

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.take_pending_edit(), Some(expected.clone()));
    assert!(
        app.take_pending_edit().is_none(),
        "the request is consumed once"
    );

    app.handle_key(key(KeyCode::Char('e')));
    assert_eq!(app.take_pending_edit(), Some(expected));
    assert!(
        app.issues_open,
        "the overlay stays open until the editor returns"
    );
}

#[test]
fn issue_overlay_highlights_the_selected_issue() {
    let (_dir, mut app) = setup_with_issues(&["bad-one.md", "bad-two.md"]);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));

    let first = app.vault_issues[0]
        .path
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned();
    let second = app.vault_issues[1]
        .path
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned();

    let cells = render_cells(&mut app, 100, 20);
    let style = style_at_text(&cells, &first);
    assert!(
        style.add_modifier.contains(Modifier::REVERSED),
        "the first issue is highlighted: {style:?}"
    );

    app.handle_key(key(KeyCode::Char('j')));
    let cells = render_cells(&mut app, 100, 20);
    let style = style_at_text(&cells, &second);
    assert!(
        style.add_modifier.contains(Modifier::REVERSED),
        "the second issue is highlighted: {style:?}"
    );
    let style = style_at_text(&cells, &first);
    assert!(
        !style.add_modifier.contains(Modifier::REVERSED),
        "the first issue is no longer highlighted: {style:?}"
    );
}

#[test]
fn issue_overlay_closes_with_a_toast_when_the_files_are_fixed() {
    let (dir, mut app) = setup_with_issues(&["bad-one.md", "bad-two.md"]);
    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.issues_open);

    // Simulate the editor fixing both files (tests never spawn $EDITOR):
    // remove the broken files and write valid, stem-matching ones.
    fs::remove_file(dir.path().join("bad-one.md")).expect("remove one");
    fs::remove_file(dir.path().join("bad-two.md")).expect("remove two");
    fs::write(
        dir.path().join("fixedone01.md"),
        Task::new(parse_id("fixedone01"), "Fixed one").to_document(),
    )
    .expect("fix one");
    fs::write(
        dir.path().join("fixedtwo01.md"),
        Task::new(parse_id("fixedtwo01"), "Fixed two").to_document(),
    )
    .expect("fix two");
    app.reload_now();

    assert!(
        !app.issues_open,
        "the overlay closes once the issues are gone"
    );
    assert!(app.vault_issues.is_empty());
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("all vault issues resolved")
    );
    assert!(app.issue_badge().is_none());
}

#[test]
fn issue_overlay_edit_is_a_noop_on_a_clean_vault() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.vault_issues.is_empty());
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('e')));
    assert!(app.take_pending_edit().is_none());
    assert!(app.issues_open, "the empty overlay stays open");

    app.handle_key(key(KeyCode::Esc));
    assert!(!app.issues_open);
}

#[test]
fn clean_vault_has_no_badge_and_the_overlay_reports_no_issues() {
    let (_dir, mut app) = setup();
    add_task(&mut app, "Only task", None);
    app.refresh();

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(!text.contains('⚠'), "unexpected badge: {text}");

    app.handle_key(key(KeyCode::Char('g')));
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.issues_open);
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("no vault issues"), "{text}");
    assert!(!text.contains('⚠'), "unexpected badge: {text}");
}

#[test]
fn issue_badge_disappears_after_the_files_are_fixed() {
    let (dir, mut app) = setup_with_issues(&["CONTEXT.md"]);
    assert!(app.issue_badge().is_some());

    fs::remove_file(dir.path().join("CONTEXT.md")).expect("remove the bad file");
    fs::write(
        dir.path().join("fixedissue.md"),
        Task::new(parse_id("fixedissue"), "Fixed").to_document(),
    )
    .expect("write the fixed file");
    app.reload_now();

    assert!(app.vault_issues.is_empty());
    assert!(app.issue_badge().is_none());
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(!text.contains('⚠'), "badge should clear: {text}");
}

/// Render just the launch modal to a `TestBackend`.
fn render_launch_lines(launch: &Launch, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render_launch(frame, launch))
        .expect("draw");
    let buffer = terminal.backend().buffer();

    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| {
                    buffer
                        .cell((x, y))
                        .map_or(' ', |cell| cell.symbol().chars().next().unwrap_or(' '))
                })
                .collect()
        })
        .collect()
}

/// Render just the launch modal and return each cell's symbol and style.
fn render_launch_cells(launch: &Launch, width: u16, height: u16) -> Vec<Vec<(char, Style)>> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render_launch(frame, launch))
        .expect("draw");
    let buffer = terminal.backend().buffer();

    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| {
                    let cell = buffer.cell((x, y)).expect("cell");
                    (cell.symbol().chars().next().unwrap_or(' '), cell.style())
                })
                .collect()
        })
        .collect()
}

/// An unregistered launch question backed by a real config in a temp dir.
fn register_launch() -> (tempfile::TempDir, Launch) {
    let dir = tempfile::tempdir().expect("temp dir");
    let fresh = dir.path().join("fresh");
    fs::create_dir_all(&fresh).expect("project dir");
    let config = Config::load_from(Some(dir.path().join("config.toml"))).expect("load");
    let launch = Launch::new(
        config,
        Resolution::Unregistered { path: fresh },
        dir.path().join("data"),
    );
    (dir, launch)
}

/// Unregistered cwd plus one already-registered project.
fn unregistered_launch_with_other() -> (tempfile::TempDir, Launch) {
    let dir = tempfile::tempdir().expect("temp dir");
    let other = dir.path().join("other");
    let fresh = dir.path().join("fresh");
    fs::create_dir_all(&other).expect("other");
    fs::create_dir_all(&fresh).expect("fresh");
    let config_path = dir.path().join("config.toml");
    let mut config = Config::load_from(Some(config_path)).expect("load");
    config
        .projects
        .push(project(other.to_str().expect("utf-8"), "other"));
    config.save().expect("save");
    let launch = Launch::new(
        config,
        Resolution::Unregistered { path: fresh },
        dir.path().join("data"),
    );
    (dir, launch)
}

/// A nested-directory launch question backed by a real config in a temp dir.
fn nested_launch() -> (tempfile::TempDir, Launch, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let project_dir = dir.path().join("proj");
    let nested = project_dir.join("sub");
    fs::create_dir_all(&nested).expect("nested dir");
    let config_path = dir.path().join("config.toml");
    let parent = project(project_dir.to_str().expect("utf-8"), "proj");
    let mut config = Config::load_from(Some(config_path.clone())).expect("load");
    config.projects.push(parent.clone());
    config.save().expect("save");
    let launch = Launch::new(
        config,
        Resolution::Registered {
            project: parent,
            nested: Some(nested),
        },
        dir.path().join("data"),
    );
    (dir, launch, config_path)
}

#[test]
fn project_picker_paths_share_a_fixed_column() {
    let (_dir, mut app) = setup();
    app.config.projects = vec![
        project("/one", "x"),
        project("/two", "much-longer"),
        project("/wide", "猫"),
    ];

    app.handle_key(key(KeyCode::Char('p')));
    let lines = render_lines(&mut app, 100, 20);
    let one = lines.iter().find(|row| row.contains("/one")).expect("one");
    let two = lines.iter().find(|row| row.contains("/two")).expect("two");
    let wide = lines
        .iter()
        .find(|row| row.contains("/wide"))
        .expect("wide");
    assert_eq!(
        column(one, "x"),
        column(two, "much-longer"),
        "project slugs share one left-aligned column: {lines:?}"
    );
    assert_eq!(
        column(one, "/one"),
        column(two, "/two"),
        "project paths share one left-aligned column: {lines:?}"
    );
    assert_eq!(
        column(wide, "/wide"),
        column(two, "/two"),
        "a wide slug does not shift the project path column: {lines:?}"
    );
}

#[test]
fn narrow_project_picker_reserves_a_visible_path_column() {
    let (_dir, mut app) = setup();
    app.config.projects = vec![
        project("/one", "a-very-long-project-slug-that-does-not-fit"),
        project("/two", "x"),
    ];

    app.handle_key(key(KeyCode::Char('p')));
    let text = render_lines(&mut app, 50, 12).join("\n");
    assert!(
        text.contains("/one") && text.contains("/two"),
        "narrow picker keeps paths visible: {text}"
    );
}

#[test]
fn project_picker_lists_slugs_and_enter_switches_the_store() {
    let (dir, mut app) = setup();
    add_task(&mut app, "First task", None);
    app.refresh();

    // A second project with its own store.
    let store_root = tempfile::tempdir().expect("store root");
    let second_store = store_root.path().join("second");
    let second_project = store_root.path().join("project");
    fs::create_dir_all(&second_project).expect("create second project");
    let mut second_vault = Vault::open(&second_store).expect("open second vault");
    let second_task = second_vault
        .add(NewTask::new("Second project task"))
        .expect("add second task")
        .id;
    drop(second_vault);

    app.store_root = Some(store_root.path().to_path_buf());
    app.config.projects = vec![
        Project {
            path: dir.path().to_path_buf(),
            slug: "first".to_owned(),
            never_ask_nested: false,
        },
        Project {
            path: second_project,
            slug: "second".to_owned(),
            never_ask_nested: false,
        },
    ];

    app.handle_key(key(KeyCode::Char('p')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Project)));
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("first"), "picker lists slugs: {text}");
    assert!(text.contains("second"), "picker lists slugs: {text}");

    for character in "second".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    let matches = app.project_matches();
    assert_eq!(matches.len(), 1, "filter narrows: {matches:?}");
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(app.vault.root(), second_store.as_path());
    assert!(app.vault.get(&second_task).is_some());
    assert_eq!(app.project.as_ref().expect("project").slug, "second");
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(
        text.contains("Second project task"),
        "list switched: {text}"
    );
}

#[test]
fn expand_tilde_only_expands_a_leading_tilde() {
    let home = Path::new("/home/me");
    assert_eq!(
        expand_tilde_with_home("~", Some(home)),
        Some(PathBuf::from("/home/me"))
    );
    assert_eq!(
        expand_tilde_with_home("~/work", Some(home)),
        Some(PathBuf::from("/home/me/work"))
    );
    assert_eq!(
        expand_tilde_with_home("/abs", Some(home)),
        Some(PathBuf::from("/abs"))
    );
    assert_eq!(
        expand_tilde_with_home("rel", Some(home)),
        Some(PathBuf::from("rel"))
    );
    assert_eq!(
        expand_tilde_with_home("~user/x", Some(home)),
        Some(PathBuf::from("~user/x"))
    );
    assert_eq!(expand_tilde_with_home("~/x", None), None);
    assert_eq!(expand_tilde_with_home("~", None), None);
}

#[test]
fn shift_p_opens_the_register_path_popup() {
    let (dir, mut app) = setup();
    app.store_root = Some(dir.path().join("data"));
    add_task(&mut app, "Only", None);
    app.refresh();

    app.handle_key(key(KeyCode::Char('P')));
    assert!(matches!(app.mode, InputMode::RegisterPath));
    assert_eq!(
        footer_text(&app.status_line()),
        "enter register   esc cancel"
    );
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("Register a project directory"), "{text}");
    assert!(text.contains("new project"), "popup title: {text}");
    assert!(!text.contains("projects"), "not the project picker: {text}");
}

#[test]
fn shift_p_registers_and_switches_to_a_new_project() {
    let home = tempfile::tempdir().expect("home");
    let dir = tempfile::tempdir().expect("temp dir");
    let store_root = dir.path().join("data");
    let config_path = dir.path().join("config.toml");
    let vault = Vault::open(dir.path().join("vault")).expect("open vault");
    let config = Config::load_from(Some(config_path.clone())).expect("load");
    let mut app = App::new(vault, config, None, Some(store_root.clone()));

    // Point $HOME at a temp dir so `~` expansion is testable, and restore it
    // before asserting so concurrent tests see the real value.
    let previous_home = std::env::var_os("HOME");
    std::env::set_var("HOME", home.path());
    app.handle_key(key(KeyCode::Char('P')));
    for character in "~/newproj".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }

    assert!(matches!(app.mode, InputMode::Navigate));
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("switched to newproj")
    );
    assert!(
        home.path().join("newproj").is_dir(),
        "the project directory is created"
    );
    assert_eq!(app.vault.root(), store_root.join("newproj").as_path());
    assert_eq!(app.project.as_ref().expect("project").slug, "newproj");

    let reloaded = Config::load_from(Some(config_path)).expect("reload");
    assert_eq!(reloaded.projects.len(), 1);
    assert_eq!(reloaded.projects[0].slug, "newproj");
}

#[test]
fn shift_p_empty_path_toasts_and_stays_open() {
    let dir = tempfile::tempdir().expect("temp dir");
    let vault = Vault::open(dir.path().join("vault")).expect("open vault");
    let config = Config::load_from(Some(dir.path().join("config.toml"))).expect("load");
    let mut app = App::new(vault, config, None, Some(dir.path().join("data")));

    app.handle_key(key(KeyCode::Char('P')));
    app.handle_key(key(KeyCode::Enter));

    assert!(
        matches!(app.mode, InputMode::RegisterPath),
        "the prompt stays open"
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("path cannot be empty")
    );
}

#[test]
fn shift_p_uncreatable_path_toasts_and_does_not_switch() {
    let dir = tempfile::tempdir().expect("temp dir");
    let vault = Vault::open(dir.path().join("vault")).expect("open vault");
    let config = Config::load_from(Some(dir.path().join("config.toml"))).expect("load");
    let mut app = App::new(vault, config, None, Some(dir.path().join("data")));
    let original_root = app.vault.root().to_path_buf();

    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "not a directory").expect("write blocker");
    let target = blocker.join("sub");
    app.handle_key(key(KeyCode::Char('P')));
    for character in target.to_str().expect("utf-8").chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert!(
        matches!(app.mode, InputMode::RegisterPath),
        "the prompt stays open"
    );
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|toast| toast.text.starts_with("error:")),
        "toast: {:?}",
        app.toast
    );
    assert!(app.config.projects.is_empty());
    assert_eq!(app.vault.root(), original_root);
}

#[test]
fn shift_p_esc_cancels_without_registering() {
    let dir = tempfile::tempdir().expect("temp dir");
    let vault = Vault::open(dir.path().join("vault")).expect("open vault");
    let config = Config::load_from(Some(dir.path().join("config.toml"))).expect("load");
    let mut app = App::new(vault, config, None, Some(dir.path().join("data")));

    app.handle_key(key(KeyCode::Char('P')));
    for character in "/some/where".chars() {
        app.handle_key(key(KeyCode::Char(character)));
    }
    app.handle_key(key(KeyCode::Esc));

    assert!(matches!(app.mode, InputMode::Navigate));
    assert!(app.input.is_empty());
    assert!(app.config.projects.is_empty());
    assert!(app.toast.is_none());
}

#[test]
fn shift_p_without_a_data_directory_toasts() {
    let (_dir, mut app) = setup();
    app.handle_key(key(KeyCode::Char('P')));
    assert!(
        matches!(app.mode, InputMode::Navigate),
        "no prompt opens without a store root"
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("no data directory is available")
    );
}

#[test]
fn link_picker_jumps_to_linked_and_backlinked_tasks() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Target"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("[[{target}]]"),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    let fan = app
        .vault
        .add(NewTask {
            body: format!("[[{source}]]"),
            ..NewTask::new("Fan")
        })
        .expect("add fan")
        .id;
    app.refresh();
    app.selected = Some(source.clone());

    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Link { .. })));
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("Target"), "outlink in picker: {text}");
    assert!(text.contains("Fan"), "backlink in picker: {text}");

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.selected_id(),
        Some(target),
        "enter jumps to the outlink"
    );

    // Reopen on the source and reach the backlink with the arrow keys.
    app.selected = Some(source.clone());
    app.handle_key(key(KeyCode::Char('o')));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.selected_id(), Some(fan), "arrows reach the backlink");

    // Esc keeps the selection.
    app.selected = Some(source.clone());
    app.handle_key(key(KeyCode::Char('o')));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(app.selected_id(), Some(source), "esc keeps the selection");
}

#[test]
fn link_picker_marks_missing_targets_and_enter_toasts() {
    let (_dir, mut app) = setup();
    let ghost = parse_id("missing001");
    let source = app
        .vault
        .add(NewTask {
            body: format!("[[{ghost}]]"),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.refresh();
    app.selected = Some(source.clone());

    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Link { .. })));
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(
        text.contains("missing001 (missing)"),
        "unresolvable outlinks are marked: {text}"
    );

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::Navigate);
    assert_eq!(
        app.selected_id(),
        Some(source),
        "a missing target must not move the selection"
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("not found")
    );

    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(text.contains("not found"), "toast renders: {text}");
}

#[test]
fn link_picker_jumps_through_rich_link_forms() {
    let (_dir, mut app) = setup();
    let target = app
        .vault
        .add(NewTask::new("Target"))
        .expect("add target")
        .id;
    let source = app
        .vault
        .add(NewTask {
            body: format!("[[{target}.md|Stale alias]]"),
            ..NewTask::new("Source")
        })
        .expect("add source")
        .id;
    app.refresh();
    app.selected = Some(source.clone());

    app.handle_key(key(KeyCode::Char('o')));
    assert!(matches!(app.picker_kind(), Some(PickerKind::Link { .. })));
    let text = render_lines(&mut app, 100, 20).join("\n");
    assert!(
        text.contains("Target"),
        "the picker shows the live title: {text}"
    );
    assert!(!text.contains("Stale alias"), "{text}");

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.selected_id(),
        Some(target),
        "a .md|alias link still jumps by stem"
    );
}

#[test]
fn unregistered_launch_prompts_and_declining_quits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fresh = dir.path().join("fresh");
    let mut launch = Launch::new(
        Config::default(),
        Resolution::Unregistered {
            path: fresh.clone(),
        },
        dir.path().join("data"),
    );
    assert!(!launch.ready());
    assert!(!launch.should_quit());

    let text = render_launch_lines(&launch, 90, 12).join("\n");
    assert!(text.contains("not a registered project"), "{text}");

    launch.handle_key(key(KeyCode::Char('n')));
    assert!(launch.should_quit());
}

#[test]
fn unregistered_launch_accepting_registers_the_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fresh = dir.path().join("fresh");
    fs::create_dir_all(&fresh).expect("project dir");
    let config_path = dir.path().join("config.toml");
    let config = Config::load_from(Some(config_path.clone())).expect("load");

    let mut launch = Launch::new(
        config,
        Resolution::Unregistered {
            path: fresh.clone(),
        },
        dir.path().join("data"),
    );
    launch.handle_key(key(KeyCode::Char('y')));
    assert!(launch.ready(), "registered");

    let (config, resolved) = launch.into_parts();
    assert_eq!(resolved.slug, "fresh");
    assert!(
        dir.path().join("data").join("fresh").is_dir(),
        "store created"
    );
    assert_eq!(config.projects.len(), 1);

    let reloaded = Config::load_from(Some(config_path)).expect("reload");
    assert_eq!(reloaded.projects.len(), 1);
    assert_eq!(reloaded.projects[0].slug, resolved.slug);
}

#[test]
fn unregistered_launch_buttons_default_to_yes_and_cancel() {
    let (_dir, mut launch) = register_launch();
    assert_eq!(launch.button_index(), 0, "Yes is the default");
    let text = render_launch_lines(&launch, 90, 12).join("\n");
    assert!(text.contains("[y]"), "{text}");
    assert!(text.contains("Yes"), "{text}");
    assert!(text.contains("[p]"), "{text}");
    assert!(text.contains("Projects"), "{text}");
    assert!(text.contains("[n]"), "{text}");
    assert!(text.contains("Cancel"), "{text}");

    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.ready(), "Enter on the default Yes registers");

    // Right twice moves to Cancel; Enter then quits without registering.
    let (_dir, mut launch) = register_launch();
    launch.handle_key(key(KeyCode::Right));
    assert_eq!(launch.button_index(), 1, "right lands on Projects");
    launch.handle_key(key(KeyCode::Right));
    assert_eq!(launch.button_index(), 2, "right again lands on Cancel");
    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.should_quit());
    assert!(!launch.ready());

    // Left from Yes wraps to Cancel; `h` moves back.
    let (_dir, mut launch) = register_launch();
    launch.handle_key(key(KeyCode::Left));
    assert_eq!(launch.button_index(), 2, "left wraps to Cancel");
    launch.handle_key(key(KeyCode::Char('h')));
    assert_eq!(launch.button_index(), 1, "h moves left onto Projects");
    launch.handle_key(key(KeyCode::Char('h')));
    assert_eq!(launch.button_index(), 0, "h moves left onto Yes");
}

#[test]
fn unregistered_launch_esc_cancels() {
    let (_dir, mut launch) = register_launch();
    launch.handle_key(key(KeyCode::Esc));
    assert!(launch.should_quit());
    assert!(!launch.ready());
}

#[test]
fn launch_buttons_render_with_the_default_highlighted() {
    let (_dir, launch) = register_launch();
    let cells = render_launch_cells(&launch, 90, 12);
    let yes = style_at_text(&cells, "[y]");
    assert!(
        yes.add_modifier.contains(Modifier::REVERSED),
        "Yes key is highlighted: {yes:?}"
    );
    let yes_label = style_at_text(&cells, "Yes");
    assert!(
        !yes_label.add_modifier.contains(Modifier::REVERSED),
        "Yes caption is not reversed: {yes_label:?}"
    );
    let projects = style_at_text(&cells, "[p]");
    assert!(!projects.add_modifier.contains(Modifier::REVERSED));
    let cancel = style_at_text(&cells, "[n]");
    assert!(
        !cancel.add_modifier.contains(Modifier::REVERSED),
        "Cancel key is not highlighted: {cancel:?}"
    );

    let (_dir, launch, _config_path) = nested_launch();
    let cells = render_launch_cells(&launch, 90, 12);
    let parent = style_at_text(&cells, "[ Register parent ]");
    assert!(
        parent.add_modifier.contains(Modifier::REVERSED),
        "register parent is the nested default: {parent:?}"
    );
    let separate = style_at_text(&cells, "[ Register separately ]");
    assert!(!separate.add_modifier.contains(Modifier::REVERSED));
    let cancel = style_at_text(&cells, "[ Cancel ]");
    assert!(!cancel.add_modifier.contains(Modifier::REVERSED));
}

#[test]
fn nested_launch_never_ask_persists_the_rule() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project_dir = dir.path().join("proj");
    let nested = project_dir.join("sub");
    fs::create_dir_all(&nested).expect("nested dir");
    let config_path = dir.path().join("config.toml");

    let parent = project(project_dir.to_str().expect("utf-8"), "proj");
    let mut config = Config::load_from(Some(config_path.clone())).expect("load");
    config.projects.push(parent.clone());
    config.save().expect("save");

    let mut launch = Launch::new(
        config,
        Resolution::Registered {
            project: parent,
            nested: Some(nested.clone()),
        },
        dir.path().join("data"),
    );
    assert!(!launch.ready(), "nested directories ask first");
    let text = render_launch_lines(&launch, 90, 12).join("\n");
    assert!(text.contains("separate project"), "{text}");
    assert!(text.contains("! = never ask"), "{text}");

    launch.handle_key(key(KeyCode::Char('!')));
    assert!(launch.ready());
    let (_config, resolved) = launch.into_parts();
    assert_eq!(resolved.slug, "proj", "keeps the parent project");

    let reloaded = Config::load_from(Some(config_path)).expect("reload");
    assert!(reloaded.projects[0].never_ask_nested, "rule persisted");
}

#[test]
fn nested_launch_accepting_registers_a_separate_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let project_dir = dir.path().join("proj");
    let nested = project_dir.join("sub");
    fs::create_dir_all(&nested).expect("nested dir");
    let config_path = dir.path().join("config.toml");

    let parent = project(project_dir.to_str().expect("utf-8"), "proj");
    let mut config = Config::load_from(Some(config_path)).expect("load");
    config.projects.push(parent.clone());
    config.save().expect("save");

    let mut launch = Launch::new(
        config,
        Resolution::Registered {
            project: parent,
            nested: Some(nested.clone()),
        },
        dir.path().join("data"),
    );
    launch.handle_key(key(KeyCode::Char('y')));
    assert!(launch.ready());

    let (config, resolved) = launch.into_parts();
    assert_eq!(
        resolved.slug, "sub",
        "the nested dir becomes its own project"
    );
    assert_eq!(config.projects.len(), 2);
    assert!(dir.path().join("data").join("sub").is_dir());
}

#[test]
fn nested_launch_buttons_default_to_parent_and_enter_keeps_it() {
    let (_dir, mut launch, _config_path) = nested_launch();
    assert_eq!(launch.button_index(), 0, "Register parent is the default");

    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.ready());
    let (config, resolved) = launch.into_parts();
    assert_eq!(resolved.slug, "proj", "keeps the parent project");
    assert_eq!(config.projects.len(), 1, "no new project registered");
}

#[test]
fn nested_launch_buttons_register_separately() {
    let (dir, mut launch, _config_path) = nested_launch();
    launch.handle_key(key(KeyCode::Right));
    assert_eq!(launch.button_index(), 1);
    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.ready());

    let (config, resolved) = launch.into_parts();
    assert_eq!(resolved.slug, "sub");
    assert_eq!(config.projects.len(), 2);
    assert!(dir.path().join("data").join("sub").is_dir());
}

#[test]
fn nested_launch_cancel_button_and_esc_quit_but_n_keeps_the_parent() {
    // Right twice to Cancel; Enter quits without resolving.
    let (_dir, mut launch, _config_path) = nested_launch();
    launch.handle_key(key(KeyCode::Right));
    launch.handle_key(key(KeyCode::Right));
    assert_eq!(launch.button_index(), 2);
    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.should_quit());
    assert!(!launch.ready());

    // Esc quits too (behavior change; `n` remains the keep-parent key).
    let (_dir, mut launch, _config_path) = nested_launch();
    launch.handle_key(key(KeyCode::Esc));
    assert!(launch.should_quit());
    assert!(!launch.ready());

    // `n` still keeps the parent.
    let (_dir, mut launch, _config_path) = nested_launch();
    launch.handle_key(key(KeyCode::Char('n')));
    assert!(launch.ready());
    assert!(!launch.should_quit());
}

#[test]
fn launch_is_ready_without_a_modal_for_exact_and_never_ask_hits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let exact = Launch::new(
        Config::default(),
        Resolution::Registered {
            project: project("/proj", "proj"),
            nested: None,
        },
        dir.path().join("data"),
    );
    assert!(exact.ready());
    assert!(!exact.should_quit());

    let mut parent = project("/proj", "proj");
    parent.never_ask_nested = true;
    let nested = Launch::new(
        Config::default(),
        Resolution::Registered {
            project: parent,
            nested: Some(PathBuf::from("/proj/sub")),
        },
        dir.path().join("data"),
    );
    assert!(nested.ready());
}

#[test]
fn unregistered_launch_cancel_is_right_aligned() {
    let (_dir, launch) = register_launch();
    let text = render_launch_lines(&launch, 90, 12);
    let row = text
        .iter()
        .find(|line| line.contains("[y]") && line.contains("[n]"))
        .expect("button row");
    let p = row.find("[p]").expect("p");
    let n = row.find("[n]").expect("n");
    assert!(n > p + 10, "Cancel should sit on the right: {row:?}");
}

#[test]
fn unregistered_p_opens_the_project_picker_and_esc_returns() {
    let (_dir, mut launch) = unregistered_launch_with_other();
    launch.handle_key(key(KeyCode::Char('p')));
    let pick = launch.project_pick().expect("picker");
    assert_eq!(pick.2.len(), 1);
    assert_eq!(pick.2[0].slug, "other");
    let text = render_launch_lines(&launch, 90, 16).join("\n");
    assert!(text.contains("other"), "{text}");
    assert!(text.contains("project:"), "{text}");

    launch.handle_key(key(KeyCode::Esc));
    assert!(launch.project_pick().is_none());
    assert!(!launch.should_quit());
    assert!(!launch.ready());
    let text = render_launch_lines(&launch, 90, 12).join("\n");
    assert!(text.contains("not a registered project"), "{text}");
}

#[test]
fn unregistered_p_binds_without_registering_cwd() {
    let (dir, mut launch) = unregistered_launch_with_other();
    launch.handle_key(key(KeyCode::Char('p')));
    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.ready());
    let (config, project) = launch.into_parts();
    assert_eq!(project.slug, "other");
    assert_eq!(config.projects.len(), 1);
    assert_eq!(config.projects[0].slug, "other");
    let fresh = dir.path().join("fresh");
    assert!(
        !config.projects.iter().any(|project| project.path == fresh),
        "cwd stayed unregistered"
    );
}

#[test]
fn unregistered_p_with_empty_registry_opens_the_path_prompt() {
    let (_dir, mut launch) = register_launch();
    launch.handle_key(key(KeyCode::Char('p')));
    assert_eq!(launch.path_input(), Some(""));
    let text = render_launch_lines(&launch, 90, 12).join("\n");
    assert!(text.contains("Register a project directory"), "{text}");

    launch.handle_key(key(KeyCode::Esc));
    assert!(launch.path_input().is_none());
    assert!(!launch.should_quit());
    assert!(!launch.ready());

    let (dir, mut launch) = register_launch();
    let other = dir.path().join("elsewhere");
    launch.handle_key(key(KeyCode::Char('p')));
    for character in other.to_str().expect("utf-8").chars() {
        launch.handle_key(key(KeyCode::Char(character)));
    }
    launch.handle_key(key(KeyCode::Enter));
    assert!(launch.ready(), "registered the typed path");
    let (config, project) = launch.into_parts();
    assert_eq!(config.projects.len(), 1);
    assert_eq!(
        project.path,
        fs::canonicalize(other).expect("canonical registered path")
    );
}

#[test]
fn launch_project_picker_paths_share_a_fixed_column() {
    let dir = tempfile::tempdir().expect("temp dir");
    let one = project("/one", "x");
    let two = project("/two", "much-longer");
    let wide = project("/wide", "猫");
    let mut config = Config::default();
    config.projects = vec![one, two, wide];
    let launch = Launch::new_projects(
        config,
        Resolution::Unregistered {
            path: dir.path().join("fresh"),
        },
        dir.path().join("data"),
    );

    let lines = render_launch_lines(&launch, 90, 16);
    let one = lines.iter().find(|row| row.contains("/one")).expect("one");
    let two = lines.iter().find(|row| row.contains("/two")).expect("two");
    let wide = lines
        .iter()
        .find(|row| row.contains("/wide"))
        .expect("wide");
    assert_eq!(
        column(one, "x"),
        column(two, "much-longer"),
        "launch project slugs share one left-aligned column: {lines:?}"
    );
    assert_eq!(
        column(one, "/one"),
        column(two, "/two"),
        "launch project paths share one left-aligned column: {lines:?}"
    );
    assert_eq!(
        column(wide, "/wide"),
        column(two, "/two"),
        "a wide slug does not shift the launch path column: {lines:?}"
    );
}

#[test]
fn narrow_launch_project_picker_reserves_a_visible_path_column() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = Config::default();
    config.projects = vec![
        project("/one", "a-very-long-project-slug-that-does-not-fit"),
        project("/two", "x"),
    ];
    let launch = Launch::new_projects(
        config,
        Resolution::Unregistered {
            path: dir.path().join("fresh"),
        },
        dir.path().join("data"),
    );

    let text = render_launch_lines(&launch, 24, 12).join("\n");
    assert!(
        text.contains("/one") && text.contains("/two"),
        "narrow launch picker keeps paths visible: {text}"
    );
}

#[test]
fn projects_flag_opens_the_picker_on_the_current_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let one = project("/one", "one");
    let two = project("/two", "two");
    let mut config = Config::default();
    config.projects = vec![one.clone(), two.clone()];
    let launch = Launch::new_projects(
        config,
        Resolution::Registered {
            project: two.clone(),
            nested: None,
        },
        dir.path().join("data"),
    );
    let (_query, highlight, projects) = launch.project_pick().expect("picker");
    assert_eq!(highlight, 1);
    assert_eq!(projects[1].slug, "two");
    let cells = render_launch_cells(&launch, 90, 16);
    let two_style = style_at_text(&cells, "two");
    assert!(
        two_style.add_modifier.contains(Modifier::REVERSED),
        "current project is highlighted: {two_style:?}"
    );
}

#[test]
fn projects_flag_esc_quits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = Config::default();
    config.projects.push(project("/one", "one"));
    let mut launch = Launch::new_projects(
        config,
        Resolution::Unregistered {
            path: dir.path().join("fresh"),
        },
        dir.path().join("data"),
    );
    assert!(launch.project_pick().is_some());
    launch.handle_key(key(KeyCode::Esc));
    assert!(launch.should_quit());
    assert!(!launch.ready());
}

#[test]
fn projects_flag_empty_registry_path_prompt_esc_quits() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut launch = Launch::new_projects(
        Config::default(),
        Resolution::Unregistered {
            path: dir.path().join("fresh"),
        },
        dir.path().join("data"),
    );
    assert_eq!(launch.path_input(), Some(""));
    launch.handle_key(key(KeyCode::Esc));
    assert!(launch.should_quit());
}

#[test]
fn scroll_clamp_keeps_the_selection_inside_the_viewport() {
    // Moving past the bottom scrolls the minimum amount.
    assert_eq!(ensure_selection_visible(20, 19, 0, 8), 12);
    // Moving back above the top scrolls back to it.
    assert_eq!(ensure_selection_visible(20, 0, 12, 8), 0);
    // Everything fits: no scroll.
    assert_eq!(ensure_selection_visible(3, 2, 0, 8), 0);
    // A zero-height viewport still keeps the selection visible.
    assert_eq!(ensure_selection_visible(20, 5, 0, 0), 5);
    // An empty list cannot scroll.
    assert_eq!(ensure_selection_visible(0, 0, 0, 8), 0);
}
