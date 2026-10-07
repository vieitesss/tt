import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { IssuesBadge } from "./IssuesBadge";

const issues = [
  { path: "/tmp/store/a.md", kind: "unreadable", detail: "permission denied" },
  { path: "/tmp/store/b.md", kind: "malformed", detail: "no title" },
];

describe("IssuesBadge", () => {
  it("stays absent when the store reports nothing", () => {
    const { container } = render(<IssuesBadge issues={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("reports the count and dismisses the panel on Escape", async () => {
    const user = userEvent.setup();
    render(<IssuesBadge issues={issues} />);

    const trigger = screen.getByRole("button", { name: "⚠ 2 store issues" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");

    await user.click(trigger);
    expect(screen.getByRole("region", { name: "Store issues" })).toBeInTheDocument();
    expect(screen.getByText("permission denied")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("region", { name: "Store issues" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
});
