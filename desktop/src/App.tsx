import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";

import type { TtClient } from "./api";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { Icon } from "./components/Icon";
import { IssuesBadge } from "./components/IssuesBadge";
import { Sidebar, type Placement } from "./components/Sidebar";
import { TaskDetails } from "./components/TaskDetails";
import { Toast } from "./components/Toast";
import { useTtApp, type TtApp } from "./hooks/useTtApp";
import { isTauri } from "./lib/tauri";
import { flattenTree, flattenVisible, searchContext, sidebarRows } from "./lib/tree";

export function App({ client, reconcileMs = 1000 }: { client: TtClient; reconcileMs?: number }) {
  const app = useTtApp(client, { reconcileMs });
  const searchRef = useRef<HTMLInputElement>(null);
  const addRef = useRef<HTMLInputElement>(null);
  const [addTitle, setAddTitle] = useState("");
  const [placement, setPlacement] = useState<Placement>("root");
  const [composerOpen, setComposerOpen] = useState(false);
  // Focus Search after the sidebar is shown again, so ⌘F works from the rail.
  const [focusSearch, setFocusSearch] = useState(false);

  const appRef = useRef<TtApp>(app);
  appRef.current = app;
  const dirtyRef = useRef(false);
  dirtyRef.current = app.dirty;

  const searching = app.searchHits !== null;
  const hits = useMemo(
    () => (app.searchHits ? new Set(app.searchHits) : null),
    [app.searchHits],
  );
  const rows = useMemo(() => {
    const tasks = app.searchHits
      ? flattenVisible(app.tree, searchContext(app.tree, app.searchHits))
      : flattenTree(app.tree, app.collapsed);
    return sidebarRows(
      app.projects,
      app.currentProject?.slug ?? null,
      searching || !app.projectCollapsed,
      tasks,
      app.collapsed,
    );
  }, [app.projects, app.tree, app.collapsed, app.currentProject, app.projectCollapsed, app.searchHits, searching]);

  const capture = app.snapshot?.captureTarget;
  const selectedId = app.selectedId;

  // A placement chosen for one selection must never silently apply to another:
  // any selection change falls back to the project root.
  useEffect(() => {
    setPlacement("root");
  }, [selectedId]);

  const closeComposer = () => {
    setComposerOpen(false);
    setAddTitle("");
    setPlacement("root");
  };

  // Cmd shortcuts.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return;
      // While a confirmation is open the modal owns the keyboard: no shortcut
      // may mutate state behind an unanswered prompt.
      if (app.confirmRequest) return;
      switch (event.key.toLowerCase()) {
        case "b":
          event.preventDefault();
          app.setSidebarCollapsed(!app.sidebarCollapsed);
          break;
        case "f":
          event.preventDefault();
          if (app.sidebarCollapsed) {
            app.setSidebarCollapsed(false);
            setFocusSearch(true);
          } else {
            searchRef.current?.focus();
          }
          break;
        case "n":
          event.preventDefault();
          if (app.sidebarCollapsed) app.setSidebarCollapsed(false);
          setComposerOpen(true);
          break;
        case "e":
          event.preventDefault();
          if (!app.draft && app.detail) app.startEdit();
          break;
        case "s":
          event.preventDefault();
          if (app.draft) void app.saveDraft();
          break;
        case "enter":
          event.preventDefault();
          if (app.detail) app.setState(app.detail.state === "done" ? "open" : "done");
          break;
        case "backspace":
          event.preventDefault();
          void app.deleteTask();
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [app]);

  // The composer is focused the moment it appears, from the button or Cmd+N,
  // and again when a collapsed sidebar comes back.
  useEffect(() => {
    if (composerOpen && !app.sidebarCollapsed) addRef.current?.focus();
  }, [composerOpen, app.sidebarCollapsed]);

  // Cmd+F from the collapsed rail: focus Search once the sidebar is back.
  useEffect(() => {
    if (!focusSearch || app.sidebarCollapsed) return;
    searchRef.current?.focus();
    setFocusSearch(false);
  }, [focusSearch, app.sidebarCollapsed]);

  // Closing the window hides the app to the menu bar icon, so every close is
  // intercepted; only a dirty draft asks first, and a declined confirmation
  // leaves the window open.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const stop = await getCurrentWindow().onCloseRequested(async (event) => {
        event.preventDefault();
        if (!dirtyRef.current) {
          await appRef.current.closeMainWindow();
          return;
        }
        if (await appRef.current.confirmDiscardDraft()) {
          await appRef.current.closeMainWindow();
        }
      });
      if (disposed) stop();
      else unlisten = stop;
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const submitAdd = (event: FormEvent) => {
    event.preventDefault();
    const title = addTitle.trim();
    if (!title) return;
    if (placement === "child" && selectedId) {
      void app.addTask({ title, parentId: selectedId });
    } else if (placement === "capture") {
      void app.addTask({ title, capture: true });
    } else {
      void app.addTask({ title });
    }
    closeComposer();
  };

  if (!app.ready) {
    return <div className="loading">Loading projects…</div>;
  }

  return (
    <div className="app">
      <header className="topbar">
        <span className="brand">TT</span>
        {app.externalChange && <span className="flag external">changed on disk</span>}
        {app.busy && <span className="flag">working…</span>}
        <span className="spacer" />
        <button
          type="button"
          className="icon"
          aria-label="Refresh from disk"
          title="Refresh from disk"
          disabled={app.busy}
          onClick={() => void app.refresh()}
        >
          <Icon name="refresh" />
        </button>
        <IssuesBadge issues={app.issues} />
      </header>

      <div className={`columns${app.sidebarCollapsed ? " sidebar-collapsed" : ""}`}>
        {app.sidebarCollapsed ? (
          <div className="rail">
            <button
              type="button"
              className="icon"
              aria-label="Show sidebar"
              title="Show sidebar (⌘B)"
              onClick={() => app.setSidebarCollapsed(false)}
            >
              <Icon name="layout-sidebar-left" />
            </button>
          </div>
        ) : (
          <Sidebar
            projects={app.projects}
            current={app.currentProject ?? null}
            currentEmpty={app.tree.length === 0}
            rows={rows}
            selectedId={app.selectedId}
            hits={hits}
            busy={app.busy}
            searching={searching}
            hitCount={app.searchHits?.length ?? 0}
            search={app.search}
            composer={{
              open: composerOpen,
              title: addTitle,
              placement,
              captureTitle: capture?.title,
            }}
            searchRef={searchRef}
            addRef={addRef}
            actions={{
              openProject: (slug) => void app.openProject(slug),
              register: () => void app.registerProject(),
              unregister: (slug) => void app.unregisterProject(slug),
              collapse: () => app.setSidebarCollapsed(true),
              selectTask: (id) => void app.selectTask(id),
              toggleTaskFold: app.toggleFold,
              toggleProjectFold: app.toggleProjectFold,
              setSearch: app.setSearch,
              clearSearch: app.clearSearch,
              openComposer: () => setComposerOpen(true),
              closeComposer,
              setComposerTitle: setAddTitle,
              setComposerPlacement: setPlacement,
              submitComposer: submitAdd,
            }}
          />
        )}

        <TaskDetails app={app} onExternal={(url) => void app.openExternal(url)} />
      </div>

      <ConfirmDialog request={app.confirmRequest} onResolve={app.resolveConfirm} />

      <Toast toast={app.toast} onDismiss={app.dismissToast} />
    </div>
  );
}
