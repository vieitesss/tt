import { createRef } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { flattenTree, sidebarRows, type NavRow } from "../lib/tree";
import { node } from "../test/fixtures";
import { Sidebar, type Placement, type SidebarActions } from "./Sidebar";

const projects = [
  { slug: "demo", path: "/tmp/demo", name: "demo" },
  { slug: "work", path: "/tmp/work", name: "Work" },
];

const tree = [
  node("root1", "Root", {
    hasChildren: true,
    total: 1,
    children: [node("child1", "Child", { parentId: "root1" })],
  }),
];

function makeRows(openExpanded = true): NavRow[] {
  return sidebarRows(projects, "demo", openExpanded, flattenTree(tree, new Set()), new Set());
}

function makeActions(): SidebarActions {
  return {
    openProject: vi.fn(),
    register: vi.fn(),
    unregister: vi.fn(),
    collapse: vi.fn(),
    selectTask: vi.fn(),
    toggleTaskFold: vi.fn(),
    toggleProjectFold: vi.fn(),
    setSearch: vi.fn(),
    clearSearch: vi.fn(),
    openComposer: vi.fn(),
    closeComposer: vi.fn(),
    setComposerTitle: vi.fn(),
    setComposerPlacement: vi.fn(),
    submitComposer: vi.fn(),
  };
}

function renderSidebar(overrides: Partial<Parameters<typeof Sidebar>[0]> = {}) {
  const props = {
    projects,
    current: projects[0]!,
    currentEmpty: false,
    rows: makeRows(),
    selectedId: null,
    hits: null,
    busy: false,
    searching: false,
    hitCount: 0,
    search: { query: "", tag: "", dueToday: false },
    composer: { open: false, title: "", placement: "root" as Placement },
    searchRef: createRef<HTMLInputElement>(),
    addRef: createRef<HTMLInputElement>(),
    actions: makeActions(),
    ...overrides,
  };
  const view = render(<Sidebar {...props} />);
  return { props, view };
}

const sidebar = () => screen.getByRole("complementary", { name: "Projects and tasks" });

describe("Sidebar", () => {
  it("holds Projects and the open Project's Tasks in one sidebar landmark", () => {
    renderSidebar();

    expect(within(sidebar()).getByRole("treeitem", { name: "demo" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(within(sidebar()).getByRole("treeitem", { name: "Root" })).toHaveAttribute(
      "aria-level",
      "2",
    );
    expect(within(sidebar()).getByRole("treeitem", { name: "Child" })).toHaveAttribute(
      "aria-level",
      "3",
    );
    expect(within(sidebar()).getByRole("treeitem", { name: "Work" })).toBeInTheDocument();
  });

  it("keeps Filters and the New task composer hidden until they are opened", async () => {
    const user = userEvent.setup();
    const { props } = renderSidebar();

    expect(within(sidebar()).getByLabelText("Search tasks")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Filters" })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(screen.queryByLabelText("Filter state")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("New task title")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Filters" }));
    expect(screen.getByRole("button", { name: "Filters" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    fireEvent.change(screen.getByLabelText("Filter state"), { target: { value: "open" } });
    expect(props.actions.setSearch).toHaveBeenCalledWith({ state: "open" });

    await user.click(screen.getByRole("button", { name: "New task" }));
    expect(props.actions.openComposer).toHaveBeenCalled();
    expect(screen.queryByLabelText("New task title")).not.toBeInTheDocument();
  });

  it("runs the composer from the sidebar head", () => {
    const { props } = renderSidebar({
      composer: { open: true, title: "", placement: "root" },
    });

    expect(screen.getByRole("button", { name: "Add task" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("New task title"), { target: { value: "Fresh" } });
    expect(props.actions.setComposerTitle).toHaveBeenCalledWith("Fresh");
    fireEvent.change(screen.getByLabelText("New task placement"), { target: { value: "child" } });
    expect(props.actions.setComposerPlacement).toHaveBeenCalledWith("child");
    fireEvent.submit(screen.getByLabelText("New task title").closest("form")!);
    expect(props.actions.submitComposer).toHaveBeenCalled();
  });

  it("searches and shows an active filter summary while searching", () => {
    const { props } = renderSidebar({
      searching: true,
      hitCount: 2,
      search: { query: "root", state: "open", tag: "ops", dueToday: false },
    });

    fireEvent.change(screen.getByLabelText("Search tasks"), { target: { value: "roo" } });
    expect(props.actions.setSearch).toHaveBeenCalledWith({ query: "roo" });
    expect(screen.getByTestId("filter-summary")).toHaveTextContent("state: open");
    expect(screen.getByTestId("filter-summary")).toHaveTextContent("#ops");
    expect(screen.getByTestId("hit-count")).toHaveTextContent("2 match(es)");
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(props.actions.clearSearch).toHaveBeenCalled();
  });

  it("opens another Project from its row and collapses itself", async () => {
    const user = userEvent.setup();
    const { props } = renderSidebar();

    await user.click(within(sidebar()).getByRole("treeitem", { name: "Work" }));
    expect(props.actions.openProject).toHaveBeenCalledWith("work");

    await user.click(screen.getByRole("button", { name: "Collapse sidebar" }));
    expect(props.actions.collapse).toHaveBeenCalled();
  });

  it("offers Add Project when nothing is registered and keeps the button reachable", async () => {
    const user = userEvent.setup();
    const { props } = renderSidebar({ projects: [], current: null, rows: [] });

    expect(screen.getByText(/No projects yet/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add Project…" }));
    expect(props.actions.register).toHaveBeenCalled();
  });

  it("shows a quiet No tasks line for an open Project without Tasks", () => {
    renderSidebar({
      currentEmpty: true,
      rows: sidebarRows(projects, "demo", true, [], new Set()),
    });

    expect(screen.getByText(/No tasks\./)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New task" })).toBeInTheDocument();
  });
});
