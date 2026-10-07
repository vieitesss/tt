import { describe, expect, it } from "vitest";

import { node } from "../test/fixtures";
import { changedIds, hasDueAttention, menubarRows } from "./rows";

describe("menubarRows", () => {
  const tree = [
    node("a", "Parent", {
      hasChildren: true,
      children: [
        node("a1", "Child open"),
        node("a2", "Child done", { state: "done" }),
        node("a3", "Child cancelled", { state: "cancelled" }),
      ],
    }),
    node("b", "Done root", { state: "done" }),
    node("c", "Cancelled root", { state: "cancelled" }),
  ];

  const ids = (rows: ReturnType<typeof menubarRows>) => rows.map((row) => row.node.id);

  it("shows open tasks only, with depth", () => {
    expect(ids(menubarRows(tree, false, new Set()))).toEqual(["a", "a1"]);
    expect(menubarRows(tree, false, new Set())[1]?.depth).toBe(1);
  });

  it("reveals done and cancelled tasks when finished ones are shown", () => {
    expect(ids(menubarRows(tree, true, new Set()))).toEqual([
      "a",
      "a1",
      "a2",
      "a3",
      "b",
      "c",
    ]);
  });

  it("keeps an ancestor reachable when one of its descendants is shown", () => {
    const onlyDone = [
      node("a", "Parent", {
        hasChildren: true,
        children: [node("a2", "Child done", { state: "done" })],
      }),
    ];
    // "a" is not open, but its done child is; the parent has to come along.
    expect(ids(menubarRows(onlyDone, false, new Set()))).toEqual(["a"]);
  });

  it("hides the children of a folded task and only reports folds that exist", () => {
    const rows = menubarRows(tree, false, new Set(["a"]));
    expect(ids(rows)).toEqual(["a"]);
    expect(rows[0]?.node.hasChildren).toBe(true);
    // A folded leaf keeps no children and no disclosure.
    const foldedLeaf = menubarRows(tree, false, new Set(["a1"]));
    expect(foldedLeaf.every((row) => row.node.id !== "a1" || !row.node.hasChildren)).toBe(true);
  });
});

describe("hasDueAttention", () => {
  const today = "2024-03-10";

  it("lights for open tasks due today or earlier, anywhere in the tree", () => {
    expect(
      hasDueAttention([node("a", "Overdue", { due: "2024-03-09" })], today),
    ).toBe(true);
    expect(hasDueAttention([node("a", "Today", { due: today })], today)).toBe(true);
    expect(
      hasDueAttention(
        [
          node("a", "Parent", {
            hasChildren: true,
            children: [node("a1", "Nested", { due: "2024-03-01" })],
          }),
        ],
        today,
      ),
    ).toBe(true);
  });

  it("stays dark for future dues, missing dues, and finished tasks", () => {
    expect(hasDueAttention([node("a", "Later", { due: "2024-03-11" })], today)).toBe(false);
    expect(hasDueAttention([node("a", "No due")], today)).toBe(false);
    expect(hasDueAttention([], today)).toBe(false);
    expect(
      hasDueAttention([node("a", "Done", { due: "2024-01-01", state: "done" })], today),
    ).toBe(false);
    expect(
      hasDueAttention([node("a", "Cancelled", { due: "2024-01-01", state: "cancelled" })], today),
    ).toBe(false);
  });
});

describe("changedIds", () => {
  it("reports content that an external write changed", () => {
    const before = [node("a", "Before", { due: "2024-03-01" }), node("b", "Same")];
    const after = [node("a", "After", { due: "2024-03-01" }), node("b", "Same")];
    expect([...changedIds(before, after)]).toEqual(["a"]);
  });

  it("reports state, due, priority, tags, and parent changes", () => {
    const before = [node("a", "Task", { due: "2024-03-01", tags: ["x"] })];
    for (const after of [
      [node("a", "Task", { state: "done" })],
      [node("a", "Task", { due: "2024-03-02" })],
      [node("a", "Task", { priority: "high" })],
      [node("a", "Task", { tags: ["y"] })],
      [node("a", "Task", { parentId: "p" })],
    ]) {
      expect([...changedIds(before, after)]).toEqual(["a"]);
    }
  });

  it("ignores tasks that only appeared or disappeared", () => {
    const before = [node("a", "One")];
    expect([...changedIds(before, [])]).toEqual([]);
    expect([...changedIds([], [node("b", "New")])]).toEqual([]);
  });
});
