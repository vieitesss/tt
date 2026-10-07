import { vi } from "vitest";

import type { TtClient } from "../api";
import { makeDetail, makeSnapshot } from "./fixtures";

/** A `TtClient` whose every method is a spy with a harmless default. */
export function fakeClient(overrides: Partial<TtClient> = {}): TtClient {
  const base: TtClient = {
    bootstrap: vi.fn(async () => makeSnapshot()),
    openProject: vi.fn(async () => makeSnapshot()),
    refresh: vi.fn(async () => makeSnapshot()),
    registerProject: vi.fn(async () => null),
    unregisterProject: vi.fn(async () => makeSnapshot({ project: undefined })),
    taskDetail: vi.fn(async (id: string) => makeDetail(id)),
    saveBody: vi.fn(async (id: string, body: string) => makeDetail(id, { body })),
    addTask: vi.fn(async () => makeSnapshot()),
    setState: vi.fn(async () => makeSnapshot()),
    setTitle: vi.fn(async () => makeSnapshot()),
    setTags: vi.fn(async () => makeSnapshot()),
    setPriority: vi.fn(async () => makeSnapshot()),
    setDue: vi.fn(async () => makeSnapshot()),
    setParent: vi.fn(async () => makeSnapshot()),
    shiftRank: vi.fn(async () => makeSnapshot()),
    moveToProject: vi.fn(async () => makeSnapshot()),
    deleteCount: vi.fn(async () => 0),
    deleteTask: vi.fn(async () => makeSnapshot()),
    search: vi.fn(async () => []),
    copyTask: vi.fn(async () => ""),
    copyText: vi.fn(async () => undefined),
    openExternal: vi.fn(async () => undefined),
    closeMainWindow: vi.fn(async () => undefined),
    hideMenubar: vi.fn(async () => undefined),
    setTrayAttention: vi.fn(async () => undefined),
    openInDesktop: vi.fn(async () => undefined),
    openDesktop: vi.fn(async () => undefined),
    quitApp: vi.fn(async () => undefined),
  };
  return Object.assign(base, overrides);
}
