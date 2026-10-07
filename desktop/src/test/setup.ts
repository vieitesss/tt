import "@testing-library/jest-dom/vitest";
import { beforeEach } from "vitest";

// jsdom under this Node version exposes no working storage; give the app a
// deterministic in-memory one so preference behavior is testable and quiet.
const values = new Map<string, string>();

Object.defineProperty(window, "localStorage", {
  configurable: true,
  value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, String(value));
    },
    removeItem: (key: string) => {
      values.delete(key);
    },
    clear: () => {
      values.clear();
    },
    key: (index: number) => [...values.keys()][index] ?? null,
    get length() {
      return values.size;
    },
  },
});

beforeEach(() => {
  values.clear();
});

// jsdom exposes no ResizeObserver. The no-op keeps components that observe
// layout alive; tests that assert the observation install their own recorder.
class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

if (!("ResizeObserver" in window)) {
  Object.defineProperty(window, "ResizeObserver", {
    configurable: true,
    value: NoopResizeObserver,
  });
}
