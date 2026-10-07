import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { usePresence } from "../hooks/useExit";

export interface DisclosureProps {
  /** Trigger text. */
  label: ReactNode;
  /** Tooltip for the trigger. */
  title?: string;
  /** Accessible name when the visible label is a glyph. */
  ariaLabel?: string;
  className?: string;
  panelClassName?: string;
  /**
   * Close whenever this key changes (selected Task, open Project). Panels must
   * never stay open across navigation and leak the previous context.
   */
  resetKey?: string | null;
  /** Align the panel to the trigger's right edge. */
  align?: "start" | "end";
  children: ReactNode;
}

/** Overflow values that can cut a panel off. */
const CLIPPING_OVERFLOW = new Set(["auto", "scroll", "hidden", "clip"]);

/** The nearest ancestor that scrolls or clips the panel, if there is one. */
function clippingAncestor(panel: HTMLElement): HTMLElement | null {
  for (let el = panel.parentElement; el; el = el.parentElement) {
    if (CLIPPING_OVERFLOW.has(getComputedStyle(el).overflowX)) return el;
  }
  return null;
}

/**
 * Horizontal bounds a panel has to stay inside: the clipping ancestor, or the
 * viewport when the page itself is the only limit.
 */
function clippingBounds(ancestor: HTMLElement | null): { left: number; right: number } {
  if (!ancestor) return { left: 0, right: document.documentElement.clientWidth };
  const rect = ancestor.getBoundingClientRect();
  return { left: rect.left, right: rect.right };
}

/**
 * A small named disclosure: a trigger with `aria-expanded` and a floating
 * panel that closes on Escape, outside press, or a context change. It is a
 * plain disclosure, not a menu — no fabricated menu semantics.
 */
export function Disclosure({
  label,
  title,
  ariaLabel,
  className,
  panelClassName,
  resetKey,
  align = "start",
  children,
}: DisclosureProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();
  // The panel outlives `open` by the length of its exit (`.reveal.leaving`).
  const presence = usePresence(open);

  // A floating panel must stay on screen: right-aligned inside a narrow pane
  // (Properties) and left-aligned next to the window edge (relationships) both
  // overflow the pane that clips them, so nudge the panel back inside it. The
  // pane or the window can also change size while the panel is open, so the
  // nudge is re-measured instead of trusted from the first layout.
  useLayoutEffect(() => {
    const panel = panelRef.current;
    if (!open || !panel) return;
    const ancestor = clippingAncestor(panel);
    const clamp = () => {
      panel.style.transform = "";
      const rect = panel.getBoundingClientRect();
      const bounds = clippingBounds(ancestor);
      const shift =
        rect.left < bounds.left
          ? Math.ceil(bounds.left - rect.left)
          : rect.right > bounds.right
            ? Math.floor(bounds.right - rect.right)
            : 0;
      if (shift !== 0) panel.style.transform = `translateX(${shift}px)`;
    };
    clamp();
    const observer = new ResizeObserver(clamp);
    observer.observe(ancestor ?? document.documentElement);
    window.addEventListener("resize", clamp);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", clamp);
    };
  }, [open]);

  // Skip the initial render: the reset key only matters when it *changes*.
  const mounted = useRef(false);
  useEffect(() => {
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    setOpen(false);
  }, [resetKey]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div className={`disclosure${className ? ` ${className}` : ""}`} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="disclosure-trigger"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={ariaLabel}
        title={title}
        onClick={() => setOpen((value) => !value)}
      >
        {label}
      </button>
      {presence.mounted && (
        <div
          id={panelId}
          ref={panelRef}
          className={`disclosure-panel reveal${presence.leaving ? " leaving" : ""}${
            align === "end" ? " align-end" : ""
          }${panelClassName ? ` ${panelClassName}` : ""}`}
          aria-hidden={presence.leaving || undefined}
          inert={presence.leaving}
          onTransitionEnd={presence.onTransitionEnd}
        >
          {children}
        </div>
      )}
    </div>
  );
}
