//! Derived, disposable index over the vault: the parent/child forest,
//! outgoing `[[id]]` links, and computed backlinks.
//!
//! The index is rebuilt from the loaded task set on every reload or mutation;
//! it is never stored and never trusted over the files themselves. Broken or
//! cyclic parent edges are dropped here (and reported), which keeps every
//! traversal finite while leaving the task files untouched.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;

use crate::model::{Priority, Task, TaskId, TaskState};
use crate::vault::{VaultIssue, VaultIssueKind};

/// A task together with its subtree, as returned by [`crate::Vault::tree`].
#[derive(Debug, Clone, PartialEq)]
pub struct TreeNode<'a> {
    /// The task at this node.
    pub task: &'a Task,
    /// Child nodes in display order.
    pub children: Vec<TreeNode<'a>>,
}

/// Optional filters for [`crate::Vault::filter`]. Every set field must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFilter {
    /// Tag to match, with or without a leading `#`. Matching a tag also
    /// matches its nested descendants (`work` matches `work/admin`), but not
    /// its ancestors (`work/admin` does not match `work`).
    pub tag: Option<String>,
    /// Exact lifecycle state to match.
    pub state: Option<TaskState>,
    /// Exact priority to match. Tasks without priority do not match.
    pub priority: Option<Priority>,
    /// Keep only tasks due on or before the `today` passed to
    /// [`crate::Vault::filter`] (overdue included).
    pub due_today: bool,
}

/// Derived graph structures for the vault.
#[derive(Debug, Default)]
pub(crate) struct Index {
    children: BTreeMap<TaskId, Vec<TaskId>>,
    roots: Vec<TaskId>,
    parents: BTreeMap<TaskId, TaskId>,
    links: BTreeMap<TaskId, Vec<TaskId>>,
    backlinks: BTreeMap<TaskId, Vec<TaskId>>,
}

impl Index {
    /// Build the index from loaded tasks.
    ///
    /// Dangling parent targets and parent cycles are dropped from the
    /// effective forest and reported as [`VaultIssue`]s; the tasks themselves
    /// still load as roots. Ranked siblings and roots come first by ascending
    /// rank; unranked tasks follow by lowercased title, then id.
    pub(crate) fn build(tasks: &BTreeMap<TaskId, Task>, root: &Path) -> (Self, Vec<VaultIssue>) {
        let mut issues = Vec::new();
        let mut parents: BTreeMap<TaskId, TaskId> = BTreeMap::new();

        for (id, task) in tasks {
            let Some(parent) = task.parent.as_ref() else {
                continue;
            };
            if parent == id {
                issues.push(task_issue(
                    root,
                    id,
                    VaultIssueKind::Cycle,
                    format!(
                        "parent cycle {id} → {id}; this edge removed and the task treated as root"
                    ),
                ));
                continue;
            }
            if tasks.contains_key(parent) {
                parents.insert(id.clone(), parent.clone());
            } else {
                issues.push(task_issue(
                    root,
                    id,
                    VaultIssueKind::DanglingParent,
                    format!("parent {parent} not found; task {id} treated as root"),
                ));
            }
        }

        break_cycles(tasks, &mut parents, root, &mut issues);

        let mut children: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
        let mut roots: Vec<TaskId> = Vec::new();
        for id in tasks.keys() {
            match parents.get(id) {
                Some(parent) => children.entry(parent.clone()).or_default().push(id.clone()),
                None => roots.push(id.clone()),
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| display_order(tasks, a, b));
        }
        roots.sort_by(|a, b| display_order(tasks, a, b));

        let mut links: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
        let mut backlinks: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
        for (id, task) in tasks {
            let outgoing = extract_links(&task.body);
            if outgoing.is_empty() {
                continue;
            }
            for target in &outgoing {
                backlinks
                    .entry(target.clone())
                    .or_default()
                    .push(id.clone());
            }
            links.insert(id.clone(), outgoing);
        }

        (
            Self {
                children,
                roots,
                parents,
                links,
                backlinks,
            },
            issues,
        )
    }

    /// Root ids in display order.
    pub(crate) fn roots(&self) -> &[TaskId] {
        &self.roots
    }

    /// Child ids of `id` in display order.
    pub(crate) fn children(&self, id: &TaskId) -> &[TaskId] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }

    /// Effective parent of `id` after broken edges were dropped.
    pub(crate) fn parent(&self, id: &TaskId) -> Option<&TaskId> {
        self.parents.get(id)
    }

    /// Outgoing links of `id` in body order.
    pub(crate) fn links(&self, id: &TaskId) -> &[TaskId] {
        self.links.get(id).map_or(&[], Vec::as_slice)
    }

    /// Backlinks of `id` in source-id order.
    pub(crate) fn backlinks(&self, id: &TaskId) -> &[TaskId] {
        self.backlinks.get(id).map_or(&[], Vec::as_slice)
    }
}

/// Drop one parent edge per detected cycle, reporting each dropped edge.
fn break_cycles(
    tasks: &BTreeMap<TaskId, Task>,
    parents: &mut BTreeMap<TaskId, TaskId>,
    root: &Path,
    issues: &mut Vec<VaultIssue>,
) {
    let mut safe: BTreeSet<TaskId> = BTreeSet::new();
    for start in tasks.keys() {
        if safe.contains(start) {
            continue;
        }
        let mut on_walk: BTreeSet<TaskId> = BTreeSet::new();
        let mut walk: Vec<TaskId> = Vec::new();
        let mut cursor = start.clone();
        loop {
            if safe.contains(&cursor) {
                break;
            }
            if !on_walk.insert(cursor.clone()) {
                parents.remove(&cursor);
                let start_index = walk.iter().position(|id| id == &cursor).unwrap_or(0);
                let loop_text = walk[start_index..]
                    .iter()
                    .chain(std::iter::once(&cursor))
                    .map(TaskId::as_str)
                    .collect::<Vec<_>>()
                    .join(" → ");
                issues.push(task_issue(
                    root,
                    &cursor,
                    VaultIssueKind::Cycle,
                    format!(
                        "parent cycle {loop_text}; this edge removed and the task treated as root"
                    ),
                ));
                break;
            }
            walk.push(cursor.clone());
            match parents.get(&cursor) {
                Some(parent) => cursor = parent.clone(),
                None => break,
            }
        }
        safe.extend(walk);
    }
}

/// Display order for sibling tasks: ranked first by ascending rank, then
/// unranked by lowercased title, with title/id breaking ties deterministically.
pub(crate) fn display_order(tasks: &BTreeMap<TaskId, Task>, a: &TaskId, b: &TaskId) -> Ordering {
    let key = |id: &TaskId| {
        tasks.get(id).map_or_else(
            || (None, String::new()),
            |task| (task.rank, task.title.to_lowercase()),
        )
    };
    let (a_rank, a_title) = key(a);
    let (b_rank, b_title) = key(b);
    match (a_rank, b_rank) {
        (Some(a_rank), Some(b_rank)) => a_rank.cmp(&b_rank),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
    .then_with(|| a_title.cmp(&b_title))
    .then_with(|| a.cmp(b))
}

fn task_issue(
    root: &Path,
    id: &TaskId,
    kind: VaultIssueKind,
    detail: impl Into<String>,
) -> VaultIssue {
    VaultIssue::new(root.join(format!("{id}.md")), kind, detail)
}

/// A non-code `[[...]]` wikilink occurrence in a task body.
///
/// The index keeps only [`WikiLink::target`], while title sync rewrites
/// exactly [`WikiLink::alias_span`] in place, so both agree on what counts as
/// a link and on where the alias lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WikiLink {
    /// Normalized target id: the filename stem, with any `.md` suffix and
    /// directory components removed.
    pub(crate) target: TaskId,
    /// Alias exactly as written between `|` and `]]`; `None` for a bare
    /// link. An empty alias is `Some("")`, never `None`.
    pub(crate) alias: Option<String>,
    /// Byte range of the alias text in the body, present whenever `alias`
    /// is; an empty alias has an empty (but positioned) range.
    pub(crate) alias_span: Option<Range<usize>>,
}

impl WikiLink {
    /// Parse the `[[...]]` whose closing `]]` starts at `close` (the opening
    /// `[[` is at `open`), resolving the target to its stem.
    fn parse(body: &str, open: usize, close: usize) -> Option<Self> {
        let inner_start = open + 2;
        let inner = &body[inner_start..close];
        let (target_text, alias, alias_span) = match inner.split_once('|') {
            Some((target, alias)) => {
                let alias_start = inner_start + target.len() + 1;
                (target, Some(alias.to_owned()), Some(alias_start..close))
            }
            None => (inner, None, None),
        };
        let target = TaskId::parse_link_target(target_text)?;
        Some(Self {
            target,
            alias,
            alias_span,
        })
    }
}

/// Parse every non-code `[[...]]` occurrence in `body` with absolute byte
/// offsets, in body order.
///
/// Scanning is line-based, because a link cannot span lines and the first
/// `]]` closes it. Fenced code blocks (``` or ~~~ at the start of a line,
/// after indentation) and single-backtick inline code are skipped, so the
/// index and title sync agree on what is a link. An unclosed fence skips the
/// rest of the body; an unclosed backtick skips the rest of its line.
/// Indented code is not special, matching the renderer's fenced-only rule.
pub(crate) fn parse_wikilinks(body: &str) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let mut in_fence = false;
    let mut line_start = 0usize;

    for line in body.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let content_end = line_start + content.len();
        let trimmed = content.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            scan_line(body, line_start, content_end, &mut links);
        }
        line_start = line_end;
    }
    links
}

/// Scan one line of `body` (its content excluding the line ending) for
/// `[[...]]` occurrences, skipping inline code spans.
fn scan_line(body: &str, start: usize, end: usize, links: &mut Vec<WikiLink>) {
    let line = &body[start..end];
    let mut index = 0usize;
    while index < line.len() {
        let rest = &line[index..];
        if rest.as_bytes()[0] == b'`' {
            // Inline code never holds links: jump past the closing backtick
            // or, when there is none, to the end of the line.
            match rest[1..].find('`') {
                Some(close) => index += close + 2,
                None => return,
            }
            continue;
        }
        if rest.starts_with("[[") {
            let open = start + index;
            let after_open = open + 2;
            let Some(close) = body[after_open..end].find("]]") else {
                // An unclosed `[[` swallows the rest of the line.
                return;
            };
            let close = after_open + close;
            if let Some(link) = WikiLink::parse(body, open, close) {
                links.push(link);
            }
            // Even an invalid target consumes its `]]`, matching the text
            // scanner's left-to-right behavior.
            index = close + 2 - start;
            continue;
        }
        // Advance one whole char, never one byte: `&line[index..]` above
        // must stay on a UTF-8 boundary.
        index += rest.chars().next().map_or(1, char::len_utf8);
    }
}

/// Extract unique link targets from a markdown body, in order of first
/// appearance. Every accepted link form (`[[id]]`, `[[id|alias]]`,
/// `[[id.md]]`, `[[id.md|alias]]`, and `.md`-suffixed paths) resolves to the
/// target's id; aliases and spans stay internal.
pub(crate) fn extract_links(body: &str) -> Vec<TaskId> {
    let mut links: Vec<TaskId> = Vec::new();
    for link in parse_wikilinks(body) {
        if !links.contains(&link.target) {
            links.push(link.target);
        }
    }
    links
}

/// Rewrite mirror aliases in `body`, returning the new body when at least one
/// alias actually changes and `None` otherwise (including when `diffs` is
/// empty).
///
/// `diffs` maps a target whose title changed to its `(old, new)` titles. A
/// link's alias is a **mirror** only when it equals the target's old title
/// after trimming the alias; bare links and contextual aliases are never
/// touched. Only the alias text between `|` and `]]` changes: every other
/// byte, including unknown markdown, code, and line endings, survives
/// verbatim. Replacements are applied back-to-front so earlier spans stay
/// valid.
pub(crate) fn rewrite_mirror_aliases(
    body: &str,
    diffs: &BTreeMap<TaskId, (String, String)>,
) -> Option<String> {
    if diffs.is_empty() {
        return None;
    }

    let mut edits: Vec<(Range<usize>, &str)> = Vec::new();
    for link in parse_wikilinks(body) {
        let (Some(alias), Some(span)) = (link.alias.as_deref(), link.alias_span.clone()) else {
            continue;
        };
        let Some((old, new)) = diffs.get(&link.target) else {
            continue;
        };
        if alias.trim() == old.as_str() && alias != new.as_str() {
            edits.push((span, new.as_str()));
        }
    }
    if edits.is_empty() {
        return None;
    }

    let mut rewritten = body.to_owned();
    for (span, new) in edits.into_iter().rev() {
        rewritten.replace_range(span, new);
    }
    Some(rewritten)
}

/// Whether `task_tag` matches `query`, counting nested tags as matches.
pub(crate) fn tag_matches(task_tag: &str, query: &str) -> bool {
    task_tag == query
        || task_tag
            .strip_prefix(query)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wikilinks_does_not_panic_on_multibyte_utf8() {
        // Regression: the scanner advanced one byte at a time, so any
        // multi-byte char on a scanned line made `&line[index..]` land
        // inside the char and panic (e.g. a body mentioning "muñecos").
        let body = "se ve en un diagrama a firestartr en el medio, desde los laterales hay \
                    alguien ( muñecos? humanos simplificados? ) enviando órdenes ( \
                    gráficamente se tiene que ver que les envían ordenes) y el habla con \
                    github / gitlab para gráficamente ( de alguna manera) se ve como crea \
                    componentes, grupos... y al mismo tiempo habla con un icono de la cloud - \
                    (que se vea que es multi-cloud) [[id00000001]]\n\
                    ñ only\n";
        let links = parse_wikilinks(body);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.as_str(), "id00000001");
    }

    #[test]
    fn parse_wikilinks_keeps_spans_correct_around_multibyte_chars() {
        let body = "niño [[id00000001|título ñ]] fin";
        let links = parse_wikilinks(body);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].alias.as_deref(), Some("título ñ"));
        assert_eq!(
            &body[links[0].alias_span.clone().expect("alias span")],
            "título ñ",
            "byte spans stay exact with multi-byte chars before and inside the link"
        );
    }

    #[test]
    fn extract_links_dedupes_in_order() {
        let links = extract_links("a [[one0000001]] b [[two0000001]] c [[one0000001]]");

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].as_str(), "one0000001");
        assert_eq!(links[1].as_str(), "two0000001");
    }

    #[test]
    fn extract_links_ignores_fences_and_invalid_targets() {
        let body = "[[keep000001]]\n\
                    ```\n[[nope000001]]\n```\n\
                    ~~~\n[[nope000002]]\n~~~\n\
                    `[[inline0001]]`\n\
                    [[not a link]]\n[[UPPERCASE1]]\n";
        let links = extract_links(body);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "keep000001");
    }

    #[test]
    fn extract_links_dedupes_across_rich_forms() {
        let links = extract_links(
            "[[one0000001]] [[one0000001.md|First alias]] [[sub/one0000001.md|Second]] \
             [[two0000001.md]]",
        );

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].as_str(), "one0000001");
        assert_eq!(links[1].as_str(), "two0000001");
    }

    #[test]
    fn parse_wikilinks_keeps_aliases_and_absolute_spans() {
        let body = "first\nsecond [[id00000001.md|Alias text]] tail\r\nthird [[id00000002|]]\n";
        let links = parse_wikilinks(body);

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target.as_str(), "id00000001");
        assert_eq!(links[0].alias.as_deref(), Some("Alias text"));
        assert_eq!(
            &body[links[0].alias_span.clone().expect("alias span")],
            "Alias text"
        );
        assert_eq!(links[1].target.as_str(), "id00000002");
        assert_eq!(links[1].alias.as_deref(), Some(""));
        assert_eq!(
            &body[links[1].alias_span.clone().expect("alias span")],
            "",
            "an empty alias still has a positioned span"
        );
    }

    #[test]
    fn parse_wikilinks_uses_the_first_close_and_ignores_leftovers() {
        let links = parse_wikilinks("[[one0000001|a]] b]] and [[two0000001]]");

        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target.as_str(), "one0000001");
        assert_eq!(links[0].alias.as_deref(), Some("a"));
        assert_eq!(links[1].target.as_str(), "two0000001");
    }

    #[test]
    fn parse_wikilinks_stops_after_an_unclosed_fence() {
        let body = "[[before0001]]\n```\n[[inside0001]]\n";
        let links = parse_wikilinks(body);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.as_str(), "before0001");
    }

    #[test]
    fn parse_wikilinks_skips_only_the_line_of_an_unclosed_backtick() {
        let body = "ok `then [[inside0001]]\n[[after00001]]\n";
        let links = parse_wikilinks(body);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.as_str(), "after00001");
    }

    #[test]
    fn extract_links_rejects_double_md_suffix() {
        let links = extract_links("[[abc1234567.md.md]] [[abc1234567.md]]");

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "abc1234567");
    }

    fn link_diffs(entries: &[(&str, &str, &str)]) -> BTreeMap<TaskId, (String, String)> {
        entries
            .iter()
            .map(|(id, old, new)| {
                (
                    TaskId::parse(id).expect("valid id"),
                    ((*old).to_owned(), (*new).to_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn rewrite_mirror_aliases_updates_only_matching_spans_on_one_line() {
        let body = "one [[id00000001.md|Old]] two [[id00000001.md|Context]] and [[id00000001.md]]";
        let rewritten = rewrite_mirror_aliases(body, &link_diffs(&[("id00000001", "Old", "New")]))
            .expect("the mirror changes");

        assert_eq!(
            rewritten,
            "one [[id00000001.md|New]] two [[id00000001.md|Context]] and [[id00000001.md]]",
            "only the alias equal to the old title moves"
        );
    }

    #[test]
    fn rewrite_mirror_aliases_applies_multi_id_diffs_without_transitive_chasing() {
        let body = "[[one0000001|A]] [[two0000001|B]]";
        let rewritten = rewrite_mirror_aliases(
            body,
            &link_diffs(&[("one0000001", "A", "B"), ("two0000001", "B", "C")]),
        )
        .expect("both mirrors change");

        assert_eq!(
            rewritten, "[[one0000001|B]] [[two0000001|C]]",
            "each diff applies against its own old title exactly once"
        );
    }

    #[test]
    fn rewrite_mirror_aliases_leaves_code_literal() {
        let body = "`[[id00000001|Old]]`\n\n```\n[[id00000001|Old]]\n```\n\n[[id00000001|Old]]";
        let rewritten = rewrite_mirror_aliases(body, &link_diffs(&[("id00000001", "Old", "New")]))
            .expect("the real link changes");

        assert_eq!(
            rewritten, "`[[id00000001|Old]]`\n\n```\n[[id00000001|Old]]\n```\n\n[[id00000001|New]]",
            "inline and fenced code are not links"
        );
    }

    #[test]
    fn rewrite_mirror_aliases_ignores_leftovers_empty_and_bare_links() {
        let body = "[[id00000001|Old]] b]] [[id00000001|]] [[id00000001]]";
        let rewritten = rewrite_mirror_aliases(body, &link_diffs(&[("id00000001", "Old", "New")]))
            .expect("the first link changes");

        assert_eq!(
            rewritten,
            "[[id00000001|New]] b]] [[id00000001|]] [[id00000001]]"
        );
    }

    #[test]
    fn rewrite_mirror_aliases_preserves_surrounding_bytes_and_crlf() {
        let body = "> quote [[id00000001|Old]] *em* <b>html</b> `code` tail\r\nnext\r\n";
        let rewritten = rewrite_mirror_aliases(body, &link_diffs(&[("id00000001", "Old", "New")]))
            .expect("the mirror changes");

        assert_eq!(
            rewritten, "> quote [[id00000001|New]] *em* <b>html</b> `code` tail\r\nnext\r\n",
            "line endings and unknown markdown survive verbatim"
        );
    }

    #[test]
    fn rewrite_mirror_aliases_cleans_padding_around_a_mirror() {
        let body = "[[id00000001|  Old  ]]";
        let rewritten = rewrite_mirror_aliases(body, &link_diffs(&[("id00000001", "Old", "New")]))
            .expect("a padded mirror matches");

        assert_eq!(rewritten, "[[id00000001|New]]");
    }

    #[test]
    fn rewrite_mirror_aliases_is_none_when_nothing_actually_changes() {
        let diffs = link_diffs(&[("id00000001", "Old", "New")]);

        assert!(rewrite_mirror_aliases("[[id00000001|New]]", &diffs).is_none());
        assert!(rewrite_mirror_aliases("[[id00000001|Context]]", &diffs).is_none());
        assert!(rewrite_mirror_aliases("plain body", &diffs).is_none());
        assert!(rewrite_mirror_aliases("[[id00000001|Old]]", &BTreeMap::new()).is_none());

        let noop = link_diffs(&[("id00000001", "Same", "Same")]);
        assert!(rewrite_mirror_aliases("[[id00000001|Same]]", &noop).is_none());
    }

    #[test]
    fn tag_matching_respects_hierarchy_boundaries() {
        assert!(tag_matches("work", "work"));
        assert!(tag_matches("work/admin", "work"));
        assert!(tag_matches("work/admin/x", "work"));
        assert!(tag_matches("work/admin/x", "work/admin"));
        assert!(!tag_matches("work", "work/admin"));
        assert!(!tag_matches("workshop", "work"));
    }
}
