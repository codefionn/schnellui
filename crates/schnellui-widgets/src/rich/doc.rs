//! The format-agnostic rich document model (SOUL §8.1) — the structure the
//! [`RichText`](crate::RichText) viewer renders.
//!
//! **Parsing is the implementor's responsibility** (by design, for now): the
//! framework defines blocks, spans, and style axes plus ergonomic builders;
//! a Markdown / OpenDocument / source-code importer maps its format onto this
//! model in application code. The model is deliberately small: a document is a
//! flat list of **blocks** (paragraphs, headings, code blocks, list items,
//! quotes, rules), each made of styled inline **spans**. Style is a set of
//! orthogonal axes (bold / italic / mono / …) plus an optional explicit color
//! (the channel a syntax highlighter uses); the viewer's theme resolves
//! everything else. Documents are built or replaced at build / change time —
//! never on the steady-state re-render path (SOUL §1, §4.1).

use schnellui_scene::Color;

/// Inline style axes for one span. Axes compose (bold + italic + link is
/// legal); the viewer maps them onto the embedded faces and theme colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    /// inline code — rendered in the mono face with the code accent color.
    pub code: bool,
    pub strike: bool,
    pub underline: bool,
    /// a hyperlink — rendered underlined in the link color. v0 renders the
    /// link *text*; carry targets in application state (no per-span hit
    /// testing yet — SOUL §11 honesty: navigation is future work).
    pub link: bool,
    /// An explicit color override (e.g. a syntax token). `None` defers to the
    /// viewer theme.
    pub color: Option<Color>,
}

impl SpanStyle {
    pub const PLAIN: SpanStyle = SpanStyle {
        bold: false,
        italic: false,
        code: false,
        strike: false,
        underline: false,
        link: false,
        color: None,
    };

    pub fn bold() -> SpanStyle {
        SpanStyle {
            bold: true,
            ..SpanStyle::PLAIN
        }
    }
    pub fn italic() -> SpanStyle {
        SpanStyle {
            italic: true,
            ..SpanStyle::PLAIN
        }
    }
    pub fn code() -> SpanStyle {
        SpanStyle {
            code: true,
            ..SpanStyle::PLAIN
        }
    }
    pub fn link() -> SpanStyle {
        SpanStyle {
            link: true,
            underline: true,
            ..SpanStyle::PLAIN
        }
    }
    /// Mono with an explicit color — the shape of one syntax-highlight token.
    pub fn token(color: Color) -> SpanStyle {
        SpanStyle {
            code: true,
            color: Some(color),
            ..SpanStyle::PLAIN
        }
    }
}

/// One styled run of text within a block.
#[derive(Clone, Debug, PartialEq)]
pub struct RichSpan {
    pub text: String,
    pub style: SpanStyle,
}

impl RichSpan {
    pub fn new(text: impl Into<String>, style: SpanStyle) -> RichSpan {
        RichSpan {
            text: text.into(),
            style,
        }
    }
    pub fn plain(text: impl Into<String>) -> RichSpan {
        RichSpan::new(text, SpanStyle::PLAIN)
    }
    pub fn bold(text: impl Into<String>) -> RichSpan {
        RichSpan::new(text, SpanStyle::bold())
    }
    pub fn italic(text: impl Into<String>) -> RichSpan {
        RichSpan::new(text, SpanStyle::italic())
    }
    pub fn code(text: impl Into<String>) -> RichSpan {
        RichSpan::new(text, SpanStyle::code())
    }
    pub fn link(text: impl Into<String>) -> RichSpan {
        RichSpan::new(text, SpanStyle::link())
    }
    /// A syntax-highlight token (mono + explicit color).
    pub fn token(text: impl Into<String>, color: Color) -> RichSpan {
        RichSpan::new(text, SpanStyle::token(color))
    }
}

/// `"text"` lifts to a plain span, so builder calls read naturally:
/// `doc.paragraph(["plain ".into(), RichSpan::bold("bold")])`.
impl From<&str> for RichSpan {
    fn from(text: &str) -> RichSpan {
        RichSpan::plain(text)
    }
}
impl From<String> for RichSpan {
    fn from(text: String) -> RichSpan {
        RichSpan::plain(text)
    }
}

/// A list item's marker: an unordered bullet or an ordered number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMarker {
    Bullet,
    Number(u32),
}

/// One block of a rich document, in reading order.
#[derive(Clone, Debug, PartialEq)]
pub enum RichBlock {
    Paragraph(Vec<RichSpan>),
    /// level 1–6 (clamped); rendered larger + bold by the viewer.
    Heading {
        level: u8,
        spans: Vec<RichSpan>,
    },
    /// code: one span list per source line (kept per-line so the viewer lays
    /// lines out without re-splitting on `\n`). Spans render in the mono face;
    /// an importer/highlighter colors them via [`SpanStyle::token`].
    CodeBlock {
        language: String,
        lines: Vec<Vec<RichSpan>>,
    },
    /// one item of a (possibly nested) list; `depth` starts at 0.
    ListItem {
        depth: u8,
        marker: ListMarker,
        spans: Vec<RichSpan>,
    },
    Quote(Vec<RichSpan>),
    /// a thematic break (horizontal rule).
    Rule,
}

/// A rich document — a flat list of blocks. Construct it directly or through
/// the chainable builders:
///
/// ```ignore
/// let doc = RichDoc::new()
///     .heading(1, ["Title"])
///     .paragraph(["Body with ".into(), RichSpan::bold("emphasis"), ".".into()])
///     .code_block("rust", [vec![RichSpan::token("fn", KEYWORD), " main() {}".into()]])
///     .quote(["Quoted."])
///     .rule();
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichDoc {
    pub blocks: Vec<RichBlock>,
}

impl RichDoc {
    pub fn new() -> RichDoc {
        RichDoc::default()
    }

    /// Plain text lifted into paragraphs (split on blank lines, single
    /// newlines soft-wrap) — a convenience for unformatted content, not a
    /// markup parser.
    pub fn plain(text: &str) -> RichDoc {
        let mut doc = RichDoc::new();
        for para in text.split("\n\n") {
            let joined = para
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !joined.is_empty() {
                doc.blocks
                    .push(RichBlock::Paragraph(vec![RichSpan::plain(joined)]));
            }
        }
        doc
    }

    /// Appends any block.
    pub fn block(mut self, block: RichBlock) -> RichDoc {
        self.blocks.push(block);
        self
    }

    pub fn heading<S: Into<RichSpan>>(
        self,
        level: u8,
        spans: impl IntoIterator<Item = S>,
    ) -> RichDoc {
        let spans = spans.into_iter().map(Into::into).collect();
        self.block(RichBlock::Heading {
            level: level.clamp(1, 6),
            spans,
        })
    }

    pub fn paragraph<S: Into<RichSpan>>(self, spans: impl IntoIterator<Item = S>) -> RichDoc {
        self.block(RichBlock::Paragraph(
            spans.into_iter().map(Into::into).collect(),
        ))
    }

    pub fn code_block(
        self,
        language: impl Into<String>,
        lines: impl IntoIterator<Item = Vec<RichSpan>>,
    ) -> RichDoc {
        self.block(RichBlock::CodeBlock {
            language: language.into(),
            lines: lines.into_iter().collect(),
        })
    }

    pub fn list_item<S: Into<RichSpan>>(
        self,
        depth: u8,
        marker: ListMarker,
        spans: impl IntoIterator<Item = S>,
    ) -> RichDoc {
        let spans = spans.into_iter().map(Into::into).collect();
        self.block(RichBlock::ListItem {
            depth,
            marker,
            spans,
        })
    }

    pub fn bullet<S: Into<RichSpan>>(self, spans: impl IntoIterator<Item = S>) -> RichDoc {
        self.list_item(0, ListMarker::Bullet, spans)
    }

    pub fn quote<S: Into<RichSpan>>(self, spans: impl IntoIterator<Item = S>) -> RichDoc {
        self.block(RichBlock::Quote(
            spans.into_iter().map(Into::into).collect(),
        ))
    }

    pub fn rule(self) -> RichDoc {
        self.block(RichBlock::Rule)
    }

    /// The document's plain text (block texts joined by newlines) — the
    /// accessible value of the viewer (SOUL §6.2: semantics carry the content
    /// a screen reader reads, not the pixels).
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            if !out.is_empty() {
                out.push('\n');
            }
            match block {
                RichBlock::Paragraph(spans)
                | RichBlock::Heading { spans, .. }
                | RichBlock::ListItem { spans, .. }
                | RichBlock::Quote(spans) => {
                    for s in spans {
                        out.push_str(&s.text);
                    }
                }
                RichBlock::CodeBlock { lines, .. } => {
                    for (i, line) in lines.iter().enumerate() {
                        if i > 0 {
                            out.push('\n');
                        }
                        for s in line {
                            out.push_str(&s.text);
                        }
                    }
                }
                RichBlock::Rule => out.push_str("---"),
            }
        }
        out
    }

    /// The first heading's text, if any — the viewer's accessible name.
    pub fn title(&self) -> Option<String> {
        self.blocks.iter().find_map(|b| match b {
            RichBlock::Heading { spans, .. } => {
                let t: String = spans.iter().map(|s| s.text.as_str()).collect();
                (!t.is_empty()).then_some(t)
            }
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_splits_paragraphs_on_blank_lines() {
        let d = RichDoc::plain("one\ntwo\n\nthree\n");
        assert_eq!(d.blocks.len(), 2);
        assert_eq!(
            d.blocks[0],
            RichBlock::Paragraph(vec![RichSpan::plain("one two")])
        );
        assert_eq!(
            d.blocks[1],
            RichBlock::Paragraph(vec![RichSpan::plain("three")])
        );
    }

    #[test]
    fn builders_compose_a_document() {
        let d = RichDoc::new()
            .heading(1, ["Title"])
            .paragraph(["a ".into(), RichSpan::bold("b")])
            .bullet(["item"])
            .code_block("rust", [vec![RichSpan::code("fn main() {}")]])
            .quote(["q"])
            .rule();
        assert_eq!(d.blocks.len(), 6);
        assert!(matches!(&d.blocks[0], RichBlock::Heading { level: 1, .. }));
        assert!(matches!(
            &d.blocks[2],
            RichBlock::ListItem {
                marker: ListMarker::Bullet,
                ..
            }
        ));
        assert!(matches!(&d.blocks[5], RichBlock::Rule));
        // heading level clamps
        let d = RichDoc::new().heading(9, ["x"]);
        assert!(matches!(&d.blocks[0], RichBlock::Heading { level: 6, .. }));
    }

    #[test]
    fn plain_text_roundtrip_and_title() {
        let d = RichDoc::new()
            .heading(1, ["Title"])
            .paragraph(["a ".into(), RichSpan::bold("b")]);
        assert_eq!(d.title().as_deref(), Some("Title"));
        assert_eq!(d.plain_text(), "Title\na b");
    }
}
