import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";

import { useExit } from "../hooks/useExit";
import type { ConfirmRequest } from "../hooks/useTtApp";

export interface ConfirmDialogProps {
  request: ConfirmRequest | null;
  onResolve: (ok: boolean) => void;
}

/**
 * Small accessible confirmation modal, fail-closed by construction:
 * Cancel is focused on open, Escape cancels, a backdrop click cancels, and an
 * unanswered request never authorizes anything. The destructive button is a
 * plain button — there is no default/Return key binding for it.
 */
export function ConfirmDialog({ request, onResolve }: ConfirmDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  // The effects below follow the live `request`; only the markup follows
  // `item`, so a dialog that is fading out answers nothing.
  const { item, leaving, onTransitionEnd } = useExit(request);

  useEffect(() => {
    if (request) cancelRef.current?.focus();
  }, [request]);

  useEffect(() => {
    if (!request) return;
    // An unanswered prompt owns the keyboard: Escape cancels it here and no
    // other global handler (an open disclosure, a shortcut) also reacts.
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onResolve(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [request, onResolve]);

  if (!item) return null;

  const trapTab = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab") return;
    const first = cancelRef.current;
    const last = confirmRef.current;
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className={`modal-backdrop${leaving ? " leaving" : ""}`}
      role="presentation"
      aria-hidden={leaving || undefined}
      inert={leaving}
      onTransitionEnd={onTransitionEnd}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onResolve(false);
      }}
    >
      <div
        className="modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        aria-describedby="confirm-message"
        data-testid="confirm-dialog"
        onKeyDown={trapTab}
      >
        <h2 id="confirm-title">{item.title}</h2>
        <p id="confirm-message">{item.message}</p>
        <div className="modal-actions">
          <button ref={cancelRef} type="button" onClick={() => onResolve(false)}>
            Cancel
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={item.destructive ? "danger-solid" : "primary"}
            onClick={() => onResolve(true)}
          >
            {item.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
