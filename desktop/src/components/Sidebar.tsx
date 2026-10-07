import type { FormEvent, RefObject } from "react";

import type { SearchState } from "../hooks/useTtApp";
import type { NavRow } from "../lib/tree";
import type { Priority, Project, TaskState } from "../types";
import { Disclosure } from "./Disclosure";
import { Icon } from "./Icon";
import { NavTree } from "./NavTree";

/** Where a new Task goes: the project root, under the selected Task, or capture. */
export type Placement = "root" | "child" | "capture";

export interface SidebarComposer {
  open: boolean;
  title: string;
  placement: Placement;
  /** Title of the project's capture target, when it has one. */
  captureTitle?: string;
}

export interface SidebarActions {
  openProject: (slug: string) => void;
  register: () => void;
  unregister: (slug: string) => void;
  collapse: () => void;
  selectTask: (id: string) => void;
  toggleTaskFold: (id: string) => void;
  toggleProjectFold: () => void;
  setSearch: (partial: Partial<SearchState>) => void;
  clearSearch: () => void;
  openComposer: () => void;
  closeComposer: () => void;
  setComposerTitle: (title: string) => void;
  setComposerPlacement: (placement: Placement) => void;
  submitComposer: (event: FormEvent) => void;
}

export interface SidebarProps {
  projects: Project[];
  /** The one Project the main window has open, if any. */
  current: Project | null;
  /** True when the open Project has no Tasks at all. */
  currentEmpty: boolean;
  rows: NavRow[];
  selectedId: string | null;
  hits: ReadonlySet<string> | null;
  busy: boolean;
  searching: boolean;
  hitCount: number;
  search: SearchState;
  composer: SidebarComposer;
  searchRef: RefObject<HTMLInputElement | null>;
  addRef: RefObject<HTMLInputElement | null>;
  actions: SidebarActions;
}

/**
 * The window's only sidebar: Projects with the open Project's Tasks nested
 * under them, plus the open Project's search, filters and New task. Everything
 * secondary stays behind its disclosure, as in the task-first redesign.
 */
export function Sidebar({
  projects,
  current,
  currentEmpty,
  rows,
  selectedId,
  hits,
  busy,
  searching,
  hitCount,
  search,
  composer,
  searchRef,
  addRef,
  actions,
}: SidebarProps) {
  const filterSummary = [
    search.state ? `state: ${search.state}` : "",
    search.priority ? `priority: ${search.priority}` : "",
    search.tag.trim() ? `#${search.tag.trim()}` : "",
    search.dueToday ? "due today" : "",
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <aside className="sidebar" aria-label="Projects and tasks">
      <div className="sidebar-head">
        <h2>Projects</h2>
        <button
          type="button"
          className="icon"
          aria-label="Collapse sidebar"
          onClick={actions.collapse}
          title="Collapse sidebar (⌘B)"
        >
          <Icon name="layout-sidebar-left" />
        </button>
      </div>

      {current && (
        <div className="sidebar-tools">
          <div className="panel-head">
            <h3 className="tools-title">{current.name}</h3>
            <span className="spacer" />
            <button type="button" className="primary" onClick={actions.openComposer}>
              New task
            </button>
          </div>

          <div className="tree-tools">
            <input
              ref={searchRef}
              className="search-input"
              aria-label="Search tasks"
              placeholder="Search"
              value={search.query}
              onChange={(event) => actions.setSearch({ query: event.target.value })}
              onKeyDown={(event) => {
                if (event.key === "Escape") actions.clearSearch();
              }}
            />
            <Disclosure
              label="Filters"
              title="State, priority, tag, due"
              align="end"
              resetKey={current.slug}
              panelClassName="filter-panel"
            >
              <div className="filter-form">
                <label>
                  State
                  <select
                    aria-label="Filter state"
                    value={search.state ?? ""}
                    onChange={(event) =>
                      actions.setSearch({
                        state: (event.target.value || undefined) as TaskState | undefined,
                      })
                    }
                  >
                    <option value="">any</option>
                    <option value="open">open</option>
                    <option value="done">done</option>
                    <option value="cancelled">cancelled</option>
                  </select>
                </label>
                <label>
                  Priority
                  <select
                    aria-label="Filter priority"
                    value={search.priority ?? ""}
                    onChange={(event) =>
                      actions.setSearch({
                        priority: (event.target.value || undefined) as Priority | undefined,
                      })
                    }
                  >
                    <option value="">any</option>
                    <option value="high">high</option>
                    <option value="med">med</option>
                    <option value="low">low</option>
                  </select>
                </label>
                <label>
                  Tag
                  <input
                    aria-label="Filter tag"
                    placeholder="#tag"
                    value={search.tag}
                    onChange={(event) => actions.setSearch({ tag: event.target.value })}
                  />
                </label>
                <label className="inline">
                  <input
                    type="checkbox"
                    aria-label="Filter due today"
                    checked={search.dueToday}
                    onChange={(event) => actions.setSearch({ dueToday: event.target.checked })}
                  />
                  Due today
                </label>
              </div>
            </Disclosure>
          </div>

          {searching && (
            <div className="tree-status">
              {filterSummary && (
                <span className="filter-summary" data-testid="filter-summary">
                  {filterSummary}
                </span>
              )}
              <span className="muted" data-testid="hit-count">
                {hitCount} match(es)
              </span>
              <button type="button" className="quiet" onClick={actions.clearSearch}>
                Clear
              </button>
            </div>
          )}

          {composer.open && (
            <form className="composer" onSubmit={actions.submitComposer}>
              <input
                ref={addRef}
                aria-label="New task title"
                placeholder="New task title"
                value={composer.title}
                onChange={(event) => actions.setComposerTitle(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    actions.closeComposer();
                  }
                }}
              />
              <select
                aria-label="New task placement"
                value={composer.placement}
                onChange={(event) =>
                  actions.setComposerPlacement(event.target.value as Placement)
                }
              >
                <option value="root">at project root</option>
                <option value="child" disabled={!selectedId}>
                  under selected task
                </option>
                {composer.captureTitle && (
                  <option value="capture">capture to {composer.captureTitle}</option>
                )}
              </select>
              <button
                type="submit"
                className="primary"
                disabled={!composer.title.trim() || busy}
              >
                Add task
              </button>
              <button type="button" onClick={actions.closeComposer}>
                Cancel
              </button>
            </form>
          )}
        </div>
      )}

      <NavTree
        rows={rows}
        openSlug={current?.slug ?? null}
        selectedId={selectedId}
        hits={hits}
        busy={busy}
        onSelectTask={actions.selectTask}
        onOpenProject={actions.openProject}
        onToggleTaskFold={actions.toggleTaskFold}
        onToggleProjectFold={actions.toggleProjectFold}
        onUnregister={actions.unregister}
        emptyHint={
          current && currentEmpty ? (
            <span className="muted">No tasks. Use New task to add one.</span>
          ) : undefined
        }
      />

      {projects.length === 0 && <p className="muted">No projects yet. Add a folder to begin.</p>}

      <button type="button" className="block" disabled={busy} onClick={actions.register}>
        Add Project…
      </button>
    </aside>
  );
}
