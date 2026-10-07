import { useExit } from "../hooks/useExit";
import type { ToastMessage } from "../hooks/useTtApp";

export function Toast({
  toast,
  onDismiss,
}: {
  toast: ToastMessage | null;
  onDismiss: () => void;
}) {
  const { item, leaving, onTransitionEnd } = useExit(toast);
  if (!item) return null;
  return (
    <div
      className={`toast ${item.kind}${leaving ? " leaving" : ""}`}
      role={item.kind === "error" ? "alert" : "status"}
      data-testid="toast"
      // A fading toast is already gone as far as the user and a screen reader
      // are concerned; it only stays mounted so the exit can play.
      aria-hidden={leaving || undefined}
      inert={leaving}
      onTransitionEnd={onTransitionEnd}
    >
      <span>{item.text}</span>
      <button type="button" className="icon" onClick={onDismiss} aria-label="Dismiss">
        ×
      </button>
    </div>
  );
}
