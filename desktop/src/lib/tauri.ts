/** Whether the frontend runs inside a Tauri webview. */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Subscribe to a Tauri event; returns the unsubscribe. Listening is async, so
 * an unsubscribe that lands first still drops the listener once it resolves.
 * Outside a webview there is nothing to listen to.
 */
export function listenTauriEvent<T>(event: string, handler: (payload: T) => void): () => void {
  if (!isTauri()) return () => {};
  let stop: (() => void) | undefined;
  let disposed = false;
  void (async () => {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<T>(event, (e) => handler(e.payload));
    if (disposed) unlisten();
    else stop = unlisten;
  })();
  return () => {
    disposed = true;
    stop?.();
  };
}
