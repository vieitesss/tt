import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { TtError } from "../api";
import { memoryPreferences } from "../hooks/useTtApp";
import { todayIso } from "../lib/display";
import { fakeClient } from "../test/fakeClient";
import { fakeMenubarHost } from "../test/fakeHost";
import { makeDetail, makeSnapshot, node } from "../test/fixtures";
import { Menubar } from "./Menubar";
import { MENUBAR_PROJECT_KEY, type UseMenubarOptions } from "./useMenubar";

const alpha = { slug: "alpha", path: "/tmp/alpha", name: "alpha" };
const beta = { slug: "beta", path: "/tmp/beta", name: "beta" };

/** Polling is off in every test; the delays under test are the popover's own. */
const options = (overrides: Partial<UseMenubarOptions> = {}): UseMenubarOptions => ({
  visibleMs: 0,
  hiddenMs: 0,
  preferences: memoryPreferences(),
  ...overrides,
});

function renderPopover(
  client: ReturnType<typeof fakeClient>,
  overrides: Partial<UseMenubarOptions> = {},
) {
  const shell = fakeMenubarHost();
  const preferences = overrides.preferences ?? memoryPreferences();
  render(<Menubar client={client} host={shell.host} options={options({ ...overrides, preferences })} />);
  return { ...shell, preferences, user: userEvent.setup() };
}

const openTaskSnapshot = (tree = [node("t1", "Task one")], revision = "rev1") =>
  makeSnapshot({ projects: [alpha], project: alpha, tree, taskCount: tree.length, revision });

describe("Menubar completion", () => {
  it("checks and strikes a completed row, drops it after the delay, and Undo reopens it", async () => {
    let state = "open";
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      setState: vi.fn(async (_id: string, next: string) => {
        state = next;
        return makeSnapshot({
          projects: [alpha],
          project: alpha,
          tree: state === "open" ? [node("t1", "Task one")] : [],
          revision: state,
        });
      }),
    });
    const { user } = renderPopover(client, { completeDelayMs: 300 });

    await user.click(await screen.findByLabelText("Complete Task one"));

    const reopen = await screen.findByLabelText("Reopen Task one");
    expect(reopen).toBeChecked();
    expect(reopen.closest(".mb-row")).toHaveClass("state-done");
    expect(client.setState).toHaveBeenCalledWith("t1", "done");

    await waitFor(
      () => expect(screen.queryByLabelText("Reopen Task one")).not.toBeInTheDocument(),
      { timeout: 3000 },
    );

    await user.click(screen.getByRole("button", { name: "Undo" }));
    expect(client.setState).toHaveBeenLastCalledWith("t1", "open");
    expect(await screen.findByRole("button", { name: "Task one" })).toBeInTheDocument();
  });

  it("reopens a done task when its checkbox is clicked", async () => {
    const tree = [node("t1", "Task one", { state: "done" })];
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot(tree)),
      refresh: vi.fn(async () => openTaskSnapshot(tree)),
      setState: vi.fn(async () => openTaskSnapshot(tree)),
    });
    renderPopover(client);

    await screen.findByLabelText("Show done");
    expect(screen.queryByRole("button", { name: "Task one" })).not.toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(screen.getByLabelText("Show done"));
    await user.click(await screen.findByLabelText("Reopen Task one"));
    expect(client.setState).toHaveBeenCalledWith("t1", "open");
  });
});

describe("Menubar quick add", () => {
  it("adds a root task on Enter and a subtask of the expanded task on Tab", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "body" }),
      ),
      addTask: vi.fn(async () => openTaskSnapshot()),
    });
    const { user } = renderPopover(client);

    const input = await screen.findByLabelText("Quick add a task");
    await user.type(input, "Root task{Enter}");
    expect(client.addTask).toHaveBeenCalledWith({ title: "Root task", parentId: undefined });

    await user.click(screen.getByRole("button", { name: "Task one" }));
    await screen.findByText(/Tab adds a subtask of/);
    await user.type(input, "Child task{Tab}");
    expect(client.addTask).toHaveBeenLastCalledWith({ title: "Child task", parentId: "t1" });
  });
});

describe("Menubar detail", () => {
  const detailClient = () =>
    fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "# Heading\n\nBody text", revision: "rev1" }),
      ),
    });

  it("renders Markdown inline and offers the row actions", async () => {
    const client = detailClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    expect(await screen.findByRole("heading", { name: "Heading" })).toBeInTheDocument();
    expect(screen.getByText("Body text")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByRole("button", { name: "Copy task" }));
    expect(screen.getByRole("button", { name: "Open in TT" }));
    expect(client.taskDetail).toHaveBeenCalledWith("t1");
  });

  it("hands the task to the main window on Open in TT", async () => {
    const client = detailClient();
    const { host, user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await screen.findByRole("heading", { name: "Heading" });
    await user.click(screen.getByRole("button", { name: "Open in TT" }));
    expect(host.openInDesktop).toHaveBeenCalledWith("alpha", "t1");
  });

  it("opens the main window and quits from the footer", async () => {
    const { host, user } = renderPopover(detailClient());

    await user.click(await screen.findByRole("button", { name: "Open TT" }));
    expect(host.openDesktop).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Quit" }));
    expect(host.quit).toHaveBeenCalledTimes(1);
  });
});

/**
 * A one-Task disk whose revision really moves on every write, and whose
 * `saveBody` rejects a stale base revision the way the backend does.
 */
function diskClient() {
  const disk = { title: "Task one", body: "original", revision: "rev1", writes: 0 };
  const write = (patch: Partial<typeof disk>) => {
    disk.writes += 1;
    Object.assign(disk, patch, { revision: `rev${disk.writes + 1}` });
  };
  const client = fakeClient({
    bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
    openProject: vi.fn(async () => openTaskSnapshot()),
    refresh: vi.fn(async () => openTaskSnapshot()),
    taskDetail: vi.fn(async (id: string) => makeDetail(id, { ...disk })),
    setTitle: vi.fn(async (_id: string, title: string) => {
      write({ title });
      return openTaskSnapshot();
    }),
    saveBody: vi.fn(async (id: string, body: string, baseRevision: string) => {
      if (baseRevision !== disk.revision) {
        throw new TtError({
          code: "stale_draft",
          message: "the task file changed on disk",
          conflict: { reason: "changed", currentRevision: disk.revision, currentBody: disk.body },
        });
      }
      write({ body });
      return makeDetail(id, { ...disk });
    }),
  });
  return { client, disk, externalWrite: (body: string) => write({ body }) };
}

describe("Menubar stale saves", () => {
  it("fails a body-only save when an external write landed after the edit began", async () => {
    const { client, disk, externalWrite } = diskClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Task description"));
    await user.type(screen.getByLabelText("Task description"), "my draft");

    externalWrite("CLI edit to preserve");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await screen.findByText(/changed on disk/);
    expect(client.saveBody).toHaveBeenCalledWith("t1", "my draft", "rev1");
    expect(disk.body).toBe("CLI edit to preserve");
    expect(screen.getByLabelText("Task description")).toHaveValue("my draft");
    expect(screen.getByRole("button", { name: "Reload latest" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy draft" })).toBeInTheDocument();
  });

  it("fails a renamed save before set_title when an external write landed after the edit began", async () => {
    const { client, disk, externalWrite } = diskClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Task title"));
    await user.type(screen.getByLabelText("Task title"), "Renamed");
    await user.clear(screen.getByLabelText("Task description"));
    await user.type(screen.getByLabelText("Task description"), "my draft");

    externalWrite("CLI edit to preserve");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await screen.findByText(/changed on disk/);
    expect(client.setTitle).not.toHaveBeenCalled();
    expect(client.saveBody).not.toHaveBeenCalled();
    expect(disk.title).toBe("Task one");
    expect(disk.body).toBe("CLI edit to preserve");
    expect(screen.getByLabelText("Task title")).toHaveValue("Renamed");
    expect(screen.getByLabelText("Task description")).toHaveValue("my draft");
    expect(screen.getByRole("button", { name: "Reload latest" })).toBeInTheDocument();
  });

  it("saves a renamed draft against the revision its own rename produced", async () => {
    const { client, disk } = diskClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Task title"));
    await user.type(screen.getByLabelText("Task title"), "Renamed");
    await user.clear(screen.getByLabelText("Task description"));
    await user.type(screen.getByLabelText("Task description"), "edited");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await waitFor(() => expect(screen.queryByLabelText("Task description")).not.toBeInTheDocument());
    expect(client.saveBody).toHaveBeenCalledWith("t1", "edited", "rev2");
    expect(disk).toMatchObject({ title: "Renamed", body: "edited" });
  });
});

describe("Menubar vanished tasks", () => {
  const vanishingClient = () => {
    let gone = false;
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => (gone ? openTaskSnapshot([], "rev2") : openTaskSnapshot())),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "body", revision: "rev1" }),
      ),
    });
    return { client, delete: () => (gone = true) };
  };

  it("keeps a recovery panel with the draft, Copy draft and a guarded Discard when its Task is deleted", async () => {
    const { client, delete: remove } = vanishingClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), " my draft");

    remove();
    await user.click(screen.getByRole("button", { name: "Refresh from disk" }));

    expect(await screen.findByText(/deleted or moved/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Task one" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Unsaved draft")).toHaveValue("body my draft");

    await user.click(screen.getByRole("button", { name: "Copy draft" }));
    expect(client.copyText).toHaveBeenCalledWith("body my draft");

    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(screen.getByRole("alertdialog", { name: "Discard changes?" })).toBeInTheDocument();
    expect(screen.getByLabelText("Unsaved draft")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(screen.getByLabelText("Unsaved draft")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    await user.click(await screen.findByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByLabelText("Unsaved draft")).not.toBeInTheDocument());
    expect(screen.queryByText(/deleted or moved/)).not.toBeInTheDocument();
    expect(screen.queryByTestId("subtask-hint")).not.toBeInTheDocument();
  });

  it("lets Escape discard the recovery draft through the same guard", async () => {
    const { client, delete: remove } = vanishingClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), "!");
    remove();
    await user.click(screen.getByRole("button", { name: "Refresh from disk" }));
    await screen.findByLabelText("Unsaved draft");

    await user.keyboard("{Escape}");
    await user.click(await screen.findByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByLabelText("Unsaved draft")).not.toBeInTheDocument());
    expect(screen.queryByTestId("subtask-hint")).not.toBeInTheDocument();
  });

  it("simply collapses a clean expanded Task that disappears", async () => {
    const { client, delete: remove } = vanishingClient();
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await screen.findByRole("button", { name: "Edit" });

    remove();
    await user.click(screen.getByRole("button", { name: "Refresh from disk" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument());
    expect(screen.queryByLabelText("Unsaved draft")).not.toBeInTheDocument();
    expect(screen.queryByText(/deleted or moved/)).not.toBeInTheDocument();
  });
});

describe("Menubar held draft rows", () => {
  const completedElsewhere = (tree: () => ReturnType<typeof node>[]) =>
    fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot(tree(), "rev2")),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "body", revision: "rev1" }),
      ),
    });

  it("keeps the editor of a Task completed externally while Show done is off, and Save still works", async () => {
    let done = false;
    const client = completedElsewhere(() => [node("t1", "Task one", { state: done ? "done" : "open" })]);
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), " my draft");

    done = true;
    await user.click(screen.getByRole("button", { name: "Refresh from disk" }));
    await waitFor(() => expect(client.refresh).toHaveBeenCalled());

    expect(screen.getByLabelText("Task description")).toHaveValue("body my draft");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(client.saveBody).toHaveBeenCalledWith("t1", "body my draft", "rev1"),
    );
  });

  it("keeps a draft Task and its ancestors in the rows under a folded parent", async () => {
    const tree = () => [node("p", "Parent", { hasChildren: true, children: [node("c", "Child")] })];
    const client = completedElsewhere(tree);
    client.openProject = vi.fn(async () => openTaskSnapshot(tree()));
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Child" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), "!");
    await user.click(screen.getByRole("button", { name: "Collapse Parent" }));

    expect(screen.getByLabelText("Task description")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Parent" })).toBeInTheDocument();
  });
});

describe("Menubar draft", () => {
  it("saves a changed title and body with the revision our own rename produced", async () => {
    let renamed = false;
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, {
          title: renamed ? "Renamed" : "Task one",
          body: "original",
          revision: renamed ? "rev2" : "rev1",
        }),
      ),
      setTitle: vi.fn(async () => {
        renamed = true;
        return openTaskSnapshot();
      }),
      saveBody: vi.fn(async (id: string, body: string) =>
        makeDetail(id, { title: "Renamed", body, revision: "rev3" }),
      ),
    });
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Task title"));
    await user.type(screen.getByLabelText("Task title"), "Renamed");
    await user.clear(screen.getByLabelText("Task description"));
    await user.type(screen.getByLabelText("Task description"), "edited body");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await waitFor(() => expect(client.setTitle).toHaveBeenCalledWith("t1", "Renamed"));
    expect(client.saveBody).toHaveBeenCalledWith("t1", "edited body", "rev2");
    // The draft is gone: the read-only actions are back.
    expect(await screen.findByRole("button", { name: "Edit" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Task description")).not.toBeInTheDocument();
  });

  it("keeps a stale draft and offers Reload latest and Copy draft", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "mine", revision: "rev1" }),
      ),
      saveBody: vi.fn(async () => {
        throw new TtError({
          code: "stale_draft",
          message: "the task file changed on disk",
          conflict: { reason: "changed", currentRevision: "rev2", currentBody: "theirs" },
        });
      }),
    });
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Task description"));
    await user.type(screen.getByLabelText("Task description"), "mine, edited");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await screen.findByText(/changed on disk/);
    expect(screen.getByLabelText("Task description")).toHaveValue("mine, edited");
    expect(screen.getByRole("button", { name: "Reload latest" }));
    expect(screen.getByRole("button", { name: "Copy draft" }));

    await user.click(screen.getByRole("button", { name: "Copy draft" }));
    expect(client.copyText).toHaveBeenCalledWith("mine, edited");
  });

  it("keeps an externally deleted draft visible with Copy draft only", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "mine", revision: "rev1" }),
      ),
      saveBody: vi.fn(async () => {
        throw new TtError({
          code: "stale_draft",
          message: "the task file was deleted outside the app",
          conflict: { reason: "deleted" },
        });
      }),
    });
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), "!");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await screen.findByText(/deleted outside the app/);
    expect(screen.getByLabelText("Task description")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reload latest" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy draft" }));
  });

  it("asks before a dirty Escape discards, keeping the draft until Discard is chosen", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { title: "Task one", body: "body" })),
    });
    const { host, user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), "!");

    await user.keyboard("{Escape}");
    const guard = await screen.findByRole("alertdialog", { name: "Discard changes?" });
    expect(guard).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Keep" })).toHaveFocus();

    // The guard owns the keyboard: Escape neither hides the popover nor discards.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("alertdialog", { name: "Discard changes?" }));
    expect(host.hide).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(screen.getByLabelText("Task description")).toHaveValue("body!");
    expect(host.hide).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    await user.click(await screen.findByRole("button", { name: "Discard" }));
    await waitFor(() =>
      expect(screen.queryByLabelText("Task description")).not.toBeInTheDocument(),
    );
  });

  it("never lets a refresh replace the draft", async () => {
    let revision = "rev1";
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => {
        revision = "rev2";
        return openTaskSnapshot([node("t1", "Renamed elsewhere")], revision);
      }),
      taskDetail: vi.fn(async (id: string) => makeDetail(id, { title: "Task one", body: "body" })),
    });
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Task description"), " my draft");

    await user.click(screen.getByRole("button", { name: "Refresh from disk" }));
    await waitFor(() => expect(client.refresh).toHaveBeenCalled());

    expect(client.taskDetail).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Task description")).toHaveValue("body my draft");
    // The row itself is disk truth; only the draft is untouched.
    expect(screen.getAllByText("Renamed elsewhere").length).toBeGreaterThan(0);
  });

  it("hides the popover on Escape when nothing is open", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
    });
    const { host, user } = renderPopover(client);

    await screen.findByRole("button", { name: "Task one" });
    await user.keyboard("{Escape}");
    expect(host.hide).toHaveBeenCalled();

    // Focus inside quick add is still "nothing open": Escape hides.
    const input = await screen.findByLabelText("Quick add a task");
    await user.click(input);
    await user.keyboard("{Escape}");
    expect(host.hide).toHaveBeenCalledTimes(2);
  });

  it("guards a title-only change before Escape drops it", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Task one", body: "body" }),
      ),
    });
    const { user } = renderPopover(client);

    await user.click(await screen.findByRole("button", { name: "Task one" }));
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.clear(screen.getByLabelText("Task title"));
    await user.type(screen.getByLabelText("Task title"), "Renamed but unsaved");

    await user.keyboard("{Escape}");
    expect(await screen.findByRole("alertdialog", { name: "Discard changes?" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(screen.getByLabelText("Task title")).toHaveValue("Renamed but unsaved");
  });
});

describe("Menubar Project switcher", () => {
  const switcherClient = () =>
    fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha, beta] })),
      openProject: vi.fn(async () => openTaskSnapshot()),
      refresh: vi.fn(async () => openTaskSnapshot()),
    });
  const openSwitcher = async (user: ReturnType<typeof userEvent.setup>) => {
    await user.click(await screen.findByRole("button", { name: /alpha/ }));
    return screen.findByRole("listbox", { name: "Projects" });
  };

  it("closes on an outside click without hiding the popover", async () => {
    const { host, user } = renderPopover(switcherClient());
    await openSwitcher(user);

    await user.click(screen.getByLabelText("Quick add a task"));

    expect(screen.queryByRole("listbox", { name: "Projects" })).not.toBeInTheDocument();
    expect(host.hide).not.toHaveBeenCalled();
  });

  it("stays open for a click inside it, and a click on its button closes it", async () => {
    const { user } = renderPopover(switcherClient());
    const list = await openSwitcher(user);

    await user.click(list);
    expect(screen.getByRole("listbox", { name: "Projects" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /alpha/ }));
    expect(screen.queryByRole("listbox", { name: "Projects" })).not.toBeInTheDocument();
  });

  it("keeps the list mounted but out of the accessibility tree while it unrolls away", async () => {
    const { user } = renderPopover(switcherClient());
    const list = await openSwitcher(user);
    expect(list).toHaveClass("reveal");

    await user.click(screen.getByRole("button", { name: /alpha/ }));
    expect(list).toBeInTheDocument();
    expect(list).toHaveClass("leaving");
    expect(list).toHaveAttribute("inert");
    expect(screen.queryByRole("listbox", { name: "Projects" })).not.toBeInTheDocument();

    fireEvent.transitionEnd(list);
    expect(list).not.toBeInTheDocument();
  });

  it("consumes Escape before hiding, so the next Escape hides", async () => {
    const { host, user } = renderPopover(switcherClient());
    await openSwitcher(user);

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: "Projects" })).not.toBeInTheDocument();
    expect(host.hide).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    expect(host.hide).toHaveBeenCalledTimes(1);
  });

  it("is closed after the popover hides and shows again", async () => {
    const { setVisible, user } = renderPopover(switcherClient());
    act(() => setVisible(true));
    await openSwitcher(user);

    act(() => setVisible(false));
    act(() => setVisible(true));

    expect(screen.queryByRole("listbox", { name: "Projects" })).not.toBeInTheDocument();
  });
});

describe("Menubar projects and attention", () => {
  it("switches to the Nth registered Project with ⌥digit and remembers it", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha, beta] })),
      openProject: vi.fn(async (slug: string) =>
        makeSnapshot({
          projects: [alpha, beta],
          project: slug === "beta" ? beta : alpha,
          tree: [node("t1", `${slug} task`)],
        }),
      ),
      refresh: vi.fn(async () => openTaskSnapshot()),
    });
    const { preferences, user } = renderPopover(client);

    await screen.findByRole("button", { name: "alpha task" });
    await user.keyboard("{Alt>}2{/Alt}");

    await waitFor(() => expect(client.openProject).toHaveBeenCalledWith("beta"));
    expect(preferences.get(MENUBAR_PROJECT_KEY)).toBe("beta");
  });

  it("lights the tray dot exactly for overdue and due-today open tasks", async () => {
    const today = todayIso();
    const yesterday = new Date(Date.now() - 86_400_000);
    const past = `${yesterday.getFullYear()}-${String(yesterday.getMonth() + 1).padStart(2, "0")}-${String(yesterday.getDate()).padStart(2, "0")}`;
    const tree = [
      node("t1", "Overdue", { due: past }),
      node("t2", "Due today", { due: today }),
      node("t3", "Later", { due: "2999-01-01" }),
      node("t4", "Done overdue", { due: past, state: "done" }),
    ];
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () => openTaskSnapshot(tree)),
      refresh: vi.fn(async () => openTaskSnapshot(tree)),
    });
    const { host } = renderPopover(client);

    await screen.findByRole("button", { name: "Overdue" });
    await waitFor(() => expect(host.setTrayAttention).toHaveBeenCalledWith(true));
  });

  it("leaves the tray dot dark without an overdue or due-today open task", async () => {
    const client = fakeClient({
      bootstrap: vi.fn(async () => makeSnapshot({ projects: [alpha] })),
      openProject: vi.fn(async () =>
        openTaskSnapshot([node("t1", "Later", { due: "2999-01-01" })]),
      ),
      refresh: vi.fn(async () => openTaskSnapshot()),
    });
    const { host } = renderPopover(client);

    await screen.findByRole("button", { name: "Later" });
    await waitFor(() => expect(host.setTrayAttention).toHaveBeenCalled());
    expect(host.setTrayAttention).not.toHaveBeenCalledWith(true);
  });
});
