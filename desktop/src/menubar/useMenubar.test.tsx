import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { memoryPreferences } from "../hooks/useTtApp";
import { fakeClient } from "../test/fakeClient";
import { fakeMenubarHost } from "../test/fakeHost";
import { makeSnapshot, node } from "../test/fixtures";
import { useMenubar } from "./useMenubar";

const alpha = { slug: "alpha", path: "/tmp/alpha", name: "alpha" };

const snapshot = () =>
  makeSnapshot({
    projects: [alpha],
    project: alpha,
    tree: [node("t1", "Task one")],
    taskCount: 1,
  });

async function boot(options: Parameters<typeof useMenubar>[2] = {}) {
  const shell = fakeMenubarHost();
  const client = fakeClient({
    bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
    openProject: vi.fn(async () => snapshot()),
    refresh: vi.fn(async () => snapshot()),
  });
  const view = renderHook(() =>
    useMenubar(client, shell.host, {
      preferences: memoryPreferences(),
      ...options,
    }),
  );
  await waitFor(() => expect(view.result.current.ready).toBe(true));
  return { client, shell, view };
}

describe("useMenubar live refresh", () => {
  it("refreshes at once when shown, then on the visible interval", async () => {
    const { client, shell, view } = await boot({ visibleMs: 20, hiddenMs: 10_000 });
    // Hidden by default: the slow interval has not fired.
    expect(client.refresh).not.toHaveBeenCalled();

    await act(async () => {
      shell.setVisible(true);
    });
    expect(client.refresh).toHaveBeenCalledTimes(1);

    const first = vi.mocked(client.refresh).mock.calls.length;
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 70));
    });
    expect(vi.mocked(client.refresh).mock.calls.length).toBeGreaterThan(first);

    // Hidden again: the fast interval stops.
    await act(async () => {
      shell.setVisible(false);
    });
    const paused = vi.mocked(client.refresh).mock.calls.length;
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 70));
    });
    expect(vi.mocked(client.refresh).mock.calls.length).toBe(paused);
    expect(view.result.current.visible).toBe(false);
  });

  it("unsubscribes from the shell when it unmounts", async () => {
    const { shell, view } = await boot({ visibleMs: 0, hiddenMs: 0 });
    expect(shell.listeners()).toBe(1);
    view.unmount();
    expect(shell.listeners()).toBe(0);
  });
});
