import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { flattenTree, sidebarRows, type NavRow } from "../lib/tree";
import { node } from "../test/fixtures";
import { NavTree } from "./NavTree";

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
  node("other1", "Other"),
];

function makeRows(openExpanded = true, openSlug: string | null = "demo"): NavRow[] {
  return sidebarRows(projects, openSlug, openExpanded, flattenTree(tree, new Set()), new Set());
}

function renderTree(overrides: Partial<Parameters<typeof NavTree>[0]> = {}) {
  const props = {
    rows: makeRows(),
    openSlug: "demo",
    selectedId: null,
    hits: null,
    busy: false,
    onSelectTask: vi.fn(),
    onOpenProject: vi.fn(),
    onToggleTaskFold: vi.fn(),
    onToggleProjectFold: vi.fn(),
    onUnregister: vi.fn(),
    ...overrides,
  };
  const view = render(<NavTree {...props} />);
  return { props, view };
}

const treeEl = () => screen.getByRole("tree", { name: "Projects and tasks" });
const activeId = () => treeEl().getAttribute("aria-activedescendant");

describe("NavTree", () => {
  it("renders Project and Task rows as one tree with levels and expansion", () => {
    renderTree();

    expect(screen.getByRole("treeitem", { name: "demo" })).toHaveAttribute("aria-level", "1");
    expect(screen.getByRole("treeitem", { name: "demo" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("treeitem", { name: "Work" })).toHaveAttribute("aria-level", "1");
    expect(screen.getByRole("treeitem", { name: "Work" })).not.toHaveAttribute("aria-expanded");
    // Tasks are nested one level under the open Project, subtasks one deeper.
    expect(screen.getByRole("treeitem", { name: "Root" })).toHaveAttribute("aria-level", "2");
    expect(screen.getByRole("treeitem", { name: "Child" })).toHaveAttribute("aria-level", "3");
    expect(screen.getByRole("treeitem", { name: "Other" })).toHaveAttribute("aria-level", "2");
  });

  it("keeps one selection: the open Project until a Task is selected", () => {
    const { view } = renderTree();

    // With no Task selected the open Project row is the selected, current row.
    const demo = screen.getByRole("treeitem", { name: "demo" });
    expect(demo).toHaveAttribute("aria-selected", "true");
    expect(demo).toHaveAttribute("aria-current", "true");
    view.unmount();

    renderTree({ selectedId: "child1" });
    expect(screen.getByRole("treeitem", { name: "demo" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
    expect(screen.getByRole("treeitem", { name: "demo" })).toHaveAttribute(
      "aria-current",
      "true",
    );
    expect(screen.getByRole("treeitem", { name: "Child" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("moves onto a Project row without opening it, and opens it on Enter", () => {
    const { props } = renderTree({ rows: makeRows(false), onOpenProject: vi.fn() });

    // The cursor starts on the open Project; one step down is the other Project.
    expect(activeId()).toBe(screen.getByRole("treeitem", { name: "demo" }).id);
    fireEvent.keyDown(treeEl(), { key: "ArrowDown" });

    const work = screen.getByRole("treeitem", { name: "Work" });
    expect(activeId()).toBe(work.id);
    expect(props.onOpenProject).not.toHaveBeenCalled();
    expect(work).toHaveAttribute("aria-selected", "false");

    fireEvent.keyDown(treeEl(), { key: "Enter" });
    expect(props.onOpenProject).toHaveBeenCalledWith("work");
  });

  it("moves onto a Task row by selecting it", () => {
    const { props } = renderTree();

    fireEvent.keyDown(treeEl(), { key: "ArrowDown" });

    expect(props.onSelectTask).toHaveBeenCalledWith("root1");
    expect(props.onOpenProject).not.toHaveBeenCalled();
  });

  it("expands a collapsed open Project with ArrowRight, and opens another Project", () => {
    const folded = renderTree({ rows: makeRows(false) });
    fireEvent.keyDown(treeEl(), { key: "ArrowRight" });
    expect(folded.props.onToggleProjectFold).toHaveBeenCalledTimes(1);
    expect(folded.props.onOpenProject).not.toHaveBeenCalled();
    folded.view.unmount();

    const { props } = renderTree();
    fireEvent.keyDown(treeEl(), { key: "ArrowDown" });
    fireEvent.keyDown(treeEl(), { key: "ArrowDown" });
    fireEvent.keyDown(treeEl(), { key: "ArrowDown" });
    fireEvent.keyDown(treeEl(), { key: "ArrowDown" });
    // demo → Root → Child → Other → Work
    expect(activeId()).toBe(screen.getByRole("treeitem", { name: "Work" }).id);
    fireEvent.keyDown(treeEl(), { key: "ArrowRight" });
    expect(props.onOpenProject).toHaveBeenCalledWith("work");
  });

  it("walks a root Task back to its Project row on ArrowLeft without opening it", () => {
    const { props } = renderTree();

    fireEvent.click(screen.getByRole("treeitem", { name: "Other" }));
    expect(props.onSelectTask).toHaveBeenCalledWith("other1");

    fireEvent.keyDown(treeEl(), { key: "ArrowLeft" });

    expect(activeId()).toBe(screen.getByRole("treeitem", { name: "demo" }).id);
    expect(props.onOpenProject).not.toHaveBeenCalled();
  });

  it("selects the parent Task on ArrowLeft and folds an expanded Task", () => {
    const { props } = renderTree();

    fireEvent.click(screen.getByRole("treeitem", { name: "Child" }));
    fireEvent.keyDown(treeEl(), { key: "ArrowLeft" });
    expect(props.onSelectTask).toHaveBeenLastCalledWith("root1");

    fireEvent.click(screen.getByRole("treeitem", { name: "Root" }));
    fireEvent.keyDown(treeEl(), { key: "ArrowLeft" });
    expect(props.onToggleTaskFold).toHaveBeenCalledWith("root1");
  });

  it("keeps task folding and folding the open Project's tasks reachable", async () => {
    const user = userEvent.setup();
    const { props } = renderTree();

    await user.click(screen.getByRole("button", { name: "Collapse Root" }));
    expect(props.onToggleTaskFold).toHaveBeenCalledWith("root1");

    await user.click(screen.getByRole("button", { name: "Collapse demo" }));
    expect(props.onToggleProjectFold).toHaveBeenCalled();
  });

  it("rotates one chevron in place instead of swapping glyphs", async () => {
    const user = userEvent.setup();
    const { props, view } = renderTree();

    const expanded = screen.getByRole("button", { name: "Collapse Root" });
    expect(expanded.querySelector("svg")).toHaveClass("rotated");
    await user.click(expanded);
    expect(props.onToggleTaskFold).toHaveBeenCalledWith("root1");
    view.rerender(
      <NavTree
        {...props}
        rows={sidebarRows(
          projects,
          "demo",
          true,
          flattenTree(tree, new Set(["root1"])),
          new Set(["root1"]),
        )}
      />,
    );
    expect(screen.getByRole("button", { name: "Expand Root" }).querySelector("svg")).not.toHaveClass(
      "rotated",
    );
  });

  it("keeps unregister behind a Project's options disclosure and closes it on navigation", async () => {
    const user = userEvent.setup();
    const { props, view } = renderTree();

    const options = screen.getByRole("button", { name: "Options for demo" });
    expect(options).toHaveAttribute("aria-expanded", "false");
    await user.click(options);
    expect(screen.getByText("/tmp/demo")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Unregister project…" }));
    expect(props.onUnregister).toHaveBeenCalledWith("demo");

    // The open Project changing closes a panel left open on the old context.
    expect(screen.getByText("/tmp/demo")).toBeInTheDocument();
    view.rerender(<NavTree {...props} openSlug="work" />);
    // The panel stays mounted (inert) until its close transition ends.
    await waitFor(() => expect(screen.queryByText("/tmp/demo")).not.toBeInTheDocument());
  });
});
