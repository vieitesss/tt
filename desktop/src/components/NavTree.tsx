import {
  Fragment,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";

import { dueTone, formatDue, STATE_GLYPH, STATE_LABEL, todayIso } from "../lib/display";
import { projectRowKey, taskRowKey, type NavRow } from "../lib/tree";
import { Disclosure } from "./Disclosure";
import { Icon } from "./Icon";

export interface NavTreeProps {
  rows: NavRow[];
  openSlug: string | null;
  selectedId: string | null;
  hits: ReadonlySet<string> | null;
  busy: boolean;
  onSelectTask: (id: string) => void;
  onOpenProject: (slug: string) => void;
  onToggleTaskFold: (id: string) => void;
  onToggleProjectFold: () => void;
  onUnregister: (slug: string) => void;
  /** Quiet line shown under the open Project's row when it has no Tasks. */
  emptyHint?: ReactNode;
}

/**
 * The window's single tree: Project rows with the open Project's Tasks nested
 * one level under them. Arrow keys move a cursor through both kinds of row;
 * landing on a Project does not open it (Enter or → does), so browsing the
 * tree never navigates. Rows keep treeitem semantics with aria-level,
 * aria-expanded and aria-selected.
 */
export function NavTree({
  rows,
  openSlug,
  selectedId,
  hits,
  busy,
  onSelectTask,
  onOpenProject,
  onToggleTaskFold,
  onToggleProjectFold,
  onUnregister,
  emptyHint,
}: NavTreeProps) {
  const container = useRef<HTMLDivElement>(null);
  const uid = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const today = todayIso();
  const [cursorKey, setCursorKey] = useState<string | null>(null);

  // The cursor follows an explicit selection (click, wikilink, menu bar) while
  // still being allowed to rest on a Project row that is not open.
  useEffect(() => {
    if (selectedId) setCursorKey(taskRowKey(selectedId));
  }, [selectedId]);

  const fallbackKey = useMemo(() => {
    if (selectedId) return taskRowKey(selectedId);
    if (openSlug) return projectRowKey(openSlug);
    return rows[0]?.key ?? null;
  }, [selectedId, openSlug, rows]);

  const indexOf = (key: string | null): number =>
    key === null ? -1 : rows.findIndex((row) => row.key === key);
  const activeKey = indexOf(cursorKey) === -1 ? fallbackKey : cursorKey;

  /** Move the cursor; landing on a Task selects it, on a Project it only rests. */
  const focusRow = (index: number): void => {
    const clamped = Math.max(0, Math.min(rows.length - 1, index));
    const row = rows[clamped];
    if (!row) return;
    setCursorKey(row.key);
    if (row.kind === "task") onSelectTask(row.node.id);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (rows.length === 0) return;
    const index = indexOf(activeKey);
    const current = index === -1 ? undefined : rows[index];
    switch (event.key) {
      case "ArrowDown":
        focusRow(index + 1);
        break;
      case "ArrowUp":
        focusRow(index - 1);
        break;
      case "Home":
        focusRow(0);
        break;
      case "End":
        focusRow(rows.length - 1);
        break;
      case "ArrowRight":
        if (!current) break;
        if (current.kind === "project") {
          if (current.project.slug !== openSlug) onOpenProject(current.project.slug);
          else if (!current.expanded) onToggleProjectFold();
          else focusRow(index + 1);
        } else if (current.node.hasChildren && !current.expanded) {
          onToggleTaskFold(current.node.id);
        } else {
          focusRow(index + 1);
        }
        break;
      case "ArrowLeft":
        if (!current) break;
        if (current.kind === "project") {
          if (current.project.slug === openSlug && current.expanded) onToggleProjectFold();
        } else if (current.node.hasChildren && current.expanded) {
          onToggleTaskFold(current.node.id);
        } else if (current.node.parentId) {
          setCursorKey(taskRowKey(current.node.parentId));
          onSelectTask(current.node.parentId);
        } else if (openSlug) {
          setCursorKey(projectRowKey(openSlug));
        }
        break;
      case "Enter":
      case " ":
        if (!current) break;
        if (current.kind === "project") onOpenProject(current.project.slug);
        else onSelectTask(current.node.id);
        break;
      default:
        return;
    }
    event.preventDefault();
  };

  const domId = (key: string): string => `${uid}-${key}`;
  // Only the open Project has Task rows, so any Task row means it is not empty.
  const openHasTasks = rows.some((row) => row.kind === "task");

  return (
    <div
      className="tree"
      role="tree"
      aria-label="Projects and tasks"
      tabIndex={0}
      ref={container}
      aria-activedescendant={activeKey ? domId(activeKey) : undefined}
      onKeyDown={onKeyDown}
      onMouseDown={() => container.current?.focus()}
    >
      {rows.map((row) => {
        const active = row.key === activeKey;
        if (row.kind === "project") {
          const isOpen = row.project.slug === openSlug;
          const hint =
            isOpen && row.expanded && !openHasTasks && emptyHint !== undefined ? (
              <div className="tree-empty nav-empty" role="presentation">
                {emptyHint}
              </div>
            ) : null;
          return (
            <Fragment key={row.key}>
              <div
                id={domId(row.key)}
                role="treeitem"
                aria-level={1}
                // The tree has one selection: the selected Task, or the open
                // Project when no Task is selected. The open Project row is
                // marked current and expanded as well as tinted.
                aria-selected={isOpen && selectedId === null}
                aria-current={isOpen ? "true" : undefined}
                aria-expanded={isOpen ? row.expanded : undefined}
                aria-label={row.project.name}
                className={`tree-row project-row${isOpen ? " selected" : ""}${
                  active ? " cursor" : ""
                }`}
                style={{ paddingLeft: "8px" }}
                onClick={() => {
                  setCursorKey(row.key);
                  onOpenProject(row.project.slug);
                }}
              >
                <button
                  type="button"
                  className="fold"
                  aria-label={
                    isOpen
                      ? row.expanded
                        ? `Collapse ${row.project.name}`
                        : `Expand ${row.project.name}`
                      : `Open ${row.project.name}`
                  }
                  onClick={(event) => {
                    event.stopPropagation();
                    setCursorKey(row.key);
                    if (isOpen) onToggleProjectFold();
                    else onOpenProject(row.project.slug);
                  }}
                >
                  <Icon
                    name="chevron-right"
                    size={14}
                    className={isOpen && row.expanded ? "rotated" : undefined}
                  />
                </button>
                <span className="row-title project-name">{row.project.name}</span>
                <span className="spacer" />
                <Disclosure
                  label="⋯"
                  ariaLabel={`Options for ${row.project.name}`}
                  title="Project options"
                  align="end"
                  panelClassName="project-options"
                  resetKey={openSlug}
                >
                  <p className="project-path">{row.project.path}</p>
                  <button
                    type="button"
                    className="danger"
                    disabled={busy}
                    onClick={() => onUnregister(row.project.slug)}
                  >
                    Unregister project…
                  </button>
                </Disclosure>
              </div>
              {hint}
            </Fragment>
          );
        }
        const selected = row.node.id === selectedId;
        const match = hits?.has(row.node.id) ?? false;
        const tags = row.node.tags.slice(0, 2);
        const extraTags = row.node.tags.length - tags.length;
        return (
          <div
            key={row.key}
            id={domId(row.key)}
            role="treeitem"
            aria-level={row.depth + 1}
            aria-selected={selected}
            aria-expanded={row.node.hasChildren ? row.expanded : undefined}
            aria-label={row.node.title}
            className={`tree-row${selected ? " selected" : ""}${match ? " match" : ""}${
              active ? " cursor" : ""
            }`}
            style={{ paddingLeft: `${8 + row.depth * 16}px` }}
            onClick={() => {
              setCursorKey(row.key);
              onSelectTask(row.node.id);
            }}
          >
            {row.node.hasChildren ? (
              <button
                type="button"
                className="fold"
                aria-label={
                  row.expanded ? `Collapse ${row.node.title}` : `Expand ${row.node.title}`
                }
                onClick={(event) => {
                  event.stopPropagation();
                  setCursorKey(row.key);
                  onToggleTaskFold(row.node.id);
                }}
              >
                <Icon
                  name="chevron-right"
                  size={14}
                  className={row.expanded ? "rotated" : undefined}
                />
              </button>
            ) : (
              <span className="fold spacer" aria-hidden="true" />
            )}
            <span className={`glyph state-${row.node.state}`} title={STATE_LABEL[row.node.state]}>
              {STATE_GLYPH[row.node.state]}
            </span>
            <span className="row-title">{row.node.title}</span>
            <span className="row-meta">
              {row.node.priority && (
                <span className={`prio prio-${row.node.priority}`}>{row.node.priority}</span>
              )}
              {row.node.total > 0 && (
                <span className="rollup" title="done / non-cancelled descendants">
                  {row.node.done}/{row.node.total}
                </span>
              )}
              {row.node.due && (
                <span className={`due tone-${dueTone(row.node.due, today)}`} title={row.node.due}>
                  {formatDue(row.node.due)}
                </span>
              )}
              {tags.map((tag) => (
                <span key={tag} className="tag">
                  #{tag}
                </span>
              ))}
              {extraTags > 0 && <span className="tag">+{extraTags}</span>}
            </span>
          </div>
        );
      })}
    </div>
  );
}
