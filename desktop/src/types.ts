// Wire types mirroring the Rust DTOs in `desktop/src-tauri/src/service.rs`.

export type TaskState = "open" | "done" | "cancelled";
export type Priority = "high" | "med" | "low";

export interface Project {
  slug: string;
  path: string;
  name: string;
}

export interface TaskNode {
  id: string;
  title: string;
  state: TaskState;
  due?: string;
  priority?: Priority;
  tags: string[];
  parentId?: string;
  hasChildren: boolean;
  done: number;
  total: number;
  children: TaskNode[];
}

export interface TaskRef {
  id: string;
  title?: string;
}

export interface Issue {
  path: string;
  kind: string;
  detail: string;
}

export interface Snapshot {
  projects: Project[];
  project?: Project;
  issues: Issue[];
  tree: TaskNode[];
  taskCount: number;
  captureTarget?: TaskRef;
  revision: string;
}

export interface TaskDetail {
  id: string;
  title: string;
  state: TaskState;
  due?: string;
  priority?: Priority;
  tags: string[];
  parentId?: string;
  projectSlug: string;
  projectPath: string;
  filePath: string;
  revision: string;
  body: string;
  links: TaskRef[];
  backlinks: TaskRef[];
  children: TaskRef[];
  done: number;
  total: number;
}

export interface ConflictInfo {
  reason: "changed" | "deleted";
  currentRevision?: string;
  currentBody?: string;
}

/** Serialized backend error, as produced by `ServiceError`. */
export interface ServiceErrorShape {
  code: string;
  message: string;
  conflict?: ConflictInfo;
}
