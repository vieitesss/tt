import { useEffect, useState, type TransitionEvent } from "react";

/**
 * How long a leaving node may stay mounted if its exit transition never ends
 * (a hidden window, no transition defined, a missed `transitionend`). Longer
 * than the slowest exit in the stylesheets, so the transition normally wins.
 */
export const EXIT_FALLBACK_MS = 400;

export interface Exit<T> {
  /** What to render: the live value, or the last one while it fades out. */
  item: T | null;
  /** True from the moment the value goes away until the node may unmount. */
  leaving: boolean;
  /** Put on the node that carries the exit transition. */
  onTransitionEnd: (event: TransitionEvent<HTMLElement>) => void;
}

/**
 * Keep a node mounted while its exit transition plays. The caller renders
 * `item` with a `leaving` class (and `inert`) and wires `onTransitionEnd`; the
 * node unmounts when its own transition ends, or after `EXIT_FALLBACK_MS`.
 * This is the only JS an exit needs: the entrance is `@starting-style`.
 */
export function useExit<T>(value: T | null): Exit<T> {
  const [held, setHeld] = useState<T | null>(value);

  useEffect(() => {
    if (value !== null) setHeld(value);
  }, [value]);

  const leaving = value === null && held !== null;

  useEffect(() => {
    if (!leaving) return;
    const timer = window.setTimeout(() => setHeld(null), EXIT_FALLBACK_MS);
    return () => window.clearTimeout(timer);
  }, [leaving]);

  return {
    item: value ?? held,
    leaving,
    onTransitionEnd: (event) => {
      if (leaving && event.target === event.currentTarget) setHeld(null);
    },
  };
}

/** `useExit` for a node with no payload: a panel that is either open or not. */
export function usePresence(open: boolean): Omit<Exit<true>, "item"> & { mounted: boolean } {
  const { item, leaving, onTransitionEnd } = useExit(open ? true : null);
  return { mounted: item !== null, leaving, onTransitionEnd };
}
