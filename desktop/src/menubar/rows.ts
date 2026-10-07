// Row projection and attention rules for the menu bar popover.
//
// The popover shows open Tasks by default; "Show done" reveals the finished
// ones. Either way the tree stays a tree: an ancestor of a kept Task is kept
// too, and folding still hides its children.

import { ancestorsOf, flattenTree, type FlatRow } from "../lib/tree";
import type { TaskNode } from "../types";

/**
 * Keep a node when it is open, when finished Tasks are shown, when it is the
 * `held` Task, or when a descendant is kept. `hasChildren` is recomputed so the disclosure control
 * only appears when there is something to disclose.
 */
function prune(nodes: TaskNode[], showDone: boolean, held?: string): TaskNode[] {
  const kept: TaskNode[] = [];
  for (const node of nodes) {
    const children = prune(node.children, showDone, held);
    if (showDone || node.state === "open" || node.id === held || children.length > 0) {
      kept.push({ ...node, children, hasChildren: children.length > 0 });
    }
  }
  return kept;
}

/**
 * Pre-order rows for the popover, honoring the folds. The `held` Task (the
 * one with an open draft) stays visible with its ancestors even when it is
 * filtered out or folded away, so its editor never becomes unreachable.
 */
export function menubarRows(
  tree: TaskNode[],
  showDone: boolean,
  collapsed: ReadonlySet<string>,
  held?: string,
): FlatRow[] {
  const folds = new Set(collapsed);
  if (held) for (const id of ancestorsOf(tree, held)) folds.delete(id);
  return flattenTree(prune(tree, showDone, held), folds);
}

/**
 * Whether any open Task in the forest is due today or earlier, which is what
 * lights the tray icon's dot. Done and cancelled Tasks never do.
 */
export function hasDueAttention(tree: TaskNode[], today: string): boolean {
  for (const node of tree) {
    if (node.state === "open" && node.due !== undefined && node.due <= today) return true;
    if (hasDueAttention(node.children, today)) return true;
  }
  return false;
}

/** The fields the popover renders, so a changed row can be noticed cheaply. */
function signature(node: TaskNode): string {
  return [
    node.title,
    node.state,
    node.due ?? "",
    node.priority ?? "",
    node.parentId ?? "",
    node.tags.join(","),
  ].join("\u0000");
}

/**
 * Ids whose rendered fields differ between two forests, ignoring Tasks that
 * only appeared or disappeared: the popover highlights content that changed
 * under an external write, not the completion it animated itself.
 */
export function changedIds(before: TaskNode[], after: TaskNode[]): Set<string> {
  const previous = new Map<string, string>();
  const collect = (nodes: TaskNode[]): void => {
    for (const node of nodes) {
      previous.set(node.id, signature(node));
      collect(node.children);
    }
  };
  collect(before);

  const changed = new Set<string>();
  const compare = (nodes: TaskNode[]): void => {
    for (const node of nodes) {
      const seen = previous.get(node.id);
      if (seen !== undefined && seen !== signature(node)) changed.add(node.id);
      compare(node.children);
    }
  };
  compare(after);
  return changed;
}
