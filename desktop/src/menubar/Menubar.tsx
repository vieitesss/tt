// The menu bar popover: one Project, its open Tasks, and inline task detail.
//
// It is deliberately small: a header with the Project switcher, a quick-add
// field, the Task rows, and a footer. Everything heavier (tags, priority, due,
// moving, deleting) lives in the main window behind "Open in TT".

import { useEffect, useRef, useState, type KeyboardEvent } from "react";

import type { TtClient } from "../api";
import { Icon } from "../components/Icon";
import { MarkdownPreview } from "../components/MarkdownPreview";
import { usePresence } from "../hooks/useExit";
import { dueTone, formatDue, STATE_LABEL, todayIso } from "../lib/display";
import type { MenubarHost } from "./host";
import { useMenubar, type MenubarApp, type UseMenubarOptions } from "./useMenubar";

export interface MenubarProps {
  client: TtClient;
  host: MenubarHost;
  options?: UseMenubarOptions;
}

export function Menubar({ client, host, options }: MenubarProps) {
  const app = useMenubar(client, host, options);
  const [title, setTitle] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const switcher = usePresence(switcherOpen);
  const switcherRef = useRef<HTMLDivElement>(null);
  const keepRef = useRef<HTMLButtonElement>(null);
  const addRef = useRef<HTMLInputElement>(null);
  const today = todayIso();

  // A Project change (or a switch request) closes the switcher, so its list
  // never leaks across Projects.
  useEffect(() => {
    setSwitcherOpen(false);
  }, [app.project?.slug]);

  // Hiding the popover closes the switcher too, so it never survives a reopen.
  useEffect(() => {
    if (!app.visible) setSwitcherOpen(false);
  }, [app.visible]);

  // Outside pointer interaction dismisses the list.
  useEffect(() => {
    if (!switcherOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!switcherRef.current?.contains(event.target as Node)) setSwitcherOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [switcherOpen]);

  // The discard guard is fail-closed: Keep owns the focus the moment it opens.
  useEffect(() => {
    if (app.guard) keepRef.current?.focus();
  }, [app.guard]);

  const appRef = useRef(app);
  appRef.current = app;
  const activeRef = useRef(activeId);
  activeRef.current = activeId;
  const switcherOpenRef = useRef(switcherOpen);
  switcherOpenRef.current = switcherOpen;

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      const current = appRef.current;
      // While the guard is up nothing but its own buttons may answer it, and
      // Escape must not hide the popover behind an unanswered prompt.
      if (current.guard) return;
      const target = event.target as HTMLElement | null;
      const tag = target?.tagName;
      const typing = tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable === true;
      // A focused control (checkbox, row button, quick-add field) owns its own
      // activation keys; only the popover itself handles them.
      const ownsKeys = typing || tag === "BUTTON";

      // ⌥1–⌥9: macOS Option changes `event.key`, so the physical key code is
      // what identifies the digit.
      if (event.altKey && /^Digit[1-9]$/.test(event.code)) {
        const project = current.projects[Number(event.code.slice(5)) - 1];
        if (project) {
          event.preventDefault();
          current.openProject(project.slug);
        }
        return;
      }
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        addRef.current?.focus();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        // An open switcher list is the innermost layer: it consumes Escape.
        if (switcherOpenRef.current) {
          setSwitcherOpen(false);
          return;
        }
        // An open draft wins (cancel asks first when it is dirty); then the
        // expanded Task; then nothing is open, so the popover hides.
        if (current.draft) {
          current.cancelEdit();
          return;
        }
        if (current.expandedId) {
          current.collapseExpanded();
          return;
        }
        void current.hide();
        return;
      }
      if (event.key === "/" && !typing) {
        event.preventDefault();
        addRef.current?.focus();
        return;
      }
      if (ownsKeys) return;

      const rows = current.rows;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        if (rows.length === 0) return;
        event.preventDefault();
        const at = rows.findIndex((row) => row.node.id === activeRef.current);
        const next =
          event.key === "ArrowDown"
            ? at === -1
              ? 0
              : Math.min(rows.length - 1, at + 1)
            : at === -1
              ? rows.length - 1
              : Math.max(0, at - 1);
        const row = rows[next];
        if (row) setActiveId(row.node.id);
        return;
      }
      if (event.key === "Enter" || event.key === " ") {
        const row = rows.find((candidate) => candidate.node.id === activeRef.current);
        if (!row) return;
        event.preventDefault();
        current.expand(row.node.id);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const submit = (asSubtask: boolean) => {
    const value = title.trim();
    if (!value) return;
    app.quickAdd(value, asSubtask ? (app.subtaskTargetId ?? undefined) : undefined);
    setTitle("");
  };

  return (
    <div className="menubar" data-shown={app.visible}>
      <header className="mb-head">
        <div className="mb-switcher" ref={switcherRef}>
          <button
            type="button"
            className="mb-project"
            aria-haspopup="listbox"
            aria-expanded={switcherOpen}
            onClick={() => setSwitcherOpen((open) => !open)}
          >
            <span className="mb-project-name">{app.project?.name ?? "No project"}</span>
            <Icon name="chevron-down" size={14} />
          </button>
          {switcher.mounted && (
            <ul
              className={`mb-projects reveal${switcher.leaving ? " leaving" : ""}`}
              role="listbox"
              aria-label="Projects"
              aria-hidden={switcher.leaving || undefined}
              inert={switcher.leaving}
              onTransitionEnd={switcher.onTransitionEnd}
            >
              {app.projects.map((project, index) => (
                <li key={project.slug}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={project.slug === app.project?.slug}
                    onClick={() => {
                      setSwitcherOpen(false);
                      app.openProject(project.slug);
                    }}
                  >
                    <span>{project.name}</span>
                    {index < 9 && <kbd>⌥{index + 1}</kbd>}
                  </button>
                </li>
              ))}
              {app.projects.length === 0 && <li className="muted">No projects registered</li>}
            </ul>
          )}
        </div>
        <span className="spacer" />
        {app.busy && <span className="muted">working…</span>}
        <button
          type="button"
          className="mb-icon"
          aria-label="Refresh from disk"
          title="Refresh from disk"
          onClick={() => void app.refresh()}
        >
          <Icon name="refresh" size={14} />
        </button>
      </header>

      <form
        className="mb-add"
        onSubmit={(event) => {
          event.preventDefault();
          submit(false);
        }}
      >
        <input
          ref={addRef}
          aria-label="Quick add a task"
          placeholder={app.subtaskTargetTitle ? `Add a task · Tab for under ${app.subtaskTargetTitle}` : "Add a task"}
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Tab" && app.subtaskTargetId) {
              event.preventDefault();
              submit(true);
            }
          }}
        />
      </form>
      {app.subtaskTargetTitle && (
        <p className="mb-subtask-hint" data-testid="subtask-hint">
          Tab adds a subtask of <strong>{app.subtaskTargetTitle}</strong>
        </p>
      )}

      {app.error && (
        <div className="mb-error" role="alert">
          <span>{app.error}</span>
          <button type="button" className="mb-icon" aria-label="Dismiss" onClick={app.dismissError}>
            ×
          </button>
        </div>
      )}

      {app.orphanedDraft && (
        <section className="mb-conflict mb-orphan" role="alert" aria-label="Task no longer listed">
          <p>
            “{app.orphanedDraft.baselineTitle}” was deleted or moved outside the app. Your draft
            is still here.
          </p>
          <textarea
            aria-label="Unsaved draft"
            readOnly
            value={app.orphanedDraft.text}
          />
          <div className="mb-actions">
            <button type="button" onClick={() => void app.copyDraft()}>
              Copy draft
            </button>
            <button type="button" className="danger" onClick={app.cancelEdit}>
              Discard draft
            </button>
          </div>
        </section>
      )}

      <ul className="mb-rows">
        {app.rows.map((row) => {
          const exiting = app.exiting.some((entry) => entry.row.node.id === row.node.id);
          const expanded = app.expandedId === row.node.id;
          return (
            <li key={row.node.id} className={expanded ? "expanded" : undefined}>
              <div
                className={[
                  "mb-row",
                  `state-${row.node.state}`,
                  expanded ? "active" : "",
                  row.node.id === activeId ? "current" : "",
                  app.highlight.has(row.node.id) ? "changed" : "",
                  exiting ? "leaving" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                style={{ paddingLeft: `${8 + row.depth * 14}px` }}
              >
                {row.node.hasChildren ? (
                  <button
                    type="button"
                    className="mb-fold"
                    aria-label={`${app.collapsed.has(row.node.id) ? "Expand" : "Collapse"} ${row.node.title}`}
                    aria-expanded={!app.collapsed.has(row.node.id)}
                    onClick={() => app.toggleFold(row.node.id)}
                  >
                    <Icon
                      name="chevron-right"
                      size={14}
                      className={app.collapsed.has(row.node.id) ? undefined : "rotated"}
                    />
                  </button>
                ) : (
                  <span className="mb-fold" aria-hidden="true" />
                )}
                <input
                  type="checkbox"
                  className="mb-check"
                  aria-label={
                    row.node.state === "done"
                      ? `Reopen ${row.node.title}`
                      : `Complete ${row.node.title}`
                  }
                  checked={row.node.state === "done"}
                  onChange={() => app.toggleState(row)}
                />
                <button
                  type="button"
                  className="mb-title"
                  aria-expanded={expanded}
                  onClick={() => app.expand(row.node.id)}
                >
                  {row.node.title}
                </button>
                <span className="mb-meta">
                  {row.node.due && (
                    <span className={`due ${dueTone(row.node.due, today)}`}>
                      {formatDue(row.node.due)}
                    </span>
                  )}
                  {row.node.priority && (
                    <span className={`prio ${row.node.priority}`}>{row.node.priority}</span>
                  )}
                </span>
              </div>

              {expanded && <Expanded app={app} today={today} />}
            </li>
          );
        })}
      </ul>
      {app.rows.length === 0 && (
        <p className="mb-empty muted">
          {app.project ? "No open tasks." : "Pick a Project to see its tasks."}
        </p>
      )}

      <footer className="mb-foot">
        <label className="mb-showdone">
          <input
            type="checkbox"
            aria-label="Show done"
            checked={app.showDone}
            onChange={(event) => app.setShowDone(event.target.checked)}
          />
          Show done
        </label>
        <span className="spacer" />
        {app.undo && (
          <span className="mb-undo" role="status">
            Completed “{app.undo.title}”
            <button type="button" onClick={app.undoComplete}>
              Undo
            </button>
          </span>
        )}
        {!app.undo && (
          <>
            <button type="button" className="mb-foot-action" onClick={() => void host.openDesktop()}>
              Open TT
            </button>
            <button type="button" className="mb-foot-action" onClick={() => void host.quit()}>
              Quit
            </button>
          </>
        )}
      </footer>

      {app.guard && (
        <div className="mb-guard" role="alertdialog" aria-label="Discard changes?">
          <p>{app.guard.message}</p>
          <div className="mb-guard-actions">
            <button ref={keepRef} type="button" onClick={() => app.resolveGuard(false)}>
              Keep
            </button>
            <button
              type="button"
              className="danger"
              onClick={() => app.resolveGuard(true)}
            >
              Discard
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function Expanded({ app, today }: { app: MenubarApp; today: string }) {
  const { detail, draft, conflict } = app;
  const saveOnChord = (event: KeyboardEvent<HTMLElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      void app.saveEdit();
    }
  };

  return (
    <div className="mb-detail">
      {draft ? (
        <div className="mb-edit">
          <label>
            Title
            <input
              aria-label="Task title"
              value={draft.title}
              onChange={(event) => app.setDraftTitle(event.target.value)}
              onKeyDown={saveOnChord}
            />
          </label>
          <label>
            Description
            <textarea
              aria-label="Task description"
              value={draft.text}
              onChange={(event) => app.setDraftText(event.target.value)}
              onKeyDown={saveOnChord}
            />
          </label>
          <div className="mb-actions">
            <button type="button" className="primary" disabled={app.saving} onClick={() => void app.saveEdit()}>
              Save
            </button>
            <button type="button" onClick={app.cancelEdit}>
              Cancel
            </button>
            <span className="muted">⌘↩ saves · Esc cancels</span>
          </div>
        </div>
      ) : (
        <>
          <dl className="mb-facts">
            <div>
              <dt>State</dt>
              <dd>{detail ? STATE_LABEL[detail.state] : "…"}</dd>
            </div>
            {detail?.due && (
              <div>
                <dt>Due</dt>
                <dd className={`due ${dueTone(detail.due, today)}`}>{formatDue(detail.due)}</dd>
              </div>
            )}
            {detail?.priority && (
              <div>
                <dt>Priority</dt>
                <dd className={`prio ${detail.priority}`}>{detail.priority}</dd>
              </div>
            )}
            {detail && detail.tags.length > 0 && (
              <div>
                <dt>Tags</dt>
                <dd>{detail.tags.map((tag) => `#${tag}`).join(" ")}</dd>
              </div>
            )}
          </dl>
          {detail ? (
            <MarkdownPreview
              body={detail.body}
              resolveTitle={app.resolveTitle}
              onNavigate={(id) => app.expand(id)}
              onExternal={(url) => void app.openExternal(url)}
            />
          ) : (
            <p className="muted">Loading…</p>
          )}
          <div className="mb-actions">
            <button type="button" onClick={app.startEdit}>
              Edit
            </button>
            <button type="button" onClick={() => detail && void app.copyTask(detail.id)}>
              Copy task
            </button>
            <button type="button" onClick={() => detail && void app.openInDesktop(detail.id)}>
              Open in TT
            </button>
          </div>
        </>
      )}

      {conflict && (
        <div className="mb-conflict" role="alert">
          <p>
            {conflict.reason === "deleted"
              ? "This task was deleted outside the app. Your draft is still here."
              : "This task changed on disk. Your draft is still here."}
          </p>
          <div className="mb-actions">
            {conflict.reason !== "deleted" && (
              <button type="button" onClick={() => void app.reloadDraft()}>
                Reload latest
              </button>
            )}
            <button type="button" onClick={() => void app.copyDraft()}>
              Copy draft
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
