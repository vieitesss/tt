//! Flattened, pre-order projection of the vault tree for the list view.
//!
//! The list is derived fresh from the vault on every reload; nothing is
//! stored. Each row carries enough tree context to draw its guides, and the
//! selection is an index into the flat row order, so movement is purely
//! linear. Collapsed parents hide their whole subtree; the fold state is held
//! by the caller and never persisted.

use std::collections::HashSet;

use tt::{TaskId, TreeNode, Vault};

/// One row of the flattened task list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListRow {
    /// Task this row points at.
    pub(crate) id: TaskId,
    /// Depth from the nearest root (0 for roots).
    pub(crate) depth: usize,
    /// For every ancestor column except the row's own, whether the line
    /// continues (`│ `) or stays blank (`  `).
    pub(crate) ancestors_continue: Vec<bool>,
    /// Whether the row is the last child of its parent (or the last root).
    pub(crate) is_last: bool,
    /// Whether the task has children in the vault; parents draw a fold
    /// marker.
    pub(crate) has_children: bool,
    /// Whether the row is a parent whose children are currently hidden.
    pub(crate) folded: bool,
}

impl ListRow {
    /// Tree guides drawn before the state glyph.
    pub(crate) fn guides(&self) -> String {
        if self.depth == 0 {
            return String::new();
        }
        let mut guides = String::new();
        for continues in &self.ancestors_continue {
            guides.push_str(if *continues { "│ " } else { "  " });
        }
        guides.push_str(if self.is_last { "└ " } else { "├ " });
        guides
    }
}

/// Every task in pre-order, with its position in the tree.
#[derive(Debug, Default)]
pub(crate) struct TaskList {
    rows: Vec<ListRow>,
}

impl TaskList {
    /// Flatten the vault tree in pre-order: parents above their children,
    /// siblings in vault display order. Children of a collapsed task are
    /// skipped along with their whole subtree.
    pub(crate) fn build(vault: &Vault, collapsed: &HashSet<TaskId>) -> Self {
        let tree = vault.tree();
        let mut rows = Vec::new();
        let root_count = tree.len();
        for (index, node) in tree.iter().enumerate() {
            push_node(node, 0, &[], index + 1 == root_count, collapsed, &mut rows);
        }
        Self { rows }
    }

    /// Rows in pre-order.
    pub(crate) fn rows(&self) -> &[ListRow] {
        &self.rows
    }

    /// Number of rows.
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the list has no rows.
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Position of `id` in the flat order.
    pub(crate) fn index_of(&self, id: &TaskId) -> Option<usize> {
        self.rows.iter().position(|row| &row.id == id)
    }

    /// Id at `index`.
    pub(crate) fn id_at(&self, index: usize) -> Option<&TaskId> {
        self.rows.get(index).map(|row| &row.id)
    }
}

/// Append `node` and its subtree, carrying the guide context down. A
/// collapsed node is emitted with `folded` set and its children skipped.
fn push_node(
    node: &TreeNode<'_>,
    depth: usize,
    ancestors: &[bool],
    is_last: bool,
    collapsed: &HashSet<TaskId>,
    rows: &mut Vec<ListRow>,
) {
    let has_children = !node.children.is_empty();
    let folded = has_children && collapsed.contains(&node.task.id);
    rows.push(ListRow {
        id: node.task.id.clone(),
        depth,
        ancestors_continue: ancestors.to_vec(),
        is_last,
        has_children,
        folded,
    });
    if folded {
        return;
    }

    // The child's guide columns are the parent's, plus the parent's own
    // continuation flag. Roots contribute no column (their children start at
    // depth 1 with only their own connector).
    let mut child_ancestors = if depth == 0 {
        Vec::new()
    } else {
        ancestors.to_vec()
    };
    if depth > 0 {
        child_ancestors.push(!is_last);
    }

    let child_count = node.children.len();
    for (index, child) in node.children.iter().enumerate() {
        push_node(
            child,
            depth + 1,
            &child_ancestors,
            index + 1 == child_count,
            collapsed,
            rows,
        );
    }
}

#[cfg(test)]
mod tests {
    use tt::NewTask;

    use super::*;

    /// A vault with a fixed tree, plus its ids in insertion order:
    /// `root > [first > [grandchild], last]` and `second_root`.
    fn tree_vault() -> (tempfile::TempDir, Vault, Vec<TaskId>) {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut vault = Vault::open(dir.path()).expect("open vault");
        let root = vault.add(NewTask::new("Root")).expect("add").id;
        let first = vault
            .add(NewTask {
                parent: Some(root.clone()),
                ..NewTask::new("First child")
            })
            .expect("add")
            .id;
        let grandchild = vault
            .add(NewTask {
                parent: Some(first.clone()),
                ..NewTask::new("Grandchild")
            })
            .expect("add")
            .id;
        let last = vault
            .add(NewTask {
                parent: Some(root.clone()),
                ..NewTask::new("Last child")
            })
            .expect("add")
            .id;
        let second_root = vault.add(NewTask::new("Second root")).expect("add").id;
        (dir, vault, vec![root, first, grandchild, last, second_root])
    }

    fn expanded() -> TaskList {
        let (_dir, vault, _ids) = tree_vault();
        TaskList::build(&vault, &HashSet::new())
    }

    #[test]
    fn rows_are_pre_order_with_depths() {
        let (_dir, vault, ids) = tree_vault();
        let list = TaskList::build(&vault, &HashSet::new());
        assert_eq!(list.len(), 5);
        for (row, id) in list.rows().iter().zip(&ids) {
            assert_eq!(&row.id, id, "pre-order matches insertion order");
        }
        assert_eq!(list.rows()[0].depth, 0, "root first");
        assert_eq!(list.rows()[1].depth, 1, "child next");
        assert_eq!(list.rows()[2].depth, 2, "grandchild");
        assert_eq!(list.rows()[3].depth, 1, "second child");
        assert_eq!(list.rows()[4].depth, 0, "second root");
    }

    #[test]
    fn guides_continue_through_non_last_ancestors() {
        let list = expanded();
        assert_eq!(list.rows()[0].guides(), "", "roots have no guides");
        assert_eq!(list.rows()[1].guides(), "├ ", "first child continues");
        assert_eq!(
            list.rows()[2].guides(),
            "│ └ ",
            "grandchild keeps the parent's line"
        );
        assert_eq!(list.rows()[3].guides(), "└ ", "last child closes");
        assert_eq!(list.rows()[4].guides(), "", "second root has no guides");
    }

    #[test]
    fn lookup_and_bounds() {
        let list = expanded();
        assert_eq!(list.len(), 5);
        assert!(!list.is_empty());
        let id = list.rows()[2].id.clone();
        assert_eq!(list.index_of(&id), Some(2));
        assert_eq!(list.id_at(2), Some(&id));
        assert_eq!(list.id_at(5), None);
    }

    #[test]
    fn collapsed_parent_hides_its_subtree_and_is_marked() {
        let (_dir, vault, ids) = tree_vault();
        let mut collapsed = HashSet::new();
        collapsed.insert(ids[1].clone()); // "First child" owns the grandchild
        let list = TaskList::build(&vault, &collapsed);

        assert_eq!(list.len(), 4, "the grandchild row is gone");
        assert!(list.index_of(&ids[2]).is_none(), "grandchild hidden");
        assert_eq!(list.index_of(&ids[1]), Some(1), "parent stays visible");
        assert!(list.rows()[1].has_children);
        assert!(list.rows()[1].folded, "the parent is marked as folded");
        assert_eq!(list.rows()[2].id, ids[3], "the parent's sibling follows");
        assert_eq!(list.rows()[3].id, ids[4], "the second root follows");

        // Collapsing a root hides its whole subtree.
        collapsed.insert(ids[0].clone());
        let list = TaskList::build(&vault, &collapsed);
        assert_eq!(list.len(), 2, "root and second root");
        assert!(list.index_of(&ids[1]).is_none());
        assert!(list.index_of(&ids[2]).is_none());
        assert!(list.index_of(&ids[3]).is_none());
        assert!(list.rows()[0].folded);
        assert!(!list.rows()[1].folded, "a leaf is never folded");
        assert!(!list.rows()[1].has_children);
    }

    #[test]
    fn guides_are_unchanged_by_folds() {
        let (_dir, vault, ids) = tree_vault();
        let mut collapsed = HashSet::new();
        collapsed.insert(ids[1].clone());
        let folded = TaskList::build(&vault, &collapsed);
        let expanded = TaskList::build(&vault, &HashSet::new());
        // The parent row keeps the guide string it had while expanded.
        assert_eq!(folded.rows()[1].guides(), expanded.rows()[1].guides());
    }
}
