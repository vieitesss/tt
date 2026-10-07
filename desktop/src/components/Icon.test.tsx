import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Icon } from "./Icon";

describe("Icon", () => {
  it("draws a decorative 16 px svg that inherits the text colour", () => {
    const { container } = render(<Icon name="chevron-right" />);
    const svg = container.querySelector("svg");

    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(svg).toHaveAttribute("viewBox", "0 0 16 16");
    expect(svg).toHaveAttribute("stroke", "currentColor");
    expect(svg).toHaveAttribute("fill", "none");
    expect(svg).toHaveClass("icon-svg");
    expect(svg?.querySelector("path")?.getAttribute("d")).toBeTruthy();
  });

  it("marks a rotated fold chevron with the shared class", () => {
    const { container } = render(<Icon name="chevron-right" className="rotated" />);

    expect(container.querySelector("svg")).toHaveClass("icon-svg", "rotated");
  });
});
