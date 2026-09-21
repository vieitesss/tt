//! Integration tests for the derived vault index (tree, tags, links,
//! backlinks, rollup) through the public API.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use chrono::NaiveDate;
use tt::{NewTask, Task, TaskFilter, TaskId, TaskState, Vault, VaultIssueKind};

fn open_vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("temp dir");
    let vault = Vault::open(dir.path()).expect("open vault");
    (dir, vault)
}

fn parse_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("valid id")
}

fn add_task(
    vault: &mut Vault,
    title: &str,
    parent: Option<&TaskId>,
    state: TaskState,
    tags: &[&str],
    body: &str,
) -> Task {
    let task = vault
        .add(NewTask {
            parent: parent.cloned(),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            body: body.to_owned(),
            ..NewTask::new(title)
        })
        .expect("add task");
    if state == TaskState::Open {
        task
    } else {
        vault.set_state(&task.id, state).expect("set state")
    }
}

/// Ids matching `filter`; the due filter is not exercised through this
/// helper, so a fixed date pins the clock out of the tag/state tests.
fn ids_matching(vault: &Vault, filter: &TaskFilter) -> BTreeSet<TaskId> {
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    vault
        .filter(filter, today)
        .iter()
        .map(|task| task.id.clone())
        .collect()
}

#[test]
fn three_level_tree_has_correct_roots_and_children() {
    let (_dir, mut vault) = open_vault();
    let root = add_task(&mut vault, "Root", None, TaskState::Open, &[], "");
    let child = add_task(
        &mut vault,
        "Child",
        Some(&root.id),
        TaskState::Open,
        &[],
        "",
    );
    let grandchild = add_task(
        &mut vault,
        "Grandchild",
        Some(&child.id),
        TaskState::Open,
        &[],
        "",
    );
    let second_root = add_task(&mut vault, "Another root", None, TaskState::Open, &[], "");

    assert_eq!(
        vault.roots().to_vec(),
        vec![second_root.id.clone(), root.id.clone()],
        "roots are ordered by title, case-insensitively"
    );
    assert_eq!(vault.children(&root.id).to_vec(), vec![child.id.clone()]);
    assert_eq!(
        vault.children(&child.id).to_vec(),
        vec![grandchild.id.clone()]
    );
    assert_eq!(vault.parent(&grandchild.id), Some(&child.id));
    assert_eq!(vault.parent(&root.id), None);

    let tree = vault.tree();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].task.id, second_root.id);
    assert_eq!(tree[1].task.id, root.id);
    assert_eq!(tree[1].children.len(), 1);
    assert_eq!(tree[1].children[0].task.id, child.id);
    assert_eq!(tree[1].children[0].children[0].task.id, grandchild.id);
    assert!(tree[1].children[0].children[0].children.is_empty());
}

#[test]
fn filter_tree_root_order_matches_tree_order() {
    let (_dir, mut vault) = open_vault();
    add_task(&mut vault, "zeta", None, TaskState::Open, &[], "");
    add_task(&mut vault, "Alpha", None, TaskState::Open, &[], "");
    add_task(&mut vault, "beta", None, TaskState::Open, &[], "");

    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    let unfiltered: Vec<String> = vault
        .tree()
        .iter()
        .map(|node| node.task.title.clone())
        .collect();
    let filtered: Vec<String> = vault
        .filter_tree(&TaskFilter::default(), today)
        .iter()
        .map(|node| node.task.title.clone())
        .collect();

    assert_eq!(filtered, unfiltered, "filtered roots keep the Index order");
    assert_eq!(filtered, vec!["Alpha", "beta", "zeta"]);
}

#[test]
fn dangling_parent_makes_task_a_root_and_reattaches_when_it_appears() {
    let (dir, mut vault) = open_vault();
    let ghost = parse_id("ghost00001");
    let orphan = add_task(&mut vault, "Orphan", Some(&ghost), TaskState::Open, &[], "");
    let child = add_task(
        &mut vault,
        "Child",
        Some(&orphan.id),
        TaskState::Open,
        &[],
        "",
    );

    assert_eq!(vault.parent(&orphan.id), None);
    assert!(vault.roots().contains(&orphan.id));
    assert_eq!(vault.tree().len(), 1);
    assert_eq!(vault.children(&orphan.id), &[child.id.clone()][..]);
    assert_eq!(
        vault.rollup(&orphan.id),
        (0, 1),
        "a dangling-parent root still rolls up its own subtree"
    );

    let issues: Vec<_> = vault
        .issues()
        .iter()
        .filter(|issue| issue.kind == VaultIssueKind::DanglingParent)
        .collect();
    assert_eq!(issues.len(), 1);
    assert!(issues[0].detail.contains("ghost00001"));
    assert!(issues[0].detail.contains(orphan.id.as_str()));

    // The orphan's file was never touched: when the parent id appears, the
    // next scan reattaches it automatically.
    fs::write(
        dir.path().join("ghost00001.md"),
        Task::new(ghost.clone(), "Ghost").to_document(),
    )
    .expect("write ghost");
    vault.reload();

    assert_eq!(vault.parent(&orphan.id), Some(&ghost));
    assert!(vault.roots().contains(&ghost));
    assert!(!vault.roots().contains(&orphan.id));
    assert_eq!(
        vault.rollup(&ghost),
        (0, 2),
        "orphan and child now nest under ghost"
    );
    assert!(
        vault
            .issues()
            .iter()
            .all(|issue| issue.kind != VaultIssueKind::DanglingParent),
        "reattaching clears the dangling-parent issue"
    );
}

#[test]
fn parent_cycles_are_broken_reported_and_traversal_terminates() {
    let (dir, mut vault) = open_vault();
    let a_id = parse_id("cycle00001");
    let b_id = parse_id("cycle00002");

    let mut a = Task::new(a_id.clone(), "A");
    a.parent = Some(b_id.clone());
    let mut b = Task::new(b_id.clone(), "B");
    b.parent = Some(a_id.clone());
    fs::write(dir.path().join("cycle00001.md"), a.to_document()).expect("write a");
    fs::write(dir.path().join("cycle00002.md"), b.to_document()).expect("write b");

    vault.reload();

    assert_eq!(vault.len(), 2);
    assert!(vault.get(&a_id).is_some());
    assert!(vault.get(&b_id).is_some());
    let cycles: Vec<_> = vault
        .issues()
        .iter()
        .filter(|issue| issue.kind == VaultIssueKind::Cycle)
        .collect();
    assert_eq!(cycles.len(), 1, "one cycle edge is reported");
    assert!(
        cycles[0].detail.contains("cycle00001")
            && cycles[0].detail.contains("cycle00002")
            && cycles[0].detail.contains("→"),
        "the issue names the loop: {}",
        cycles[0].detail
    );

    let tree = vault.tree();
    assert_eq!(tree.len(), 1, "one cycle edge is dropped, leaving one root");
    assert_eq!(tree[0].children.len(), 1);
    assert_eq!(tree[0].task.id, a_id);
    assert_eq!(tree[0].children[0].task.id, b_id);
    assert!(vault.rollup(&a_id).0 <= 1);
}

#[test]
fn self_parent_is_reported_and_treated_as_root() {
    let (dir, mut vault) = open_vault();
    let id = parse_id("self000001");
    let mut task = Task::new(id.clone(), "Self");
    task.parent = Some(id.clone());
    fs::write(dir.path().join("self000001.md"), task.to_document()).expect("write");

    vault.reload();

    assert_eq!(vault.parent(&id), None);
    assert!(vault.roots().contains(&id));
    let cycles: Vec<_> = vault
        .issues()
        .iter()
        .filter(|issue| issue.kind == VaultIssueKind::Cycle)
        .collect();
    assert_eq!(cycles.len(), 1, "a self-parent is a one-node cycle");
    assert!(
        cycles[0].detail.contains("self000001") && cycles[0].detail.contains("→"),
        "the issue names the loop: {}",
        cycles[0].detail
    );
}

#[test]
fn tag_filter_matches_nested_tags_only_downward() {
    let (_dir, mut vault) = open_vault();
    let work = add_task(&mut vault, "Work", None, TaskState::Open, &["work"], "");
    let admin = add_task(
        &mut vault,
        "Admin",
        None,
        TaskState::Open,
        &["work/admin"],
        "",
    );
    let deep = add_task(
        &mut vault,
        "Deep",
        None,
        TaskState::Open,
        &["work/admin/x"],
        "",
    );
    let other = add_task(
        &mut vault,
        "Other",
        None,
        TaskState::Open,
        &["workshop"],
        "",
    );

    let matched = ids_matching(
        &vault,
        &TaskFilter {
            tag: Some("work".to_owned()),
            ..TaskFilter::default()
        },
    );
    assert_eq!(
        matched,
        BTreeSet::from([work.id.clone(), admin.id.clone(), deep.id.clone()])
    );

    let matched = ids_matching(
        &vault,
        &TaskFilter {
            tag: Some("#work/admin".to_owned()),
            ..TaskFilter::default()
        },
    );
    assert_eq!(matched, BTreeSet::from([admin.id.clone(), deep.id.clone()]));

    let matched = ids_matching(
        &vault,
        &TaskFilter {
            tag: Some("work/admin/x".to_owned()),
            ..TaskFilter::default()
        },
    );
    assert_eq!(matched, BTreeSet::from([deep.id.clone()]));
    assert!(!matched.contains(&other.id));
}

#[test]
fn links_and_backlinks_ignore_fenced_code_blocks() {
    let (_dir, mut vault) = open_vault();
    let target = add_task(&mut vault, "Target", None, TaskState::Open, &[], "");
    let target_id = target.id.clone();
    let source = add_task(
        &mut vault,
        "Source",
        None,
        TaskState::Open,
        &[],
        &format!(
            "see [[{target_id}]] and [[{target_id}]] again\n\
             ```\n[[ghost00001]]\n```\n\
             ~~~\n[[ghost00002]]\n~~~\n"
        ),
    );

    assert_eq!(vault.links(&source.id), &[target.id.clone()][..]);
    assert_eq!(vault.links(&source.id).len(), 1, "duplicate links collapse");
    assert_eq!(vault.backlinks(&target.id), &[source.id.clone()][..]);
}

#[test]
fn rich_link_forms_resolve_to_stems() {
    let (_dir, mut vault) = open_vault();
    let target = add_task(&mut vault, "Target", None, TaskState::Open, &[], "");
    let source = add_task(
        &mut vault,
        "Source",
        None,
        TaskState::Open,
        &[],
        &format!(
            "see [[{}.md|Target title]], [[{}.md]], and [[sub/{}.md|elsewhere]]",
            target.id, target.id, target.id
        ),
    );

    assert_eq!(vault.links(&source.id), &[target.id.clone()][..]);
    assert_eq!(vault.backlinks(&target.id), &[source.id.clone()][..]);
}

#[test]
fn dangling_links_are_allowed() {
    let (_dir, mut vault) = open_vault();
    let ghost = parse_id("missing001");
    let linker = add_task(
        &mut vault,
        "Linker",
        None,
        TaskState::Open,
        &[],
        &format!("points at [[{ghost}]]"),
    );

    assert_eq!(vault.links(&linker.id), &[ghost.clone()][..]);
    assert_eq!(vault.backlinks(&ghost), &[linker.id.clone()][..]);
}

#[test]
fn rollup_counts_non_cancelled_descendants_recursively() {
    let (_dir, mut vault) = open_vault();
    let parent = add_task(&mut vault, "Parent", None, TaskState::Open, &[], "");
    let done_one = add_task(
        &mut vault,
        "Done one",
        Some(&parent.id),
        TaskState::Done,
        &[],
        "",
    );
    let _done_two = add_task(
        &mut vault,
        "Done two",
        Some(&parent.id),
        TaskState::Done,
        &[],
        "",
    );
    let _open_one = add_task(
        &mut vault,
        "Open one",
        Some(&parent.id),
        TaskState::Open,
        &[],
        "",
    );
    let cancelled = add_task(
        &mut vault,
        "Cancelled",
        Some(&parent.id),
        TaskState::Cancelled,
        &[],
        "",
    );

    assert_eq!(vault.rollup(&parent.id), (2, 3));
    assert_eq!(vault.rollup(&cancelled.id), (0, 0));

    let nested_open = add_task(
        &mut vault,
        "Nested open",
        Some(&done_one.id),
        TaskState::Open,
        &[],
        "",
    );
    assert_eq!(vault.rollup(&parent.id), (2, 4));
    assert_eq!(vault.rollup(&done_one.id), (0, 1));
    assert_eq!(vault.rollup(&nested_open.id), (0, 0));
}

#[test]
fn filter_combines_state_tag_and_due_today() {
    let (_dir, mut vault) = open_vault();
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    let overdue = today.pred_opt().expect("overdue date");
    let tomorrow = today.succ_opt().expect("future date");

    let due_today = vault
        .add(NewTask {
            tags: vec!["work".to_owned()],
            due: Some(today),
            ..NewTask::new("Due today")
        })
        .expect("add due today");
    let overdue_done = vault
        .add(NewTask {
            tags: vec!["work".to_owned()],
            due: Some(overdue),
            ..NewTask::new("Overdue done")
        })
        .expect("add overdue");
    vault
        .set_state(&overdue_done.id, TaskState::Done)
        .expect("set state");
    let future = vault
        .add(NewTask {
            tags: vec!["work".to_owned()],
            due: Some(tomorrow),
            ..NewTask::new("Future")
        })
        .expect("add future");
    let no_due = vault
        .add(NewTask {
            tags: vec!["work".to_owned()],
            ..NewTask::new("No due")
        })
        .expect("add no due");

    let due_ids: BTreeSet<_> = vault
        .filter(
            &TaskFilter {
                due_today: true,
                ..TaskFilter::default()
            },
            today,
        )
        .iter()
        .map(|task| task.id.clone())
        .collect();
    assert_eq!(
        due_ids,
        BTreeSet::from([due_today.id.clone(), overdue_done.id.clone()])
    );
    assert!(!due_ids.contains(&future.id));
    assert!(!due_ids.contains(&no_due.id));

    let combined = vault.filter(
        &TaskFilter {
            tag: Some("work".to_owned()),
            state: Some(TaskState::Open),
            due_today: true,
        },
        today,
    );
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].id, due_today.id);
}

#[test]
fn due_today_includes_yesterday_and_today_but_not_tomorrow() {
    let (_dir, mut vault) = open_vault();
    let today = NaiveDate::from_ymd_opt(2026, 6, 15).expect("date");
    let yesterday = today.pred_opt().expect("yesterday");
    let tomorrow = today.succ_opt().expect("tomorrow");

    for (title, due) in [
        ("Yesterday", yesterday),
        ("Today", today),
        ("Tomorrow", tomorrow),
    ] {
        vault
            .add(NewTask {
                due: Some(due),
                ..NewTask::new(title)
            })
            .expect("add");
    }

    let titles: BTreeSet<String> = vault
        .filter(
            &TaskFilter {
                due_today: true,
                ..TaskFilter::default()
            },
            today,
        )
        .iter()
        .map(|task| task.title.clone())
        .collect();
    assert_eq!(
        titles,
        BTreeSet::from(["Today".to_owned(), "Yesterday".to_owned()])
    );
}

#[test]
fn due_today_handles_month_and_year_rollovers() {
    let (_dir, mut vault) = open_vault();
    for due in [
        NaiveDate::from_ymd_opt(2025, 12, 31).expect("date"),
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("date"),
        NaiveDate::from_ymd_opt(2026, 2, 28).expect("date"),
        NaiveDate::from_ymd_opt(2026, 3, 1).expect("date"),
        NaiveDate::from_ymd_opt(2026, 3, 2).expect("date"),
    ] {
        vault
            .add(NewTask {
                due: Some(due),
                ..NewTask::new(due.to_string())
            })
            .expect("add");
    }

    let due_titles = |today: NaiveDate| -> BTreeSet<String> {
        vault
            .filter(
                &TaskFilter {
                    due_today: true,
                    ..TaskFilter::default()
                },
                today,
            )
            .iter()
            .map(|task| task.title.clone())
            .collect()
    };

    // March 1: February's last day is due, March 2 is not.
    assert_eq!(
        due_titles(NaiveDate::from_ymd_opt(2026, 3, 1).expect("date")),
        BTreeSet::from([
            "2025-12-31".to_owned(),
            "2026-01-01".to_owned(),
            "2026-02-28".to_owned(),
            "2026-03-01".to_owned(),
        ])
    );
    // January 1: the previous year's last day is due, later dates are not.
    assert_eq!(
        due_titles(NaiveDate::from_ymd_opt(2026, 1, 1).expect("date")),
        BTreeSet::from(["2025-12-31".to_owned(), "2026-01-01".to_owned()])
    );
}

#[test]
fn reload_rebuilds_links_and_parent_edges_after_external_edits() {
    let (dir, mut vault) = open_vault();
    let first = add_task(&mut vault, "First", None, TaskState::Open, &[], "");
    let second = add_task(&mut vault, "Second", None, TaskState::Open, &[], "");

    assert!(vault.children(&first.id).is_empty());
    assert!(vault.backlinks(&first.id).is_empty());

    let mut edited = second.clone();
    edited.parent = Some(first.id.clone());
    edited.body = format!("now links to [[{}]]", first.id);
    fs::write(
        dir.path().join(format!("{}.md", second.id)),
        edited.to_document(),
    )
    .expect("external edit");

    vault.reload();

    assert_eq!(vault.children(&first.id), &[second.id.clone()][..]);
    assert_eq!(vault.backlinks(&first.id), &[second.id.clone()][..]);
    assert_eq!(vault.roots(), &[first.id.clone()][..]);
    assert!(vault.links(&second.id).contains(&first.id));
}

/// Snapshot the vault's id → title map, the shape title sync diffs against.
fn titles(vault: &Vault) -> BTreeMap<TaskId, String> {
    vault
        .tasks()
        .map(|task| (task.id.clone(), task.title.clone()))
        .collect()
}

#[test]
fn sync_mirror_aliases_rewrites_only_mirrors_and_counts_files() {
    let (dir, mut vault) = open_vault();
    let target = add_task(&mut vault, "Old title", None, TaskState::Open, &[], "");
    let mirror = add_task(
        &mut vault,
        "Mirror",
        None,
        TaskState::Open,
        &[],
        &format!("see [[{}.md|Old title]]", target.id),
    );
    let mirror_two = add_task(
        &mut vault,
        "Mirror two",
        None,
        TaskState::Open,
        &[],
        &format!("also [[{}.md|Old title]]", target.id),
    );
    let contextual = add_task(
        &mut vault,
        "Contextual",
        None,
        TaskState::Open,
        &[],
        &format!(
            "see [[{}.md|my own words]] and [[{}.md]]",
            target.id, target.id
        ),
    );
    let old = titles(&vault);

    let mut renamed = target.clone();
    renamed.title = "New title".to_owned();
    fs::write(
        dir.path().join(format!("{}.md", target.id)),
        renamed.to_document(),
    )
    .expect("external rename");
    vault.reload();

    let written = vault.sync_mirror_aliases(&old).expect("sync");

    assert_eq!(written, 2, "one write per mirror file, not per alias");
    assert_eq!(
        vault.get(&mirror.id).expect("mirror").body,
        format!("see [[{}.md|New title]]", target.id)
    );
    assert_eq!(
        vault.get(&mirror_two.id).expect("mirror two").body,
        format!("also [[{}.md|New title]]", target.id)
    );
    assert_eq!(
        vault.get(&contextual.id).expect("contextual").body,
        format!(
            "see [[{}.md|my own words]] and [[{}.md]]",
            target.id, target.id
        ),
        "contextual aliases and bare links are never touched"
    );
}

#[test]
fn sync_mirror_aliases_cascades_all_renames_in_one_pass() {
    let (dir, mut vault) = open_vault();
    let first = add_task(&mut vault, "First old", None, TaskState::Open, &[], "");
    let second = add_task(&mut vault, "Second old", None, TaskState::Open, &[], "");
    let source = add_task(
        &mut vault,
        "Source",
        None,
        TaskState::Open,
        &[],
        &format!(
            "[[{}.md|First old]] and [[{}.md|Second old]]",
            first.id, second.id
        ),
    );
    let old = titles(&vault);

    for (task, title) in [(&first, "First new"), (&second, "Second new")] {
        let mut renamed = task.clone();
        renamed.title = title.to_owned();
        fs::write(
            dir.path().join(format!("{}.md", task.id)),
            renamed.to_document(),
        )
        .expect("external rename");
    }
    vault.reload();

    let written = vault.sync_mirror_aliases(&old).expect("sync");

    assert_eq!(written, 1, "both aliases live in one file");
    assert_eq!(
        vault.get(&source.id).expect("source").body,
        format!(
            "[[{}.md|First new]] and [[{}.md|Second new]]",
            first.id, second.id
        )
    );
}

#[test]
fn sync_mirror_aliases_updates_self_links() {
    let (_dir, mut vault) = open_vault();
    let task = add_task(&mut vault, "Old self", None, TaskState::Open, &[], "");
    vault
        .set_body(&task.id, &format!("I am [[{}.md|Old self]]", task.id))
        .expect("set body");
    let old = titles(&vault);

    vault.set_title(&task.id, "New self").expect("rename");

    let written = vault.sync_mirror_aliases(&old).expect("sync");
    assert_eq!(written, 1);
    assert_eq!(
        vault.get(&task.id).expect("task").body,
        format!("I am [[{}.md|New self]]", task.id)
    );
}

#[test]
fn sync_mirror_aliases_leaves_dangling_and_cross_store_links_alone() {
    let (_dir, mut vault) = open_vault();
    let target = add_task(&mut vault, "Old title", None, TaskState::Open, &[], "");
    let ghost = parse_id("ghost00001");
    let source = add_task(
        &mut vault,
        "Source",
        None,
        TaskState::Open,
        &[],
        &format!(
            "[[{}.md|Old title]] and [[{ghost}.md|Old title]]",
            target.id
        ),
    );
    let old = titles(&vault);
    vault.set_title(&target.id, "New title").expect("rename");

    let written = vault.sync_mirror_aliases(&old).expect("sync");

    assert_eq!(written, 1);
    assert_eq!(
        vault.get(&source.id).expect("source").body,
        format!(
            "[[{}.md|New title]] and [[{ghost}.md|Old title]]",
            target.id
        ),
        "a dangling target has no old title and is exempt"
    );
}

#[test]
fn sync_mirror_aliases_never_chases_a_renamed_target_transitively() {
    let (dir, mut vault) = open_vault();
    let first = add_task(&mut vault, "A", None, TaskState::Open, &[], "");
    let second = add_task(&mut vault, "B", None, TaskState::Open, &[], "");
    let source = add_task(
        &mut vault,
        "Source",
        None,
        TaskState::Open,
        &[],
        &format!("[[{}.md|A]] [[{}.md|B]]", first.id, second.id),
    );
    let old = titles(&vault);

    for (task, title) in [(&first, "B"), (&second, "C")] {
        let mut renamed = task.clone();
        renamed.title = title.to_owned();
        fs::write(
            dir.path().join(format!("{}.md", task.id)),
            renamed.to_document(),
        )
        .expect("external rename");
    }
    vault.reload();

    assert_eq!(vault.sync_mirror_aliases(&old).expect("sync"), 1);
    assert_eq!(
        vault.get(&source.id).expect("source").body,
        format!("[[{}.md|B]] [[{}.md|C]]", first.id, second.id),
        "each diff applies against its own old title exactly once"
    );
}

#[test]
fn sync_mirror_aliases_is_idempotent() {
    let (dir, mut vault) = open_vault();
    let target = add_task(&mut vault, "Old title", None, TaskState::Open, &[], "");
    let mirror = add_task(
        &mut vault,
        "Mirror",
        None,
        TaskState::Open,
        &[],
        &format!("[[{}.md|Old title]]", target.id),
    );
    let old = titles(&vault);
    vault.set_title(&target.id, "New title").expect("rename");

    assert_eq!(vault.sync_mirror_aliases(&old).expect("sync"), 1);
    let mirror_path = dir.path().join(format!("{}.md", mirror.id));
    let after_first = fs::read_to_string(&mirror_path).expect("read");

    let clean = titles(&vault);
    vault.reload();
    assert_eq!(
        vault.sync_mirror_aliases(&clean).expect("second sync"),
        0,
        "a post-cascade reload diffs clean"
    );
    assert_eq!(
        fs::read_to_string(&mirror_path).expect("read"),
        after_first,
        "the second pass writes nothing"
    );
}

#[test]
fn sync_mirror_aliases_preserves_unknown_frontmatter_and_bytes() {
    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(
        dir.path().join("target0001.md"),
        "---\nid: target0001\ntitle: Old title\nstate: open\n---\n",
    )
    .expect("write target");
    fs::write(
        dir.path().join("source0001.md"),
        "---\nid: source0001\ntitle: Source\nstate: open\ncustom: keep-me\n---\n\
         > quote [[target0001.md|Old title]] *em* `code`\n",
    )
    .expect("write source");

    let mut vault = Vault::open(dir.path()).expect("open");
    let old = titles(&vault);

    fs::write(
        dir.path().join("target0001.md"),
        "---\nid: target0001\ntitle: New title\nstate: open\n---\n",
    )
    .expect("external rename");
    vault.reload();

    let written = vault.sync_mirror_aliases(&old).expect("sync");
    assert_eq!(written, 1);

    let contents = fs::read_to_string(dir.path().join("source0001.md")).expect("read");
    assert!(contents.contains("custom: keep-me"), "{contents}");
    assert!(
        contents.contains("> quote [[target0001.md|New title]] *em* `code`"),
        "non-alias bytes survive: {contents}"
    );
}

#[test]
fn cold_open_and_reload_never_write() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = "---\nid: source0001\ntitle: Source\nstate: open\n---\n\
                  see [[target0001.md|Old title]]\n";
    fs::write(
        dir.path().join("target0001.md"),
        "---\nid: target0001\ntitle: New title\nstate: open\n---\n",
    )
    .expect("write target");
    fs::write(dir.path().join("source0001.md"), source).expect("write source");

    let mut vault = Vault::open(dir.path()).expect("open");
    assert_eq!(
        fs::read_to_string(dir.path().join("source0001.md")).expect("read"),
        source,
        "a cold open has no prior index and never rewrites"
    );

    vault.reload();
    assert_eq!(
        fs::read_to_string(dir.path().join("source0001.md")).expect("read"),
        source,
        "a plain reload never rewrites either"
    );
}
