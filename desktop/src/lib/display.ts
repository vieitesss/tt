import type { TaskNode, TaskState } from "../types";

export const STATE_GLYPH: Record<TaskState, string> = {
  open: "○",
  done: "●",
  cancelled: "—",
};

export const STATE_LABEL: Record<TaskState, string> = {
  open: "Open",
  done: "Done",
  cancelled: "Cancelled",
};

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

/** `YYYY-MM-DD` to a short quiet label; never uses a locale-dependent parser. */
export function formatDue(due: string): string {
  const parts = due.split("-");
  if (parts.length !== 3) return due;
  const month = Number(parts[1]);
  const day = Number(parts[2]);
  if (!Number.isFinite(month) || !Number.isFinite(day) || month < 1 || month > 12) return due;
  return `${MONTHS[month - 1]} ${day}`;
}

export type DueTone = "overdue" | "today" | "future";

/** Compare a task due date to an injected ISO `today`. */
export function dueTone(due: string, today: string): DueTone {
  if (due < today) return "overdue";
  if (due === today) return "today";
  return "future";
}

export function todayIso(now: Date = new Date()): string {
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${now.getFullYear()}-${month}-${day}`;
}

/** Self plus every descendant id, for "cannot reparent into own subtree". */
export function subtreeIds(tree: TaskNode[], id: string): Set<string> {
  const found = new Set<string>();
  const walk = (nodes: TaskNode[]): boolean => {
    for (const node of nodes) {
      if (node.id === id) {
        const collect = (current: TaskNode): void => {
          found.add(current.id);
          current.children.forEach(collect);
        };
        collect(node);
        return true;
      }
      if (walk(node.children)) return true;
    }
    return false;
  };
  walk(tree);
  return found;
}
