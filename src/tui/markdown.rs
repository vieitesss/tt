//! Markdown body rendering for the preview.
//!
//! The body is parsed from the in-memory task (never re-read from disk) with
//! `pulldown-cmark` and mapped onto ratatui [`Line`]s. Styling is expressive
//! but conservative: terminals have no font size, so headings change weight
//! and color, not scale, and the preview's `Paragraph` keeps owning wrapping.
//! Tables and other opt-in constructs are not enabled and render as plain
//! text.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tt::TaskId;

/// Blockquote line prefix, repeated per nesting depth.
const QUOTE_PREFIX: &str = "│ ";
/// Horizontal rule rendered for a thematic break.
const RULE_LINE: &str = "────────";
/// Background shared by inline code and fenced code blocks.
const CODE_BG: Color = Color::DarkGray;

/// Parse `body` and render it as preview lines.
///
/// `resolve` maps a `[[id]]` wikilink target to the target's title; returning
/// `None` renders the raw id. Wikilinks inside code stay literal.
pub(crate) fn preview_body_lines(
    body: &str,
    resolve: impl Fn(&str) -> Option<String>,
) -> Vec<Line<'static>> {
    let mut renderer = Renderer::default();
    for event in Parser::new_ext(body, options()) {
        renderer.event(event, &resolve);
    }
    renderer.finish(&resolve)
}

/// Enabled extensions: enough for the task bodies people write, no more.
/// Tables stay off so `| pipe |` rows degrade to plain text lines.
fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Style for headings: weight and color carry the level, never size.
fn heading_style(level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 => Style::default()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED),
        HeadingLevel::H2 => Style::default().add_modifier(Modifier::BOLD),
        _ => Style::default()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::DIM),
    }
}

/// Style for markdown links and resolved wikilinks.
fn link_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED)
}

/// Event-to-line renderer state.
#[derive(Default)]
struct Renderer {
    /// Finished lines.
    lines: Vec<Line<'static>>,
    /// Spans of the line being built.
    current: Vec<Span<'static>>,
    /// Active inline/heading styles, outermost first.
    styles: Vec<Style>,
    /// Open lists; `Some(next_number)` for ordered lists.
    lists: Vec<Option<u64>>,
    /// Open blockquote nesting depth.
    blockquote_depth: usize,
    /// Whether text events belong to a fenced or indented code block.
    in_code_block: bool,
    /// Inline text not yet split for wikilinks. pulldown-cmark emits runs
    /// like `[[id]]` as several text events, so they are only recognizable
    /// once the run is reassembled.
    pending_text: String,
    /// Emit one blank line before the next line begins.
    pending_blank: bool,
}

impl Renderer {
    fn event(&mut self, event: Event<'_>, resolve: &impl Fn(&str) -> Option<String>) {
        // Text runs are buffered so `[[id]]` stays in one piece; any other
        // event ends the run and is applied after it.
        if !matches!(event, Event::Text(_)) {
            self.flush_text(resolve);
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.in_code_block {
                    self.code_text(&text);
                } else {
                    self.pending_text.push_str(&text);
                }
            }
            Event::Code(code) => {
                let style = Style::default().bg(CODE_BG);
                self.push_span(Span::styled(code.into_string(), style));
            }
            Event::SoftBreak => self.push_span(Span::raw(" ")),
            Event::HardBreak => self.end_line(),
            Event::Rule => {
                self.start_block();
                self.push_span(Span::styled(RULE_LINE, self.base_style()));
                self.end_line();
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                self.push_span(Span::styled(marker, self.base_style()));
            }
            // HTML, math, footnotes, and any future event are not rendered:
            // their raw text would be noise without their semantics.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.lists.is_empty() {
                    self.start_block();
                }
            }
            Tag::Heading { level, .. } => {
                self.start_block();
                self.styles.push(heading_style(level));
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                self.in_code_block = true;
                // A fenced block's info string is shown dim, so the reader
                // can tell what the block is.
                if let CodeBlockKind::Fenced(info) = kind {
                    let info = info.trim();
                    if !info.is_empty() {
                        self.push_span(Span::styled(info.to_owned(), self.base_style()));
                        self.end_line();
                    }
                }
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.start_block();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.end_line();
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                let marker = match self.lists.last_mut() {
                    Some(Some(next)) => {
                        let marker = format!("{next}. ");
                        *next += 1;
                        marker
                    }
                    _ => "- ".to_owned(),
                };
                self.push_span(Span::styled(format!("{indent}{marker}"), self.base_style()));
            }
            Tag::Emphasis => self
                .styles
                .push(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self
                .styles
                .push(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self
                .styles
                .push(Style::default().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } | Tag::Image { .. } => self.styles.push(link_style()),
            // Tables and other opt-in constructs fall back to their text.
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Item => self.end_line(),
            TagEnd::Heading(_) => {
                self.end_line();
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.end_line();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.end_line();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.end_line();
                self.lists.pop();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                self.styles.pop();
            }
            _ => {}
        }
    }

    /// Append a line-structured code block's text, preserving blank lines but
    /// never adding a trailing empty line for the block's final newline.
    fn code_text(&mut self, text: &str) {
        let trimmed = text.strip_suffix('\n').unwrap_or(text);
        for (index, line) in trimmed.split('\n').enumerate() {
            if index > 0 {
                self.end_line();
            }
            let style = Style::default().bg(CODE_BG);
            self.push_span(Span::styled(line.to_owned(), style));
        }
    }

    /// Split and attach the buffered inline text. Called before every
    /// non-text event and once at the end, so styles are still active when
    /// the text lands (for example inside `**strong**`).
    fn flush_text(&mut self, resolve: &impl Fn(&str) -> Option<String>) {
        if self.pending_text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending_text);
        self.inline_text(&text, resolve);
    }

    /// Append plain text, turning every accepted `[[...]]` form into a link
    /// labeled with the target's live title. Aliases are display-only in the
    /// raw text and are never rendered; a dangling target falls back to its
    /// normalized id.
    fn inline_text(&mut self, text: &str, resolve: &impl Fn(&str) -> Option<String>) {
        let style = self.current_style();
        let mut rest = text;
        while let Some(start) = rest.find("[[") {
            let after_open = &rest[start + 2..];
            let Some(end) = after_open.find("]]") else {
                break;
            };
            let inner = after_open[..end].trim();
            if start > 0 {
                self.push_span(Span::styled(rest[..start].to_owned(), style));
            }
            let target = inner.split('|').next().unwrap_or(inner);
            match TaskId::parse_link_target(target) {
                Some(id) => {
                    let label = resolve(id.as_str()).unwrap_or_else(|| id.to_string());
                    self.push_span(Span::styled(label, link_style()));
                }
                None => {
                    self.push_span(Span::styled(
                        rest[start..start + 2 + end + 2].to_owned(),
                        style,
                    ));
                }
            }
            rest = &after_open[end + 2..];
        }
        if !rest.is_empty() {
            self.push_span(Span::styled(rest.to_owned(), style));
        }
    }

    /// Finish the current line and mark a blank separator before the next
    /// block. Inside a list no separator is inserted, so items stay tight.
    fn start_block(&mut self) {
        self.end_line();
        if !self.lines.is_empty() && self.lists.is_empty() {
            self.pending_blank = true;
        }
    }

    fn begin_line(&mut self) {
        if !self.current.is_empty() {
            return;
        }
        if self.pending_blank {
            self.pending_blank = false;
            self.lines.push(Line::default());
        }
        if self.blockquote_depth > 0 {
            self.current.push(Span::styled(
                QUOTE_PREFIX.repeat(self.blockquote_depth),
                self.base_style(),
            ));
        }
    }

    fn push_span(&mut self, span: Span<'static>) {
        self.begin_line();
        self.current.push(span);
    }

    fn end_line(&mut self) {
        if !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    /// Style for plain text: the active stack merged over the default base.
    fn current_style(&self) -> Style {
        self.styles
            .iter()
            .fold(Style::default(), |style, next| style.patch(*next))
    }

    /// Style for block furniture: bullets, rules, and quote bars.
    fn base_style(&self) -> Style {
        Style::default().fg(Color::DarkGray)
    }

    fn finish(mut self, resolve: &impl Fn(&str) -> Option<String>) -> Vec<Line<'static>> {
        self.flush_text(resolve);
        self.end_line();
        self.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_resolve(_: &str) -> Option<String> {
        None
    }

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn style_of(lines: &[Line<'_>], needle: &str) -> Style {
        for line in lines {
            for span in &line.spans {
                if span.content.contains(needle) {
                    return span.style;
                }
            }
        }
        panic!("{needle:?} not rendered");
    }

    #[test]
    fn headings_use_weight_not_size() {
        let lines = preview_body_lines("# One\n\n## Two\n\n### Three", no_resolve);

        let h1 = style_of(&lines, "One");
        assert!(h1.add_modifier.contains(Modifier::BOLD));
        assert!(h1.add_modifier.contains(Modifier::UNDERLINED));
        assert!(
            !h1.add_modifier.contains(Modifier::DIM),
            "h1 is not dim: {h1:?}"
        );

        let h2 = style_of(&lines, "Two");
        assert!(h2.add_modifier.contains(Modifier::BOLD));
        assert!(!h2.add_modifier.contains(Modifier::UNDERLINED));

        let h3 = style_of(&lines, "Three");
        assert!(h3.add_modifier.contains(Modifier::BOLD));
        assert!(h3.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn emphasis_strong_and_strikethrough_style_text() {
        let lines = preview_body_lines("*italic* and **bold** and ~~struck~~", no_resolve);

        assert!(style_of(&lines, "italic")
            .add_modifier
            .contains(Modifier::ITALIC));
        assert!(style_of(&lines, "bold")
            .add_modifier
            .contains(Modifier::BOLD));
        assert!(style_of(&lines, "struck")
            .add_modifier
            .contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn inline_and_fenced_code_get_a_distinct_background() {
        let lines = preview_body_lines("inline `code` here", no_resolve);
        assert_eq!(style_of(&lines, "code").bg, Some(CODE_BG));

        let lines = preview_body_lines("```rust\nlet x = 1;\n```", no_resolve);
        assert_eq!(style_of(&lines, "let x = 1;").bg, Some(CODE_BG));
        assert!(
            text_of(&lines).contains("rust"),
            "fence info is shown: {}",
            text_of(&lines)
        );

        // Markdown inside code stays literal.
        let lines = preview_body_lines("`a * b`", no_resolve);
        assert_eq!(text_of(&lines), "a * b");
    }

    #[test]
    fn lists_indent_and_task_markers_render() {
        let lines = preview_body_lines("- one\n- two\n  - nested", no_resolve);
        let text = text_of(&lines);
        assert!(text.contains("- one"), "{text}");
        assert!(text.contains("- two"), "{text}");
        assert!(text.contains("  - nested"), "nested list indents: {text}");

        let lines = preview_body_lines("1. first\n2. second", no_resolve);
        let text = text_of(&lines);
        assert!(text.contains("1. first"), "{text}");
        assert!(text.contains("2. second"), "{text}");

        let lines = preview_body_lines("- [ ] todo\n- [x] done", no_resolve);
        let text = text_of(&lines);
        assert!(text.contains("- [ ] todo"), "{text}");
        assert!(text.contains("- [x] done"), "{text}");
    }

    #[test]
    fn blockquotes_and_rules_render() {
        let lines = preview_body_lines("> quoted", no_resolve);
        assert!(text_of(&lines).contains("│ quoted"), "{}", text_of(&lines));

        let lines = preview_body_lines("before\n\n---\n\nafter", no_resolve);
        assert!(text_of(&lines).contains(RULE_LINE), "{}", text_of(&lines));
    }

    #[test]
    fn markdown_links_show_text_not_the_url() {
        let lines = preview_body_lines("[Rust](https://rust-lang.org)", no_resolve);

        assert_eq!(text_of(&lines), "Rust");
        let style = style_of(&lines, "Rust");
        assert_eq!(style.fg, Some(Color::Cyan));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn wikilinks_resolve_or_fall_back_to_the_raw_id() {
        let resolve = |id: &str| (id == "target0001").then(|| "Target title".to_owned());
        let lines = preview_body_lines("see [[target0001]] and [[missing001]]", resolve);
        let text = text_of(&lines);

        assert!(text.contains("Target title"), "{text}");
        assert!(text.contains("missing001"), "{text}");
        assert!(!text.contains("[[target0001]]"), "{text}");
        assert_eq!(style_of(&lines, "Target title").fg, Some(Color::Cyan));
        assert_eq!(style_of(&lines, "missing001").fg, Some(Color::Cyan));
    }

    #[test]
    fn wikilinks_ignore_aliases_and_resolve_rich_forms() {
        let resolve = |id: &str| (id == "target0001").then(|| "Target title".to_owned());
        let lines = preview_body_lines(
            "[[target0001.md|Stale alias]] and [[sub/target0001.md]] and [[missing001.md|Ghost alias]]",
            resolve,
        );
        let text = text_of(&lines);

        assert!(text.contains("Target title"), "{text}");
        assert!(
            !text.contains("Stale alias"),
            "the alias is never displayed: {text}"
        );
        assert!(
            text.contains("missing001"),
            "a dangling target shows its stem: {text}"
        );
        assert!(!text.contains("Ghost alias"), "{text}");
    }

    #[test]
    fn wikilinks_inside_code_stay_literal() {
        let resolve = |_: &str| Some("SHOULD NOT RESOLVE".to_owned());
        let lines = preview_body_lines("`[[target0001]]`\n\n```\n[[target0001]]\n```", resolve);
        let text = text_of(&lines);

        assert_eq!(text.matches("[[target0001]]").count(), 2, "{text}");
        assert!(!text.contains("SHOULD NOT RESOLVE"), "{text}");
    }
}
