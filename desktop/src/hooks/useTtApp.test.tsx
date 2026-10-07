import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TtError } from "../api";
import type { Snapshot, TaskDetail } from "../types";
import { deferred, makeDetail, makeSnapshot, node } from "../test/fixtures";
import { fakeClient } from "../test/fakeClient";
import type { FocusTaskRequest, FocusTaskSource } from "./focusTask";
import { memoryPreferences, useTtApp } from "./useTtApp";

const project = (slug: string) => ({ slug, path: `/tmp/${slug}`, name: slug });

async function boot(client: ReturnType<typeof fakeClient>) {
  const view = renderHook(() =>
    useTtApp(client, { reconcileMs: 0, preferences: memoryPreferences() }),
  );
  await waitFor(() => expect(view.result.current.ready).toBe(true));
  return view;
}

describe("useTtApp draft lifecycle", () => {
  it("a live refresh never replaces a dirty draft", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () =>
        makeSnapshot({ tree: [node("t1", "Task one")], taskCount: 1 }),
      ),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      refresh: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")], revision: "rev1" })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    await waitFor(() => expect(view.result.current.detail?.body).toBe("original"));

    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });
    expect(view.result.current.dirty).toBe(true);

    vi.mocked(client.refresh).mockResolvedValue(
      makeSnapshot({ tree: [node("t1", "Task one")], revision: "rev2" }),
    );
    await act(async () => {
      await view.result.current.refresh();
    });

    expect(view.result.current.draft?.text).toBe("edited");
    expect(view.result.current.detail?.body).toBe("original");
    expect(view.result.current.externalChange).toBe(true);
  });

  it("a stale save keeps the draft and records the conflict", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")] })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      saveBody: vi.fn(async () => {
        throw new TtError({
          code: "stale_draft",
          message: "the task file changed on disk",
          conflict: { reason: "changed", currentRevision: "rev2" },
        });
      }),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });
    await act(async () => {
      await view.result.current.saveDraft();
    });

    expect(view.result.current.draft?.text).toBe("edited");
    expect(view.result.current.conflict?.reason).toBe("changed");
    expect(view.result.current.detail?.body).toBe("original");
  });

  it("switching task while dirty asks first and keeps the draft when declined", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () =>
        makeSnapshot({ tree: [node("t1", "Task one"), node("t2", "Task two")] }),
      ),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, {
          title: id === "t1" ? "Task one" : "Task two",
          body: "body",
          revision: "rev1",
        }),
      ),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    let pending!: Promise<void>;
    act(() => {
      pending = view.result.current.selectTask("t2");
    });
    await waitFor(() => expect(view.result.current.confirmRequest?.destructive).toBe(true));
    act(() => view.result.current.resolveConfirm(false));
    await act(async () => {
      await pending;
    });
    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.draft?.text).toBe("edited");

    act(() => {
      pending = view.result.current.selectTask("t2");
    });
    await waitFor(() => expect(view.result.current.confirmRequest).not.toBeNull());
    act(() => view.result.current.resolveConfirm(true));
    await act(async () => {
      await pending;
    });
    await waitFor(() => expect(view.result.current.selectedId).toBe("t2"));
    expect(view.result.current.draft).toBeNull();
  });

  it("an older project load cannot replace a newer one", async () => {
    const slow = deferred<Snapshot>();
    const fast = deferred<Snapshot>();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [project("a"), project("b")] })),
      openProject: vi.fn((slug: string) => (slug === "a" ? slow.promise : fast.promise)),
    });
    const view = await boot(client);

    let first!: Promise<void>;
    let second!: Promise<void>;
    act(() => {
      first = view.result.current.openProject("a");
      second = view.result.current.openProject("b");
    });
    fast.resolve(makeSnapshot({ projects: [project("a"), project("b")], project: project("b") }));
    await act(async () => {
      await second;
    });
    slow.resolve(makeSnapshot({ projects: [project("a"), project("b")], project: project("a") }));
    await act(async () => {
      await first;
    });

    expect(view.result.current.snapshot?.project?.slug).toBe("b");
  });

  it("an older detail load cannot replace a newer selection", async () => {
    const slow = deferred<TaskDetail>();
    const client = fakeClient({
      bootstrap: vi.fn(async () =>
        makeSnapshot({ tree: [node("t1", "One"), node("t2", "Two")] }),
      ),
      taskDetail: vi.fn((id: string) =>
        id === "t1"
          ? slow.promise
          : Promise.resolve(makeDetail("t2", { title: "Two", body: "two" })),
      ),
    });
    const view = await boot(client);

    let first!: Promise<void>;
    act(() => {
      first = view.result.current.selectTask("t1");
    });
    await act(async () => {
      await view.result.current.selectTask("t2");
    });
    await waitFor(() => expect(view.result.current.detail?.id).toBe("t2"));

    slow.resolve(makeDetail("t1", { title: "One", body: "one" }));
    await act(async () => {
      await first;
    });

    expect(view.result.current.selectedId).toBe("t2");
    expect(view.result.current.detail?.id).toBe("t2");
    expect(view.result.current.detail?.body).toBe("two");
  });

  it("reload latest adopts disk truth and clears the conflict", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")] })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      saveBody: vi.fn(async () => {
        throw new TtError({
          code: "stale_draft",
          message: "changed",
          conflict: { reason: "changed", currentRevision: "rev2" },
        });
      }),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("mine");
    });
    await act(async () => {
      await view.result.current.saveDraft();
    });
    expect(view.result.current.conflict).not.toBeNull();

    vi.mocked(client.taskDetail).mockResolvedValue(
      makeDetail("t1", { body: "theirs", revision: "rev2" }),
    );
    await act(async () => {
      await view.result.current.reloadDraft();
    });
    expect(view.result.current.draft?.text).toBe("theirs");
    expect(view.result.current.draft?.baselineRevision).toBe("rev2");
    expect(view.result.current.conflict).toBeNull();
  });

  it("an externally deleted task retains the draft and marks it missing", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () =>
        makeSnapshot({ tree: [node("t1", "Task one")], revision: "rev1" }),
      ),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      refresh: vi.fn(async () => makeSnapshot({ tree: [], taskCount: 0, revision: "rev2" })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    await act(async () => {
      await view.result.current.refresh();
    });

    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.selectedMissing).toBe(true);
    expect(view.result.current.draft?.text).toBe("edited");
    expect(view.result.current.detail?.body).toBe("original");
  });

  it("unregistering the open project while dirty asks about the draft first", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")] })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      unregisterProject: vi.fn(async () =>
        makeSnapshot({ projects: [], project: undefined, tree: [] }),
      ),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    let pending!: Promise<void>;
    act(() => {
      pending = view.result.current.unregisterProject("demo");
    });
    await waitFor(() => expect(view.result.current.confirmRequest).not.toBeNull());
    act(() => view.result.current.resolveConfirm(false));
    await act(async () => {
      await pending;
    });
    expect(client.unregisterProject).not.toHaveBeenCalled();
    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.draft?.text).toBe("edited");
  });

  it("unregistering with a discarded draft then confirms before removing the project", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")] })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      unregisterProject: vi.fn(async () =>
        makeSnapshot({ projects: [], project: undefined, tree: [] }),
      ),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    let pending!: Promise<void>;
    act(() => {
      pending = view.result.current.unregisterProject("demo");
    });
    // First request: discard the draft.
    await waitFor(() => expect(view.result.current.confirmRequest?.confirmLabel).toBe("Discard"));
    act(() => view.result.current.resolveConfirm(true));
    // Second request: unregister the project.
    await waitFor(() => expect(view.result.current.confirmRequest?.confirmLabel).toBe("Unregister"));
    expect(client.unregisterProject).not.toHaveBeenCalled();
    act(() => view.result.current.resolveConfirm(true));
    await act(async () => {
      await pending;
    });
    expect(client.unregisterProject).toHaveBeenCalledWith("demo");
  });

  it("a background refresh cannot cancel navigation or leave busy stuck", async () => {
    const opened = deferred<Snapshot>();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [project("a"), project("b")] })),
      openProject: vi.fn(async () => opened.promise),
      refresh: vi.fn(async () => makeSnapshot({ projects: [project("a"), project("b")] })),
    });
    const view = await boot(client);

    let pending!: Promise<void>;
    act(() => {
      pending = view.result.current.openProject("b");
    });
    await act(async () => {
      await view.result.current.refresh();
    });
    expect(view.result.current.busy).toBe(true);

    opened.resolve(makeSnapshot({ projects: [project("a"), project("b")], project: project("b") }));
    await act(async () => {
      await pending;
    });
    expect(view.result.current.snapshot?.project?.slug).toBe("b");
    expect(view.result.current.busy).toBe(false);
  });

  it("a background refresh never clobbers an in-flight mutation", async () => {
    const mutation = deferred<Snapshot>();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")] })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      setState: vi.fn(() => mutation.promise),
      refresh: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")], revision: "stale" })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });

    act(() => {
      view.result.current.setState("done");
    });
    await waitFor(() => expect(view.result.current.busy).toBe(true));
    await act(async () => {
      await view.result.current.refresh();
    });
    expect(view.result.current.snapshot?.revision).toBe("rev1");

    mutation.resolve(
      makeSnapshot({ tree: [node("t1", "Task one", { state: "done" })], revision: "mutated" }),
    );
    await waitFor(() => expect(view.result.current.busy).toBe(false));
    expect(view.result.current.snapshot?.revision).toBe("mutated");
  });

  it("a delayed save refuses later keystrokes instead of losing them", async () => {
    const pending = deferred<TaskDetail>();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One"), node("t2", "Two")] })),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { body: `body-${id}`, revision: `rev-${id}` }),
      ),
      saveBody: vi.fn(() => pending.promise),
      refresh: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One"), node("t2", "Two")] })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("submitted");
    });

    let saving!: Promise<void>;
    act(() => {
      saving = view.result.current.saveDraft();
    });
    expect(view.result.current.saving).toBe(true);

    // Editing is disabled while the save owns the draft, so nothing typed now
    // can be overwritten by the response.
    act(() => view.result.current.setDraftText("typed during save"));
    expect(view.result.current.draft?.text).toBe("submitted");

    pending.resolve(makeDetail("t1", { body: "submitted", revision: "rev-t1-saved" }));
    await act(async () => {
      await saving;
    });

    expect(client.saveBody).toHaveBeenCalledWith("t1", "submitted", "rev-t1");
    expect(view.result.current.draft).toBeNull();
    expect(view.result.current.detail?.body).toBe("submitted");
    expect(view.result.current.saving).toBe(false);
  });

  it("an in-flight save holds navigation and applies only to its own task", async () => {
    const pending = deferred<TaskDetail>();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One"), node("t2", "Two")] })),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { body: `body-${id}`, revision: `rev-${id}` }),
      ),
      saveBody: vi.fn(() => pending.promise),
      refresh: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One"), node("t2", "Two")] })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    let saving!: Promise<void>;
    act(() => {
      saving = view.result.current.saveDraft();
    });

    // Navigation is refused while the save owns the draft.
    await act(async () => {
      await view.result.current.selectTask("t2");
    });
    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.detail?.id).toBe("t1");
    expect(view.result.current.draft?.text).toBe("edited");

    // A metadata mutation cannot slip behind the save either.
    act(() => view.result.current.setState("done"));
    expect(client.setState).not.toHaveBeenCalled();

    pending.resolve(makeDetail("t1", { body: "edited", revision: "rev-t1-saved" }));
    await act(async () => {
      await saving;
    });
    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.detail?.body).toBe("edited");
    expect(view.result.current.draft).toBeNull();
  });

  it("an older save response cannot clear a newer draft", async () => {
    const pending = deferred<TaskDetail>();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One")] })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      saveBody: vi.fn(() => pending.promise),
      refresh: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One")] })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    let saving!: Promise<void>;
    act(() => {
      saving = view.result.current.saveDraft();
    });
    // A newer draft replaces the submitted one before the response lands.
    act(() => view.result.current.startEdit());
    const newer = view.result.current.draft;
    expect(newer?.text).toBe("original");

    pending.resolve(makeDetail("t1", { body: "edited", revision: "rev1-saved" }));
    await act(async () => {
      await saving;
    });

    expect(view.result.current.detail?.body).toBe("original");
    expect(view.result.current.draft).toBe(newer);
  });

  it("an unchanged draft does not follow the selection to another task", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One"), node("t2", "Two")] })),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, {
          title: id === "t1" ? "One" : "Two",
          body: `body-${id}`,
          revision: `rev-${id}`,
        }),
      ),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => view.result.current.startEdit());
    expect(view.result.current.draft?.text).toBe("body-t1");

    await act(async () => {
      await view.result.current.selectTask("t2");
    });

    expect(view.result.current.detail?.id).toBe("t2");
    expect(view.result.current.detail?.body).toBe("body-t2");
    expect(view.result.current.draft).toBeNull();
  });

  it("an unchanged draft is cleared when the open project changes", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () =>
        makeSnapshot({ projects: [project("a"), project("b")], project: project("a"), tree: [node("t1", "One")] }),
      ),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      openProject: vi.fn(async () =>
        makeSnapshot({ projects: [project("a"), project("b")], project: project("b"), tree: [node("t1", "One")] }),
      ),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => view.result.current.startEdit());
    expect(view.result.current.draft).not.toBeNull();

    await act(async () => {
      await view.result.current.openProject("b");
    });

    expect(view.result.current.selectedId).toBeNull();
    expect(view.result.current.draft).toBeNull();
  });

  it("closing the recovery view asks first and keeps the draft when declined", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "One")], revision: "rev1" })),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { body: "original", revision: "rev1" })),
      refresh: vi.fn(async () => makeSnapshot({ tree: [], taskCount: 0, revision: "rev2" })),
    });
    const view = await boot(client);
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("recovery text");
    });
    await act(async () => {
      await view.result.current.refresh();
    });
    expect(view.result.current.selectedMissing).toBe(true);

    let closing!: Promise<void>;
    act(() => {
      closing = view.result.current.clearSelection();
    });
    await waitFor(() => expect(view.result.current.confirmRequest?.confirmLabel).toBe("Discard"));
    act(() => view.result.current.resolveConfirm(false));
    await act(async () => {
      await closing;
    });
    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.selectedMissing).toBe(true);
    expect(view.result.current.draft?.text).toBe("recovery text");

    act(() => {
      closing = view.result.current.clearSelection();
    });
    await waitFor(() => expect(view.result.current.confirmRequest).not.toBeNull());
    act(() => view.result.current.resolveConfirm(true));
    await act(async () => {
      await closing;
    });
    expect(view.result.current.selectedId).toBeNull();
    expect(view.result.current.selectedMissing).toBe(false);
    expect(view.result.current.draft).toBeNull();
    expect(view.result.current.detail).toBeNull();
  });
});

describe("useTtApp focus-task requests", () => {
  const other = project("other");

  function focusSource() {
    const handlers = new Set<(request: FocusTaskRequest) => void>();
    return {
      source: {
        subscribe(handler: (request: FocusTaskRequest) => void) {
          handlers.add(handler);
          return () => {
            handlers.delete(handler);
          };
        },
      } satisfies FocusTaskSource,
      emit(request: FocusTaskRequest) {
        for (const handler of [...handlers]) handler(request);
      },
    };
  }

  function renderWithFocus(
    client: ReturnType<typeof fakeClient>,
    source: FocusTaskSource,
  ) {
    return renderHook(() =>
      useTtApp(client, {
        reconcileMs: 0,
        preferences: memoryPreferences(),
        focusTaskSource: source,
      }),
    );
  }

  it("opens the Project and selects the Task the popover asked for", async () => {
    const focus = focusSource();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [project("demo"), other] })),
      openProject: vi.fn(async (slug: string) =>
        makeSnapshot({
          projects: [project("demo"), other],
          project: slug === "other" ? other : project("demo"),
          tree: [node("o1", "Other task")],
        }),
      ),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { title: "Other task" })),
    });
    const view = renderWithFocus(client, focus.source);
    await waitFor(() => expect(view.result.current.ready).toBe(true));

    await act(async () => {
      focus.emit({ projectSlug: "other", taskId: "o1" });
    });

    expect(client.openProject).toHaveBeenCalledWith("other");
    await waitFor(() => expect(view.result.current.selectedId).toBe("o1"));
  });

  it("honors the dirty-draft guard: declined keeps the draft and the selection", async () => {
    const focus = focusSource();
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ tree: [node("t1", "Task one")] })),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { body: "original", revision: "rev1" }),
      ),
      openProject: vi.fn(async () =>
        makeSnapshot({ projects: [other], project: other, tree: [node("o1", "Other task")] }),
      ),
    });
    const view = renderWithFocus(client, focus.source);
    await waitFor(() => expect(view.result.current.ready).toBe(true));
    await act(async () => {
      await view.result.current.selectTask("t1");
    });
    act(() => {
      view.result.current.startEdit();
      view.result.current.setDraftText("edited");
    });

    await act(async () => {
      focus.emit({ projectSlug: "other", taskId: "o1" });
    });
    await waitFor(() => expect(view.result.current.confirmRequest).not.toBeNull());
    // Fail closed: neither the switch nor the selection happened.
    expect(client.openProject).not.toHaveBeenCalled();
    expect(view.result.current.selectedId).toBe("t1");

    act(() => view.result.current.resolveConfirm(false));
    await act(async () => {
      await Promise.resolve();
    });
    expect(client.openProject).not.toHaveBeenCalled();
    expect(view.result.current.selectedId).toBe("t1");
    expect(view.result.current.draft?.text).toBe("edited");

    // Accepting the discard lets the same request through.
    await act(async () => {
      focus.emit({ projectSlug: "other", taskId: "o1" });
    });
    await waitFor(() => expect(view.result.current.confirmRequest).not.toBeNull());
    act(() => view.result.current.resolveConfirm(true));
    await waitFor(() => expect(client.openProject).toHaveBeenCalledWith("other"));
    await waitFor(() => expect(view.result.current.selectedId).toBe("o1"));
  });

describe("useTtApp sidebar accordion", () => {
  it("folds the open Project's Tasks and re-expands the Project when it changes", async () => {
    const demo = { slug: "demo", path: "/tmp/demo", name: "demo" };
    const work = { slug: "work", path: "/tmp/work", name: "Work" };
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [demo, work], project: demo })),
      openProject: vi.fn(async () =>
        makeSnapshot({ projects: [demo, work], project: work, tree: [node("w1", "Work task")] }),
      ),
    });
    const view = await boot(client);

    expect(view.result.current.projectCollapsed).toBe(false);
    act(() => view.result.current.toggleProjectFold());
    expect(view.result.current.projectCollapsed).toBe(true);
    act(() => view.result.current.toggleProjectFold());
    expect(view.result.current.projectCollapsed).toBe(false);

    act(() => view.result.current.toggleProjectFold());
    await act(async () => {
      await view.result.current.openProject("work");
    });
    expect(view.result.current.snapshot?.project?.slug).toBe("work");
    expect(view.result.current.projectCollapsed).toBe(false);
  });
});
});
