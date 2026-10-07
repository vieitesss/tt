// The popover's state machine: its own Project, its own draft, and the live
// loop that keeps it current without ever clobbering what the user typed.
//
// Rules the tests pin down (the design doc's safety rules apply here too):
// - a refresh never resets an open draft;
// - a save whose baseline revision is stale keeps the draft and offers
//   Reload / Copy draft, and an externally deleted Task keeps it visible;
// - leaving a dirty draft (Escape, switching Project, expanding another Task)
//   asks first, in a fail-closed inline guard whose Keep is the default;
// - completing a row animates it out and can be undone for a moment;
// - the tray dot follows this Project's own overdue/due-today open Tasks.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { TtError, type TtClient } from "../api";
import { browserPreferences, type PreferenceStore } from "../hooks/useTtApp";
import { todayIso } from "../lib/display";
import { collectTasks, type FlatRow } from "../lib/tree";
import type { ConflictInfo, Project, Snapshot, TaskDetail, TaskState } from "../types";
import type { MenubarHost } from "./host";
import { changedIds, hasDueAttention, menubarRows } from "./rows";

/** Where the popover remembers the Project it last showed. */
export const MENUBAR_PROJECT_KEY = "tt.menubar.lastProject";

/** An open title + description draft, with both baselines it started from. */
export interface MenubarDraft {
  taskId: string;
  projectSlug: string;
  baselineRevision: string;
  baselineTitle: string;
  title: string;
  text: string;
  original: string;
}

/** Anything the user changed and has not saved: title or description. */
function isDirty(draft: MenubarDraft): boolean {
  return draft.text !== draft.original || draft.title !== draft.baselineTitle;
}

/** A pending fail-closed discard confirmation. */
export interface DiscardGuard {
  message: string;
  action: () => void;
}

export interface UseMenubarOptions {
  /** Reconcile interval while the popover is visible. */
  visibleMs?: number;
  /** Reconcile interval while it is hidden (keeps the tray dot current). */
  hiddenMs?: number;
  /** How long a completed row stays on screen before it leaves. */
  completeDelayMs?: number;
  /** How long Undo reopens the last completion. */
  undoMs?: number;
  /** How long an externally changed row stays highlighted. */
  highlightMs?: number;
  preferences?: PreferenceStore;
}

export function useMenubar(
  client: TtClient,
  host: MenubarHost,
  options: UseMenubarOptions = {},
) {
  const {
    visibleMs = 1000,
    hiddenMs = 15000,
    completeDelayMs = 2000,
    undoMs = 5000,
    highlightMs = 1200,
  } = options;
  const preferencesRef = useRef<PreferenceStore | null>(null);
  if (!preferencesRef.current) {
    preferencesRef.current = options.preferences ?? browserPreferences();
  }
  const preferences = preferencesRef.current;

  const [ready, setReady] = useState(false);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [showDone, setShowDone] = useState(false);
  const [visible, setVisible] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<TaskDetail | null>(null);
  const [draft, setDraft] = useState<MenubarDraft | null>(null);
  const [conflict, setConflict] = useState<ConflictInfo | null>(null);
  const [saving, setSaving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [exiting, setExiting] = useState<{ row: FlatRow; index: number }[]>([]);
  const [undo, setUndo] = useState<{ taskId: string; title: string } | null>(null);
  const [highlight, setHighlight] = useState<ReadonlySet<string>>(() => new Set());
  const [guard, setGuard] = useState<DiscardGuard | null>(null);
  const [error, setError] = useState<string | null>(null);

  const clientRef = useRef(client);
  clientRef.current = client;
  const hostRef = useRef(host);
  hostRef.current = host;

  const snapshotRef = useRef<Snapshot | null>(null);
  const draftRef = useRef<MenubarDraft | null>(null);
  const guardRef = useRef<DiscardGuard | null>(null);
  const expandedRef = useRef<string | null>(null);
  // User operations in flight; a poll never races them.
  const opsRef = useRef(0);
  const refreshSeq = useRef(0);
  const detailSeq = useRef(0);
  const busyRef = useRef(false);
  const savingRef = useRef(false);
  // Mutations this UI performed since the last snapshot: they are not the
  // "external write" a row highlight is for.
  const localWrites = useRef(0);
  const timers = useRef<number[]>([]);
  const errorTimer = useRef<number | null>(null);
  const rowsRef = useRef<FlatRow[]>([]);

  /** Schedule work that unmount cancels. */
  const later = useCallback((ms: number, work: () => void) => {
    const id = window.setTimeout(() => {
      timers.current = timers.current.filter((timer) => timer !== id);
      work();
    }, ms);
    timers.current.push(id);
  }, []);

  useEffect(() => {
    return () => {
      for (const timer of timers.current) window.clearTimeout(timer);
      timers.current = [];
    };
  }, []);

  const showError = useCallback(
    (thrown: unknown) => {
      setError(thrown instanceof Error ? thrown.message : String(thrown));
      if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
      errorTimer.current = window.setTimeout(() => setError(null), 4000);
    },
    [],
  );

  const clearDraft = useCallback(() => {
    draftRef.current = null;
    setDraft(null);
    setConflict(null);
  }, []);

  const collapse = useCallback(() => {
    expandedRef.current = null;
    setExpandedId(null);
    setDetail(null);
  }, []);

  const applySnapshot = useCallback(
    (next: Snapshot) => {
      const previous = snapshotRef.current;
      snapshotRef.current = next;
      setSnapshot(next);
      const tracked = expandedRef.current;
      if (tracked && !collectTasks(next.tree).has(tracked)) {
        // The Task vanished outside the app. With a draft, keep the expanded
        // panel so the draft stays reachable; without one, close it.
        if (!draftRef.current) collapse();
      }
      if (previous && previous.revision !== next.revision) {
        if (localWrites.current === 0) {
          const changed = changedIds(previous.tree, next.tree);
          if (changed.size > 0) {
            setHighlight(changed);
            later(highlightMs, () => setHighlight(new Set()));
          }
        }
      }
      localWrites.current = 0;
    },
    [collapse, highlightMs, later],
  );

  const loadDetail = useCallback(
    async (id: string) => {
      const seq = (detailSeq.current += 1);
      try {
        const next = await clientRef.current.taskDetail(id);
        if (seq !== detailSeq.current || expandedRef.current !== id) return;
        setDetail(next);
        if (!draftRef.current) setConflict(null);
      } catch (thrown) {
        showError(thrown);
      }
    },
    [showError],
  );

  const refresh = useCallback(async () => {
    // Never race a user operation: nothing to reconcile mid-flight.
    if (opsRef.current > 0) return;
    const seq = (refreshSeq.current += 1);
    try {
      const next = await clientRef.current.refresh();
      if (seq !== refreshSeq.current) return;
      applySnapshot(next);
      const tracked = expandedRef.current;
      // A draft owns its detail; a refresh may not replace it.
      if (tracked && !draftRef.current && collectTasks(next.tree).has(tracked)) {
        void loadDetail(tracked);
      }
    } catch (thrown) {
      if (seq === refreshSeq.current) showError(thrown);
    }
  }, [applySnapshot, loadDetail, showError]);

  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  const mutate = useCallback(
    async (work: () => Promise<Snapshot>) => {
      if (busyRef.current || savingRef.current) return false;
      busyRef.current = true;
      opsRef.current += 1;
      localWrites.current += 1;
      setBusy(true);
      try {
        const next = await work();
        applySnapshot(next);
        const tracked = expandedRef.current;
        if (tracked && !draftRef.current && collectTasks(next.tree).has(tracked)) {
          void loadDetail(tracked);
        }
        return true;
      } catch (thrown) {
        showError(thrown);
        return false;
      } finally {
        busyRef.current = false;
        opsRef.current -= 1;
        setBusy(false);
      }
    },
    [applySnapshot, loadDetail, showError],
  );

  const openProjectBySlug = useCallback(
    async (slug: string) => {
      opsRef.current += 1;
      setBusy(true);
      try {
        const next = await clientRef.current.openProject(slug);
        preferences.set(MENUBAR_PROJECT_KEY, slug);
        collapse();
        setCollapsed(new Set());
        setExiting([]);
        setUndo(null);
        applySnapshot(next);
      } catch (thrown) {
        showError(thrown);
      } finally {
        opsRef.current -= 1;
        setBusy(false);
      }
    },
    [applySnapshot, collapse, preferences, showError],
  );

  /**
   * Run `action` now when there is nothing to lose, or after the inline
   * discard guard is answered. Only Discard authorizes the action, and an
   * unanswered guard is replaced fail-closed by the next request.
   */
  const withDraftGuard = useCallback(
    (message: string, action: () => void) => {
      const current = draftRef.current;
      if (!current || !isDirty(current)) {
        action();
        return;
      }
      guardRef.current = { message, action };
      setGuard(guardRef.current);
    },
    [],
  );

  const resolveGuard = useCallback((discard: boolean) => {
    const pending = guardRef.current;
    guardRef.current = null;
    setGuard(null);
    // Keep (or Escape/outside) keeps the draft and drops the request.
    if (discard) pending?.action();
  }, []);

  const openProject = useCallback(
    (slug: string) => {
      if (slug === snapshotRef.current?.project?.slug) return;
      withDraftGuard("Discard changes? Switching Project drops the draft.", () => {
        clearDraft();
        collapse();
        void openProjectBySlug(slug);
      });
    },
    [clearDraft, collapse, openProjectBySlug, withDraftGuard],
  );

  // Initial load: the last Project, else the first registered one.
  useEffect(() => {
    void (async () => {
      try {
        const bootstrapped = await clientRef.current.bootstrap();
        applySnapshot(bootstrapped);
        const last = preferences.get(MENUBAR_PROJECT_KEY);
        const target =
          bootstrapped.projects.find((candidate) => candidate.slug === last) ??
          bootstrapped.projects[0];
        if (target) await openProjectBySlug(target.slug);
      } catch (thrown) {
        showError(thrown);
      } finally {
        setReady(true);
      }
    })();
    // Runs once; every helper below only touches stable setters and refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The shell tells the popover when it becomes visible; becoming visible
  // refreshes immediately instead of waiting a tick.
  useEffect(() => {
    return hostRef.current.onVisibility((nextVisible) => {
      setVisible(nextVisible);
      if (nextVisible) void refreshRef.current();
    });
  }, []);

  useEffect(() => {
    if (!ready) return;
    const interval = visible ? visibleMs : hiddenMs;
    if (!interval) return;
    const timer = window.setInterval(() => void refreshRef.current(), interval);
    return () => window.clearInterval(timer);
  }, [ready, visible, visibleMs, hiddenMs]);

  const tree = snapshot?.tree ?? [];
  const projects: Project[] = snapshot?.projects ?? [];
  const project = snapshot?.project;

  const attention = useMemo(() => hasDueAttention(tree, todayIso()), [tree]);
  useEffect(() => {
    void hostRef.current.setTrayAttention(attention).catch(() => undefined);
  }, [attention]);

  const heldId = draft?.taskId;
  const baseRows = useMemo(
    () => menubarRows(tree, showDone, collapsed, heldId),
    [tree, showDone, collapsed, heldId],
  );
  const rows = useMemo(() => {
    if (exiting.length === 0) return baseRows;
    const merged = baseRows.slice();
    for (const entry of exiting) {
      if (merged.some((candidate) => candidate.node.id === entry.row.node.id)) continue;
      merged.splice(Math.min(entry.index, merged.length), 0, entry.row);
    }
    return merged;
  }, [baseRows, exiting]);
  rowsRef.current = rows;

  const titles = useMemo(() => collectTasks(tree), [tree]);
  const resolveTitle = useCallback((id: string) => titles.get(id)?.title, [titles]);

  const toggleFold = useCallback((id: string) => {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const expand = useCallback(
    (id: string) => {
      const same = expandedRef.current === id;
      withDraftGuard("Discard changes? Opening another task drops the draft.", () => {
        clearDraft();
        if (same) {
          collapse();
          return;
        }
        expandedRef.current = id;
        setExpandedId(id);
        setDetail(null);
        setConflict(null);
        void loadDetail(id);
      });
    },
    [clearDraft, collapse, loadDetail, withDraftGuard],
  );

  const collapseExpanded = useCallback(() => {
    withDraftGuard("Discard changes? Closing the task drops the draft.", () => {
      clearDraft();
      collapse();
    });
  }, [clearDraft, collapse, withDraftGuard]);

  const toggleState = useCallback(
    (row: FlatRow) => {
      const node = row.node;
      if (node.state === "done") {
        setUndo(null);
        setExiting((previous) => previous.filter((entry) => entry.row.node.id !== node.id));
        void mutate(() => clientRef.current.setState(node.id, "open"));
        return;
      }
      const index = rowsRef.current.findIndex((candidate) => candidate.node.id === node.id);
      // Hold the row on screen, checked and struck through, for one beat; the
      // Task is already done in the store, so the rendered node says so.
      setExiting((previous) => [
        ...previous.filter((entry) => entry.row.node.id !== node.id),
        {
          row: { node: { ...node, state: "done" as TaskState }, depth: row.depth },
          index: index < 0 ? Number.MAX_SAFE_INTEGER : index,
        },
      ]);
      setUndo({ taskId: node.id, title: node.title });
      later(completeDelayMs, () =>
        setExiting((previous) => previous.filter((entry) => entry.row.node.id !== node.id)),
      );
      later(undoMs, () =>
        setUndo((previous) => (previous?.taskId === node.id ? null : previous)),
      );
      void mutate(() => clientRef.current.setState(node.id, "done"));
    },
    [completeDelayMs, later, mutate, undoMs],
  );

  const undoComplete = useCallback(() => {
    if (!undo) return;
    const { taskId } = undo;
    setUndo(null);
    setExiting((previous) => previous.filter((entry) => entry.row.node.id !== taskId));
    void mutate(() => clientRef.current.setState(taskId, "open"));
  }, [mutate, undo]);

  const quickAdd = useCallback(
    (title: string, parentId?: string) => {
      const value = title.trim();
      if (!value) return;
      if (parentId) {
        setCollapsed((previous) => {
          const next = new Set(previous);
          next.delete(parentId);
          return next;
        });
      }
      void mutate(() => clientRef.current.addTask({ title: value, parentId }));
    },
    [mutate],
  );

  const startEdit = useCallback(() => {
    if (!detail) return;
    const next: MenubarDraft = {
      taskId: detail.id,
      projectSlug: detail.projectSlug,
      baselineRevision: detail.revision,
      baselineTitle: detail.title,
      title: detail.title,
      text: detail.body,
      original: detail.body,
    };
    draftRef.current = next;
    setDraft(next);
    setConflict(null);
  }, [detail]);

  const updateDraft = useCallback((patch: Partial<MenubarDraft>) => {
    if (savingRef.current) return;
    setDraft((previous) => {
      if (!previous) return previous;
      const next = { ...previous, ...patch };
      draftRef.current = next;
      return next;
    });
  }, []);

  const cancelEdit = useCallback(() => {
    withDraftGuard("Discard changes?", () => {
      clearDraft();
      // A draft whose Task vanished has no row left to fall back to.
      const tracked = expandedRef.current;
      if (tracked && !collectTasks(snapshotRef.current?.tree ?? []).has(tracked)) collapse();
    });
  }, [clearDraft, collapse, withDraftGuard]);

  const saveEdit = useCallback(async () => {
    const current = draftRef.current;
    if (!current || savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    const seq = (detailSeq.current += 1);
    opsRef.current += 1;
    localWrites.current += 1;
    setBusy(true);
    try {
      const title = current.title.trim();
      const titleChanged = title !== current.baselineTitle;
      const bodyChanged = current.text !== current.original;
      let baseRevision = current.baselineRevision;
      if (titleChanged) {
        // set_title is not revision-checked, so verify the baseline first: an
        // external write since the edit began is a conflict, not a rename.
        const latest = await clientRef.current.taskDetail(current.taskId);
        if (latest.revision !== current.baselineRevision) {
          throw new TtError({
            code: "stale_draft",
            message: "the task file changed on disk",
            conflict: {
              reason: "changed",
              currentRevision: latest.revision,
              currentBody: latest.body,
            },
          });
        }
        await clientRef.current.setTitle(current.taskId, title);
        // The rename rewrote the file; adopt the revision our own write made.
        baseRevision = (await clientRef.current.taskDetail(current.taskId)).revision;
      }
      let saved: TaskDetail;
      if (bodyChanged) {
        saved = await clientRef.current.saveBody(current.taskId, current.text, baseRevision);
      } else {
        saved = await clientRef.current.taskDetail(current.taskId);
      }
      if (draftRef.current !== current || seq !== detailSeq.current) return;
      setDetail(saved);
      clearDraft();
    } catch (thrown) {
      if (draftRef.current !== current) return;
      if (thrown instanceof TtError && thrown.code === "stale_draft") {
        // Keep the draft and every recovery option.
        setConflict(thrown.conflict ?? { reason: "changed" });
      } else {
        showError(thrown);
      }
    } finally {
      savingRef.current = false;
      localWrites.current = 0;
      setSaving(false);
      opsRef.current -= 1;
      setBusy(false);
    }
  }, [clearDraft, showError]);

  const reloadDraft = useCallback(async () => {
    const current = draftRef.current;
    if (!current || savingRef.current) return;
    try {
      const fresh = await clientRef.current.taskDetail(current.taskId);
      if (draftRef.current !== current) return;
      setDetail(fresh);
      const next: MenubarDraft = {
        ...current,
        baselineRevision: fresh.revision,
        baselineTitle: fresh.title,
        title: fresh.title,
        text: fresh.body,
        original: fresh.body,
      };
      draftRef.current = next;
      setDraft(next);
      setConflict(null);
    } catch (thrown) {
      showError(thrown);
    }
  }, [showError]);

  const copyDraft = useCallback(async () => {
    const current = draftRef.current;
    if (!current) return;
    try {
      await clientRef.current.copyText(current.text);
    } catch (thrown) {
      showError(thrown);
    }
  }, [showError]);

  const copyTask = useCallback(
    async (id: string) => {
      try {
        await clientRef.current.copyTask(id, true);
      } catch (thrown) {
        showError(thrown);
      }
    },
    [showError],
  );

  const openInDesktop = useCallback(
    async (id: string) => {
      const slug =
        draftRef.current?.projectSlug ?? snapshotRef.current?.project?.slug ?? "";
      if (!slug) return;
      try {
        await hostRef.current.openInDesktop(slug, id);
      } catch (thrown) {
        showError(thrown);
      }
    },
    [showError],
  );

  const hide = useCallback(async () => {
    try {
      await hostRef.current.hide();
    } catch (thrown) {
      showError(thrown);
    }
  }, [showError]);

  const openExternal = useCallback(
    async (url: string) => {
      try {
        await clientRef.current.openExternal(url);
      } catch (thrown) {
        showError(thrown);
      }
    },
    [showError],
  );

  const dirty = draft !== null && isDirty(draft);

  /** A draft whose Task no longer exists in the Project: kept for recovery. */
  const orphanedDraft = draft && !titles.has(draft.taskId) ? draft : null;

  /** The expanded Task, whose subtask the quick-add Tab targets. */
  const subtaskTarget = useMemo(() => {
    if (!expandedId) return null;
    return rows.find((row) => row.node.id === expandedId)?.node ?? titles.get(expandedId) ?? null;
  }, [expandedId, rows, titles]);

  return {
    ready,
    projects,
    project,
    tree,
    rows,
    visible,
    busy,
    saving,
    error,
    showDone,
    setShowDone,
    collapsed,
    toggleFold,
    expandedId,
    detail,
    draft,
    orphanedDraft,
    dirty,
    conflict,
    undo,
    guard,
    exiting,
    highlight,
    attention,
    resolveTitle,
    openProject,
    expand,
    collapseExpanded,
    toggleState,
    undoComplete,
    quickAdd,
    subtaskTargetId: subtaskTarget?.id ?? null,
    subtaskTargetTitle: subtaskTarget?.title ?? null,
    refresh,
    startEdit,
    setDraftTitle: (title: string) => updateDraft({ title }),
    setDraftText: (text: string) => updateDraft({ text }),
    cancelEdit,
    saveEdit,
    reloadDraft,
    copyDraft,
    copyTask,
    openInDesktop,
    hide,
    openExternal,
    resolveGuard,
    dismissError: () => setError(null),
  };
}

export type MenubarApp = ReturnType<typeof useMenubar>;
