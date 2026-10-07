// Tree helpers: flattening with folds, ancestor lookup, and search context.
//
// Everything here consumes the backend's core-ordered tree verbatim; no new
// ordering is introduced.

import type { Project, TaskNode } from "../types";

export interface FlatRow {
  node: TaskNode;
  depth: number;
}

/** Stable row keys for the sidebar's single tree, also used as DOM ids. */
export const projectRowKey = (slug: string): string => `project:${slug}`;
export const taskRowKey = (id: string): string => `task:${id}`;

/** One row of the sidebar tree: a Project row or a Task row nested under it. */
export type NavRow =
  | { kind: "project"; key: string; depth: number; expanded: boolean; project: Project }
  | { kind: "task"; key: string; depth: number; expanded: boolean; node: TaskNode };

/** Pre-order rows, hiding the children of collapsed nodes. */
export function flattenTree(tree: TaskNode[], collapsed: ReadonlySet<string>): FlatRow[] {
  const rows: FlatRow[] = [];
  const walk = (nodes: TaskNode[], depth: number): void => {
    for (const node of nodes) {
      rows.push({ node, depth });
      if (node.hasChildren && !collapsed.has(node.id)) walk(node.children, depth + 1);
    }
  };
  walk(tree, 0);
  return rows;
}

/** Every task by id. */
export function collectTasks(tree: TaskNode[]): Map<string, TaskNode> {
  const map = new Map<string, TaskNode>();
  const walk = (nodes: TaskNode[]): void => {
    for (const node of nodes) {
      map.set(node.id, node);
      walk(node.children);
    }
  };
  walk(tree);
  return map;
}

/** Ids of every ancestor of `id`, nearest first. */
export function ancestorsOf(tree: TaskNode[], id: string): string[] {
  const parents = new Map<string, string>();
  const walk = (nodes: TaskNode[], parent?: string): void => {
    for (const node of nodes) {
      if (parent !== undefined) parents.set(node.id, parent);
      walk(node.children, node.id);
    }
  };
  walk(tree);
  const ancestors: string[] = [];
  let cursor = parents.get(id);
  while (cursor !== undefined) {
    ancestors.push(cursor);
    cursor = parents.get(cursor);
  }
  return ancestors;
}

/** Search results plus the ancestors needed to understand them. */
export function searchContext(tree: TaskNode[], hits: string[]): Set<string> {
  const visible = new Set<string>();
  for (const hit of hits) {
    visible.add(hit);
    for (const ancestor of ancestorsOf(tree, hit)) visible.add(ancestor);
  }
  return visible;
}

/**
 * The sidebar's single tree: Projects in registry order, with the open
 * Project's Task rows (already flattened) nested one level under its row. The
 * service holds one open Project, so only it is ever expanded; every other
 * Project is an accordion row that opens it when activated.
 */
export function sidebarRows(
  projects: Project[],
  openSlug: string | null,
  openExpanded: boolean,
  tasks: FlatRow[],
  collapsedTasks: ReadonlySet<string>,
): NavRow[] {
  const rows: NavRow[] = [];
  for (const project of projects) {
    const expanded = project.slug === openSlug && openExpanded;
    rows.push({
      kind: "project",
      key: projectRowKey(project.slug),
      depth: 0,
      expanded,
      project,
    });
    if (!expanded) continue;
    for (const { node, depth } of tasks) {
      rows.push({
        kind: "task",
        key: taskRowKey(node.id),
        depth: depth + 1,
        expanded: node.hasChildren && !collapsedTasks.has(node.id),
        node,
      });
    }
  }
  return rows;
}

/** Pre-order rows limited to `visible`, folds ignored (search never hides a hit). */
export function flattenVisible(tree: TaskNode[], visible: ReadonlySet<string>): FlatRow[] {
  const rows: FlatRow[] = [];
  const walk = (nodes: TaskNode[], depth: number): void => {
    for (const node of nodes) {
      if (visible.has(node.id)) rows.push({ node, depth });
      walk(node.children, node.hasChildren ? depth + 1 : depth);
    }
  };
  walk(tree, 0);
  return rows;
}
