import { describe, expect, it } from "vitest";

import { parseWikilinkTarget, splitWikilinks } from "./wikilink";

describe("parseWikilinkTarget", () => {
  it("accepts the documented id forms", () => {
    expect(parseWikilinkTarget("abc1234567")).toBe("abc1234567");
    expect(parseWikilinkTarget("abc1234567.md")).toBe("abc1234567");
    expect(parseWikilinkTarget("./abc1234567.md")).toBe("abc1234567");
    expect(parseWikilinkTarget("sub/abc1234567.md")).toBe("abc1234567");
    expect(parseWikilinkTarget("a-b_c")).toBe("a-b_c");
  });

  it("rejects uppercase, double suffixes, and non-id stems", () => {
    expect(parseWikilinkTarget("UPPERCASE1")).toBeNull();
    expect(parseWikilinkTarget("abc1234567.md.md")).toBeNull();
    expect(parseWikilinkTarget("not a link")).toBeNull();
    expect(parseWikilinkTarget("")).toBeNull();
    expect(parseWikilinkTarget("a".repeat(65))).toBeNull();
  });
});

describe("splitWikilinks", () => {
  it("splits text around a wikilink and keeps the alias as a label", () => {
    expect(splitWikilinks("see [[abc1234567|Stale alias]] now")).toEqual([
      { kind: "text", value: "see " },
      { kind: "link", target: "abc1234567", label: "Stale alias" },
      { kind: "text", value: " now" },
    ]);
  });

  it("labels a bare link with its id", () => {
    expect(splitWikilinks("[[abc1234567]]")).toEqual([
      { kind: "link", target: "abc1234567", label: "abc1234567" },
    ]);
  });

  it("leaves invalid links literal", () => {
    expect(splitWikilinks("[[not a link]] after")).toEqual([
      { kind: "text", value: "[[not a link]] after" },
    ]);
  });

  it("stops at an unclosed opener without dropping text", () => {
    expect(splitWikilinks("before [[abc1234567")).toEqual([
      { kind: "text", value: "before [[abc1234567" },
    ]);
  });

  it("handles several links on one run", () => {
    expect(splitWikilinks("[[abc1234567]]/[[def1234567]]")).toEqual([
      { kind: "link", target: "abc1234567", label: "abc1234567" },
      { kind: "text", value: "/" },
      { kind: "link", target: "def1234567", label: "def1234567" },
    ]);
  });
});
