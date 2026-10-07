import { describe, expect, it } from "vitest";

import iconCss from "./components/icon.css?raw";
import menubarCss from "./menubar/menubar.css?raw";
import motionCss from "./motion.css?raw";
import stylesCss from "./styles.css?raw";

const stripComments = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, "");
const sheets = {
  "motion.css": stripComments(motionCss),
  "styles.css": stripComments(stylesCss),
  "menubar/menubar.css": stripComments(menubarCss),
  "components/icon.css": stripComments(iconCss),
};

/** The body of every `@media (prefers-reduced-motion: no-preference)` block. */
function noPreferenceBlocks(css: string): string[] {
  const blocks: string[] = [];
  const marker = "@media (prefers-reduced-motion: no-preference) {";
  for (let at = css.indexOf(marker); at !== -1; at = css.indexOf(marker, at + 1)) {
    let depth = 1;
    let end = at + marker.length;
    while (depth > 0 && end < css.length) {
      if (css[end] === "{") depth += 1;
      if (css[end] === "}") depth -= 1;
      end += 1;
    }
    blocks.push(css.slice(at + marker.length, end - 1));
  }
  return blocks;
}

describe("motion stylesheets", () => {
  it("reads real stylesheet text, not blanks", () => {
    for (const css of Object.values(sheets)) expect(css.length).toBeGreaterThan(100);
  });

  for (const [name, css] of Object.entries(sheets)) {
    it(`${name} never strips motion afterwards, uses ease-in, or transitions all`, () => {
      expect(css).not.toMatch(/prefers-reduced-motion:\s*reduce/);
      expect(css).not.toMatch(/!important/);
      expect(css).not.toMatch(/transition:\s*all\b/);
      expect(css).not.toMatch(/\bease-in\b(?!-out)/);
    });
  }

  it("keeps movement out of the rules every user gets", () => {
    for (const [name, css] of Object.entries(sheets)) {
      let outside = css;
      for (const block of noPreferenceBlocks(css)) outside = outside.replace(block, "");
      // Keyframes are named, not applied: what matters is where they are used,
      // so only look at declarations that move something.
      const withoutKeyframes = outside.replace(/@keyframes[^{]+\{(?:[^{}]|\{[^{}]*\})*\}/g, "");
      expect(withoutKeyframes, name).not.toMatch(/\b(?:translate|scale|rotate):/);
      expect(withoutKeyframes, name).not.toMatch(/transition:[^;]*\b(?:transform|translate|scale|clip-path)\b/);
    }
  });

  it("fades the Undo-window row out for everyone and slides it only with motion allowed", () => {
    const css = sheets["menubar/menubar.css"];
    expect(css).toMatch(/\.mb-row\.leaving\s*\{[^}]*animation:\s*row-fade-out/);
    const motion = noPreferenceBlocks(css).join("\n");
    expect(motion).toMatch(/\.mb-row\.leaving\s*\{[^}]*animation-name:\s*row-out/);
  });

  it("leaves exiting panels, dialogs and toasts a plain opacity fade under reduced motion", () => {
    for (const [file, selector] of [
      ["motion.css", ".reveal.leaving"],
      ["styles.css", ".modal-backdrop.leaving"],
      ["styles.css", ".toast.leaving"],
    ] as const) {
      let outside = sheets[file];
      for (const block of noPreferenceBlocks(sheets[file])) outside = outside.replace(block, "");
      expect(outside, selector).toContain(`${selector} {\n  opacity: 0;`);
    }
  });
});
