// The single frontend seam over the Tauri backend.
//
// Components never call `invoke` directly: they receive a `TtClient`, so tests
// can drive the whole UI from a fake without a webview.

import { invoke } from "@tauri-apps/api/core";
import type {
  Priority,
  ServiceErrorShape,
  Snapshot,
  TaskDetail,
  TaskState,
} from "./types";

export interface SearchFilters {
  query: string;
  state?: TaskState;
  tag?: string;
  priority?: Priority;
  dueToday?: boolean;
}

export interface AddTaskInput {
  title: string;
  parentId?: string;
  capture?: boolean;
}

export interface TtClient {
  bootstrap(): Promise<Snapshot>;
  openProject(slug: string): Promise<Snapshot>;
  refresh(): Promise<Snapshot>;
  registerProject(): Promise<Snapshot | null>;
  unregisterProject(slug: string): Promise<Snapshot>;
  taskDetail(id: string): Promise<TaskDetail>;
  saveBody(id: string, body: string, baseRevision: string): Promise<TaskDetail>;
  addTask(input: AddTaskInput): Promise<Snapshot>;
  setState(id: string, state: TaskState): Promise<Snapshot>;
  setTitle(id: string, title: string): Promise<Snapshot>;
  setTags(id: string, tags: string[]): Promise<Snapshot>;
  setPriority(id: string, priority?: Priority): Promise<Snapshot>;
  setDue(id: string, due?: string): Promise<Snapshot>;
  setParent(id: string, parentId?: string): Promise<Snapshot>;
  shiftRank(id: string, delta: number): Promise<Snapshot>;
  moveToProject(id: string, targetSlug: string, parentId?: string): Promise<Snapshot>;
  deleteCount(id: string): Promise<number>;
  deleteTask(id: string): Promise<Snapshot>;
  search(filters: SearchFilters): Promise<string[]>;
  copyTask(id: string, includeBody: boolean): Promise<string>;
  copyText(text: string): Promise<void>;
  openExternal(url: string): Promise<void>;
  closeMainWindow(): Promise<void>;
  /** Hide the menu bar popover (Escape, or after handing a task to the main window). */
  hideMenubar(): Promise<void>;
  /** Show or clear the tray icon's attention dot. */
  setTrayAttention(attention: boolean): Promise<void>;
  /** Show the main window with a Project and Task selected. */
  openInDesktop(projectSlug: string, taskId: string): Promise<void>;
  /** Hide the popover and show the main window. */
  openDesktop(): Promise<void>;
  /** Quit the whole app. */
  quitApp(): Promise<void>;
}

/** An error carrying the backend's machine-readable `code` and conflict data. */
export class TtError extends Error {
  readonly code: string;
  readonly conflict?: ServiceErrorShape["conflict"];

  constructor(shape: ServiceErrorShape) {
    super(shape.message);
    this.name = "TtError";
    this.code = shape.code;
    this.conflict = shape.conflict;
  }
}

/** Normalize anything `invoke` rejects with into a `TtError`. */
export function toTtError(error: unknown): TtError {
  if (error instanceof TtError) return error;
  if (typeof error === "object" && error !== null && "code" in error && "message" in error) {
    const shape = error as ServiceErrorShape;
    return new TtError({ code: shape.code, message: shape.message, conflict: shape.conflict });
  }
  return new TtError({ code: "unknown", message: String(error) });
}

/**
 * Every native rejection crosses this boundary, so a serialized `ServiceError`
 * object becomes a `TtError` and callers never see `[object Object]`.
 */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toTtError(error);
  }
}

export const tauriClient: TtClient = {
  bootstrap: () => call<Snapshot>("bootstrap"),
  openProject: (slug) => call<Snapshot>("open_project", { slug }),
  refresh: () => call<Snapshot>("refresh"),
  registerProject: () => call<Snapshot | null>("register_project"),
  unregisterProject: (slug) => call<Snapshot>("unregister_project", { slug }),
  taskDetail: (id) => call<TaskDetail>("task_detail", { id }),
  saveBody: (id, body, baseRevision) =>
    call<TaskDetail>("save_body", { id, body, baseRevision }),
  addTask: (input) =>
    call<Snapshot>("add_task", {
      title: input.title,
      parentId: input.parentId ?? null,
      capture: input.capture ?? false,
    }),
  setState: (id, state) => call<Snapshot>("set_state", { id, state }),
  setTitle: (id, title) => call<Snapshot>("set_title", { id, title }),
  setTags: (id, tags) => call<Snapshot>("set_tags", { id, tags }),
  setPriority: (id, priority) => call<Snapshot>("set_priority", { id, priority: priority ?? null }),
  setDue: (id, due) => call<Snapshot>("set_due", { id, due: due ?? null }),
  setParent: (id, parentId) =>
    call<Snapshot>("set_parent", { id, parentId: parentId ?? null }),
  shiftRank: (id, delta) => call<Snapshot>("shift_rank", { id, delta }),
  moveToProject: (id, targetSlug, parentId) =>
    call<Snapshot>("move_to_project", { id, targetSlug, parentId: parentId ?? null }),
  deleteCount: (id) => call<number>("delete_count", { id }),
  deleteTask: (id) => call<Snapshot>("delete_task", { id }),
  search: (filters) =>
    call<string[]>("search", {
      query: filters.query,
      state: filters.state ?? null,
      tag: filters.tag ?? null,
      priority: filters.priority ?? null,
      dueToday: filters.dueToday ?? false,
    }),
  copyTask: (id, includeBody) => call<string>("copy_task", { id, includeBody }),
  copyText: (text) => call<void>("copy_text", { text }),
  openExternal: (url) => call<void>("open_external", { url }),
  closeMainWindow: () => call<void>("close_main_window"),
  hideMenubar: () => call<void>("hide_menubar"),
  setTrayAttention: (attention) => call<void>("set_tray_attention", { attention }),
  openInDesktop: (projectSlug, taskId) =>
    call<void>("open_in_desktop", { projectSlug, taskId }),
  openDesktop: () => call<void>("open_desktop"),
  quitApp: () => call<void>("quit_app"),
};
