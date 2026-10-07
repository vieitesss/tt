import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MarkdownPreview } from "./MarkdownPreview";

function renderBody(body: string, overrides: Partial<Parameters<typeof MarkdownPreview>[0]> = {}) {
  const onNavigate = vi.fn();
  const onExternal = vi.fn();
  const utils = render(
    <MarkdownPreview
      body={body}
      resolveTitle={() => undefined}
      onNavigate={onNavigate}
      onExternal={onExternal}
      {...overrides}
    />,
  );
  return { ...utils, onNavigate, onExternal };
}

describe("MarkdownPreview", () => {
  it("never executes raw HTML", () => {
    const { container } = renderBody("before\n\n<script>alert(1)</script>\n\nafter");
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("before");
    expect(container.textContent).toContain("after");
  });

  it("never loads remote images", () => {
    const { container } = renderBody("![tracker](https://evil.example/pixel.png)");
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("[image: tracker]");
  });

  it("routes external links through the validated opener", () => {
    const { onExternal } = renderBody("[docs](https://example.com/a)");
    const link = screen.getByRole("link", { name: "docs" });
    fireEvent.click(link);
    expect(onExternal).toHaveBeenCalledWith("https://example.com/a");
  });

  it("renders non-http schemes as inert text", () => {
    const { container } = renderBody("[run](javascript:alert(1))");
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toContain("run");
  });

  it("resolves a wikilink to the live title and navigates by id", () => {
    const { onNavigate } = renderBody("see [[abc1234567|stale alias]]", {
      resolveTitle: (id) => (id === "abc1234567" ? "Live title" : undefined),
    });
    const button = screen.getByRole("button", { name: "Live title" });
    fireEvent.click(button);
    expect(onNavigate).toHaveBeenCalledWith("abc1234567");
  });

  it("marks a dangling wikilink with its alias and still targets the id", () => {
    const { onNavigate } = renderBody("see [[missing001|Ghost alias]]");
    const button = screen.getByRole("button", { name: "Ghost alias" });
    expect(button).toHaveClass("dangling");
    fireEvent.click(button);
    expect(onNavigate).toHaveBeenCalledWith("missing001");
  });

  it("keeps wikilinks inside code literal", () => {
    const { container } = renderBody("`[[abc1234567]]`");
    expect(container.textContent).toContain("[[abc1234567]]");
    expect(container.querySelector("button")).toBeNull();
  });
});
