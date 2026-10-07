import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { EXIT_FALLBACK_MS } from "../hooks/useExit";
import { ConfirmDialog } from "./ConfirmDialog";

const request = {
  title: "Delete task",
  message: 'Delete "X"?',
  confirmLabel: "Delete",
  destructive: true,
};

describe("ConfirmDialog", () => {
  it("focuses Cancel when it opens", () => {
    render(<ConfirmDialog request={request} onResolve={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
  });

  it("Escape cancels and never confirms", () => {
    const onResolve = vi.fn();
    render(<ConfirmDialog request={request} onResolve={onResolve} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onResolve).toHaveBeenCalledWith(false);
    expect(onResolve).not.toHaveBeenCalledWith(true);
  });

  it("a backdrop click cancels", () => {
    const onResolve = vi.fn();
    render(<ConfirmDialog request={request} onResolve={onResolve} />);
    const backdrop = screen.getByTestId("confirm-dialog").parentElement;
    expect(backdrop).not.toBeNull();
    fireEvent.mouseDown(backdrop as HTMLElement);
    expect(onResolve).toHaveBeenCalledWith(false);
  });

  it("only the explicit destructive button confirms", () => {
    const onResolve = vi.fn();
    render(<ConfirmDialog request={request} onResolve={onResolve} />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onResolve).toHaveBeenLastCalledWith(false);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onResolve).toHaveBeenLastCalledWith(true);
  });

  it("renders nothing without a request", () => {
    const { container } = render(<ConfirmDialog request={null} onResolve={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  describe("exit", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it("stays mounted, marked leaving and inert, until its transition ends", () => {
      const { rerender } = render(<ConfirmDialog request={request} onResolve={vi.fn()} />);
      const dialog = screen.getByTestId("confirm-dialog");
      const backdrop = dialog.parentElement as HTMLElement;
      expect(backdrop).not.toHaveClass("leaving");

      rerender(<ConfirmDialog request={null} onResolve={vi.fn()} />);
      expect(screen.getByTestId("confirm-dialog")).toBe(dialog);
      expect(backdrop).toHaveClass("leaving");
      expect(backdrop).toHaveAttribute("inert");
      expect(backdrop).toHaveAttribute("aria-hidden", "true");
      expect(dialog).toHaveTextContent("Delete task");

      fireEvent.transitionEnd(backdrop);
      expect(screen.queryByTestId("confirm-dialog")).not.toBeInTheDocument();
    });

    it("unmounts after the fallback timeout when no transition ever ends", () => {
      const { rerender } = render(<ConfirmDialog request={request} onResolve={vi.fn()} />);
      rerender(<ConfirmDialog request={null} onResolve={vi.fn()} />);

      act(() => vi.advanceTimersByTime(EXIT_FALLBACK_MS - 1));
      expect(screen.getByTestId("confirm-dialog")).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(1));
      expect(screen.queryByTestId("confirm-dialog")).not.toBeInTheDocument();
    });

    it("no longer answers Escape once it is leaving", () => {
      const onResolve = vi.fn();
      const { rerender } = render(<ConfirmDialog request={request} onResolve={onResolve} />);
      rerender(<ConfirmDialog request={null} onResolve={onResolve} />);
      fireEvent.keyDown(window, { key: "Escape" });
      expect(onResolve).not.toHaveBeenCalled();
    });

    it("still fades out when the user asks for reduced motion", () => {
      vi.stubGlobal("matchMedia", (query: string) => ({
        matches: query.includes("reduce"),
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      }));
      try {
        const { rerender } = render(<ConfirmDialog request={request} onResolve={vi.fn()} />);
        rerender(<ConfirmDialog request={null} onResolve={vi.fn()} />);
        expect(screen.getByTestId("confirm-dialog").parentElement).toHaveClass("leaving");
      } finally {
        vi.unstubAllGlobals();
      }
    });
  });
});
