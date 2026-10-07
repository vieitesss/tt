// The app's state machine: one hook owning selection, drafts, conflicts,
// search, the bounded reconciliation loop, and the in-app confirmation flow.
//
// Rules the tests pin down:
// - a dirty draft is never replaced by a refresh, a detail reload, or the
//   disappearance of its task from the store (recovery view stays available);
// - switching task/project, unregistering the open project, or deleting while
//   dirty asks first and keeps the draft when declined;
// - a save whose baseline revision is stale keeps the draft and records the
//   conflict;
// - destructive confirmations are fail-closed: Escape, backdrop clicks, a
//   declined answer, or no answer never authorize the destructive action;
// - responses from an older project load, detail load, or background refresh
//   can never overwrite a newer selection or a user operation.

import { useCallback, useEffect, useRef, useState } from "react";

import { TtError, type AddTaskInput, type SearchFilters, type TtClient } from "../api";
import { ancestorsOf, collectTasks } from "../lib/tree";
import { tauriFocusTaskSource, type FocusTaskRequest, type FocusTaskSource } from "./focusTask";
import type {
  ConflictInfo,
  Issue,
  Priority,
  Project,
  Snapshot,
  TaskDetail,
  TaskState,
} from "../types";

export const LAST_PROJECT_KEY = "tt.desktop.lastProject";

export interface PreferenceStore {
  get(key: string): string | null;
  set(key: string, value: string): void;
}

export interface Draft {
  /** The Task this draft belongs to, so a stale response can never apply elsewhere. */
  taskId: string;
  projectSlug: string;
  baselineRevision: string;
  text: string;
  original: string;
}

export interface SearchState {
  query: string;
  state?: TaskState;
  tag: string;
  priority?: Priority;
  dueToday: boolean;
}

export interface ToastMessage {
  id: number;
  kind: "info" | "error";
  text: string;
}

/** A pending confirmation shown by the in-app modal. */
export interface ConfirmRequest {
  title: string;
  message: string;
  confirmLabel: string;
  destructive: boolean;
}

export interface UseTtAppOptions {
  reconcileMs?: number;
  preferences?: PreferenceStore;
  /** Where "Open in TT" requests arrive from; defaults to the Rust shell. */
  focusTaskSource?: FocusTaskSource;
}

export function memoryPreferences(): PreferenceStore {
  const values = new Map<string, string>();
  return {
    get: (key) => values.get(key) ?? null,
    set: (key, value) => {
      values.set(key, value);
    },
  };
}

/** Browser-backed preferences, or an in-memory store in sandboxed contexts. */
export function browserPreferences(): PreferenceStore {
  try {
    const storage = typeof window !== "undefined" ? window.localStorage : undefined;
    if (storage) {
      return {
        get: (key) => storage.getItem(key),
        set: (key, value) => storage.setItem(key, value),
      };
    }
  } catch {
    // Fall through to an in-memory store (sandboxed/test environments).
  }
  return memoryPreferences();
}

const EMPTY_SEARCH: SearchState = { query: "", tag: "", dueToday: false };

export function useTtApp(client: TtClient, options: UseTtAppOptions = {}) {
  const { reconcileMs = 1000 } = options;
  const preferencesRef = useRef<PreferenceStore | null>(null);
  if (!preferencesRef.current) {
    preferencesRef.current = options.preferences ?? browserPreferences();
  }
  const preferences = preferencesRef.current;
  const focusSourceRef = useRef<FocusTaskSource | null>(null);
  if (!focusSourceRef.current) {
    focusSourceRef.current = options.focusTaskSource ?? tauriFocusTaskSource();
  }

  const [ready, setReady] = useState(false);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [issues, setIssues] = useState<Issue[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectedMissing, setSelectedMissing] = useState(false);
  const [detail, setDetail] = useState<TaskDetail | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [conflict, setConflict] = useState<ConflictInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const [toast, setToast] = useState<ToastMessage | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  // Whether the open Project's Task rows are hidden in the sidebar tree. The
  // Project stays open; only its children fold away.
  const [projectCollapsed, setProjectCollapsed] = useState(false);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [search, setSearchState] = useState<SearchState>(EMPTY_SEARCH);
  const [searchHits, setSearchHits] = useState<string[] | null>(null);
  const [externalChange, setExternalChange] = useState(false);
  const [confirmRequest, setConfirmRequest] = useState<ConfirmRequest | null>(null);

  const snapshotRef = useRef<Snapshot | null>(null);
  const selectedIdRef = useRef<string | null>(null);
  const selectedMissingRef = useRef(false);
  const draftRef = useRef<Draft | null>(null);
  // User operations bump `dataSeq` at start; a background refresh snapshots it
  // without bumping, so it can never invalidate a navigation or mutation.
  const dataSeq = useRef(0);
  const userOps = useRef(0);
  const refreshSeq = useRef(0);
  const detailSeq = useRef(0);
  const searchSeq = useRef(0);
  const mutationInFlight = useRef(false);
  // A description save owns the draft until it settles: editing, navigation,
  // and metadata actions wait for it rather than racing or orphaning the draft.
  const savingRef = useRef(false);
  const busyCount = useRef(0);
  const toastId = useRef(0);
  const toastTimer = useRef<number | null>(null);
  const confirmResolver = useRef<((ok: boolean) => void) | null>(null);
  const clientRef = useRef(client);
  clientRef.current = client;

  const dirty = draft !== null && draft.text !== draft.original;

  const showToast = useCallback((kind: ToastMessage["kind"], text: string) => {
    toastId.current += 1;
    setToast({ id: toastId.current, kind, text });
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    if (kind === "info") {
      toastTimer.current = window.setTimeout(() => setToast(null), 4000);
    }
  }, []);

  const showError = useCallback(
    (error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      showToast("error", message);
    },
    [showToast],
  );

  const beginBusy = useCallback(() => {
    busyCount.current += 1;
    setBusy(true);
  }, []);

  const endBusy = useCallback(() => {
    busyCount.current = Math.max(0, busyCount.current - 1);
    if (busyCount.current === 0) setBusy(false);
  }, []);

  const beginUserOp = useCallback(() => {
    dataSeq.current += 1;
    userOps.current += 1;
    return dataSeq.current;
  }, []);

  const endUserOp = useCallback(() => {
    userOps.current = Math.max(0, userOps.current - 1);
  }, []);

  const clearDraft = useCallback(() => {
    draftRef.current = null;
    setDraft(null);
    setConflict(null);
  }, []);

  /**
   * Drop the selection. Only the selection invariant lives here; each caller
   * decides separately whether to also discard the draft, and what to ask
   * before doing so.
   */
  const resetSelection = useCallback(() => {
    selectedIdRef.current = null;
    selectedMissingRef.current = false;
    setSelectedId(null);
    setSelectedMissing(false);
    setDetail(null);
  }, []);

  /** Show the in-app confirmation. Any earlier unanswered request fails closed. */
  const requestConfirm = useCallback((request: ConfirmRequest): Promise<boolean> => {
    return new Promise<boolean>((resolve) => {
      confirmResolver.current?.(false);
      confirmResolver.current = resolve;
      setConfirmRequest(request);
    });
  }, []);

  /** Answer the visible confirmation; an unanswered modal never authorizes. */
  const resolveConfirm = useCallback((ok: boolean) => {
    const resolve = confirmResolver.current;
    confirmResolver.current = null;
    setConfirmRequest(null);
    resolve?.(ok);
  }, []);

  useEffect(() => {
    return () => {
      confirmResolver.current?.(false);
      confirmResolver.current = null;
    };
  }, []);

  const applySnapshot = useCallback(
    (next: Snapshot) => {
      const previous = snapshotRef.current;
      snapshotRef.current = next;
      setSnapshot(next);
      setIssues(next.issues);
      if (next.project) {
        preferences.set(LAST_PROJECT_KEY, next.project.slug);
      }
      const selected = selectedIdRef.current;
      if (selected && !collectTasks(next.tree).has(selected)) {
        // The task vanished from the store (external delete, or its Project
        // was unregistered). With a draft we keep selection, detail, and draft
        // so the recovery view stays available; without one we clear it.
        if (draftRef.current) {
          selectedMissingRef.current = true;
          setSelectedMissing(true);
        } else {
          resetSelection();
        }
      } else if (selected) {
        selectedMissingRef.current = false;
        setSelectedMissing(false);
      }
      if (previous && previous.revision !== next.revision && draftRef.current) {
        setExternalChange(true);
      }
    },
    [preferences, resetSelection],
  );

  const loadDetail = useCallback(
    async (id: string, options?: { silent?: boolean }) => {
      const seq = (detailSeq.current += 1);
      try {
        const next = await clientRef.current.taskDetail(id);
        if (seq !== detailSeq.current || selectedIdRef.current !== id) return;
        setDetail(next);
        if (!draftRef.current) setConflict(null);
        setExternalChange(false);
      } catch (error) {
        if (!options?.silent) showError(error);
      }
    },
    [showError],
  );

  /**
   * Background reconciliation. It never invalidates a user operation and never
   * clears busy state owned by one.
   */
  const refresh = useCallback(async () => {
    // Never race a user operation: nothing to reconcile mid-flight.
    if (userOps.current > 0) return;
    const data = dataSeq.current;
    const seq = (refreshSeq.current += 1);
    try {
      const next = await clientRef.current.refresh();
      if (seq !== refreshSeq.current) return;
      if (userOps.current > 0 || data !== dataSeq.current) return;
      applySnapshot(next);
      const selected = selectedIdRef.current;
      if (selected && !draftRef.current && !selectedMissingRef.current) {
        void loadDetail(selected, { silent: true });
      }
    } catch (error) {
      if (seq === refreshSeq.current && data === dataSeq.current) showError(error);
    }
  }, [applySnapshot, loadDetail, showError]);

  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  useEffect(() => {
    const bootstrap = async () => {
      beginBusy();
      try {
        const next = await clientRef.current.bootstrap();
        // Read the preference before applying the snapshot, which records the
        // current project as the new preference.
        const last = preferences.get(LAST_PROJECT_KEY);
        applySnapshot(next);
        const project = next.projects.find((candidate) => candidate.slug === last);
        if (project) {
          const seq = beginUserOp();
          try {
            const opened = await clientRef.current.openProject(project.slug);
            if (seq === dataSeq.current) applySnapshot(opened);
          } finally {
            endUserOp();
          }
        }
      } catch (error) {
        showError(error);
      } finally {
        endBusy();
        setReady(true);
      }
    };
    void bootstrap();
    // Runs once; `applySnapshot`/`showError` only touch stable setters and refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!reconcileMs || !ready) return;
    const timer = window.setInterval(() => {
      void refreshRef.current();
    }, reconcileMs);
    return () => window.clearInterval(timer);
  }, [reconcileMs, ready]);

  // Project-scoped search follows the query, filters, and external revision.
  const projectSlug = snapshot?.project?.slug ?? null;
  const revision = snapshot?.revision ?? "";
  useEffect(() => {
    const active =
      search.query.trim() !== "" ||
      search.state !== undefined ||
      search.tag.trim() !== "" ||
      search.priority !== undefined ||
      search.dueToday;
    if (!projectSlug || !active) {
      setSearchHits(null);
      return;
    }
    const seq = (searchSeq.current += 1);
    const filters: SearchFilters = {
      query: search.query,
      state: search.state,
      tag: search.tag,
      priority: search.priority,
      dueToday: search.dueToday,
    };
    void (async () => {
      try {
        const hits = await clientRef.current.search(filters);
        if (seq === searchSeq.current) setSearchHits(hits);
      } catch (error) {
        if (seq === searchSeq.current) showError(error);
      }
    })();
  }, [client, projectSlug, revision, search, showError]);

  const guardDraft = useCallback(async () => {
    // An in-flight save still owns the draft; nothing may navigate away from it.
    if (savingRef.current) return false;
    if (!draftRef.current || draftRef.current.text === draftRef.current.original) return true;
    const discard = await requestConfirm({
      title: "Unsaved changes",
      message: "You have unsaved changes. Discard them?",
      confirmLabel: "Discard",
      destructive: true,
    });
    if (discard) clearDraft();
    return discard;
  }, [clearDraft, requestConfirm]);

  const confirmDiscardDraft = useCallback(
    () =>
      requestConfirm({
        title: "Unsaved changes",
        message: "You have unsaved changes. Close without saving?",
        confirmLabel: "Discard",
        destructive: true,
      }),
    [requestConfirm],
  );

  const openProject = useCallback(
    async (slug: string) => {
      if (slug === snapshotRef.current?.project?.slug) return;
      if (!(await guardDraft())) return;
      const seq = beginUserOp();
      beginBusy();
      try {
        const next = await clientRef.current.openProject(slug);
        if (seq !== dataSeq.current) return;
        resetSelection();
        clearDraft();
        setCollapsed(new Set());
        setProjectCollapsed(false);
        setSearchState(EMPTY_SEARCH);
        setExternalChange(false);
        applySnapshot(next);
      } catch (error) {
        if (seq === dataSeq.current) showError(error);
      } finally {
        endUserOp();
        endBusy();
      }
    },
    [
      applySnapshot,
      beginBusy,
      beginUserOp,
      clearDraft,
      endBusy,
      endUserOp,
      guardDraft,
      resetSelection,
      showError,
    ],
  );

  const registerProject = useCallback(async () => {
    const seq = beginUserOp();
    beginBusy();
    try {
      const next = await clientRef.current.registerProject();
      if (next && seq === dataSeq.current) {
        applySnapshot(next);
        showToast("info", "project registered");
      }
    } catch (error) {
      showError(error);
    } finally {
      endUserOp();
      endBusy();
    }
  }, [applySnapshot, beginBusy, beginUserOp, endBusy, endUserOp, showError, showToast]);

  const unregisterProject = useCallback(
    async (slug: string) => {
      const project = snapshotRef.current?.projects.find((candidate) => candidate.slug === slug);
      const isOpen = snapshotRef.current?.project?.slug === slug;
      // Removing the open project while a draft is dirty would strand the
      // draft, so ask about it first.
      if (isOpen && !(await guardDraft())) return;
      const confirmed = await requestConfirm({
        title: "Unregister project",
        message: project
          ? `Unregister "${project.name}"? Its tasks stay on disk and can be re-registered later.`
          : "Unregister this project? Its tasks stay on disk.",
        confirmLabel: "Unregister",
        destructive: true,
      });
      if (!confirmed) return;
      const seq = beginUserOp();
      beginBusy();
      try {
        const next = await clientRef.current.unregisterProject(slug);
        if (seq !== dataSeq.current) return;
        applySnapshot(next);
        if (!next.project && !draftRef.current) resetSelection();
        showToast("info", "project unregistered; its tasks were kept on disk");
      } catch (error) {
        if (seq === dataSeq.current) showError(error);
      } finally {
        endUserOp();
        endBusy();
      }
    },
    [
      applySnapshot,
      beginBusy,
      beginUserOp,
      endBusy,
      endUserOp,
      guardDraft,
      requestConfirm,
      resetSelection,
      showError,
      showToast,
    ],
  );

  const selectTask = useCallback(
    async (id: string) => {
      if (id === selectedIdRef.current) return;
      if (!(await guardDraft())) return;
      // The editor belongs to the old Task. A clean draft would otherwise follow
      // the selection and attach its baseline to the new Task.
      clearDraft();
      const tree = snapshotRef.current?.tree ?? [];
      setCollapsed((previous) => {
        const next = new Set(previous);
        for (const ancestor of ancestorsOf(tree, id)) next.delete(ancestor);
        return next;
      });
      selectedIdRef.current = id;
      selectedMissingRef.current = false;
      setSelectedId(id);
      setSelectedMissing(false);
      setDetail(null);
      setExternalChange(false);
      await loadDetail(id);
    },
    [clearDraft, guardDraft, loadDetail],
  );

  /**
   * "Open in TT" from the menu bar popover. It navigates through the same path
   * as a click, so a dirty draft is asked about first and a declined guard
   * simply drops the request.
   */
  const focusTask = useCallback(
    async (request: FocusTaskRequest) => {
      const { projectSlug, taskId } = request;
      if (projectSlug !== snapshotRef.current?.project?.slug) {
        await openProject(projectSlug);
        // Declined, or the Project could not be opened: never select a Task in
        // a Project we did not actually switch to.
        if (snapshotRef.current?.project?.slug !== projectSlug) return;
      }
      await selectTask(taskId);
    },
    [openProject, selectTask],
  );

  const focusTaskRef = useRef(focusTask);
  focusTaskRef.current = focusTask;

  useEffect(() => {
    return focusSourceRef.current?.subscribe((request) => {
      void focusTaskRef.current(request);
    });
    // Subscribes once; the handler is read through a ref so a re-render never
    // resubscribes (a missed event would be worse than a stale closure).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggleFold = useCallback((id: string) => {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const toggleProjectFold = useCallback(() => setProjectCollapsed((folded) => !folded), []);

  const mutate = useCallback(
    async (work: () => Promise<Snapshot>, toastText?: string) => {
      if (mutationInFlight.current || savingRef.current) return false;
      mutationInFlight.current = true;
      const seq = beginUserOp();
      beginBusy();
      let result = false;
      let failed = false;
      try {
        const next = await work();
        if (seq !== dataSeq.current) return false;
        applySnapshot(next);
        if (toastText) showToast("info", toastText);
        const selected = selectedIdRef.current;
        if (selected && collectTasks(next.tree).has(selected) && !draftRef.current) {
          void loadDetail(selected, { silent: true });
        } else if (selected && !collectTasks(next.tree).has(selected)) {
          setSearchHits(null);
        }
        result = true;
      } catch (error) {
        showError(error);
        failed = true;
      } finally {
        mutationInFlight.current = false;
        endUserOp();
        endBusy();
      }
      // Never hide a partial commit: refresh to disk truth once the user
      // operation has fully unwound.
      if (failed) await refreshRef.current();
      return result;
    },
    [applySnapshot, beginBusy, beginUserOp, endBusy, endUserOp, loadDetail, showError, showToast],
  );

  const startEdit = useCallback(() => {
    const current = detail;
    if (!current) return;
    const next = {
      taskId: current.id,
      projectSlug: current.projectSlug,
      baselineRevision: current.revision,
      text: current.body,
      original: current.body,
    };
    draftRef.current = next;
    setDraft(next);
    setConflict(null);
  }, [detail]);

  const setDraftText = useCallback((text: string) => {
    // Source editing is disabled while a Save is in flight, so no keystroke can
    // be overwritten by the response.
    if (savingRef.current) return;
    setDraft((previous) => {
      if (!previous) return previous;
      const next = { ...previous, text };
      draftRef.current = next;
      return next;
    });
  }, []);

  const cancelEdit = useCallback(async () => {
    if (!(await guardDraft())) return;
    clearDraft();
  }, [clearDraft, guardDraft]);

  const saveDraft = useCallback(async () => {
    const current = draftRef.current;
    const id = selectedIdRef.current;
    if (!current || !id || savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    const seq = beginUserOp();
    beginBusy();
    try {
      const next = await clientRef.current.saveBody(id, current.text, current.baselineRevision);
      // Ownership: an older save may only apply while it still owns the draft
      // and the selection it started from.
      if (selectedIdRef.current !== id || draftRef.current !== current) return;
      setDetail(next);
      selectedMissingRef.current = false;
      setSelectedMissing(false);
      clearDraft();
      showToast("info", "description saved");
    } catch (error) {
      if (selectedIdRef.current !== id || draftRef.current !== current) return;
      if (error instanceof TtError && error.code === "stale_draft") {
        setConflict(error.conflict ?? { reason: "changed" });
      } else {
        showError(error);
      }
      return;
    } finally {
      savingRef.current = false;
      setSaving(false);
      endUserOp();
      endBusy();
    }
    try {
      const fresh = await clientRef.current.refresh();
      if (seq === dataSeq.current) applySnapshot(fresh);
    } catch (error) {
      showError(error);
    }
  }, [applySnapshot, beginBusy, beginUserOp, clearDraft, endBusy, endUserOp, showError, showToast]);

  const reloadDraft = useCallback(async () => {
    const id = selectedIdRef.current;
    if (!id || savingRef.current) return;
    try {
      const next = await clientRef.current.taskDetail(id);
      if (selectedIdRef.current !== id) return;
      setDetail(next);
      selectedMissingRef.current = false;
      setSelectedMissing(false);
      const draftState = {
        taskId: next.id,
        projectSlug: next.projectSlug,
        baselineRevision: next.revision,
        text: next.body,
        original: next.body,
      };
      draftRef.current = draftState;
      setDraft(draftState);
      setConflict(null);
      setExternalChange(false);
      showToast("info", "reloaded latest from disk");
    } catch (error) {
      showError(error);
    }
  }, [showError, showToast]);

  const copyDraft = useCallback(async () => {
    const current = draftRef.current;
    if (!current) return;
    try {
      await clientRef.current.copyText(current.text);
      showToast("info", "draft copied");
    } catch (error) {
      showError(error);
    }
  }, [showError, showToast]);

  const addTask = useCallback(
    async (input: AddTaskInput) => {
      const ok = await mutate(
        () => clientRef.current.addTask(input),
        input.capture ? "captured task" : "task added",
      );
      if (ok && input.parentId) {
        setCollapsed((previous) => {
          const next = new Set(previous);
          next.delete(input.parentId as string);
          return next;
        });
      }
    },
    [mutate],
  );

  const setState = useCallback(
    (state: TaskState) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.setState(id, state), `state: ${state}`);
    },
    [mutate],
  );

  const setTitle = useCallback(
    (title: string) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.setTitle(id, title), "title renamed");
    },
    [mutate],
  );

  const setTags = useCallback(
    (tags: string[]) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.setTags(id, tags), "tags updated");
    },
    [mutate],
  );

  const setPriority = useCallback(
    (priority?: Priority) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.setPriority(id, priority), "priority updated");
    },
    [mutate],
  );

  const setDue = useCallback(
    (due?: string) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.setDue(id, due), "due date updated");
    },
    [mutate],
  );

  const setParent = useCallback(
    (parentId?: string) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.setParent(id, parentId), "task moved");
    },
    [mutate],
  );

  const shiftRank = useCallback(
    (delta: number) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void mutate(() => clientRef.current.shiftRank(id, delta));
    },
    [mutate],
  );

  const moveToProject = useCallback(
    (targetSlug: string, parentId?: string) => {
      const id = selectedIdRef.current;
      if (!id) return;
      void (async () => {
        const ok = await mutate(
          () => clientRef.current.moveToProject(id, targetSlug, parentId),
          "task moved to another project",
        );
        if (ok) await loadDetail(id, { silent: true });
      })();
    },
    [loadDetail, mutate],
  );

  const deleteTask = useCallback(async () => {
    const id = selectedIdRef.current;
    const current = detail;
    if (!id) return;
    if (!(await guardDraft())) return;
    try {
      const descendants = await clientRef.current.deleteCount(id);
      const confirmed = await requestConfirm({
        title: "Delete task",
        message:
          descendants === 0
            ? `Delete "${current?.title ?? id}"?`
            : `Delete "${current?.title ?? id}" and its ${descendants} descendant(s)?`,
        confirmLabel: "Delete",
        destructive: true,
      });
      if (!confirmed) return;
      const next = await mutate(() => clientRef.current.deleteTask(id), "task deleted");
      if (next) {
        resetSelection();
        clearDraft();
      }
    } catch (error) {
      showError(error);
    }
  }, [clearDraft, detail, guardDraft, mutate, requestConfirm, resetSelection, showError]);

  const copyMetadata = useCallback(
    async (includeBody: boolean) => {
      const id = selectedIdRef.current;
      if (!id) return;
      try {
        await clientRef.current.copyTask(id, includeBody);
        showToast("info", includeBody ? "copied task with description" : "copied task metadata");
      } catch (error) {
        showError(error);
      }
    },
    [showError, showToast],
  );

  const closeMainWindow = useCallback(() => clientRef.current.closeMainWindow(), []);

  const clearSelection = useCallback(async () => {
    // Closing the recovery view discards a dirty draft, so it must ask first;
    // Escape, backdrop, or Cancel never authorise the discard.
    if (!(await guardDraft())) return;
    resetSelection();
    clearDraft();
  }, [clearDraft, guardDraft, resetSelection]);

  const openExternal = useCallback(
    async (url: string) => {
      try {
        await clientRef.current.openExternal(url);
      } catch (error) {
        showError(error);
      }
    },
    [showError],
  );

  const setSearch = useCallback((partial: Partial<SearchState>) => {
    setSearchState((previous) => ({ ...previous, ...partial }));
  }, []);

  const clearSearch = useCallback(() => {
    setSearchState(EMPTY_SEARCH);
    setSearchHits(null);
  }, []);

  const dismissToast = useCallback(() => setToast(null), []);

  const projects: Project[] = snapshot?.projects ?? [];
  const currentProject = snapshot?.project;

  return {
    ready,
    snapshot,
    projects,
    currentProject,
    issues,
    tree: snapshot?.tree ?? [],
    selectedId,
    selectedMissing,
    detail,
    draft,
    dirty,
    saving,
    conflict,
    busy,
    toast,
    sidebarCollapsed,
    projectCollapsed,
    collapsed,
    search,
    searchHits,
    externalChange,
    confirmRequest,
    resolveConfirm,
    openProject,
    registerProject,
    unregisterProject,
    refresh,
    selectTask,
    toggleFold,
    toggleProjectFold,
    setSidebarCollapsed,
    startEdit,
    setDraftText,
    cancelEdit,
    saveDraft,
    reloadDraft,
    copyDraft,
    addTask,
    setState,
    setTitle,
    setTags,
    setPriority,
    setDue,
    setParent,
    shiftRank,
    moveToProject,
    deleteTask,
    copyMetadata,
    confirmDiscardDraft,
    discardDraft: clearDraft,
    clearSelection,
    closeMainWindow,
    openExternal,
    setSearch,
    clearSearch,
    dismissToast,
  };
}

export type TtApp = ReturnType<typeof useTtApp>;
