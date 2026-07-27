//! The transcript: typed messages, rich text, and pixel-accurate geometry.
//!
//! The transcript used to be one flat `String` paginated by counting `'\n'`.
//! That was wrong in three separate ways at once, and all three were visible
//! on screen: a wrapped paragraph is taller than its newline count, so tall
//! content overflowed its region and drew straight through the composer;
//! scrolling moved by logical lines while the screen moves by pixels, so the
//! two disagreed as soon as anything wrapped; and a `String` has no structure,
//! so a user's message could only be distinguished from the model's reply by
//! prefixing it with a shell-style `>`.
//!
//! This module replaces all of that:
//!
//! - [`Message`] is the unit: who said it, and their markdown source.
//! - Markdown and LaTeX come from [`jcode_render_core`], the backend-neutral
//!   document model already shared with the TUI. Nothing about emphasis, code
//!   spans, lists, tables, or math is re-implemented here; this module only
//!   maps [`StyleRole`] onto the desktop theme and Parley font attributes.
//! - Geometry is measured, never estimated: every message is laid out by
//!   Parley and reports a real height in logical units, so scrolling is in
//!   pixels and the visible window is found by walking measured blocks.

use crate::text::{ParagraphStyle, TextSystem};
use crate::theme::Theme;
use jcode_render_core::{
    Block, BlockKind, Document, FillRole, StyleRole, StyledLine, parse_markdown,
};
use parley::{Layout, StyleProperty};
use vello::peniko::{Brush, Color};

/// Who produced a message. The transcript's structure, and the thing that
/// replaces the `>` marker: roles are styled, not labelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// One turn of the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    /// Markdown source. Kept raw so a streaming message can be re-parsed as it
    /// grows, and so copy yields what the model actually wrote.
    pub source: String,
}

impl Message {
    pub fn user(source: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            source: source.into(),
        }
    }

    pub fn assistant(source: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            source: source.into(),
        }
    }
}

/// The conversation. Streaming appends to the trailing assistant message
/// rather than pushing a new one per delta, so a reply is one block however
/// many chunks it arrived in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    messages: Vec<Message>,
}

impl Transcript {
    pub fn is_empty(&self) -> bool {
        self.messages
            .iter()
            .all(|message| message.source.trim().is_empty())
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Append streamed assistant text, continuing the current reply when there
    /// is one. Without this a reply would be split into one block per network
    /// chunk, and markdown spanning a chunk boundary would never parse.
    pub fn append_assistant(&mut self, text: &str) {
        match self.messages.last_mut() {
            Some(last) if last.role == Role::Assistant => last.source.push_str(text),
            _ => self.messages.push(Message::assistant(text)),
        }
    }

    /// Plain-text rendering, for tests and for copying the conversation.
    pub fn plain_text(&self) -> String {
        self.messages
            .iter()
            .map(|message| message.source.trim())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Characters in the trailing assistant reply, which is the message the
    /// streaming reveal is animating. Zero when the last turn is the user's,
    /// because nothing is arriving.
    pub fn streaming_len(&self) -> usize {
        match self.messages.last() {
            Some(last) if last.role == Role::Assistant => last.source.chars().count(),
            _ => 0,
        }
    }
}

impl From<&[Message]> for Transcript {
    fn from(messages: &[Message]) -> Self {
        Self {
            messages: messages.to_vec(),
        }
    }
}

/// A message laid out at a known width: its Parley layouts and their heights.
///
/// A message is several layouts rather than one, because block kinds do not
/// share a paragraph style: a code block is drawn on a wash at a different
/// inset from body copy, and a heading is a different size. Each entry
/// therefore carries its own offset within the message.
pub struct LaidMessage {
    pub role: Role,
    /// Laid-out blocks, in order, with their vertical offset from the top of
    /// the message and the kind that produced them.
    pub blocks: Vec<LaidBlock>,
    /// Total height of the message in logical units, including inter-block
    /// spacing but not the gap to the next message.
    pub height: f64,
}

impl LaidMessage {
    /// Vertical inset from the top of the message to its first block. A user
    /// message is drawn in a padded card; an assistant reply is not. Shared by
    /// drawing and hit-testing so a click cannot land a padding's worth away
    /// from the glyph it aimed at.
    pub fn top_padding(&self) -> f64 {
        match self.role {
            Role::User => USER_PAD_Y,
            Role::Assistant => 0.0,
        }
    }
}

pub struct LaidBlock {
    pub layout: Layout<Brush>,
    /// The block's flattened plain text, the same string the layout was built
    /// from. Kept so a pointer selection can slice it for the clipboard
    /// without re-parsing the markdown or reading back from the GPU.
    pub source: String,
    /// Offset from the top of the message, in logical units.
    pub top: f64,
    pub height: f64,
    /// Horizontal inset from the message's text left edge. Code blocks and
    /// quotes indent; body copy does not.
    pub inset: f64,
    pub kind: BlockKind,
}

/// Vertical rhythm inside and between messages, in logical units.
pub const BLOCK_GAP: f64 = 8.0;
pub const MESSAGE_GAP: f64 = 18.0;
/// Inset of a user message's text inside its tinted card.
pub const USER_PAD_X: f64 = 12.0;
pub const USER_PAD_Y: f64 = 8.0;
/// Corner radius of the user card. Matches the composer, so your message and
/// the box you typed it in are visibly the same object.
pub const USER_RADIUS: f64 = 6.0;
/// Inset of code-block text inside its wash.
pub const CODE_PAD_X: f64 = 10.0;
pub const CODE_PAD_Y: f64 = 6.0;
/// Indent applied to quoted text, leaving room for the quote rule.
pub const QUOTE_INSET: f64 = 12.0;

/// Resolve a render-core [`StyleRole`] to a concrete theme colour. This is the
/// whole of the desktop's "theme adapter": the neutral document says *what* a
/// span means, and only this function says what colour that is.
pub fn role_color(role: StyleRole, theme: &Theme) -> Color {
    match role {
        StyleRole::Text => theme.text,
        StyleRole::Dim => theme.muted,
        StyleRole::Strong => theme.text,
        StyleRole::Code => theme.text,
        StyleRole::Link => theme.text,
        StyleRole::Html => theme.muted,
        StyleRole::Reasoning => theme.muted,
        StyleRole::Math => theme.text,
    }
}

/// Whether a role implies a monospace-emphasis treatment. The whole app is
/// already a mono stack, so code is distinguished by its wash and colour
/// rather than by a family switch.
fn role_is_strong(role: StyleRole) -> bool {
    matches!(role, StyleRole::Strong)
}

/// Lay out one message to `width` logical units.
///
/// Every block is measured by Parley here, so the caller receives real
/// heights. Nothing downstream is allowed to estimate.
pub fn lay_out_message(
    text: &mut TextSystem,
    message: &Message,
    width: f64,
    theme: &Theme,
    base: ParagraphStyle,
    scale: f64,
) -> LaidMessage {
    // Markdown and math both come from render-core, so the desktop and the TUI
    // agree on what a document *is*; only the drawing differs.
    let document: Document = parse_markdown(&message.source);
    let mut blocks = Vec::new();
    let mut top = 0.0;

    for block in &document.blocks {
        let inset = block_inset(&block.kind);
        let style = block_style(&block.kind, base, theme);
        let lines = block_lines(block);
        if lines.is_empty() {
            continue;
        }
        let (source, spans) = flatten(&lines);
        if source.trim().is_empty() {
            continue;
        }
        let layout = layout_rich(
            text,
            &source,
            &spans,
            (width - inset * 2.0).max(1.0),
            style,
            theme,
            scale,
        );
        let mut height = f64::from(layout.height()) / scale;
        if matches!(block.kind, BlockKind::CodeBlock { .. }) {
            height += CODE_PAD_Y * 2.0;
        }
        blocks.push(LaidBlock {
            layout,
            source,
            top,
            height,
            inset,
            kind: block.kind.clone(),
        });
        top += height + BLOCK_GAP;
    }

    let mut height = (top - BLOCK_GAP).max(0.0);
    if message.role == Role::User {
        height += USER_PAD_Y * 2.0;
    }
    LaidMessage {
        role: message.role,
        blocks,
        height,
    }
}

/// Horizontal inset for a block kind, relative to the message's text column.
fn block_inset(kind: &BlockKind) -> f64 {
    match kind {
        BlockKind::CodeBlock { .. } => CODE_PAD_X,
        BlockKind::BlockQuote => QUOTE_INSET,
        _ => 0.0,
    }
}

/// Paragraph style for a block kind. Headings step up in size; everything else
/// inherits the body style, because a chat transcript is body copy with
/// occasional structure, not a document.
fn block_style(kind: &BlockKind, base: ParagraphStyle, theme: &Theme) -> ParagraphStyle {
    match kind {
        BlockKind::Heading { level } => ParagraphStyle {
            font_size: base.font_size * heading_scale(*level),
            bold: true,
            color: theme.text,
            ..base
        },
        BlockKind::CodeBlock { .. } => ParagraphStyle {
            color: theme.text,
            ..base
        },
        BlockKind::BlockQuote => ParagraphStyle {
            color: theme.muted,
            ..base
        },
        _ => base,
    }
}

/// Heading sizes, as multiples of body copy. Restrained on purpose: an h1 in a
/// chat reply is a sentence, not a cover page.
fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 1.32,
        2 => 1.18,
        3 => 1.08,
        _ => 1.0,
    }
}

/// The styled lines a block contributes.
///
/// Two kinds need front-end treatment. Tables are left to the front-end by
/// render-core because column widths depend on the measure. Quotes arrive with
/// a terminal `│ ` bar on every line, which the desktop replaces with a real
/// drawn rule; keeping both would mark the quote twice.
fn block_lines(block: &Block) -> Vec<StyledLine> {
    if block.kind == BlockKind::Table && block.lines.is_empty() {
        return table_lines(&block.table);
    }
    if block.kind == BlockKind::BlockQuote {
        return block
            .lines
            .iter()
            .map(|line| StyledLine {
                spans: strip_quote_bar(&line.spans),
                alignment: line.alignment,
            })
            .collect();
    }
    block.lines.clone()
}

/// Drop the leading terminal quote-bar span from a quoted line.
fn strip_quote_bar(spans: &[jcode_render_core::StyledSpan]) -> Vec<jcode_render_core::StyledSpan> {
    let mut spans = spans.to_vec();
    if spans
        .first()
        .is_some_and(|first| first.text.contains('\u{2502}'))
    {
        let first = &mut spans[0];
        first.text = first.text.trim_start_matches(['\u{2502}', ' ']).to_string();
        if first.text.is_empty() {
            spans.remove(0);
        }
    }
    spans
}

/// Lay a GFM table out as aligned columns.
///
/// The app is a monospace stack throughout, so padding each cell to the widest
/// in its column produces true columns. A naive `join` would put every row's
/// second cell at a different x, which reads worse than the markdown source.
fn table_lines(rows: &[Vec<String>]) -> Vec<StyledLine> {
    use jcode_render_core::{StyledSpan, TextAttrs};
    use unicode_width::UnicodeWidthStr;

    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.width())
                .max()
                .unwrap_or(0)
        })
        .collect();

    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let mut text = String::new();
            for (column, width) in widths.iter().enumerate() {
                let cell = row.get(column).map(String::as_str).unwrap_or("");
                text.push_str(cell);
                // No trailing padding on the last column: invisible, and it
                // makes the line measure wider than its ink.
                if column + 1 < columns {
                    text.push_str(&" ".repeat(width.saturating_sub(cell.width()) + 2));
                }
            }
            let mut span = StyledSpan::plain(text);
            // The header is the one piece of table structure worth carrying:
            // it says which way to read the rest.
            if index == 0 {
                span = span.with_attrs(TextAttrs {
                    bold: true,
                    ..TextAttrs::none()
                });
            }
            StyledLine::from_spans(vec![span])
        })
        .collect()
}

/// A span's byte range within the flattened source, plus its styling.
pub struct SpanStyle {
    pub range: std::ops::Range<usize>,
    pub role: StyleRole,
    /// Background fill role. Carried so a future inline-code wash can be drawn
    /// per span; block-level code already draws its own.
    #[allow(dead_code)]
    pub fill: FillRole,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// Flatten styled lines into one string plus byte-ranged styling. Parley wants
/// a single string with ranged properties, which is exactly the shape the
/// neutral model already has.
pub fn flatten(lines: &[StyledLine]) -> (String, Vec<SpanStyle>) {
    let mut source = String::new();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            source.push('\n');
        }
        for span in &line.spans {
            let start = source.len();
            source.push_str(&span.text);
            spans.push(SpanStyle {
                range: start..source.len(),
                role: span.role,
                fill: span.fill,
                bold: span.attrs.bold || role_is_strong(span.role),
                italic: span.attrs.italic,
                underline: span.attrs.underline,
                strikethrough: span.attrs.strikethrough,
            });
        }
    }
    (source, spans)
}

/// Lay out rich text: one Parley layout carrying per-span colour and weight.
///
/// This is the reason emphasis, code, links, and math can look different from
/// body copy without the transcript having to draw them as separate
/// paragraphs, which would break wrapping across a style boundary.
pub fn layout_rich(
    text: &mut TextSystem,
    source: &str,
    spans: &[SpanStyle],
    width: f64,
    style: ParagraphStyle,
    theme: &Theme,
    scale: f64,
) -> Layout<Brush> {
    text.layout_rich(source, width as f32, style, scale, &mut |builder| {
        for span in spans {
            if span.range.is_empty() {
                continue;
            }
            builder.push(
                StyleProperty::Brush(Brush::Solid(role_color(span.role, theme))),
                span.range.clone(),
            );
            if span.bold {
                builder.push(
                    StyleProperty::FontWeight(parley::FontWeight::BOLD),
                    span.range.clone(),
                );
            }
            if span.italic {
                builder.push(
                    StyleProperty::FontStyle(parley::FontStyle::Italic),
                    span.range.clone(),
                );
            }
            if span.underline {
                builder.push(StyleProperty::Underline(true), span.range.clone());
            }
            if span.strikethrough {
                builder.push(StyleProperty::Strikethrough(true), span.range.clone());
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{block_lines, table_lines};

    fn theme() -> Theme {
        Theme::print_light()
    }

    fn base() -> ParagraphStyle {
        ParagraphStyle {
            font_size: crate::layout::BODY_SIZE,
            line_height: crate::layout::BODY_LEADING as f32,
            ..Default::default()
        }
    }

    fn laid(source: &str) -> LaidMessage {
        let mut text = TextSystem::default();
        lay_out_message(
            &mut text,
            &Message::assistant(source),
            600.0,
            &theme(),
            base(),
            1.75,
        )
    }

    /// Streaming must continue one reply, not create a block per chunk:
    /// markdown spanning a chunk boundary would otherwise never parse.
    #[test]
    fn streaming_deltas_accumulate_into_one_message() {
        let mut transcript = Transcript::default();
        transcript.push(Message::user("hi"));
        transcript.append_assistant("**bo");
        transcript.append_assistant("ld**");
        assert_eq!(transcript.messages().len(), 2);
        assert_eq!(transcript.messages()[1].source, "**bold**");
    }

    /// The marker is gone: role is carried in the model, so nothing needs to
    /// prefix a caret onto the user's own words.
    #[test]
    fn a_user_message_carries_no_marker_text() {
        let transcript = Transcript::from(&[Message::user("hello")][..]);
        assert!(
            !transcript.plain_text().contains('>'),
            "a shell prompt marker leaked into the transcript text"
        );
    }

    /// Markdown reaches the layout as *styling*, not as literal asterisks.
    #[test]
    fn emphasis_is_styling_rather_than_punctuation() {
        let document = parse_markdown("**bold** and *italic* and `code`");
        let lines: Vec<_> = document.lines().cloned().collect();
        let (source, spans) = flatten(&lines);
        assert!(
            !source.contains('*') && !source.contains('`'),
            "markdown punctuation survived into the drawn text: {source:?}"
        );
        assert!(
            spans.iter().any(|span| span.bold),
            "no bold span was produced"
        );
        assert!(
            spans.iter().any(|span| span.italic),
            "no italic span was produced"
        );
        assert!(
            spans.iter().any(|span| span.role == StyleRole::Code),
            "no code span was produced"
        );
    }

    /// LaTeX is rendered, not echoed. render-core turns `$x^2$` into Unicode
    /// math; the desktop must be showing that rather than the source.
    #[test]
    fn inline_latex_renders_as_math() {
        let document = parse_markdown("the value $x^2$ grows");
        let text: String = document
            .lines()
            .map(|line| line.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains('\u{00b2}'),
            "inline latex was not rendered to math: {text:?}"
        );
        assert!(!text.contains("x^2"), "raw latex source survived: {text:?}");
    }

    #[test]
    fn display_latex_becomes_its_own_block() {
        let document = parse_markdown("before\n\n$$\\frac{a}{b}$$\n\nafter");
        assert!(
            document
                .blocks
                .iter()
                .any(|block| block.kind == BlockKind::MathDisplay),
            "display math did not become a math block"
        );
    }

    /// Height must be measured, not counted: a paragraph that wraps is taller
    /// than a paragraph that does not, even with the same newline count.
    #[test]
    fn measured_height_grows_with_wrapping() {
        let short = laid("one line");
        let long = laid(&"alpha bravo charlie delta echo foxtrot ".repeat(8));
        assert!(
            long.height > short.height * 2.0,
            "wrapped text measured {:.1}, barely more than {:.1}",
            long.height,
            short.height
        );
    }

    /// A user message reserves its card padding, so the tint cannot crop the
    /// text it wraps.
    #[test]
    fn a_user_message_reserves_its_card_padding() {
        let mut text = TextSystem::default();
        let user = lay_out_message(
            &mut text,
            &Message::user("hello"),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        let assistant = lay_out_message(
            &mut text,
            &Message::assistant("hello"),
            600.0,
            &theme(),
            base(),
            1.75,
        );
        assert!(
            user.height >= assistant.height + USER_PAD_Y * 2.0 - 0.01,
            "user card did not reserve padding: {:.1} vs {:.1}",
            user.height,
            assistant.height
        );
    }

    /// Structure survives into laid-out blocks, so the renderer can draw a
    /// code wash without re-parsing.
    #[test]
    fn code_blocks_keep_their_kind() {
        let laid = laid("text\n\n```rust\nfn main() {}\n```\n");
        assert!(
            laid.blocks
                .iter()
                .any(|block| matches!(block.kind, BlockKind::CodeBlock { .. })),
            "code block lost its kind"
        );
    }

    /// Awkward and hostile inputs must lay out rather than panic. A transcript
    /// renders whatever a model emits, so this is a real input space.
    #[test]
    fn hostile_markdown_lays_out_without_panicking() {
        for source in [
            "",
            "\n\n\n",
            "```",
            "```rust",
            "$$",
            "$x^",
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "> quote\n> more",
            "- a\n  - b\n    - c",
            "#".repeat(40).as_str(),
            "ünïcödé 中文 🎉 **bold**",
            &"word ".repeat(500),
        ] {
            let _ = laid(source);
        }
    }

    /// Tables are laid out rather than silently dropped: render-core leaves
    /// their width-dependent layout to the front-end, and a front-end that
    /// ignores that renders nothing at all.
    #[test]
    fn tables_produce_visible_lines() {
        let laid = laid("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(!laid.blocks.is_empty(), "table produced no drawable blocks");
    }

    /// Table cells line up into columns. A naive `join(" ")` adapter passes
    /// the test above while rendering an unreadable ragged block, so the
    /// alignment itself has to be asserted.
    #[test]
    fn table_columns_are_aligned() {
        let rows = vec![
            vec!["frame".to_string(), "direction".to_string()],
            vec!["hello".to_string(), "client".to_string()],
            vec!["a-much-longer-frame".to_string(), "server".to_string()],
        ];
        let lines = table_lines(&rows);
        let starts: Vec<usize> = lines
            .iter()
            .map(|line| {
                let text = line.plain_text();
                text.find(|c: char| c != ' ')
                    .map(|_| {
                        // Column two starts after the padded first cell.
                        text.len() - text.trim_start_matches(|c: char| c != ' ').len()
                    })
                    .unwrap_or(0)
            })
            .collect();
        let text: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
        let second_column: Vec<usize> = text
            .iter()
            .map(|line| line.rfind("  ").map(|i| i + 2).unwrap_or(0))
            .collect();
        assert!(
            second_column.windows(2).all(|w| w[0] == w[1]),
            "table columns did not align: {text:?} (starts {starts:?})"
        );
    }

    /// A quote is drawn as a rule by the renderer, so the terminal's `│`
    /// prefix must not also survive into the text.
    #[test]
    fn quotes_do_not_carry_a_terminal_bar() {
        let document = parse_markdown("> quoted line\n> second line");
        let quote = document
            .blocks
            .iter()
            .find(|block| block.kind == BlockKind::BlockQuote)
            .expect("no quote block");
        let lines = block_lines(quote);
        for line in &lines {
            assert!(
                !line.plain_text().contains('\u{2502}'),
                "quote bar survived: {:?}",
                line.plain_text()
            );
        }
    }
}
