//! Backend-neutral styled-span and block model.
//!
//! The TUI markdown renderer emits `ratatui::Span`s whose colors are chosen
//! from *semantic* helpers (`md_dim_color()`, `bold_color()`, `code_fg()`,
//! ...), not raw colors. We preserve that: the neutral model carries a
//! semantic [`StyleRole`] plus boolean text attributes, and each front-end
//! adapter resolves the role to a concrete color for its backend (terminal
//! palette vs. desktop theme).

use serde::{Deserialize, Serialize};

/// Semantic color role. Front-ends map these to concrete colors so the same
/// document renders consistently across backends while still honoring each
/// backend's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum StyleRole {
    /// Default body text.
    #[default]
    Text,
    /// De-emphasized text (markers, rules, quote bars, metadata).
    Dim,
    /// Headings / strong emphasis color.
    Strong,
    /// Inline code and code-block foreground.
    Code,
    /// Hyperlink text.
    Link,
    /// Inline/raw HTML passthrough.
    Html,
    /// Model "reasoning" text (dim + italic latch in the TUI).
    Reasoning,
    /// Math (inline `$..$` or display `$$..$$`) content.
    Math,
}

/// Background fill role for a span. Most spans have no background; code uses a
/// distinct fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FillRole {
    #[default]
    None,
    Code,
}

/// Text attributes that are backend-independent (bold/italic/etc map cleanly
/// onto both terminal modifiers and font variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct TextAttrs {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}

impl TextAttrs {
    pub const fn none() -> Self {
        Self {
            bold: false,
            italic: false,
            strikethrough: false,
            underline: false,
        }
    }
}

/// A run of text sharing one style. The atomic unit of the neutral model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledSpan {
    pub text: String,
    pub role: StyleRole,
    pub fill: FillRole,
    pub attrs: TextAttrs,
}

impl StyledSpan {
    pub fn new(text: impl Into<String>, role: StyleRole) -> Self {
        Self {
            text: text.into(),
            role,
            fill: FillRole::None,
            attrs: TextAttrs::none(),
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, StyleRole::Text)
    }

    pub fn with_fill(mut self, fill: FillRole) -> Self {
        self.fill = fill;
        self
    }

    pub fn with_attrs(mut self, attrs: TextAttrs) -> Self {
        self.attrs = attrs;
        self
    }

    pub fn bold(mut self) -> Self {
        self.attrs.bold = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.attrs.italic = true;
        self
    }
}

/// Horizontal alignment for a rendered line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

/// One visual line: a sequence of styled spans plus alignment. This is what
/// front-ends consume; the TUI adapter turns each into a `ratatui::Line`, the
/// desktop adapter into a glyph run.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StyledLine {
    pub spans: Vec<StyledSpan>,
    pub alignment: Alignment,
}

impl StyledLine {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_spans(spans: Vec<StyledSpan>) -> Self {
        Self {
            spans,
            alignment: Alignment::Left,
        }
    }

    pub fn aligned(spans: Vec<StyledSpan>, alignment: Alignment) -> Self {
        Self { spans, alignment }
    }

    /// Plain-text content of the line (spans concatenated), for measuring and
    /// copy/extraction.
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }
}

/// Kind of a semantic block, retained so front-ends and copy/extraction logic
/// can reason about structure (e.g. "this block is a fenced code block in
/// language X") without re-parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockKind {
    Paragraph,
    Heading {
        level: u8,
    },
    CodeBlock {
        language: Option<String>,
    },
    BlockQuote,
    ListItem {
        ordered: bool,
        depth: usize,
    },
    /// GFM table. The raw cell text per row is carried in [`Block::table`]
    /// (the first row is the header); column layout/wrapping is width-dependent
    /// and therefore performed by each front-end adapter.
    Table,
    /// Display math (`$$..$$`). Raw math source lines are in [`Block::lines`]
    /// with [`StyleRole::Math`]; the adapter frames it.
    MathDisplay,
    ThematicBreak,
    Html,
}

/// A semantic block: its kind plus the already-laid-out lines it produced.
/// Front-ends mostly render `lines`; `kind` enables copy targets, spacing
/// policy, and structural styling decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub lines: Vec<StyledLine>,
    /// Raw table cells (row-major, first row is the header). Only populated for
    /// [`BlockKind::Table`]; layout is deferred to the front-end adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub table: Vec<Vec<String>>,
}

impl Block {
    pub fn new(kind: BlockKind, lines: Vec<StyledLine>) -> Self {
        Self {
            kind,
            lines,
            table: Vec::new(),
        }
    }

    /// Construct a table block carrying raw cells (row-major, header first).
    pub fn table(rows: Vec<Vec<String>>) -> Self {
        Self {
            kind: BlockKind::Table,
            lines: Vec::new(),
            table: rows,
        }
    }
}

/// A fully parsed, backend-neutral document.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Document {
    pub blocks: Vec<Block>,
}

impl Document {
    /// All lines across all blocks, in order. Convenient for line-oriented
    /// front-ends that don't need block structure.
    pub fn lines(&self) -> impl Iterator<Item = &StyledLine> {
        self.blocks.iter().flat_map(|b| b.lines.iter())
    }
}
