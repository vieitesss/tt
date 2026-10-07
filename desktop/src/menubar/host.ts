// The popover's seam to the native shell: window visibility, tray attention,
// hiding, and handing a Task to the main window. Components receive a
// `MenubarHost`, so tests drive the whole popover without a webview.

import type { TtClient } from "../api";
import { listenTauriEvent } from "../lib/tauri";

/** Everything the popover needs from the macOS shell. */
export interface MenubarHost {
  /** Called with `true` when the popover becomes visible, `false` when it hides. */
  onVisibility(handler: (visible: boolean) => void): () => void;
  setTrayAttention(attention: boolean): Promise<void>;
  hide(): Promise<void>;
  openInDesktop(projectSlug: string, taskId: string): Promise<void>;
  openDesktop(): Promise<void>;
  quit(): Promise<void>;
}

/** The real shell, over the app's own commands and the visibility event. */
export function tauriMenubarHost(client: TtClient): MenubarHost {
  return {
    onVisibility: (handler) =>
      listenTauriEvent<string>("tt://menubar-visibility", (payload) => handler(payload === "shown")),
    setTrayAttention: (attention) => client.setTrayAttention(attention),
    hide: () => client.hideMenubar(),
    openInDesktop: (projectSlug, taskId) => client.openInDesktop(projectSlug, taskId),
    openDesktop: () => client.openDesktop(),
    quit: () => client.quitApp(),
  };
}
