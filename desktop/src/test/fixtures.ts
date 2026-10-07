import type { Snapshot, TaskDetail, TaskNode } from "../types";

export function node(id: string, title: string, extra: Partial<TaskNode> = {}): TaskNode {
  return {
    id,
    title,
    state: "open",
    tags: [],
    hasChildren: false,
    done: 0,
    total: 0,
    children: [],
    ...extra,
  };
}

export function makeSnapshot(overrides: Partial<Snapshot> = {}): Snapshot {
  const project = { slug: "demo", path: "/tmp/demo", name: "demo" };
  return {
    projects: [project],
    project,
    issues: [],
    tree: [],
    taskCount: 0,
    revision: "rev1",
    ...overrides,
  };
}

export function makeDetail(id: string, overrides: Partial<TaskDetail> = {}): TaskDetail {
  return {
    id,
    title: id,
    state: "open",
    tags: [],
    projectSlug: "demo",
    projectPath: "/tmp/demo",
    filePath: `/tmp/store/${id}.md`,
    revision: "rev1",
    body: "",
    links: [],
    backlinks: [],
    children: [],
    done: 0,
    total: 0,
    ...overrides,
  };
}

export function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}
