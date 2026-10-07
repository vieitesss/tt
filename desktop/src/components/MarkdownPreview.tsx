// Read-only Markdown rendering.
//
// Safety contract: react-markdown never executes raw HTML, remote images are
// never fetched (rendered as text), and every link is either routed through the
// Rust scheme-validated opener or treated as inert text. Wikilinks use the
// `tttask:` scheme injected by the remark transform and navigate in-app.

import ReactMarkdown, { defaultUrlTransform, type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

import { remarkWikilinks } from "../lib/wikilink";

export interface MarkdownPreviewProps {
  body: string;
  resolveTitle: (id: string) => string | undefined;
  onNavigate: (id: string) => void;
  onExternal: (url: string) => void;
}

const EXTERNAL = /^(https?|mailto):/i;

export function MarkdownPreview({
  body,
  resolveTitle,
  onNavigate,
  onExternal,
}: MarkdownPreviewProps) {
  const components: Components = {
    a({ href, children }) {
      const url = href ?? "";
      if (url.startsWith("tttask:")) {
        const target = url.slice("tttask:".length);
        const title = resolveTitle(target);
        return (
          <button
            type="button"
            className={title ? "wikilink" : "wikilink dangling"}
            title={title ? `Go to ${title}` : `Unresolved link → ${target}`}
            onClick={() => onNavigate(target)}
          >
            {title ?? children}
          </button>
        );
      }
      if (EXTERNAL.test(url)) {
        return (
          <a
            href={url}
            onClick={(event) => {
              event.preventDefault();
              onExternal(url);
            }}
          >
            {children}
          </a>
        );
      }
      return <span className="md-inert">{children}</span>;
    },
    img({ src, alt }) {
      return <span className="md-image">[image: {alt ?? String(src ?? "")}]</span>;
    },
  };

  return (
    <div className="markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkWikilinks]}
        components={components}
        // Let the in-app `tttask:` scheme through; everything else keeps the
        // default protocol sanitization.
        urlTransform={(url) => (url.startsWith("tttask:") ? url : defaultUrlTransform(url))}
      >
        {body}
      </ReactMarkdown>
    </div>
  );
}
