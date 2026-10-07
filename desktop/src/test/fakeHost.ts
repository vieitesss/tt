import { vi } from "vitest";

import type { MenubarHost } from "../menubar/host";

/** A `MenubarHost` whose shell behavior the test can drive by hand. */
export interface FakeMenubarHost {
  host: MenubarHost;
  /** Emit the visibility change the macOS shell would emit. */
  setVisible(visible: boolean): void;
  /** How many visibility subscriptions are live, for unmount checks. */
  listeners(): number;
}

export function fakeMenubarHost(overrides: Partial<MenubarHost> = {}): FakeMenubarHost {
  const handlers = new Set<(visible: boolean) => void>();
  const host: MenubarHost = {
    onVisibility(handler) {
      handlers.add(handler);
      return () => handlers.delete(handler);
    },
    setTrayAttention: vi.fn(async () => undefined),
    hide: vi.fn(async () => undefined),
    openInDesktop: vi.fn(async () => undefined),
    openDesktop: vi.fn(async () => undefined),
    quit: vi.fn(async () => undefined),
    ...overrides,
  };
  return {
    host,
    setVisible: (visible) => {
      for (const handler of [...handlers]) handler(visible);
    },
    listeners: () => handlers.size,
  };
}
