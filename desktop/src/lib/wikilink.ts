// Wikilink transform for Markdown bodies.
//
// This mirrors the accepted forms of the core parser (`TaskId::parse_link_target`):
// strip a `.md` suffix, take the last path component, and accept
// `[0-9a-z_-]{1,64}`. Aliases stay display-only; identity is always the target
// id. Code spans and fences are never rewritten, because the remark plugin only
// sees text nodes.

const TASK_ID = /^[0-9a-z_-]{1,64}$/;

export interface WikiText {
  kind: "text";
  value: string;
}

export interface WikiLink {
  kind: "link";
  target: string;
  label: string;
}

export type WikiSegment = WikiText | WikiLink;

/** Normalize an accepted wikilink target form to a task id, or `null`. */
export function parseWikilinkTarget(raw: string): string | null {
  let value = raw.trim();
  if (value.endsWith(".md")) value = value.slice(0, -3);
  const base = value.split(/[/\\]/).pop() ?? "";
  return TASK_ID.test(base) ? base : null;
}

/** Split a text run into literal text and wikilink references. */
export function splitWikilinks(text: string): WikiSegment[] {
  const segments: WikiSegment[] = [];
  const pushText = (value: string): void => {
    if (value === "") return;
    const last = segments[segments.length - 1];
    if (last?.kind === "text") {
      segments[segments.length - 1] = { kind: "text", value: last.value + value };
    } else {
      segments.push({ kind: "text", value });
    }
  };
  let rest = text;
  for (;;) {
    const start = rest.indexOf("[[");
    if (start === -1) break;
    const afterOpen = rest.slice(start + 2);
    const end = afterOpen.indexOf("]]");
    if (end === -1) break;
    const inner = afterOpen.slice(0, end).trim();
    if (start > 0) pushText(rest.slice(0, start));
    const pipe = inner.indexOf("|");
    const rawTarget = pipe === -1 ? inner : inner.slice(0, pipe);
    const target = parseWikilinkTarget(rawTarget);
    if (target) {
      const alias = pipe === -1 ? "" : inner.slice(pipe + 1).trim();
      segments.push({ kind: "link", target, label: alias || target });
    } else {
      pushText(rest.slice(start, start + 2 + end + 2));
    }
    rest = afterOpen.slice(end + 2);
  }
  if (rest.length > 0) pushText(rest);
  return segments;
}

interface MdastNode {
  type: string;
  value?: string;
  children?: MdastNode[];
  url?: string;
}

/** Remark plugin: turn `[[id|alias]]` text into `tttask:<id>` links. */
export function remarkWikilinks() {
  return (tree: MdastNode): void => {
    rewriteChildren(tree);
  };
}

function rewriteChildren(node: MdastNode): void {
  if (!node.children) return;
  const next: MdastNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && typeof child.value === "string") {
      next.push(...textToNodes(child.value));
    } else {
      // Do not recurse into link children: a wikilink inside a markdown link
      // would produce an invalid nested link.
      if (child.type !== "link") rewriteChildren(child);
      next.push(child);
    }
  }
  node.children = next;
}

function textToNodes(value: string): MdastNode[] {
  return splitWikilinks(value).map((segment) =>
    segment.kind === "text"
      ? { type: "text", value: segment.value }
      : {
          type: "link",
          url: `tttask:${segment.target}`,
          children: [{ type: "text", value: segment.label }],
        },
  );
}
