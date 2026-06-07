#![allow(dead_code)]

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TranscriptRole {
    User,
    Assistant,
    Tool,
    System,
    Meta,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RichTranscriptMessage {
    pub(crate) id: TranscriptMessageId,
    pub(crate) role: TranscriptRole,
    pub(crate) content: String,
    pub(crate) attachments: Vec<RichAttachment>,
}

impl RichTranscriptMessage {
    pub(crate) fn new(
        id: impl Into<String>,
        role: TranscriptRole,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: TranscriptMessageId(id.into()),
            role,
            content: content.into(),
            attachments: Vec::new(),
        }
    }

    pub(crate) fn with_attachment(mut self, attachment: RichAttachment) -> Self {
        self.attachments.push(attachment);
        self
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TranscriptMessageId(pub(crate) String);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TranscriptBlockId(pub(crate) String);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RichAttachment {
    pub(crate) id: String,
    pub(crate) kind: RichAttachmentKind,
    pub(crate) media_type: String,
    pub(crate) label: String,
    pub(crate) byte_len: usize,
}

impl RichAttachment {
    pub(crate) fn image(
        id: impl Into<String>,
        media_type: impl Into<String>,
        label: impl Into<String>,
        byte_len: usize,
    ) -> Self {
        Self {
            id: id.into(),
            kind: RichAttachmentKind::Image,
            media_type: media_type.into(),
            label: label.into(),
            byte_len,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RichAttachmentKind {
    Image,
    Pdf,
    Mermaid,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ToolCardRenderMode {
    Compact,
    Expanded,
    RespectCardState,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RichTranscriptBuildOptions {
    pub(crate) tool_render_mode: ToolCardRenderMode,
    pub(crate) tool_collapsed_overrides: BTreeMap<String, bool>,
    pub(crate) search_query: Option<String>,
    pub(crate) collapse_completed_tools: bool,
    pub(crate) include_markdown_media_surfaces: bool,
    pub(crate) syntax_highlighting: bool,
    pub(crate) ansi_styling: bool,
}

impl Default for RichTranscriptBuildOptions {
    fn default() -> Self {
        Self {
            tool_render_mode: ToolCardRenderMode::RespectCardState,
            tool_collapsed_overrides: BTreeMap::new(),
            search_query: None,
            collapse_completed_tools: true,
            include_markdown_media_surfaces: true,
            syntax_highlighting: true,
            ansi_styling: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RichTranscriptDocument {
    pub(crate) messages: Vec<RichTranscriptMessage>,
    pub(crate) blocks: Vec<TranscriptBlock>,
    pub(crate) jumps: Vec<TranscriptJumpTarget>,
    pub(crate) cache_key: u64,
    pub(crate) total_lines: usize,
}

impl RichTranscriptDocument {
    pub(crate) fn flattened_lines(&self) -> Vec<&RichLine> {
        self.blocks
            .iter()
            .flat_map(|block| block.lines.iter())
            .collect()
    }

    pub(crate) fn block(&self, id: &TranscriptBlockId) -> Option<&TranscriptBlock> {
        self.blocks.iter().find(|block| &block.id == id)
    }

    pub(crate) fn jump_targets(
        &self,
        kind: TranscriptJumpKind,
    ) -> impl Iterator<Item = &TranscriptJumpTarget> {
        self.jumps.iter().filter(move |target| target.kind == kind)
    }

    pub(crate) fn line_window(
        &self,
        first_visible_line: usize,
        viewport_lines: usize,
        overscan: usize,
    ) -> TranscriptLineWindow<'_> {
        let window = VirtualLineWindow::for_viewport(
            self.total_lines,
            first_visible_line,
            viewport_lines,
            overscan,
        );
        let mut lines = Vec::new();
        let mut global_line_index = 0usize;
        for block in &self.blocks {
            for (block_line_index, line) in block.lines.iter().enumerate() {
                if window.contains(global_line_index) {
                    lines.push(TranscriptLineRef {
                        global_line_index,
                        block_line_index,
                        block,
                        line,
                    });
                }
                global_line_index += 1;
            }
        }
        TranscriptLineWindow { window, lines }
    }

    pub(crate) fn line_at(&self, target_line_index: usize) -> Option<TranscriptLineRef<'_>> {
        let mut global_line_index = 0usize;
        for block in &self.blocks {
            for (block_line_index, line) in block.lines.iter().enumerate() {
                if global_line_index == target_line_index {
                    return Some(TranscriptLineRef {
                        global_line_index,
                        block_line_index,
                        block,
                        line,
                    });
                }
                global_line_index += 1;
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TranscriptLineRef<'a> {
    pub(crate) global_line_index: usize,
    pub(crate) block_line_index: usize,
    pub(crate) block: &'a TranscriptBlock,
    pub(crate) line: &'a RichLine,
}

#[derive(Clone, Debug)]
pub(crate) struct TranscriptLineWindow<'a> {
    pub(crate) window: VirtualLineWindow,
    pub(crate) lines: Vec<TranscriptLineRef<'a>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TranscriptBlock {
    pub(crate) id: TranscriptBlockId,
    pub(crate) message_id: TranscriptMessageId,
    pub(crate) role: TranscriptRole,
    pub(crate) kind: TranscriptBlockKind,
    pub(crate) lines: Vec<RichLine>,
    pub(crate) copy_text: String,
    pub(crate) semantic_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TranscriptBlockKind {
    Paragraph,
    Heading { level: u8 },
    Quote,
    Table,
    CodeBlock { language: Option<String> },
    ToolCard { card: RichToolCard },
    ImageAttachment { attachment: RichAttachment },
    MediaSurface { surface: RichMediaSurface },
    Separator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RichMediaSurface {
    pub(crate) kind: RichMediaSurfaceKind,
    pub(crate) source: String,
    pub(crate) title: String,
    pub(crate) alt_text: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RichMediaSurfaceKind {
    Mermaid,
    Image,
    Pdf,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RichLine {
    pub(crate) block_id: TranscriptBlockId,
    pub(crate) text: String,
    pub(crate) style: RichLineStyle,
    pub(crate) spans: Vec<RichTextSpan>,
    pub(crate) semantic_role: Option<RichSemanticRole>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RichLineStyle {
    User,
    Assistant,
    AssistantHeading,
    AssistantQuote,
    AssistantTable,
    CodeHeader,
    Code,
    ToolHeader,
    ToolOutput,
    ToolMetadata,
    System,
    Meta,
    MediaPlaceholder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RichTextSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) style: RichSpanStyle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RichSpanStyle {
    InlineCode,
    Link { href: String },
    Emphasis,
    Strong,
    Strike,
    Syntax(SyntaxTokenKind),
    Ansi(AnsiStyle),
    SearchMatch,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SyntaxTokenKind {
    Keyword,
    String,
    Number,
    Comment,
    Function,
    Type,
    Punctuation,
    Plain,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RichSemanticRole {
    Transcript,
    Message,
    Heading,
    CodeBlock,
    ToolCard,
    Image,
    Link,
    SearchResult,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RichToolCard {
    pub(crate) message_id: TranscriptMessageId,
    pub(crate) name: String,
    pub(crate) state: ToolCardState,
    pub(crate) summary: Option<String>,
    pub(crate) collapsed: bool,
    pub(crate) input_lines: Vec<String>,
    pub(crate) output_lines: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ToolCardState {
    Preparing,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

impl ToolCardState {
    fn from_text(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "preparing" | "pending" | "queued" | "waiting" => Self::Preparing,
            "running" | "executing" | "active" => Self::Running,
            "done" | "success" | "succeeded" | "passed" => Self::Succeeded,
            "failed" | "failure" | "error" | "errored" => Self::Failed,
            _ => Self::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Succeeded => "done",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Preparing => "○",
            Self::Running => "●",
            Self::Succeeded => "✓",
            Self::Failed => "✕",
            Self::Unknown => "•",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct AnsiStyle {
    pub(crate) foreground: Option<AnsiColor>,
    pub(crate) background: Option<AnsiColor>,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) inverse: bool,
}

impl AnsiStyle {
    fn is_plain(self) -> bool {
        self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RichAnsiLine {
    pub(crate) text: String,
    pub(crate) spans: Vec<RichTextSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TranscriptSearchMatch {
    pub(crate) block_id: TranscriptBlockId,
    pub(crate) message_id: TranscriptMessageId,
    pub(crate) line_index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) preview: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TranscriptJumpKind {
    Prompt,
    AssistantTurn,
    Tool,
    CodeBlock,
    Media,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TranscriptJumpTarget {
    pub(crate) kind: TranscriptJumpKind,
    pub(crate) message_id: TranscriptMessageId,
    pub(crate) block_id: TranscriptBlockId,
    pub(crate) line_index: usize,
    pub(crate) label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TranscriptCopyMode {
    TranscriptPlainText,
    LatestAssistant,
    Message(TranscriptMessageId),
    Block(TranscriptBlockId),
    CodeBlock(TranscriptBlockId),
    Tool(TranscriptBlockId),
    SearchResult(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MediaOpenAction {
    InlinePreview { title: String, source: String },
    OpenExternal { source: String },
    DecodeImage { media_type: String, bytes: usize },
    RenderMermaid { source: String },
    PreviewPdf { source: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VirtualLineWindow {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) before: usize,
    pub(crate) after: usize,
    pub(crate) total: usize,
}

impl VirtualLineWindow {
    pub(crate) fn for_viewport(
        total: usize,
        first_visible_line: usize,
        viewport_lines: usize,
        overscan: usize,
    ) -> Self {
        if total == 0 || viewport_lines == 0 {
            return Self {
                start: 0,
                end: 0,
                before: 0,
                after: total,
                total,
            };
        }
        let start = first_visible_line.saturating_sub(overscan).min(total);
        let visible_end = first_visible_line.saturating_add(viewport_lines).min(total);
        let end = visible_end.saturating_add(overscan).min(total);
        Self {
            start,
            end,
            before: start,
            after: total.saturating_sub(end),
            total,
        }
    }

    pub(crate) fn contains(self, line_index: usize) -> bool {
        (self.start..self.end).contains(&line_index)
    }
}

pub(crate) fn build_rich_transcript(
    messages: &[RichTranscriptMessage],
    options: &RichTranscriptBuildOptions,
) -> RichTranscriptDocument {
    let mut builder = TranscriptBuilder::new(options.clone());
    for message in messages {
        builder.push_message(message);
    }
    let mut document = builder.finish(messages.to_vec());
    if let Some(query) = options
        .search_query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        apply_search_highlights(&mut document, query, false);
    }
    document
}

pub(crate) fn transcript_cache_key(
    messages: &[RichTranscriptMessage],
    options: &RichTranscriptBuildOptions,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    options.hash(&mut hasher);
    messages.len().hash(&mut hasher);
    for message in messages.iter().take(16) {
        message.hash(&mut hasher);
    }
    if messages.len() > 32 {
        messages.len().hash(&mut hasher);
        let middle = messages.len() / 2;
        messages[middle].hash(&mut hasher);
    }
    for message in messages.iter().rev().take(16) {
        message.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn search_transcript(
    document: &RichTranscriptDocument,
    query: &str,
    case_sensitive: bool,
) -> Vec<TranscriptSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let mut matches = Vec::new();
    let mut global_line = 0usize;
    for block in &document.blocks {
        for line in &block.lines {
            let haystack = if case_sensitive {
                line.text.clone()
            } else {
                line.text.to_lowercase()
            };
            for (start, _) in haystack.match_indices(&needle) {
                let end = start.saturating_add(needle.len()).min(line.text.len());
                matches.push(TranscriptSearchMatch {
                    block_id: block.id.clone(),
                    message_id: block.message_id.clone(),
                    line_index: global_line,
                    start,
                    end,
                    preview: search_preview(&line.text, start, end),
                });
            }
            global_line += 1;
        }
    }
    matches
}

pub(crate) fn apply_search_highlights(
    document: &mut RichTranscriptDocument,
    query: &str,
    case_sensitive: bool,
) -> Vec<TranscriptSearchMatch> {
    for block in &mut document.blocks {
        for line in &mut block.lines {
            line.spans
                .retain(|span| !matches!(span.style, RichSpanStyle::SearchMatch));
        }
    }
    if query.is_empty() {
        return Vec::new();
    }

    let needle = if case_sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let mut matches = Vec::new();
    let mut global_line = 0usize;
    for block in &mut document.blocks {
        for line in &mut block.lines {
            let haystack = if case_sensitive {
                line.text.clone()
            } else {
                line.text.to_lowercase()
            };
            let mut matched_line = false;
            for (start, _) in haystack.match_indices(&needle) {
                let end = start.saturating_add(query.len()).min(line.text.len());
                if !line.text.is_char_boundary(start) || !line.text.is_char_boundary(end) {
                    continue;
                }
                line.spans.push(RichTextSpan {
                    start,
                    end,
                    style: RichSpanStyle::SearchMatch,
                });
                matches.push(TranscriptSearchMatch {
                    block_id: block.id.clone(),
                    message_id: block.message_id.clone(),
                    line_index: global_line,
                    start,
                    end,
                    preview: search_preview(&line.text, start, end),
                });
                matched_line = true;
            }
            if matched_line {
                line.semantic_role = Some(RichSemanticRole::SearchResult);
            }
            global_line += 1;
        }
    }
    matches
}

pub(crate) fn copy_transcript_text(
    document: &RichTranscriptDocument,
    mode: TranscriptCopyMode,
) -> Option<String> {
    match mode {
        TranscriptCopyMode::TranscriptPlainText => Some(
            document
                .blocks
                .iter()
                .map(|block| block.copy_text.as_str())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        TranscriptCopyMode::LatestAssistant => document
            .messages
            .iter()
            .rev()
            .find(|message| message.role == TranscriptRole::Assistant)
            .map(|message| message.content.clone()),
        TranscriptCopyMode::Message(message_id) => document
            .messages
            .iter()
            .find(|message| message.id == message_id)
            .map(|message| message.content.clone()),
        TranscriptCopyMode::Block(block_id) => document
            .block(&block_id)
            .map(|block| block.copy_text.clone()),
        TranscriptCopyMode::CodeBlock(block_id) => {
            document
                .block(&block_id)
                .and_then(|block| match block.kind {
                    TranscriptBlockKind::CodeBlock { .. } => Some(block.copy_text.clone()),
                    _ => None,
                })
        }
        TranscriptCopyMode::Tool(block_id) => {
            document
                .block(&block_id)
                .and_then(|block| match block.kind {
                    TranscriptBlockKind::ToolCard { .. } => Some(block.copy_text.clone()),
                    _ => None,
                })
        }
        TranscriptCopyMode::SearchResult(index) => {
            let mut current = 0usize;
            for block in &document.blocks {
                for line in &block.lines {
                    for span in &line.spans {
                        if !matches!(span.style, RichSpanStyle::SearchMatch) {
                            continue;
                        }
                        if current == index {
                            return Some(line.text.clone());
                        }
                        current += 1;
                    }
                }
            }
            None
        }
    }
}

pub(crate) fn media_surface_open_action(surface: &RichMediaSurface) -> MediaOpenAction {
    match surface.kind {
        RichMediaSurfaceKind::Mermaid => MediaOpenAction::RenderMermaid {
            source: surface.source.clone(),
        },
        RichMediaSurfaceKind::Image => MediaOpenAction::InlinePreview {
            title: surface.title.clone(),
            source: surface.source.clone(),
        },
        RichMediaSurfaceKind::Pdf => MediaOpenAction::PreviewPdf {
            source: surface.source.clone(),
        },
        RichMediaSurfaceKind::Unknown => MediaOpenAction::OpenExternal {
            source: surface.source.clone(),
        },
    }
}

pub(crate) fn attachment_open_action(attachment: &RichAttachment) -> MediaOpenAction {
    match attachment.kind {
        RichAttachmentKind::Image => MediaOpenAction::DecodeImage {
            media_type: attachment.media_type.clone(),
            bytes: attachment.byte_len,
        },
        RichAttachmentKind::Pdf => MediaOpenAction::PreviewPdf {
            source: attachment.id.clone(),
        },
        RichAttachmentKind::Mermaid => MediaOpenAction::RenderMermaid {
            source: attachment.id.clone(),
        },
        RichAttachmentKind::Other => MediaOpenAction::InlinePreview {
            title: attachment.label.clone(),
            source: attachment.id.clone(),
        },
    }
}

pub(crate) fn block_media_open_action(block: &TranscriptBlock) -> Option<MediaOpenAction> {
    match &block.kind {
        TranscriptBlockKind::ImageAttachment { attachment } => {
            Some(attachment_open_action(attachment))
        }
        TranscriptBlockKind::MediaSurface { surface } => Some(media_surface_open_action(surface)),
        _ => None,
    }
}

pub(crate) fn parse_ansi_line(input: &str) -> RichAnsiLine {
    let mut output = String::new();
    let mut spans = Vec::new();
    let mut style = AnsiStyle::default();
    let mut style_start: Option<usize> = None;
    let bytes = input.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == 0x1b
            && input[index..].starts_with("\x1b[")
            && let Some(end_offset) = input[index + 2..].find('m')
        {
            let sgr = &input[index + 2..index + 2 + end_offset];
            if let Some(start) = style_start.take().filter(|_| !style.is_plain()) {
                spans.push(RichTextSpan {
                    start,
                    end: output.len(),
                    style: RichSpanStyle::Ansi(style),
                });
            }
            apply_sgr_codes(&mut style, sgr);
            if !style.is_plain() {
                style_start = Some(output.len());
            }
            index += 2 + end_offset + 1;
            continue;
        }

        let Some(ch) = input[index..].chars().next() else {
            break;
        };
        output.push(ch);
        index += ch.len_utf8();
    }

    if let Some(start) = style_start.filter(|_| !style.is_plain()) {
        spans.push(RichTextSpan {
            start,
            end: output.len(),
            style: RichSpanStyle::Ansi(style),
        });
    }

    RichAnsiLine {
        text: output,
        spans,
    }
}

pub(crate) fn syntax_highlight_line(language: Option<&str>, line: &str) -> Vec<RichTextSpan> {
    let language = language.unwrap_or_default().to_ascii_lowercase();
    let mut spans = Vec::new();
    push_comment_span(&language, line, &mut spans);
    push_string_spans(line, &mut spans);
    push_number_spans(line, &mut spans);
    push_keyword_spans(&language, line, &mut spans);
    push_function_spans(line, &mut spans);
    spans.sort_by_key(|span| (span.start, span.end));
    spans
}

struct TranscriptBuilder {
    options: RichTranscriptBuildOptions,
    blocks: Vec<TranscriptBlock>,
    jumps: Vec<TranscriptJumpTarget>,
    total_lines: usize,
    next_block: usize,
}

impl TranscriptBuilder {
    fn new(options: RichTranscriptBuildOptions) -> Self {
        Self {
            options,
            blocks: Vec::new(),
            jumps: Vec::new(),
            total_lines: 0,
            next_block: 0,
        }
    }

    fn push_message(&mut self, message: &RichTranscriptMessage) {
        match message.role {
            TranscriptRole::Assistant => self.push_assistant_markdown(message),
            TranscriptRole::Tool => self.push_tool_message(message),
            TranscriptRole::User => {
                self.push_plain_message(message, RichLineStyle::User, "user message")
            }
            TranscriptRole::System => {
                self.push_plain_message(message, RichLineStyle::System, "system message")
            }
            TranscriptRole::Meta => {
                self.push_plain_message(message, RichLineStyle::Meta, "metadata")
            }
        }

        for attachment in &message.attachments {
            self.push_attachment(message, attachment.clone());
        }
    }

    fn finish(self, messages: Vec<RichTranscriptMessage>) -> RichTranscriptDocument {
        let cache_key = transcript_cache_key(&messages, &self.options);
        RichTranscriptDocument {
            messages,
            blocks: self.blocks,
            jumps: self.jumps,
            cache_key,
            total_lines: self.total_lines,
        }
    }

    fn next_block_id(
        &mut self,
        message: &RichTranscriptMessage,
        suffix: &str,
    ) -> TranscriptBlockId {
        let block = self.next_block;
        self.next_block += 1;
        TranscriptBlockId(format!("{}:{block}:{suffix}", message.id.0))
    }

    fn push_block(
        &mut self,
        message: &RichTranscriptMessage,
        kind: TranscriptBlockKind,
        style: RichLineStyle,
        raw_lines: Vec<String>,
        copy_text: String,
        semantic_label: impl Into<String>,
    ) -> TranscriptBlockId {
        let block_id = self.next_block_id(message, block_kind_suffix(&kind));
        let semantic_role = semantic_role_for_block(&kind);
        let lines = raw_lines
            .into_iter()
            .map(|text| RichLine {
                block_id: block_id.clone(),
                text,
                style,
                spans: Vec::new(),
                semantic_role: Some(semantic_role),
            })
            .collect::<Vec<_>>();
        self.total_lines += lines.len();
        self.blocks.push(TranscriptBlock {
            id: block_id.clone(),
            message_id: message.id.clone(),
            role: message.role,
            kind,
            lines,
            copy_text,
            semantic_label: semantic_label.into(),
        });
        block_id
    }

    fn push_rich_block(
        &mut self,
        message: &RichTranscriptMessage,
        kind: TranscriptBlockKind,
        lines: Vec<RichLine>,
        copy_text: String,
        semantic_label: impl Into<String>,
    ) -> TranscriptBlockId {
        let block_id = self.next_block_id(message, block_kind_suffix(&kind));
        let lines = lines
            .into_iter()
            .map(|mut line| {
                line.block_id = block_id.clone();
                line
            })
            .collect::<Vec<_>>();
        self.total_lines += lines.len();
        self.blocks.push(TranscriptBlock {
            id: block_id.clone(),
            message_id: message.id.clone(),
            role: message.role,
            kind,
            lines,
            copy_text,
            semantic_label: semantic_label.into(),
        });
        block_id
    }

    fn push_plain_message(
        &mut self,
        message: &RichTranscriptMessage,
        style: RichLineStyle,
        semantic_label: &str,
    ) {
        if message.content.trim().is_empty() {
            return;
        }
        let block_id = self.push_block(
            message,
            TranscriptBlockKind::Paragraph,
            style,
            message.content.lines().map(ToString::to_string).collect(),
            message.content.clone(),
            semantic_label,
        );
        if message.role == TranscriptRole::User {
            self.push_jump(
                TranscriptJumpKind::Prompt,
                message,
                block_id,
                format!("prompt {}", self.jumps.len() + 1),
            );
        }
    }

    fn push_assistant_markdown(&mut self, message: &RichTranscriptMessage) {
        let mut markdown = MarkdownBlockBuilder::new(self, message);
        markdown.parse(&message.content);
        if !markdown.emitted_any && !message.content.trim().is_empty() {
            self.push_plain_message(message, RichLineStyle::Assistant, "assistant message");
        }
    }

    fn push_tool_message(&mut self, message: &RichTranscriptMessage) {
        let mut card = parse_tool_card(message, &message.content, &self.options);
        card.collapsed = match self.options.tool_render_mode {
            ToolCardRenderMode::Compact => true,
            ToolCardRenderMode::Expanded => false,
            ToolCardRenderMode::RespectCardState => card.collapsed,
        };
        if let Some(collapsed) = self
            .options
            .tool_collapsed_overrides
            .get(&message.id.0)
            .or_else(|| self.options.tool_collapsed_overrides.get(&card.name))
        {
            card.collapsed = *collapsed;
        }
        let lines = render_tool_card_lines(&card, self.options.ansi_styling);
        let block_id = self.push_rich_block(
            message,
            TranscriptBlockKind::ToolCard { card: card.clone() },
            lines,
            message.content.clone(),
            format!("tool {}", card.name),
        );
        self.push_jump(
            TranscriptJumpKind::Tool,
            message,
            block_id,
            format!("tool {} {}", card.name, card.state.label()),
        );
    }

    fn push_attachment(&mut self, message: &RichTranscriptMessage, attachment: RichAttachment) {
        let label = format!(
            "{} · {} · {} bytes",
            attachment.label, attachment.media_type, attachment.byte_len
        );
        let block_id = self.push_block(
            message,
            TranscriptBlockKind::ImageAttachment {
                attachment: attachment.clone(),
            },
            RichLineStyle::MediaPlaceholder,
            vec![format!("▧ image attachment · {label}")],
            label.clone(),
            "image attachment",
        );
        self.push_jump(TranscriptJumpKind::Media, message, block_id, label);
    }

    fn push_jump(
        &mut self,
        kind: TranscriptJumpKind,
        message: &RichTranscriptMessage,
        block_id: TranscriptBlockId,
        label: String,
    ) {
        let line_index = self.total_lines.saturating_sub(
            self.blocks
                .last()
                .map(|block| block.lines.len())
                .unwrap_or_default(),
        );
        self.jumps.push(TranscriptJumpTarget {
            kind,
            message_id: message.id.clone(),
            block_id,
            line_index,
            label,
        });
    }
}

struct MarkdownBlockBuilder<'a, 'b> {
    transcript: &'a mut TranscriptBuilder,
    message: &'b RichTranscriptMessage,
    emitted_any: bool,
    paragraph: String,
    heading: Option<(u8, String)>,
    quote_depth: usize,
    code: Option<MarkdownCodeCollector>,
    image: Option<MarkdownImageCollector>,
    table_cells: Vec<String>,
    table_rows: Vec<Vec<String>>,
    in_table_cell: bool,
}

struct MarkdownCodeCollector {
    language: Option<String>,
    text: String,
}

struct MarkdownImageCollector {
    source: String,
    title: String,
    alt: String,
}

impl<'a, 'b> MarkdownBlockBuilder<'a, 'b> {
    fn new(transcript: &'a mut TranscriptBuilder, message: &'b RichTranscriptMessage) -> Self {
        Self {
            transcript,
            message,
            emitted_any: false,
            paragraph: String::new(),
            heading: None,
            quote_depth: 0,
            code: None,
            image: None,
            table_cells: Vec::new(),
            table_rows: Vec::new(),
            in_table_cell: false,
        }
    }

    fn parse(&mut self, content: &str) {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_MATH
            | Options::ENABLE_GFM
            | Options::ENABLE_DEFINITION_LIST;
        for event in Parser::new_ext(content, options) {
            self.handle_event(event);
        }
        self.flush_paragraph();
        self.flush_table();
    }

    fn handle_event(&mut self, event: Event<'_>) {
        if let Some(code) = &mut self.code {
            match event {
                Event::End(TagEnd::CodeBlock) => self.flush_code(),
                Event::Text(text) => code.text.push_str(text.as_ref()),
                Event::SoftBreak | Event::HardBreak => code.text.push('\n'),
                _ => {}
            }
            return;
        }

        if let Some(image) = &mut self.image {
            match event {
                Event::End(TagEnd::Image) => self.flush_image(),
                Event::Text(text) | Event::Code(text) => image.alt.push_str(text.as_ref()),
                _ => {}
            }
            return;
        }

        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush_paragraph();
                self.heading = Some((heading_level_u8(level), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => self.flush_heading(),
            Event::Start(Tag::BlockQuote(_)) => {
                self.flush_paragraph();
                self.quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.flush_paragraph();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                self.flush_paragraph();
                self.code = Some(MarkdownCodeCollector {
                    language: code_block_language(kind),
                    text: String::new(),
                });
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                self.flush_paragraph();
                self.image = Some(MarkdownImageCollector {
                    source: dest_url.to_string(),
                    title: title.to_string(),
                    alt: String::new(),
                });
            }
            Event::Start(Tag::Table(_)) => {
                self.flush_paragraph();
                self.table_rows.clear();
                self.table_cells.clear();
            }
            Event::End(TagEnd::Table) => self.flush_table(),
            Event::Start(Tag::TableCell) => {
                self.in_table_cell = true;
                self.paragraph.clear();
            }
            Event::End(TagEnd::TableCell) => {
                self.in_table_cell = false;
                self.table_cells.push(self.paragraph.trim().to_string());
                self.paragraph.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !self.table_cells.is_empty() {
                    self.table_rows.push(std::mem::take(&mut self.table_cells));
                }
            }
            Event::Start(Tag::Item) => self.push_text("• "),
            Event::TaskListMarker(checked) => self.push_text(if checked { "✓ " } else { "☐ " }),
            Event::Text(text) => self.push_text(text.as_ref()),
            Event::Code(code) => {
                let start = self.active_text().len();
                self.push_text(code.as_ref());
                let end = self.active_text().len();
                let _ = (start, end); // Inline spans are represented in the legacy renderer for now.
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.push_text("\n"),
            Event::Rule => {
                self.flush_paragraph();
                self.emit_plain_block(
                    TranscriptBlockKind::Separator,
                    RichLineStyle::Meta,
                    vec!["────────".to_string()],
                    "────────".to_string(),
                    "separator",
                );
            }
            Event::InlineMath(math) => {
                self.push_text("$");
                self.push_text(math.as_ref());
                self.push_text("$");
            }
            Event::DisplayMath(math) => {
                self.flush_paragraph();
                self.emit_plain_block(
                    TranscriptBlockKind::CodeBlock {
                        language: Some("math".to_string()),
                    },
                    RichLineStyle::Code,
                    math.lines().map(ToString::to_string).collect(),
                    math.to_string(),
                    "math block",
                );
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: &str) {
        if let Some((_, heading)) = &mut self.heading {
            heading.push_str(text);
        } else {
            self.paragraph.push_str(text);
        }
    }

    fn active_text(&self) -> &str {
        self.heading
            .as_ref()
            .map(|(_, heading)| heading.as_str())
            .unwrap_or(&self.paragraph)
    }

    fn flush_heading(&mut self) {
        let Some((level, text)) = self.heading.take() else {
            return;
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let block_id = self.emit_plain_block(
            TranscriptBlockKind::Heading { level },
            RichLineStyle::AssistantHeading,
            vec![text.clone()],
            text.clone(),
            format!("heading level {level}"),
        );
        self.transcript.push_jump(
            TranscriptJumpKind::AssistantTurn,
            self.message,
            block_id,
            text,
        );
    }

    fn flush_paragraph(&mut self) {
        let text = self.paragraph.trim().to_string();
        self.paragraph.clear();
        if text.is_empty() {
            return;
        }
        let style = if self.quote_depth > 0 {
            RichLineStyle::AssistantQuote
        } else {
            RichLineStyle::Assistant
        };
        let kind = if self.quote_depth > 0 {
            TranscriptBlockKind::Quote
        } else {
            TranscriptBlockKind::Paragraph
        };
        let lines = text
            .lines()
            .map(|line| {
                if self.quote_depth > 0 {
                    format!("{}{}", "│ ".repeat(self.quote_depth), line.trim())
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>();
        self.emit_plain_block(kind, style, lines, text, "assistant paragraph");
    }

    fn flush_code(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let language = code.language.clone();
        if self.transcript.options.include_markdown_media_surfaces
            && language
                .as_deref()
                .is_some_and(|language| language.eq_ignore_ascii_case("mermaid"))
        {
            self.emit_media_surface(RichMediaSurface {
                kind: RichMediaSurfaceKind::Mermaid,
                source: code.text.clone(),
                title: "mermaid diagram".to_string(),
                alt_text: "Mermaid diagram output surface".to_string(),
            });
        }
        let block_kind = TranscriptBlockKind::CodeBlock {
            language: language.clone(),
        };
        let block_id = self
            .transcript
            .next_block_id(self.message, block_kind_suffix(&block_kind));
        let mut lines = Vec::new();
        let header = language
            .as_deref()
            .filter(|language| !language.trim().is_empty())
            .unwrap_or("code");
        lines.push(RichLine {
            block_id: block_id.clone(),
            text: format!("  {header}"),
            style: RichLineStyle::CodeHeader,
            spans: Vec::new(),
            semantic_role: Some(RichSemanticRole::CodeBlock),
        });
        for raw_line in code.text.lines() {
            lines.push(RichLine {
                block_id: block_id.clone(),
                text: raw_line.to_string(),
                style: RichLineStyle::Code,
                spans: if self.transcript.options.syntax_highlighting {
                    syntax_highlight_line(language.as_deref(), raw_line)
                } else {
                    Vec::new()
                },
                semantic_role: Some(RichSemanticRole::CodeBlock),
            });
        }
        if lines.len() == 1 {
            lines.push(RichLine {
                block_id: block_id.clone(),
                text: String::new(),
                style: RichLineStyle::Code,
                spans: Vec::new(),
                semantic_role: Some(RichSemanticRole::CodeBlock),
            });
        }
        self.transcript.total_lines += lines.len();
        self.transcript.blocks.push(TranscriptBlock {
            id: block_id.clone(),
            message_id: self.message.id.clone(),
            role: self.message.role,
            kind: block_kind,
            lines,
            copy_text: code.text.clone(),
            semantic_label: language
                .as_deref()
                .map(|language| format!("{language} code block"))
                .unwrap_or_else(|| "code block".to_string()),
        });
        self.transcript.push_jump(
            TranscriptJumpKind::CodeBlock,
            self.message,
            block_id,
            language.unwrap_or_else(|| "code".to_string()),
        );
        self.emitted_any = true;
    }

    fn flush_image(&mut self) {
        let Some(image) = self.image.take() else {
            return;
        };
        let kind = media_kind_from_source(&image.source);
        self.emit_media_surface(RichMediaSurface {
            kind,
            source: image.source,
            title: if image.title.is_empty() {
                "image".to_string()
            } else {
                image.title
            },
            alt_text: image.alt,
        });
    }

    fn flush_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }
        let widths = table_widths(&self.table_rows);
        let rendered = self
            .table_rows
            .iter()
            .map(|row| format_table_row(row, &widths))
            .collect::<Vec<_>>();
        let copy_text = self
            .table_rows
            .iter()
            .map(|row| row.join("\t"))
            .collect::<Vec<_>>()
            .join("\n");
        self.emit_plain_block(
            TranscriptBlockKind::Table,
            RichLineStyle::AssistantTable,
            rendered,
            copy_text,
            "markdown table",
        );
        self.table_rows.clear();
    }

    fn emit_media_surface(&mut self, surface: RichMediaSurface) {
        let label = match surface.kind {
            RichMediaSurfaceKind::Mermaid => "mermaid",
            RichMediaSurfaceKind::Image => "image",
            RichMediaSurfaceKind::Pdf => "pdf",
            RichMediaSurfaceKind::Unknown => "media",
        };
        let title = if surface.title.trim().is_empty() {
            label.to_string()
        } else {
            surface.title.clone()
        };
        let block_id = self.emit_plain_block(
            TranscriptBlockKind::MediaSurface {
                surface: surface.clone(),
            },
            RichLineStyle::MediaPlaceholder,
            vec![format!("▧ {label} surface · {title}")],
            surface.source.clone(),
            format!("{label} output surface"),
        );
        self.transcript
            .push_jump(TranscriptJumpKind::Media, self.message, block_id, title);
    }

    fn emit_plain_block(
        &mut self,
        kind: TranscriptBlockKind,
        style: RichLineStyle,
        lines: Vec<String>,
        copy_text: String,
        semantic_label: impl Into<String>,
    ) -> TranscriptBlockId {
        self.emitted_any = true;
        self.transcript
            .push_block(self.message, kind, style, lines, copy_text, semantic_label)
    }
}

fn parse_tool_card(
    message: &RichTranscriptMessage,
    content: &str,
    options: &RichTranscriptBuildOptions,
) -> RichToolCard {
    let mut lines = content.lines();
    let header = lines.next().unwrap_or("tool").trim();
    let header_collapsed = header.trim_start().starts_with('▸');
    let header_expanded = header.trim_start().starts_with('▾');
    let normalized_header = header.trim_start_matches(['▾', '▸']).trim();
    let mut parts = normalized_header.splitn(2, char::is_whitespace);
    let name = parts
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string();
    let rest = parts.next().unwrap_or_default().trim();
    let (state, summary) = rest
        .split_once(':')
        .map(|(state, summary)| {
            (
                ToolCardState::from_text(state),
                Some(summary.trim().to_string()),
            )
        })
        .unwrap_or((ToolCardState::from_text(rest), None));

    let mut input_lines = Vec::new();
    let mut output_lines = Vec::new();
    for line in lines {
        if let Some(input) = line.trim_start().strip_prefix("input: ") {
            input_lines.push(input.to_string());
        } else if !line.trim().is_empty() {
            output_lines.push(line.trim().to_string());
        }
    }

    let collapsed = if header_collapsed {
        true
    } else if header_expanded {
        false
    } else {
        options.collapse_completed_tools && state.is_terminal()
    };

    RichToolCard {
        message_id: message.id.clone(),
        name,
        state,
        summary: summary.filter(|summary| !summary.is_empty()),
        collapsed,
        input_lines,
        output_lines,
    }
}

fn render_tool_card_lines(card: &RichToolCard, ansi_styling: bool) -> Vec<RichLine> {
    let synthetic_block = TranscriptBlockId(format!("{}:tool", card.message_id.0));
    let mut lines = Vec::new();
    let mut header = format!(
        "{} {} · {}",
        card.state.icon(),
        card.name,
        card.state.label()
    );
    if let Some(summary) = &card.summary {
        header.push_str(" · ");
        header.push_str(summary);
    }
    if card.collapsed && !card.output_lines.is_empty() {
        header.push_str(&format!(" · {} hidden line(s)", card.output_lines.len()));
    }
    lines.push(RichLine {
        block_id: synthetic_block.clone(),
        text: header,
        style: RichLineStyle::ToolHeader,
        spans: Vec::new(),
        semantic_role: Some(RichSemanticRole::ToolCard),
    });

    for input in &card.input_lines {
        lines.push(RichLine {
            block_id: synthetic_block.clone(),
            text: format!("input: {input}"),
            style: RichLineStyle::ToolMetadata,
            spans: Vec::new(),
            semantic_role: Some(RichSemanticRole::ToolCard),
        });
    }

    if !card.collapsed {
        for output in &card.output_lines {
            let parsed = if ansi_styling {
                parse_ansi_line(output)
            } else {
                RichAnsiLine {
                    text: output.clone(),
                    spans: Vec::new(),
                }
            };
            lines.push(RichLine {
                block_id: synthetic_block.clone(),
                text: parsed.text,
                style: RichLineStyle::ToolOutput,
                spans: parsed.spans,
                semantic_role: Some(RichSemanticRole::ToolCard),
            });
        }
    }

    lines
}

fn apply_sgr_codes(style: &mut AnsiStyle, sgr: &str) {
    let codes = if sgr.trim().is_empty() {
        vec![0]
    } else {
        sgr.split(';')
            .filter_map(|part| part.parse::<u16>().ok())
            .collect::<Vec<_>>()
    };
    let mut index = 0usize;
    while index < codes.len() {
        match codes[index] {
            0 => *style = AnsiStyle::default(),
            1 => style.bold = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.inverse = true,
            22 => style.bold = false,
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.inverse = false,
            30..=37 => style.foreground = ansi_color(codes[index], false),
            39 => style.foreground = None,
            40..=47 => style.background = ansi_color(codes[index] - 10, false),
            49 => style.background = None,
            90..=97 => style.foreground = ansi_color(codes[index] - 60, true),
            100..=107 => style.background = ansi_color(codes[index] - 70, true),
            _ => {}
        }
        index += 1;
    }
}

fn ansi_color(code: u16, bright: bool) -> Option<AnsiColor> {
    match (code, bright) {
        (30, false) => Some(AnsiColor::Black),
        (31, false) => Some(AnsiColor::Red),
        (32, false) => Some(AnsiColor::Green),
        (33, false) => Some(AnsiColor::Yellow),
        (34, false) => Some(AnsiColor::Blue),
        (35, false) => Some(AnsiColor::Magenta),
        (36, false) => Some(AnsiColor::Cyan),
        (37, false) => Some(AnsiColor::White),
        (30, true) => Some(AnsiColor::BrightBlack),
        (31, true) => Some(AnsiColor::BrightRed),
        (32, true) => Some(AnsiColor::BrightGreen),
        (33, true) => Some(AnsiColor::BrightYellow),
        (34, true) => Some(AnsiColor::BrightBlue),
        (35, true) => Some(AnsiColor::BrightMagenta),
        (36, true) => Some(AnsiColor::BrightCyan),
        (37, true) => Some(AnsiColor::BrightWhite),
        _ => None,
    }
}

fn push_comment_span(language: &str, line: &str, spans: &mut Vec<RichTextSpan>) {
    let marker = if matches!(
        language,
        "py" | "python" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml"
    ) {
        "#"
    } else {
        "//"
    };
    if let Some(start) = line.find(marker) {
        spans.push(RichTextSpan {
            start,
            end: line.len(),
            style: RichSpanStyle::Syntax(SyntaxTokenKind::Comment),
        });
    }
}

fn push_string_spans(line: &str, spans: &mut Vec<RichTextSpan>) {
    let mut quote_start: Option<(usize, char)> = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if let Some((start, quote)) = quote_start {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                spans.push(RichTextSpan {
                    start,
                    end: index + ch.len_utf8(),
                    style: RichSpanStyle::Syntax(SyntaxTokenKind::String),
                });
                quote_start = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote_start = Some((index, ch));
        }
    }
    if let Some((start, _)) = quote_start {
        spans.push(RichTextSpan {
            start,
            end: line.len(),
            style: RichSpanStyle::Syntax(SyntaxTokenKind::String),
        });
    }
}

fn push_number_spans(line: &str, spans: &mut Vec<RichTextSpan>) {
    let mut start = None;
    for (index, ch) in line.char_indices() {
        if ch.is_ascii_digit() {
            start.get_or_insert(index);
        } else if let Some(span_start) = start.take() {
            spans.push(RichTextSpan {
                start: span_start,
                end: index,
                style: RichSpanStyle::Syntax(SyntaxTokenKind::Number),
            });
        }
    }
    if let Some(span_start) = start {
        spans.push(RichTextSpan {
            start: span_start,
            end: line.len(),
            style: RichSpanStyle::Syntax(SyntaxTokenKind::Number),
        });
    }
}

fn push_keyword_spans(language: &str, line: &str, spans: &mut Vec<RichTextSpan>) {
    let keywords = keywords_for_language(language);
    if keywords.is_empty() {
        return;
    }
    for (start, word) in identifier_ranges(line) {
        if keywords.contains(&word.as_str()) {
            spans.push(RichTextSpan {
                start,
                end: start + word.len(),
                style: RichSpanStyle::Syntax(SyntaxTokenKind::Keyword),
            });
        }
    }
}

fn push_function_spans(line: &str, spans: &mut Vec<RichTextSpan>) {
    for (start, word) in identifier_ranges(line) {
        let after = &line[start + word.len()..];
        if after.trim_start().starts_with('(') {
            spans.push(RichTextSpan {
                start,
                end: start + word.len(),
                style: RichSpanStyle::Syntax(SyntaxTokenKind::Function),
            });
        }
    }
}

fn identifier_ranges(line: &str) -> Vec<(usize, String)> {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, ch) in line.char_indices() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(span_start) = start.take() {
            ranges.push((span_start, line[span_start..index].to_string()));
        }
    }
    if let Some(span_start) = start {
        ranges.push((span_start, line[span_start..].to_string()));
    }
    ranges
}

fn keywords_for_language(language: &str) -> &'static [&'static str] {
    match language {
        "rs" | "rust" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "else", "enum", "fn",
            "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
            "return", "self", "Self", "static", "struct", "trait", "type", "unsafe", "use",
            "where", "while",
        ],
        "js" | "javascript" | "ts" | "typescript" => &[
            "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
            "else", "export", "extends", "finally", "for", "from", "function", "if", "import",
            "let", "new", "return", "switch", "throw", "try", "type", "var", "while",
        ],
        "py" | "python" => &[
            "and", "async", "await", "break", "class", "continue", "def", "elif", "else", "except",
            "False", "finally", "for", "from", "if", "import", "in", "is", "lambda", "None", "not",
            "or", "pass", "raise", "return", "True", "try", "while", "with", "yield",
        ],
        "json" | "jsonc" => &["true", "false", "null"],
        _ => &[],
    }
}

fn search_preview(text: &str, start: usize, end: usize) -> String {
    let prefix_start = start.saturating_sub(32);
    let suffix_end = end.saturating_add(32).min(text.len());
    let mut preview = String::new();
    if prefix_start > 0 {
        preview.push('…');
    }
    preview.push_str(&text[prefix_start..suffix_end]);
    if suffix_end < text.len() {
        preview.push('…');
    }
    preview
}

fn heading_level_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn code_block_language(kind: CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Fenced(info) => info
            .split_whitespace()
            .next()
            .filter(|language| !language.is_empty())
            .map(ToString::to_string),
        CodeBlockKind::Indented => None,
    }
}

fn block_kind_suffix(kind: &TranscriptBlockKind) -> &'static str {
    match kind {
        TranscriptBlockKind::Paragraph => "paragraph",
        TranscriptBlockKind::Heading { .. } => "heading",
        TranscriptBlockKind::Quote => "quote",
        TranscriptBlockKind::Table => "table",
        TranscriptBlockKind::CodeBlock { .. } => "code",
        TranscriptBlockKind::ToolCard { .. } => "tool",
        TranscriptBlockKind::ImageAttachment { .. } => "image-attachment",
        TranscriptBlockKind::MediaSurface { .. } => "media",
        TranscriptBlockKind::Separator => "separator",
    }
}

fn semantic_role_for_block(kind: &TranscriptBlockKind) -> RichSemanticRole {
    match kind {
        TranscriptBlockKind::Heading { .. } => RichSemanticRole::Heading,
        TranscriptBlockKind::CodeBlock { .. } => RichSemanticRole::CodeBlock,
        TranscriptBlockKind::ToolCard { .. } => RichSemanticRole::ToolCard,
        TranscriptBlockKind::ImageAttachment { .. } => RichSemanticRole::Image,
        TranscriptBlockKind::MediaSurface { .. } => RichSemanticRole::Image,
        _ => RichSemanticRole::Message,
    }
}

fn media_kind_from_source(source: &str) -> RichMediaSurfaceKind {
    let lower = source.to_ascii_lowercase();
    if lower.ends_with(".pdf") || lower.starts_with("data:application/pdf") {
        RichMediaSurfaceKind::Pdf
    } else if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
        || lower.starts_with("data:image/")
    {
        RichMediaSurfaceKind::Image
    } else {
        RichMediaSurfaceKind::Unknown
    }
}

fn table_widths(rows: &[Vec<String>]) -> Vec<usize> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    widths
}

fn format_table_row(row: &[String], widths: &[usize]) -> String {
    widths
        .iter()
        .enumerate()
        .map(|(index, width)| {
            let cell = row.get(index).map(String::as_str).unwrap_or_default();
            format!("{cell:<width$}")
        })
        .collect::<Vec<_>>()
        .join(" │ ")
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(content: &str) -> RichTranscriptMessage {
        RichTranscriptMessage::new("assistant-1", TranscriptRole::Assistant, content)
    }

    #[test]
    fn markdown_blocks_preserve_headings_code_media_and_copy_text() {
        let document = build_rich_transcript(
            &[assistant(
                "# Plan\n\nHere is code:\n\n```rust\nfn main() { let value = 42; }\n```\n\n```mermaid\ngraph TD; A-->B;\n```\n\n![plot](plot.png)",
            )],
            &RichTranscriptBuildOptions::default(),
        );

        assert!(
            document
                .blocks
                .iter()
                .any(|block| matches!(block.kind, TranscriptBlockKind::Heading { level: 1 }))
        );
        let code = document
            .blocks
            .iter()
            .find(|block| matches!(block.kind, TranscriptBlockKind::CodeBlock { language: Some(ref language) } if language == "rust"))
            .expect("rust code block");
        assert_eq!(code.lines[0].style, RichLineStyle::CodeHeader);
        assert_eq!(code.lines[0].text.trim(), "rust");
        assert!(code.lines.iter().skip(1).any(|line| {
            line.spans
                .iter()
                .any(|span| span.style == RichSpanStyle::Syntax(SyntaxTokenKind::Keyword))
        }));
        assert_eq!(
            copy_transcript_text(&document, TranscriptCopyMode::CodeBlock(code.id.clone()))
                .unwrap(),
            "fn main() { let value = 42; }\n"
        );
        assert!(document.blocks.iter().any(|block| matches!(block.kind, TranscriptBlockKind::MediaSurface { ref surface } if surface.kind == RichMediaSurfaceKind::Mermaid)));
        assert!(document.blocks.iter().any(|block| matches!(block.kind, TranscriptBlockKind::MediaSurface { ref surface } if surface.kind == RichMediaSurfaceKind::Image)));
        assert!(
            document
                .jump_targets(TranscriptJumpKind::CodeBlock)
                .next()
                .is_some()
        );
    }

    #[test]
    fn ansi_parser_strips_escape_codes_and_records_spans() {
        let parsed = parse_ansi_line("ok \x1b[31;1mfailed\x1b[0m done");
        assert_eq!(parsed.text, "ok failed done");
        assert_eq!(parsed.spans.len(), 1);
        match parsed.spans[0].style {
            RichSpanStyle::Ansi(style) => {
                assert_eq!(style.foreground, Some(AnsiColor::Red));
                assert!(style.bold);
            }
            _ => panic!("expected ansi span"),
        }
    }

    #[test]
    fn tool_cards_can_collapse_or_expand_ansi_output() {
        let message = RichTranscriptMessage::new(
            "tool-1",
            TranscriptRole::Tool,
            "▾ shell running: cargo test\n  input: cargo test\n  \x1b[32mok\x1b[0m",
        );
        let expanded = build_rich_transcript(
            std::slice::from_ref(&message),
            &RichTranscriptBuildOptions {
                tool_render_mode: ToolCardRenderMode::Expanded,
                ..RichTranscriptBuildOptions::default()
            },
        );
        assert_eq!(expanded.blocks.len(), 1);
        assert!(
            expanded.blocks[0]
                .lines
                .iter()
                .any(|line| line.text.contains("ok"))
        );
        assert!(
            expanded.blocks[0]
                .lines
                .iter()
                .any(|line| !line.spans.is_empty())
        );

        let compact = build_rich_transcript(
            &[message],
            &RichTranscriptBuildOptions {
                tool_render_mode: ToolCardRenderMode::Compact,
                ..RichTranscriptBuildOptions::default()
            },
        );
        assert_eq!(compact.blocks[0].lines.len(), 2); // header + input metadata, no output
    }

    #[test]
    fn tool_card_overrides_copy_and_media_actions_are_explicit() {
        let message = RichTranscriptMessage::new(
            "tool-1",
            TranscriptRole::Tool,
            "▸ shell success: cargo test\n  input: cargo test\n  ok",
        );
        let mut overrides = BTreeMap::new();
        overrides.insert("tool-1".to_string(), false);
        let expanded = build_rich_transcript(
            &[message],
            &RichTranscriptBuildOptions {
                tool_render_mode: ToolCardRenderMode::Compact,
                tool_collapsed_overrides: overrides,
                ..RichTranscriptBuildOptions::default()
            },
        );
        let tool = expanded.blocks.first().expect("tool block");
        assert!(tool.lines.iter().any(|line| line.text == "ok"));
        assert_eq!(
            copy_transcript_text(&expanded, TranscriptCopyMode::Tool(tool.id.clone())).unwrap(),
            "▸ shell success: cargo test\n  input: cargo test\n  ok"
        );

        let mermaid = RichMediaSurface {
            kind: RichMediaSurfaceKind::Mermaid,
            source: "graph TD; A-->B;".to_string(),
            title: "flow".to_string(),
            alt_text: "flow".to_string(),
        };
        assert_eq!(
            media_surface_open_action(&mermaid),
            MediaOpenAction::RenderMermaid {
                source: "graph TD; A-->B;".to_string(),
            }
        );
        let attachment = RichAttachment::image("img-1", "image/png", "clipboard", 128);
        assert_eq!(
            attachment_open_action(&attachment),
            MediaOpenAction::DecodeImage {
                media_type: "image/png".to_string(),
                bytes: 128,
            }
        );
    }

    #[test]
    fn transcript_search_copy_jumps_and_virtual_windows_are_stable() {
        let messages = vec![
            RichTranscriptMessage::new("user-1", TranscriptRole::User, "please inspect parser"),
            assistant("Parser result one\n\nParser result two"),
        ];
        let document = build_rich_transcript(&messages, &RichTranscriptBuildOptions::default());
        let matches = search_transcript(&document, "parser", false);
        assert_eq!(matches.len(), 3);
        assert!(
            document
                .jump_targets(TranscriptJumpKind::Prompt)
                .next()
                .is_some()
        );
        assert_eq!(
            copy_transcript_text(&document, TranscriptCopyMode::LatestAssistant).unwrap(),
            "Parser result one\n\nParser result two"
        );
        let highlighted = build_rich_transcript(
            &messages,
            &RichTranscriptBuildOptions {
                search_query: Some("parser".to_string()),
                ..RichTranscriptBuildOptions::default()
            },
        );
        assert!(highlighted.flattened_lines().iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.style == RichSpanStyle::SearchMatch)
        }));
        assert_eq!(
            copy_transcript_text(&highlighted, TranscriptCopyMode::SearchResult(0)).unwrap(),
            "please inspect parser"
        );
        let window = highlighted.line_window(1, 1, 1);
        assert_eq!(window.window.start, 0);
        assert!(window.window.end <= highlighted.total_lines);
        assert!(window.window.contains(1));
        assert!(window.lines.iter().any(|line| line.global_line_index == 1));
        assert_eq!(
            highlighted.line_at(1).map(|line| line.global_line_index),
            Some(1)
        );
    }

    #[test]
    fn image_attachments_become_media_blocks() {
        let message = RichTranscriptMessage::new("user-1", TranscriptRole::User, "see attached")
            .with_attachment(RichAttachment::image(
                "img-1",
                "image/png",
                "clipboard",
                128,
            ));
        let document = build_rich_transcript(&[message], &RichTranscriptBuildOptions::default());
        assert!(
            document
                .blocks
                .iter()
                .any(|block| matches!(block.kind, TranscriptBlockKind::ImageAttachment { .. }))
        );
        assert!(
            document
                .jump_targets(TranscriptJumpKind::Media)
                .next()
                .is_some()
        );
    }
}
