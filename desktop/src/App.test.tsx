import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { App } from "./App";
import { tauriClient } from "./api";
import type { Snapshot } from "./types";
import { deferred, makeDetail, makeSnapshot, node } from "./test/fixtures";
import { fakeClient } from "./test/fakeClient";

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

const project = { slug: "demo", path: "/tmp/demo", name: "demo" };

const tree = [
  node("root1", "Root", {
    hasChildren: true,
    total: 1,
    children: [node("child1", "Child", { parentId: "root1" })],
  }),
  node("other1", "Other"),
];

function snapshot(overrides: Partial<Snapshot> = {}): Snapshot {
  return makeSnapshot({ projects: [project], project, tree, taskCount: 3, ...overrides });
}

function appClient() {
  return fakeClient({
    bootstrap: vi.fn(async () => snapshot()),
    refresh: vi.fn(async () => snapshot()),
    openProject: vi.fn(async () => snapshot()),
    taskDetail: vi.fn(async (id: string) =>
      id === "child1"
        ? makeDetail("child1", {
            title: "Child",
            body: "see [[root1]]",
            links: [{ id: "root1", title: "Root" }],
          })
        : makeDetail("root1", { title: "Root", body: "root body" }),
    ),
    search: vi.fn(async () => ["child1"]),
  });
}

describe("App", () => {
  it("searches project-scoped titles, keeping ancestors as context", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.change(screen.getByLabelText("Search tasks"), { target: { value: "child" } });
    await screen.findByText("1 match(es)");
    expect(screen.getByText("Child")).toBeInTheDocument();
    expect(screen.getByText("Root")).toBeInTheDocument();
    expect(screen.queryByText("Other")).not.toBeInTheDocument();
    expect(client.search).toHaveBeenCalledWith({
      query: "child",
      state: undefined,
      tag: "",
      priority: undefined,
      dueToday: false,
    });
  });

  it("selects a task and follows a wikilink to its live title", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.click(screen.getByText("Child"));
    await waitFor(() => expect(screen.getByTestId("body-preview")).toHaveTextContent("see"));

    const wikilink = await within(await screen.findByTestId("body-preview")).findByRole(
      "button",
      { name: "Root" },
    );
    expect(wikilink).toHaveClass("wikilink");
    fireEvent.click(wikilink);

    await waitFor(() =>
      expect(screen.getByTestId("detail-title")).toHaveTextContent("Root"),
    );
    expect(client.taskDetail).toHaveBeenCalledWith("root1");
  });

  it("keeps the first sight of a task free of forms, disk metadata, and empty relationships", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");

    // Read mode, not a form.
    expect(screen.getByTestId("detail-title")).toHaveTextContent("Child");
    expect(screen.queryByLabelText("Task title")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Priority")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Due date")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Tags")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Move to project")).not.toBeInTheDocument();
    // Disk identity stays out of first sight.
    expect(screen.queryByText(/child1\.md/)).not.toBeInTheDocument();
    expect(screen.queryByText(/demo · child1/)).not.toBeInTheDocument();
    // Empty relationship panels are absent; the populated one is a disclosure.
    expect(screen.queryByRole("button", { name: /Backlinks/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Sub-tasks/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Links/ })).toBeInTheDocument();
    // The secondary triggers exist but stay collapsed.
    expect(screen.getByRole("button", { name: "Properties" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.getByRole("button", { name: "More actions" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("renders populated metadata as quiet text and reveals property forms on request", async () => {
    const client = appClient();
    vi.mocked(client.taskDetail).mockImplementation(async (id: string) =>
      id === "child1"
        ? makeDetail("child1", {
            title: "Child",
            body: "see [[root1]]",
            priority: "high",
            due: "2030-03-02",
            tags: ["ops", "urgent"],
            links: [{ id: "root1", title: "Root" }],
          })
        : makeDetail("root1", { title: "Root", body: "root body" }),
    );
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");

    const meta = screen.getByTestId("detail-meta");
    expect(meta).toHaveTextContent("High");
    expect(meta).toHaveTextContent("Due Mar 2");
    expect(meta).toHaveTextContent("#ops");
    expect(meta).toHaveTextContent("#urgent");
    // Populated metadata does not turn into a form at first sight.
    expect(screen.queryByLabelText("Priority")).not.toBeInTheDocument();
    expect(screen.getByTestId("detail-title")).toHaveTextContent("Child");

    fireEvent.click(screen.getByRole("button", { name: "Properties" }));
    expect(screen.getByLabelText("Task title")).toHaveValue("Child");
    expect(screen.getByLabelText("Priority")).toHaveValue("high");
    expect(screen.getByLabelText("Due date")).toHaveValue("2030-03-02");
    expect(screen.getByLabelText("Tags")).toHaveValue("ops, urgent");
  });

  it("keeps every secondary task action reachable under More actions", async () => {
    const client = appClient();
    vi.mocked(client.shiftRank).mockResolvedValue(snapshot());
    vi.mocked(client.setState).mockResolvedValue(snapshot());
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    const more = screen.getByRole("button", { name: "More actions" });

    expect(screen.queryByRole("button", { name: "Delete…" })).not.toBeInTheDocument();
    fireEvent.click(more);
    expect(more).toHaveAttribute("aria-expanded", "true");

    expect(screen.getByRole("button", { name: "Copy metadata" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy task" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move up" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Move down" })).toBeInTheDocument();
    expect(screen.getByLabelText("Move to project")).toBeInTheDocument();
    expect(screen.getByLabelText("Move under parent")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel task" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete…" })).toBeInTheDocument();
    expect(screen.getByText("/tmp/store/child1.md")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Copy metadata" }));
    await waitFor(() => expect(client.copyTask).toHaveBeenCalledWith("child1", false));
    fireEvent.click(screen.getByRole("button", { name: "Move up" }));
    await waitFor(() => expect(client.shiftRank).toHaveBeenCalledWith("child1", -1));
    fireEvent.click(screen.getByRole("button", { name: "Cancel task" }));
    await waitFor(() => expect(client.setState).toHaveBeenCalledWith("child1", "cancelled"));
  });

  it("collapses the filter form but keeps an active summary and Clear visible", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    const filters = screen.getByRole("button", { name: "Filters" });
    expect(filters).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByLabelText("Filter state")).not.toBeInTheDocument();

    fireEvent.click(filters);
    expect(filters).toHaveAttribute("aria-expanded", "true");
    fireEvent.change(screen.getByLabelText("Filter state"), { target: { value: "open" } });
    await waitFor(() =>
      expect(client.search).toHaveBeenCalledWith({
        query: "",
        state: "open",
        tag: "",
        priority: undefined,
        dueToday: false,
      }),
    );

    fireEvent.click(filters);
    expect(filters).toHaveAttribute("aria-expanded", "false");
    // The panel stays mounted (inert) until its close transition ends.
    await waitFor(() => expect(screen.queryByLabelText("Filter state")).not.toBeInTheDocument());
    expect(screen.getByTestId("filter-summary")).toHaveTextContent("open");

    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    await waitFor(() => expect(screen.queryByTestId("filter-summary")).not.toBeInTheDocument());
    expect(screen.queryByLabelText("Filter state")).not.toBeInTheDocument();
  });

  it("opens a focused composer from New task, defaulting to the project root", async () => {
    const client = appClient();
    vi.mocked(client.addTask).mockResolvedValue(snapshot());
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    expect(screen.queryByLabelText("New task title")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "New task" }));

    const title = screen.getByLabelText("New task title");
    await waitFor(() => expect(title).toHaveFocus());
    expect(screen.getByLabelText("New task placement")).toHaveValue("root");

    fireEvent.change(title, { target: { value: "Fresh" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(client.addTask).toHaveBeenCalledWith({ title: "Fresh" }));
    await waitFor(() => expect(screen.queryByLabelText("New task title")).not.toBeInTheDocument());
  });

  it("reaches subtask and capture placement from the composer", async () => {
    const client = appClient();
    vi.mocked(client.bootstrap).mockResolvedValue(
      snapshot({ captureTarget: { id: "other1", title: "Other" } }),
    );
    vi.mocked(client.refresh).mockResolvedValue(
      snapshot({ captureTarget: { id: "other1", title: "Other" } }),
    );
    vi.mocked(client.addTask).mockResolvedValue(
      snapshot({ captureTarget: { id: "other1", title: "Other" } }),
    );
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "New task" }));

    expect(screen.getByText("capture to Other")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("New task title"), { target: { value: "Sub" } });
    fireEvent.change(screen.getByLabelText("New task placement"), { target: { value: "child" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() =>
      expect(client.addTask).toHaveBeenCalledWith({ title: "Sub", parentId: "child1" }),
    );

    fireEvent.click(screen.getByRole("button", { name: "New task" }));
    fireEvent.change(screen.getByLabelText("New task title"), { target: { value: "Cap" } });
    fireEvent.change(screen.getByLabelText("New task placement"), { target: { value: "capture" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() =>
      expect(client.addTask).toHaveBeenCalledWith({ title: "Cap", capture: true }),
    );
  });

  it("resets a stale composer placement when the selection changes", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "New task" }));
    fireEvent.change(screen.getByLabelText("New task placement"), { target: { value: "child" } });
    expect(screen.getByLabelText("New task placement")).toHaveValue("child");

    fireEvent.click(screen.getByText("Other"));
    await waitFor(() => expect(screen.getByLabelText("New task placement")).toHaveValue("root"));
  });

  it("opens the composer with Cmd+N, focuses Search with Cmd+F, and closes on Escape", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.keyDown(window, { key: "n", metaKey: true });
    const title = screen.getByLabelText("New task title");
    await waitFor(() => expect(title).toHaveFocus());

    fireEvent.keyDown(title, { key: "Escape" });
    await waitFor(() => expect(screen.queryByLabelText("New task title")).not.toBeInTheDocument());

    fireEvent.keyDown(window, { key: "f", metaKey: true });
    expect(screen.getByLabelText("Search tasks")).toHaveFocus();
  });

  it("closes an open task panel when the selection changes", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    expect(screen.getByRole("button", { name: "Delete…" })).toBeInTheDocument();

    fireEvent.click(within(screen.getByRole("tree", { name: "Projects and tasks" })).getByText("Other"));
    const reopened = await screen.findByRole("button", { name: "More actions" });
    await waitFor(() => expect(reopened).toHaveAttribute("aria-expanded", "false"));
    expect(screen.queryByRole("button", { name: "Delete…" })).not.toBeInTheDocument();
  });

  it("collapses the sidebar to a rail and releases its column width", async () => {
    const client = appClient();
    const { container } = render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    const columns = container.querySelector(".columns");
    expect(columns).not.toHaveClass("sidebar-collapsed");

    fireEvent.keyDown(window, { key: "b", metaKey: true });
    expect(await screen.findByRole("button", { name: "Show sidebar" })).toBeInTheDocument();
    expect(columns).toHaveClass("sidebar-collapsed");
  });

  it("completes an open task from the header control", async () => {
    const client = appClient();
    vi.mocked(client.setState).mockResolvedValue(snapshot());
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Mark task done" }));
    await waitFor(() => expect(client.setState).toHaveBeenCalledWith("child1", "done"));
  });

  it("reopens a cancelled task from the header control", async () => {
    const client = appClient();
    vi.mocked(client.taskDetail).mockImplementation(async (id: string) =>
      makeDetail(id, { title: "Child", state: "cancelled", body: "body" }),
    );
    vi.mocked(client.setState).mockResolvedValue(snapshot());
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Reopen task" }));
    await waitFor(() => expect(client.setState).toHaveBeenCalledWith("child1", "open"));
  });

  it("closes the Filters panel when the open project changes", async () => {
    const client = appClient();
    const work = { slug: "work", path: "/tmp/work", name: "Work" };
    vi.mocked(client.bootstrap).mockResolvedValue(snapshot({ projects: [project, work] }));
    vi.mocked(client.openProject).mockResolvedValue(
      makeSnapshot({
        projects: [project, work],
        project: work,
        tree: [node("w1", "Work task")],
        taskCount: 1,
        revision: "rev2",
      }),
    );
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    expect(screen.getByRole("button", { name: "Filters" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    fireEvent.click(screen.getByRole("treeitem", { name: "Work" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Filters" })).toHaveAttribute(
        "aria-expanded",
        "false",
      ),
    );
    // The panel stays mounted (inert) until its close transition ends.
    await waitFor(() => expect(screen.queryByLabelText("Filter state")).not.toBeInTheDocument());
  });

  it("folds and unfolds a subtree with the fold control", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByRole("button", { name: "Collapse Root" }));
    expect(screen.queryByText("Child")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Expand Root" }));
    expect(screen.getByText("Child")).toBeInTheDocument();
  });

  it("registers a project through the native dialog action", async () => {
    const client = appClient();
    vi.mocked(client.registerProject).mockResolvedValue(
      snapshot({ projects: [project, { slug: "work", path: "/tmp/work", name: "work" }] }),
    );
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.click(screen.getByRole("button", { name: "Add Project…" }));
    await screen.findByText("work");
    expect(client.registerProject).toHaveBeenCalled();
  });

  it("never deletes a task on Escape, only on an explicit confirmation", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");

    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    const dialog = await screen.findByTestId("confirm-dialog");
    expect(dialog).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("confirm-dialog")).not.toBeInTheDocument());
    expect(client.deleteTask).not.toHaveBeenCalled();

    // The confirmation owned Escape; the More actions panel is still open.
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    await screen.findByTestId("confirm-dialog");
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(client.deleteTask).toHaveBeenCalledWith("child1"));
  });

  it("keeps a recovery view when the selected task disappears while dirty", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "my draft" },
    });

    vi.mocked(client.refresh).mockResolvedValue(
      snapshot({ tree: [node("other1", "Other")], taskCount: 1, revision: "rev2" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Refresh from disk" }));

    const banner = await screen.findByTestId("missing-task");
    expect(banner).toHaveTextContent("no longer in the project store");
    expect(screen.getByLabelText("Task description")).toHaveValue("my draft");
    expect(screen.getByRole("button", { name: "Copy draft" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Discard draft" })).toBeInTheDocument();
  });

  it("disables the description editor while a save is in flight", async () => {
    const client = appClient();
    const pending = deferred<ReturnType<typeof makeDetail>>();
    vi.mocked(client.saveBody).mockReturnValue(pending.promise);
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "submitted" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(client.saveBody).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByLabelText("Task description")).toBeDisabled());

    // Typing during the save cannot land, so the submitted text is not lost.
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "typed during save" },
    });
    expect(screen.getByLabelText("Task description")).toHaveValue("submitted");

    await act(async () => {
      pending.resolve(makeDetail("child1", { title: "Child", body: "submitted", revision: "rev2" }));
    });
    await waitFor(() =>
      expect(screen.queryByLabelText("Task description")).not.toBeInTheDocument(),
    );
  });

  it("surfaces recovery for a serialized stale save and keeps the draft", async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: "stale_draft",
      message: "the task file changed on disk",
      conflict: { reason: "changed", currentRevision: "rev2" },
    });
    const client = fakeClient({
      bootstrap: vi.fn(async () => snapshot()),
      refresh: vi.fn(async () => snapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Child", body: "body", revision: "rev1" }),
      ),
      saveBody: tauriClient.saveBody,
    });
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "my draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    const banner = await screen.findByTestId("conflict");
    expect(banner).toHaveTextContent("changed on disk");
    expect(within(banner).getByRole("button", { name: "Reload latest" })).toBeInTheDocument();
    expect(within(banner).getByRole("button", { name: "Copy draft" })).toBeInTheDocument();
    expect(screen.getByLabelText("Task description")).toHaveValue("my draft");
    expect(screen.queryByText(/\[object Object\]/)).not.toBeInTheDocument();
  });

  it("shows a serialized non-conflict failure's message, never [object Object]", async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: "io_error",
      message: "could not write the task file",
    });
    const client = fakeClient({
      bootstrap: vi.fn(async () => snapshot()),
      refresh: vi.fn(async () => snapshot()),
      taskDetail: vi.fn(async (id: string) =>
        makeDetail(id, { title: "Child", body: "body", revision: "rev1" }),
      ),
      saveBody: tauriClient.saveBody,
    });
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "my draft" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    const toast = await screen.findByTestId("toast");
    expect(toast).toHaveTextContent("could not write the task file");
    expect(screen.queryByText(/\[object Object\]/)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Task description")).toHaveValue("my draft");
  });

  it("asks before discarding a dirty recovery draft on Close, and keeps it on Cancel", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "recovery text" },
    });

    vi.mocked(client.refresh).mockResolvedValue(
      snapshot({ tree: [node("other1", "Other")], taskCount: 1, revision: "rev2" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Refresh from disk" }));
    await screen.findByTestId("missing-task");

    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    const dialog = await screen.findByTestId("confirm-dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByTestId("confirm-dialog")).not.toBeInTheDocument());
    expect(screen.getByLabelText("Task description")).toHaveValue("recovery text");
    expect(screen.getByTestId("missing-task")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    const discardDialog = await screen.findByTestId("confirm-dialog");
    fireEvent.click(within(discardDialog).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByTestId("missing-task")).not.toBeInTheDocument());
    expect(screen.getByText("No task selected")).toBeInTheDocument();
  });

  it("blocks mutating shortcuts behind an open confirmation", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete…" }));
    await screen.findByTestId("confirm-dialog");

    fireEvent.keyDown(window, { key: "Enter", metaKey: true });
    fireEvent.keyDown(window, { key: "Backspace", metaKey: true });

    expect(client.setState).not.toHaveBeenCalled();
    expect(client.deleteTask).not.toHaveBeenCalled();
    expect(screen.getByTestId("confirm-dialog")).toBeInTheDocument();
  });

  it("ignores a metadata Enter while the editor is locked by a dirty draft", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "dirty" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Properties" }));
    const title = screen.getByLabelText("Task title");
    expect(title).toBeDisabled();
    expect(screen.getByRole("button", { name: "Rename" })).toBeDisabled();
    fireEvent.change(title, { target: { value: "Renamed" } });
    fireEvent.keyDown(title, { key: "Enter" });

    expect(client.setTitle).not.toHaveBeenCalled();
  });
  it("holds Projects and the open Project's Tasks in one sidebar, with no Tasks column", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    const sidebar = screen.getByRole("complementary", { name: "Projects and tasks" });
    expect(within(sidebar).getByRole("treeitem", { name: "demo" })).toBeInTheDocument();
    expect(within(sidebar).getByRole("treeitem", { name: "Root" })).toBeInTheDocument();
    expect(within(sidebar).getByRole("treeitem", { name: "Child" })).toBeInTheDocument();
    expect(within(sidebar).getByRole("treeitem", { name: "Other" })).toBeInTheDocument();
    // The three-column layout is gone: the Tasks column no longer exists.
    expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Task details" })).toBeInTheDocument();
  });

  it("opens another Project from the sidebar only after the dirty guard", async () => {
    const client = appClient();
    const work = { slug: "work", path: "/tmp/work", name: "Work" };
    vi.mocked(client.bootstrap).mockResolvedValue(snapshot({ projects: [project, work] }));
    vi.mocked(client.openProject).mockResolvedValue(
      makeSnapshot({
        projects: [project, work],
        project: work,
        tree: [node("w1", "Work task")],
        taskCount: 1,
        revision: "rev2",
      }),
    );
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.click(screen.getByText("Child"));
    await screen.findByTestId("body-preview");
    fireEvent.click(screen.getByRole("button", { name: "Edit description" }));
    fireEvent.change(screen.getByLabelText("Task description"), {
      target: { value: "my draft" },
    });

    fireEvent.click(screen.getByRole("treeitem", { name: "Work" }));
    const dialog = await screen.findByTestId("confirm-dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByTestId("confirm-dialog")).not.toBeInTheDocument());
    expect(client.openProject).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Task description")).toHaveValue("my draft");

    fireEvent.click(screen.getByRole("treeitem", { name: "Work" }));
    const again = await screen.findByTestId("confirm-dialog");
    fireEvent.click(within(again).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(client.openProject).toHaveBeenCalledWith("work"));
    expect(await screen.findByText("Work task")).toBeInTheDocument();
  });

  it("moves one tree cursor across Project and Task rows without opening a Project", async () => {
    const client = appClient();
    const work = { slug: "work", path: "/tmp/work", name: "Work" };
    vi.mocked(client.bootstrap).mockResolvedValue(snapshot({ projects: [project, work] }));
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    const tree = screen.getByRole("tree", { name: "Projects and tasks" });
    // demo → Root → Child → Other → Work, all in visual order.
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    await waitFor(() =>
      expect(screen.getByRole("treeitem", { name: "Root" })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    );
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    fireEvent.keyDown(tree, { key: "ArrowDown" });

    const workRow = screen.getByRole("treeitem", { name: "Work" });
    expect(tree).toHaveAttribute("aria-activedescendant", workRow.id);
    expect(client.openProject).not.toHaveBeenCalled();

    fireEvent.keyDown(tree, { key: "Enter" });
    await waitFor(() => expect(client.openProject).toHaveBeenCalledWith("work"));
  });

  it("folds the open Project's Tasks away without closing the Project", async () => {
    const client = appClient();
    render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Child");

    fireEvent.click(screen.getByRole("button", { name: "Collapse demo" }));
    expect(screen.queryByText("Child")).not.toBeInTheDocument();
    // The Project stays open: its tools and row are still there.
    expect(screen.getByRole("treeitem", { name: "demo" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.getByRole("button", { name: "New task" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Expand demo" }));
    expect(screen.getByText("Child")).toBeInTheDocument();
  });

  it("focuses Search after Cmd+F from the collapsed rail", async () => {
    const client = appClient();
    const { container } = render(<App client={client} reconcileMs={0} />);
    await screen.findByText("Other");

    fireEvent.keyDown(window, { key: "b", metaKey: true });
    expect(container.querySelector(".columns")).toHaveClass("sidebar-collapsed");

    fireEvent.keyDown(window, { key: "f", metaKey: true });
    await waitFor(() =>
      expect(container.querySelector(".columns")).not.toHaveClass("sidebar-collapsed"),
    );
    await waitFor(() => expect(screen.getByLabelText("Search tasks")).toHaveFocus());
  });
});
