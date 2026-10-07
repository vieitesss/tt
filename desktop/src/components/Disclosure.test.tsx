import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Disclosure } from "./Disclosure";

function renderDisclosure(resetKey?: string) {
  return render(
    <div>
      <button type="button">outside</button>
      <Disclosure label="Properties" resetKey={resetKey}>
        <button type="button">Rename</button>
      </Disclosure>
    </div>,
  );
}

/** A rect as jsdom reports it: only the fields the clamp reads are meaningful. */
function box(left: number, right: number): DOMRect {
  return {
    left,
    right,
    top: 0,
    bottom: 10,
    width: right - left,
    height: 10,
    x: left,
    y: 0,
    toJSON: () => ({}),
  } as DOMRect;
}

function renderClippedPanel(getPanelBox: () => DOMRect, clipBox = box(100, 400)) {
  const real = Element.prototype.getBoundingClientRect;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (
    this: Element,
  ) {
    if (this.classList.contains("disclosure-panel")) return getPanelBox();
    if (this.getAttribute("data-testid") === "clip") return clipBox;
    return real.call(this);
  });
  return render(
    <div data-testid="clip" style={{ overflowX: "hidden" }}>
      <Disclosure label="Properties">
        <button type="button">Rename</button>
      </Disclosure>
    </div>,
  );
}

async function openPanel() {
  await userEvent.setup().click(screen.getByRole("button", { name: "Properties" }));
  return document.querySelector(".disclosure-panel") as HTMLElement;
}

describe("Disclosure", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });
  it("starts collapsed and toggles its named region with aria-expanded", async () => {
    const user = userEvent.setup();
    renderDisclosure();

    const trigger = screen.getByRole("button", { name: "Properties" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Rename" })).not.toBeInTheDocument();

    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Rename" })).toBeInTheDocument();

    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Rename" })).not.toBeInTheDocument();
  });

  it("closes on Escape and returns focus to the trigger", async () => {
    const user = userEvent.setup();
    renderDisclosure();
    const trigger = screen.getByRole("button", { name: "Properties" });

    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "Rename" }));
    await user.keyboard("{Escape}");

    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveFocus();
  });

  it("closes on an outside press but not on an inside press", async () => {
    const user = userEvent.setup();
    renderDisclosure();
    const trigger = screen.getByRole("button", { name: "Properties" });

    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "Rename" }));
    expect(trigger).toHaveAttribute("aria-expanded", "true");

    await user.click(screen.getByRole("button", { name: "outside" }));
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("closes when the context key changes so a panel never leaks selection", async () => {
    const user = userEvent.setup();
    const { rerender } = renderDisclosure("task-a");
    const trigger = screen.getByRole("button", { name: "Properties" });

    await user.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");

    rerender(
      <div>
        <button type="button">outside</button>
        <Disclosure label="Properties" resetKey="task-b">
          <button type="button">Rename</button>
        </Disclosure>
      </div>,
    );
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Rename" })).not.toBeInTheDocument();
  });

  it("keeps a panel that overflows the left edge of its pane inside it", async () => {
    renderClippedPanel(() => box(60, 360));
    const panel = await openPanel();
    expect(panel.style.transform).toBe("translateX(40px)");
  });

  it("keeps a panel that overflows the right edge of its pane inside it", async () => {
    renderClippedPanel(() => box(200, 560));
    const panel = await openPanel();
    expect(panel.style.transform).toBe("translateX(-160px)");
  });

  it("leaves a panel that already fits its pane untouched", async () => {
    renderClippedPanel(() => box(120, 380));
    const panel = await openPanel();
    expect(panel.style.transform).toBe("");
  });

  it("re-clamps an open panel when the window shrinks", async () => {
    let panelBox = box(120, 380);
    renderClippedPanel(() => panelBox);
    const panel = await openPanel();
    expect(panel.style.transform).toBe("");

    panelBox = box(60, 360);
    window.dispatchEvent(new Event("resize"));

    expect(panel.style.transform).toBe("translateX(40px)");
  });

  it("re-clamps an open panel when its pane is resized", async () => {
    const callbacks: ResizeObserverCallback[] = [];
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(callback: ResizeObserverCallback) {
          callbacks.push(callback);
        }
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    );
    let panelBox = box(120, 380);
    renderClippedPanel(() => panelBox);
    const panel = await openPanel();
    expect(callbacks).toHaveLength(1);
    expect(panel.style.transform).toBe("");

    panelBox = box(200, 560);
    for (const callback of callbacks) callback([], {} as ResizeObserver);

    expect(panel.style.transform).toBe("translateX(-160px)");
  });

  it("keeps its panel mounted, leaving and inert, until the close transition ends", async () => {
    const user = userEvent.setup();
    renderDisclosure();
    const trigger = screen.getByRole("button", { name: "Properties" });
    await user.click(trigger);
    const panel = document.querySelector(".disclosure-panel") as HTMLElement;
    expect(panel).toHaveClass("reveal");

    await user.click(trigger);
    expect(document.querySelector(".disclosure-panel")).toBe(panel);
    expect(panel).toHaveClass("leaving");
    expect(panel).toHaveAttribute("inert");
    expect(panel).toHaveAttribute("aria-hidden", "true");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).not.toHaveAttribute("aria-controls");
    expect(screen.queryByRole("button", { name: "Rename" })).not.toBeInTheDocument();

    fireEvent.transitionEnd(panel);
    expect(document.querySelector(".disclosure-panel")).toBeNull();
  });

  it("reopens the same panel when it is toggled again mid-exit", async () => {
    const user = userEvent.setup();
    renderDisclosure();
    const trigger = screen.getByRole("button", { name: "Properties" });
    await user.click(trigger);
    const panel = document.querySelector(".disclosure-panel") as HTMLElement;
    await user.click(trigger);
    await user.click(trigger);

    expect(document.querySelector(".disclosure-panel")).toBe(panel);
    expect(panel).not.toHaveClass("leaving");
    expect(panel).not.toHaveAttribute("inert");
    expect(screen.getByRole("button", { name: "Rename" })).toBeInTheDocument();
  });
});
