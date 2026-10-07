import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { EXIT_FALLBACK_MS } from "../hooks/useExit";
import type { ToastMessage } from "../hooks/useTtApp";
import { Toast } from "./Toast";

const info: ToastMessage = { id: 1, kind: "info", text: "draft copied" };

describe("Toast exit", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("keeps the toast mounted, marked leaving, until its transition ends", () => {
    const { rerender } = render(<Toast toast={info} onDismiss={vi.fn()} />);
    const toast = screen.getByTestId("toast");
    expect(toast).not.toHaveClass("leaving");

    rerender(<Toast toast={null} onDismiss={vi.fn()} />);
    expect(screen.getByTestId("toast")).toBe(toast);
    expect(toast).toHaveClass("leaving");
    expect(toast).toHaveTextContent("draft copied");

    fireEvent.transitionEnd(toast);
    expect(screen.queryByTestId("toast")).not.toBeInTheDocument();
  });

  it("ignores a transition that ends on a child", () => {
    const { rerender } = render(<Toast toast={info} onDismiss={vi.fn()} />);
    rerender(<Toast toast={null} onDismiss={vi.fn()} />);
    fireEvent.transitionEnd(screen.getByRole("button", { hidden: true, name: "Dismiss" }));
    expect(screen.getByTestId("toast")).toBeInTheDocument();
  });

  it("unmounts after the fallback timeout when no transition ever ends", () => {
    const { rerender } = render(<Toast toast={info} onDismiss={vi.fn()} />);
    rerender(<Toast toast={null} onDismiss={vi.fn()} />);

    act(() => vi.advanceTimersByTime(EXIT_FALLBACK_MS - 1));
    expect(screen.getByTestId("toast")).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.queryByTestId("toast")).not.toBeInTheDocument();
  });

  it("a toast shown while another leaves replaces it instead of being removed", () => {
    const { rerender } = render(<Toast toast={info} onDismiss={vi.fn()} />);
    rerender(<Toast toast={null} onDismiss={vi.fn()} />);
    rerender(<Toast toast={{ id: 2, kind: "error", text: "could not save" }} onDismiss={vi.fn()} />);

    const toast = screen.getByTestId("toast");
    expect(toast).not.toHaveClass("leaving");
    expect(toast).toHaveTextContent("could not save");
    act(() => vi.advanceTimersByTime(EXIT_FALLBACK_MS * 2));
    expect(screen.getByTestId("toast")).toBeInTheDocument();
  });

  it("is out of the accessibility tree and inert while it leaves", () => {
    const { rerender } = render(<Toast toast={info} onDismiss={vi.fn()} />);
    rerender(<Toast toast={null} onDismiss={vi.fn()} />);
    const toast = screen.getByTestId("toast");
    expect(toast).toHaveAttribute("aria-hidden", "true");
    expect(toast).toHaveAttribute("inert");
  });

  it("still fades out when the user asks for reduced motion", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: query.includes("reduce"),
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
    try {
      const { rerender } = render(<Toast toast={info} onDismiss={vi.fn()} />);
      rerender(<Toast toast={null} onDismiss={vi.fn()} />);
      expect(screen.getByTestId("toast")).toHaveClass("leaving");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
