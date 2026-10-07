import { describe, expect, it } from "vitest";

import {
  ancestorsOf,
  collectTasks,
  flattenTree,
  flattenVisible,
  searchContext,
  sidebarRows,
} from "./tree";
import { node } from "../test/fixtures";

const tree = [
  node("root1", "Root", {
    hasChildren: true,
    total: 2,
    children: [
      node("child1", "Child", {
        parentId: "root1",
        hasChildren: true,
        children: [node("grand1", "Grandchild", { parentId: "child1" })],
      }),
      node("child2", "Sibling", { parentId: "root1" }),
    ],
  }),
  node("root2", "Other root"),
];

describe("tree helpers", () => {
  it("flattens in core pre-order and honours folds", () => {
    expect(flattenTree(tree, new Set()).map((row) => row.node.id)).toEqual([
      "root1",
      "child1",
      "grand1",
      "child2",
      "root2",
    ]);
    expect(flattenTree(tree, new Set(["child1"])).map((row) => row.node.id)).toEqual([
      "root1",
      "child1",
      "child2",
      "root2",
    ]);
    expect(flattenTree(tree, new Set(["root1"])).map((row) => row.node.id)).toEqual([
      "root1",
      "root2",
    ]);
  });

  it("reports depths", () => {
    expect(flattenTree(tree, new Set()).map((row) => row.depth)).toEqual([0, 1, 2, 1, 0]);
  });

  it("walks ancestors nearest first", () => {
    expect(ancestorsOf(tree, "grand1")).toEqual(["child1", "root1"]);
    expect(ancestorsOf(tree, "root1")).toEqual([]);
  });

  it("indexes every task by id", () => {
    expect([...collectTasks(tree).keys()]).toEqual([
      "root1",
      "child1",
      "grand1",
      "child2",
      "root2",
    ]);
  });

  it("keeps search hits and their ancestors visible", () => {
    const visible = searchContext(tree, ["grand1"]);
    expect(visible).toEqual(new Set(["grand1", "child1", "root1"]));
    expect(flattenVisible(tree, visible).map((row) => row.node.id)).toEqual([
      "root1",
      "child1",
      "grand1",
    ]);
    expect(flattenVisible(tree, visible).map((row) => row.depth)).toEqual([0, 1, 2]);
  });
});

describe("sidebarRows", () => {
  const projects = [
    { slug: "demo", path: "/tmp/demo", name: "demo" },
    { slug: "work", path: "/tmp/work", name: "Work" },
  ];

  it("nests the open project's tasks under its row and leaves the others collapsed", () => {
    const rows = sidebarRows(projects, "demo", true, flattenTree(tree, new Set()), new Set());

    expect(rows.map((row) => row.key)).toEqual([
      "project:demo",
      "task:root1",
      "task:child1",
      "task:grand1",
      "task:child2",
      "task:root2",
      "project:work",
    ]);
    expect(rows.map((row) => row.depth)).toEqual([0, 1, 2, 3, 2, 1, 0]);
    expect(rows.map((row) => row.expanded)).toEqual([
      true,
      true,
      true,
      false,
      false,
      false,
      false,
    ]);
  });

  it("keeps the open project's row when its tasks are folded away", () => {
    const rows = sidebarRows(projects, "demo", false, flattenTree(tree, new Set()), new Set());

    expect(rows.map((row) => row.key)).toEqual(["project:demo", "project:work"]);
    expect(rows.map((row) => row.expanded)).toEqual([false, false]);
  });

  it("honours task folds for the nested rows", () => {
    const rows = sidebarRows(projects, "demo", true, flattenTree(tree, new Set(["child1"])), new Set(["child1"]));

    expect(rows.map((row) => row.key)).toEqual([
      "project:demo",
      "task:root1",
      "task:child1",
      "task:child2",
      "task:root2",
      "project:work",
    ]);
    const child = rows.find((row) => row.key === "task:child1");
    expect(child?.expanded).toBe(false);
  });

  it("keeps every task row under the open project when searching, folds ignored", () => {
    const visible = searchContext(tree, ["grand1"]);
    const rows = sidebarRows(projects, "demo", true, flattenVisible(tree, visible), new Set());

    expect(rows.map((row) => row.key)).toEqual([
      "project:demo",
      "task:root1",
      "task:child1",
      "task:grand1",
      "project:work",
    ]);
  });

  it("renders every project when none is open", () => {
    const rows = sidebarRows(projects, null, false, [], new Set());

    expect(rows.map((row) => row.key)).toEqual(["project:demo", "project:work"]);
    expect(rows.every((row) => row.kind === "project")).toBe(true);
  });
});
