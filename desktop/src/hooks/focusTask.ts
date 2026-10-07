// The main window's side of "Open in TT": the menu bar popover asks the shell
// to show the main window and emit this event, and the main window navigates
// through its normal selection path, so its own dirty-draft guards apply.

import { listenTauriEvent } from "../lib/tauri";

export interface FocusTaskRequest {
  projectSlug: string;
  taskId: string;
}

export interface FocusTaskSource {
  subscribe(handler: (request: FocusTaskRequest) => void): () => void;
}

/** The real source: `tt://focus-task` from the Rust shell. */
export function tauriFocusTaskSource(): FocusTaskSource {
  return {
    subscribe: (handler) => listenTauriEvent<FocusTaskRequest>("tt://focus-task", handler),
  };
}
