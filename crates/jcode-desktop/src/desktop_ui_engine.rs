#![allow(dead_code)]

use crate::desktop_rich_text::{
    AnsiColor, RichLine, RichLineStyle, RichSpanStyle, RichTranscriptDocument, TranscriptBlockKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct UiId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct UiRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl UiRect {
    pub(crate) fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x <= self.x + self.width && y <= self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct UiSize {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayoutConstraints {
    pub(crate) min: UiSize,
    pub(crate) max: UiSize,
}

impl LayoutConstraints {
    pub(crate) fn tight(size: UiSize) -> Self {
        Self {
            min: size,
            max: size,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UiNodeKind {
    Root,
    Row,
    Column,
    Stack,
    SplitPane,
    ScrollContainer,
    VirtualList,
    Surface,
    Text,
    Image,
    Overlay,
    SemanticOnly,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DirtyFlags {
    pub(crate) layout: bool,
    pub(crate) paint: bool,
    pub(crate) text: bool,
    pub(crate) semantics: bool,
}

impl DirtyFlags {
    pub(crate) fn any(&self) -> bool {
        self.layout || self.paint || self.text || self.semantics
    }

    pub(crate) fn mark_all(&mut self) {
        self.layout = true;
        self.paint = true;
        self.text = true;
        self.semantics = true;
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UiNode {
    pub(crate) id: UiId,
    pub(crate) kind: UiNodeKind,
    pub(crate) bounds: UiRect,
    pub(crate) children: Vec<UiId>,
    pub(crate) dirty: DirtyFlags,
    pub(crate) semantic_role: Option<AccessibilityRole>,
    pub(crate) label: Option<String>,
    pub(crate) cache_key: Option<u64>,
}

impl UiNode {
    pub(crate) fn new(id: UiId, kind: UiNodeKind) -> Self {
        Self {
            id,
            kind,
            bounds: UiRect::default(),
            children: Vec::new(),
            dirty: DirtyFlags::default(),
            semantic_role: None,
            label: None,
            cache_key: None,
        }
    }

    pub(crate) fn with_semantics(
        mut self,
        role: AccessibilityRole,
        label: impl Into<String>,
    ) -> Self {
        self.semantic_role = Some(role);
        self.label = Some(label.into());
        self
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RetainedUiTree {
    pub(crate) root: Option<UiId>,
    pub(crate) nodes: BTreeMap<UiId, UiNode>,
    dirty_nodes: BTreeSet<UiId>,
}

impl RetainedUiTree {
    pub(crate) fn upsert(&mut self, mut node: UiNode) {
        let existing_key = self
            .nodes
            .get(&node.id)
            .and_then(|existing| existing.cache_key);
        if existing_key != node.cache_key {
            node.dirty.mark_all();
        }
        if node.dirty.any() {
            self.dirty_nodes.insert(node.id);
        }
        self.nodes.insert(node.id, node);
    }

    pub(crate) fn mark_dirty(&mut self, id: UiId, flags: DirtyFlags) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.dirty.layout |= flags.layout;
            node.dirty.paint |= flags.paint;
            node.dirty.text |= flags.text;
            node.dirty.semantics |= flags.semantics;
            self.dirty_nodes.insert(id);
        }
    }

    pub(crate) fn dirty_nodes(&self) -> impl Iterator<Item = &UiNode> {
        self.dirty_nodes.iter().filter_map(|id| self.nodes.get(id))
    }

    pub(crate) fn clear_dirty(&mut self) {
        for id in std::mem::take(&mut self.dirty_nodes) {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.dirty.clear();
            }
        }
    }

    pub(crate) fn semantics(&self) -> Vec<SemanticNode> {
        self.nodes
            .values()
            .filter_map(|node| {
                Some(SemanticNode {
                    id: node.id,
                    role: node.semantic_role?,
                    label: node.label.clone().unwrap_or_default(),
                    bounds: node.bounds,
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DisplayList {
    pub(crate) commands: Vec<DisplayCommand>,
    pub(crate) semantic_nodes: Vec<SemanticNode>,
}

impl DisplayList {
    pub(crate) fn push(&mut self, command: DisplayCommand) {
        self.commands.push(command);
    }

    pub(crate) fn extend_semantics(&mut self, nodes: impl IntoIterator<Item = SemanticNode>) {
        self.semantic_nodes.extend(nodes);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DisplayCommand {
    Rect {
        id: UiId,
        rect: UiRect,
        color: ColorRgba,
    },
    RoundedRect {
        id: UiId,
        rect: UiRect,
        radius: f32,
        color: ColorRgba,
    },
    Border {
        id: UiId,
        rect: UiRect,
        width: f32,
        color: ColorRgba,
    },
    Text {
        id: UiId,
        origin: (f32, f32),
        runs: Vec<DisplayTextRun>,
    },
    Image {
        id: UiId,
        rect: UiRect,
        image: DisplayImageRef,
    },
    ClipStart {
        id: UiId,
        rect: UiRect,
    },
    ClipEnd {
        id: UiId,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ColorRgba {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl ColorRgba {
    pub(crate) const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DisplayTextRun {
    pub(crate) text: String,
    pub(crate) font_stack: FontFallbackStack,
    pub(crate) size_px: f32,
    pub(crate) color: ColorRgba,
    pub(crate) attrs: TextAttributes,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FontFallbackStack {
    pub(crate) primary: String,
    pub(crate) fallbacks: Vec<String>,
}

impl FontFallbackStack {
    pub(crate) fn new(
        primary: impl Into<String>,
        fallbacks: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            primary: primary.into(),
            fallbacks: fallbacks.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct TextAttributes {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) monospace: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DisplayImageRef {
    TextureId(String),
    AttachmentId(String),
    PendingDecode(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TextShapingMode {
    BasicAscii,
    UnicodeShaping,
    PlatformNative,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextEngineConfig {
    pub(crate) shaping: TextShapingMode,
    pub(crate) font_stack: FontFallbackStack,
    pub(crate) enable_ligatures: bool,
    pub(crate) enable_emoji_fallback: bool,
}

impl TextEngineConfig {
    pub(crate) fn desktop_default() -> Self {
        Self {
            shaping: TextShapingMode::UnicodeShaping,
            font_stack: FontFallbackStack::new(
                "JetBrainsMono Nerd Font",
                [
                    "JetBrainsMono Nerd Font Mono",
                    "JetBrains Mono",
                    "monospace",
                ],
            ),
            enable_ligatures: false,
            enable_emoji_fallback: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct GlyphAtlasLifecycle {
    pub(crate) font_epoch: u64,
    pub(crate) atlas_generation: u64,
    pub(crate) glyph_count: usize,
    pub(crate) byte_estimate: usize,
    pub(crate) evictions: u64,
}

impl GlyphAtlasLifecycle {
    pub(crate) fn note_font_stack_changed(&mut self) {
        self.font_epoch += 1;
        self.atlas_generation += 1;
        self.glyph_count = 0;
        self.byte_estimate = 0;
    }

    pub(crate) fn note_glyphs_uploaded(&mut self, glyph_count: usize, bytes: usize) {
        self.glyph_count = self.glyph_count.saturating_add(glyph_count);
        self.byte_estimate = self.byte_estimate.saturating_add(bytes);
    }

    pub(crate) fn evict_all(&mut self) {
        self.atlas_generation += 1;
        self.glyph_count = 0;
        self.byte_estimate = 0;
        self.evictions += 1;
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ImeState {
    pub(crate) active: bool,
    pub(crate) preedit: String,
    pub(crate) cursor_byte_range: Option<(usize, usize)>,
}

impl ImeState {
    pub(crate) fn apply_preedit(
        &mut self,
        text: impl Into<String>,
        cursor: Option<(usize, usize)>,
    ) {
        self.active = true;
        self.preedit = text.into();
        self.cursor_byte_range = cursor;
    }

    pub(crate) fn commit(&mut self) -> String {
        self.active = false;
        self.cursor_byte_range = None;
        std::mem::take(&mut self.preedit)
    }

    pub(crate) fn composed_text(&self, base: &str, cursor_byte: usize) -> String {
        if !self.active || self.preedit.is_empty() {
            return base.to_string();
        }
        let cursor = cursor_byte.min(base.len());
        let cursor = clamp_to_char_boundary(base, cursor);
        let mut composed = String::with_capacity(base.len() + self.preedit.len());
        composed.push_str(&base[..cursor]);
        composed.push_str(&self.preedit);
        composed.push_str(&base[cursor..]);
        composed
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextSelectionRange {
    pub(crate) anchor: usize,
    pub(crate) focus: usize,
}

impl TextSelectionRange {
    pub(crate) fn normalized(&self) -> std::ops::Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }

    pub(crate) fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TextCursorModel {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) selection: Option<TextSelectionRange>,
}

impl TextCursorModel {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.len(),
            text,
            selection: None,
        }
    }

    pub(crate) fn move_left(&mut self, extend_selection: bool) {
        let next = previous_char_boundary(&self.text, self.cursor);
        self.set_cursor(next, extend_selection);
    }

    pub(crate) fn move_right(&mut self, extend_selection: bool) {
        let next = next_char_boundary(&self.text, self.cursor);
        self.set_cursor(next, extend_selection);
    }

    pub(crate) fn set_cursor(&mut self, cursor: usize, extend_selection: bool) {
        let cursor = clamp_to_char_boundary(&self.text, cursor.min(self.text.len()));
        if extend_selection {
            let anchor = self
                .selection
                .as_ref()
                .map(|selection| selection.anchor)
                .unwrap_or(self.cursor);
            self.selection = Some(TextSelectionRange {
                anchor,
                focus: cursor,
            });
        } else {
            self.selection = None;
        }
        self.cursor = cursor;
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        let range = self.selection.as_ref()?.normalized();
        (!range.is_empty()).then_some(&self.text[range])
    }

    pub(crate) fn replace_selection_or_insert(&mut self, insert: &str) {
        if let Some(selection) = self.selection.take() {
            let range = selection.normalized();
            self.text.replace_range(range.clone(), insert);
            self.cursor = range.start + insert.len();
        } else {
            self.text.insert_str(self.cursor, insert);
            self.cursor += insert.len();
        }
        self.cursor = clamp_to_char_boundary(&self.text, self.cursor.min(self.text.len()));
    }

    pub(crate) fn apply_ime_commit(&mut self, ime: &mut ImeState) {
        let committed = ime.commit();
        self.replace_selection_or_insert(&committed);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AccessibilityRole {
    Window,
    Workspace,
    Surface,
    Transcript,
    Message,
    Button,
    TextInput,
    StaticText,
    Image,
    Code,
    ToolCard,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SemanticNode {
    pub(crate) id: UiId,
    pub(crate) role: AccessibilityRole,
    pub(crate) label: String,
    pub(crate) bounds: UiRect,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesktopTheme {
    pub(crate) mode: ThemeMode,
    pub(crate) background: ColorRgba,
    pub(crate) panel: ColorRgba,
    pub(crate) text: ColorRgba,
    pub(crate) muted_text: ColorRgba,
    pub(crate) accent: ColorRgba,
    pub(crate) error: ColorRgba,
}

impl DesktopTheme {
    pub(crate) fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            background: ColorRgba::rgba(0.965, 0.972, 0.985, 1.0),
            panel: ColorRgba::rgba(1.0, 1.0, 1.0, 0.82),
            text: ColorRgba::rgba(0.12, 0.13, 0.16, 1.0),
            muted_text: ColorRgba::rgba(0.38, 0.40, 0.45, 1.0),
            accent: ColorRgba::rgba(0.30, 0.42, 0.95, 1.0),
            error: ColorRgba::rgba(0.85, 0.12, 0.16, 1.0),
        }
    }

    pub(crate) fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            background: ColorRgba::rgba(0.055, 0.060, 0.075, 1.0),
            panel: ColorRgba::rgba(0.11, 0.12, 0.15, 0.86),
            text: ColorRgba::rgba(0.88, 0.90, 0.94, 1.0),
            muted_text: ColorRgba::rgba(0.60, 0.63, 0.70, 1.0),
            accent: ColorRgba::rgba(0.50, 0.62, 1.0, 1.0),
            error: ColorRgba::rgba(1.0, 0.38, 0.42, 1.0),
        }
    }

    pub(crate) fn for_preferences(preferences: &UiPreferences, system_dark: bool) -> Self {
        match preferences.theme_mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
            ThemeMode::System if system_dark => Self::dark(),
            ThemeMode::System => Self::light(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UiPreferences {
    pub(crate) theme_mode: ThemeMode,
    pub(crate) font_scale: f32,
    pub(crate) reduced_motion: bool,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::System,
            font_scale: 1.0,
            reduced_motion: false,
        }
    }
}

impl UiPreferences {
    pub(crate) fn clamped_font_scale(&self) -> f32 {
        self.font_scale.clamp(0.65, 1.60)
    }

    pub(crate) fn animation_duration_ms(&self, default_ms: u64) -> u64 {
        if self.reduced_motion { 0 } else { default_ms }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VirtualListState {
    pub(crate) total_items: usize,
    pub(crate) first_visible: usize,
    pub(crate) visible_count: usize,
    pub(crate) overscan: usize,
}

impl VirtualListState {
    pub(crate) fn materialized_range(self) -> std::ops::Range<usize> {
        let start = self
            .first_visible
            .saturating_sub(self.overscan)
            .min(self.total_items);
        let visible_end = self
            .first_visible
            .saturating_add(self.visible_count)
            .min(self.total_items);
        let end = visible_end
            .saturating_add(self.overscan)
            .min(self.total_items);
        start..end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SurfaceRenderCache {
    pub(crate) surface_id: UiId,
    pub(crate) layout_key: u64,
    pub(crate) display_key: u64,
    pub(crate) semantic_key: u64,
    pub(crate) invalidation_epoch: u64,
}

impl SurfaceRenderCache {
    pub(crate) fn new(surface_id: UiId) -> Self {
        Self {
            surface_id,
            layout_key: 0,
            display_key: 0,
            semantic_key: 0,
            invalidation_epoch: 0,
        }
    }

    pub(crate) fn update_keys(
        &mut self,
        layout_key: u64,
        display_key: u64,
        semantic_key: u64,
    ) -> DirtyFlags {
        let mut flags = DirtyFlags::default();
        if self.layout_key != layout_key {
            self.layout_key = layout_key;
            flags.layout = true;
        }
        if self.display_key != display_key {
            self.display_key = display_key;
            flags.paint = true;
        }
        if self.semantic_key != semantic_key {
            self.semantic_key = semantic_key;
            flags.semantics = true;
        }
        if flags.any() {
            self.invalidation_epoch += 1;
        }
        flags
    }
}

pub(crate) fn stable_hash<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TranscriptDisplayListConfig {
    pub(crate) origin: (f32, f32),
    pub(crate) width: f32,
    pub(crate) font_size_px: f32,
    pub(crate) line_height_px: f32,
    pub(crate) first_visible_line: usize,
    pub(crate) viewport_lines: usize,
    pub(crate) overscan_lines: usize,
}

impl Default for TranscriptDisplayListConfig {
    fn default() -> Self {
        Self {
            origin: (0.0, 0.0),
            width: 640.0,
            font_size_px: 15.0,
            line_height_px: 22.0,
            first_visible_line: 0,
            viewport_lines: 40,
            overscan_lines: 4,
        }
    }
}

pub(crate) fn build_transcript_display_list(
    document: &RichTranscriptDocument,
    theme: &DesktopTheme,
    config: TranscriptDisplayListConfig,
) -> DisplayList {
    let mut display_list = DisplayList::default();
    let font_stack = TextEngineConfig::desktop_default().font_stack;
    let window = document.line_window(
        config.first_visible_line,
        config.viewport_lines,
        config.overscan_lines,
    );

    for line_ref in window.lines {
        let line_top = config.origin.1 + line_ref.global_line_index as f32 * config.line_height_px;
        let id = UiId(0x5452_414e_5343_0000u64 ^ line_ref.global_line_index as u64);
        let rect = UiRect {
            x: config.origin.0,
            y: line_top,
            width: config.width,
            height: config.line_height_px,
        };
        display_list.push(DisplayCommand::Text {
            id,
            origin: (config.origin.0, line_top + config.font_size_px),
            runs: rich_line_display_runs(line_ref.line, theme, &font_stack, config.font_size_px),
        });
        display_list.semantic_nodes.push(SemanticNode {
            id,
            role: accessibility_role_for_transcript_block(&line_ref.block.kind),
            label: if line_ref.line.text.is_empty() {
                line_ref.block.semantic_label.clone()
            } else {
                line_ref.line.text.clone()
            },
            bounds: rect,
        });
    }

    display_list
}

pub(crate) fn rich_line_display_runs(
    line: &RichLine,
    theme: &DesktopTheme,
    font_stack: &FontFallbackStack,
    font_size_px: f32,
) -> Vec<DisplayTextRun> {
    let valid_spans = line
        .spans
        .iter()
        .filter(|span| {
            span.start < span.end
                && span.end <= line.text.len()
                && line.text.is_char_boundary(span.start)
                && line.text.is_char_boundary(span.end)
        })
        .collect::<Vec<_>>();
    if valid_spans.is_empty() {
        return vec![DisplayTextRun {
            text: line.text.clone(),
            font_stack: font_stack.clone(),
            size_px: font_size_px,
            color: rich_line_color(line.style, theme),
            attrs: rich_line_attrs(line.style),
        }];
    }

    let mut boundaries = Vec::with_capacity(valid_spans.len().saturating_mul(2) + 2);
    boundaries.push(0);
    boundaries.push(line.text.len());
    for span in &valid_spans {
        boundaries.push(span.start);
        boundaries.push(span.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut runs = Vec::new();
    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }
        let active = valid_spans
            .iter()
            .filter_map(|span| (span.start <= start && end <= span.end).then_some(&span.style))
            .collect::<Vec<_>>();
        let (color, attrs) = rich_span_display_style(line.style, theme, &active);
        runs.push(DisplayTextRun {
            text: line.text[start..end].to_string(),
            font_stack: font_stack.clone(),
            size_px: font_size_px,
            color,
            attrs,
        });
    }
    runs
}

fn rich_span_display_style(
    line_style: RichLineStyle,
    theme: &DesktopTheme,
    spans: &[&RichSpanStyle],
) -> (ColorRgba, TextAttributes) {
    let mut color = rich_line_color(line_style, theme);
    let mut attrs = rich_line_attrs(line_style);
    for span in spans {
        match span {
            RichSpanStyle::InlineCode => {
                color = rich_line_color(RichLineStyle::Code, theme);
                attrs.monospace = true;
            }
            RichSpanStyle::Link { .. } => {
                color = theme.accent;
                attrs.underline = true;
            }
            RichSpanStyle::Emphasis => attrs.italic = true,
            RichSpanStyle::Strong => attrs.bold = true,
            RichSpanStyle::Strike => color = theme.muted_text,
            RichSpanStyle::Syntax(kind) => color = syntax_color(*kind, theme),
            RichSpanStyle::Ansi(style) => {
                if let Some(ansi) = ansi_color(style.foreground) {
                    color = ansi;
                }
                attrs.bold |= style.bold;
                attrs.italic |= style.italic;
                attrs.underline |= style.underline;
            }
            RichSpanStyle::SearchMatch => {
                color = theme.accent;
                attrs.bold = true;
            }
        }
    }
    (color, attrs)
}

fn rich_line_attrs(style: RichLineStyle) -> TextAttributes {
    TextAttributes {
        bold: matches!(
            style,
            RichLineStyle::AssistantHeading | RichLineStyle::ToolHeader
        ),
        italic: matches!(style, RichLineStyle::AssistantQuote),
        underline: false,
        monospace: matches!(
            style,
            RichLineStyle::Code | RichLineStyle::CodeHeader | RichLineStyle::ToolOutput
        ),
    }
}

fn rich_line_color(style: RichLineStyle, theme: &DesktopTheme) -> ColorRgba {
    match style {
        RichLineStyle::User => theme.accent,
        RichLineStyle::Assistant => theme.text,
        RichLineStyle::AssistantHeading => theme.accent,
        RichLineStyle::AssistantQuote => ColorRgba::rgba(0.50, 0.32, 0.70, 1.0),
        RichLineStyle::AssistantTable => ColorRgba::rgba(0.00, 0.38, 0.45, 1.0),
        RichLineStyle::CodeHeader | RichLineStyle::ToolMetadata | RichLineStyle::Meta => {
            theme.muted_text
        }
        RichLineStyle::Code | RichLineStyle::ToolOutput => theme.text,
        RichLineStyle::ToolHeader => ColorRgba::rgba(0.40, 0.22, 0.66, 1.0),
        RichLineStyle::System => theme.error,
        RichLineStyle::MediaPlaceholder => theme.accent,
    }
}

fn syntax_color(
    kind: crate::desktop_rich_text::SyntaxTokenKind,
    theme: &DesktopTheme,
) -> ColorRgba {
    match kind {
        crate::desktop_rich_text::SyntaxTokenKind::Keyword => {
            ColorRgba::rgba(0.46, 0.25, 0.78, 1.0)
        }
        crate::desktop_rich_text::SyntaxTokenKind::String => ColorRgba::rgba(0.02, 0.42, 0.22, 1.0),
        crate::desktop_rich_text::SyntaxTokenKind::Number => ColorRgba::rgba(0.58, 0.32, 0.06, 1.0),
        crate::desktop_rich_text::SyntaxTokenKind::Comment => theme.muted_text,
        crate::desktop_rich_text::SyntaxTokenKind::Function => {
            ColorRgba::rgba(0.00, 0.32, 0.55, 1.0)
        }
        crate::desktop_rich_text::SyntaxTokenKind::Type => ColorRgba::rgba(0.28, 0.28, 0.72, 1.0),
        crate::desktop_rich_text::SyntaxTokenKind::Punctuation
        | crate::desktop_rich_text::SyntaxTokenKind::Plain => {
            rich_line_color(RichLineStyle::Code, theme)
        }
    }
}

fn ansi_color(color: Option<AnsiColor>) -> Option<ColorRgba> {
    Some(match color? {
        AnsiColor::Black => ColorRgba::rgba(0.04, 0.04, 0.05, 1.0),
        AnsiColor::Red | AnsiColor::BrightRed => ColorRgba::rgba(0.78, 0.11, 0.15, 1.0),
        AnsiColor::Green | AnsiColor::BrightGreen => ColorRgba::rgba(0.02, 0.50, 0.28, 1.0),
        AnsiColor::Yellow | AnsiColor::BrightYellow => ColorRgba::rgba(0.70, 0.50, 0.08, 1.0),
        AnsiColor::Blue | AnsiColor::BrightBlue => ColorRgba::rgba(0.09, 0.36, 0.85, 1.0),
        AnsiColor::Magenta | AnsiColor::BrightMagenta => ColorRgba::rgba(0.56, 0.19, 0.76, 1.0),
        AnsiColor::Cyan | AnsiColor::BrightCyan => ColorRgba::rgba(0.00, 0.46, 0.58, 1.0),
        AnsiColor::White | AnsiColor::BrightWhite => ColorRgba::rgba(0.90, 0.91, 0.94, 1.0),
        AnsiColor::BrightBlack => ColorRgba::rgba(0.32, 0.35, 0.41, 1.0),
    })
}

fn accessibility_role_for_transcript_block(kind: &TranscriptBlockKind) -> AccessibilityRole {
    match kind {
        TranscriptBlockKind::CodeBlock { .. } => AccessibilityRole::Code,
        TranscriptBlockKind::ToolCard { .. } => AccessibilityRole::ToolCard,
        TranscriptBlockKind::ImageAttachment { .. } | TranscriptBlockKind::MediaSurface { .. } => {
            AccessibilityRole::Image
        }
        _ => AccessibilityRole::StaticText,
    }
}

fn clamp_to_char_boundary(text: &str, mut cursor: usize) -> usize {
    cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_to_char_boundary(text, cursor);
    if cursor == 0 {
        return 0;
    }
    text[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = clamp_to_char_boundary(text, cursor);
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_tree_tracks_dirty_nodes_and_semantics() {
        let mut tree = RetainedUiTree::default();
        let mut node = UiNode::new(UiId(1), UiNodeKind::Text)
            .with_semantics(AccessibilityRole::StaticText, "hello");
        node.bounds = UiRect {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
        };
        node.cache_key = Some(10);
        tree.upsert(node.clone());
        assert_eq!(tree.dirty_nodes().count(), 1);
        let semantics = tree.semantics();
        assert_eq!(semantics.len(), 1);
        assert_eq!(semantics[0].label, "hello");
        tree.clear_dirty();
        assert_eq!(tree.dirty_nodes().count(), 0);

        let mut updated = node;
        updated.cache_key = Some(11);
        tree.upsert(updated);
        assert_eq!(tree.dirty_nodes().count(), 1);
    }

    #[test]
    fn display_list_keeps_renderer_independent_commands() {
        let mut list = DisplayList::default();
        list.push(DisplayCommand::RoundedRect {
            id: UiId(7),
            rect: UiRect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 50.0,
            },
            radius: 8.0,
            color: DesktopTheme::light().panel,
        });
        list.push(DisplayCommand::Text {
            id: UiId(8),
            origin: (8.0, 16.0),
            runs: vec![DisplayTextRun {
                text: "hello".to_string(),
                font_stack: TextEngineConfig::desktop_default().font_stack,
                size_px: 15.0,
                color: DesktopTheme::light().text,
                attrs: TextAttributes::default(),
            }],
        });
        assert_eq!(list.commands.len(), 2);
    }

    #[test]
    fn glyph_atlas_lifecycle_records_font_changes_and_evictions() {
        let mut atlas = GlyphAtlasLifecycle::default();
        atlas.note_glyphs_uploaded(10, 4096);
        assert_eq!(atlas.glyph_count, 10);
        atlas.note_font_stack_changed();
        assert_eq!(atlas.font_epoch, 1);
        assert_eq!(atlas.glyph_count, 0);
        atlas.note_glyphs_uploaded(3, 1024);
        atlas.evict_all();
        assert_eq!(atlas.evictions, 1);
        assert_eq!(atlas.glyph_count, 0);
    }

    #[test]
    fn ime_state_tracks_preedit_and_commit() {
        let mut ime = ImeState::default();
        ime.apply_preedit("かな", Some((0, 6)));
        assert!(ime.active);
        assert_eq!(ime.composed_text("hi!", 2), "hiかな!");
        assert_eq!(ime.commit(), "かな");
        assert!(!ime.active);
        assert!(ime.preedit.is_empty());
    }

    #[test]
    fn text_cursor_model_respects_unicode_boundaries_selection_and_ime_commit() {
        let mut cursor = TextCursorModel::new("a🙂b");
        cursor.move_left(false);
        assert_eq!(cursor.cursor, "a🙂".len());
        cursor.move_left(true);
        assert_eq!(cursor.selected_text(), Some("🙂"));
        cursor.replace_selection_or_insert("かな");
        assert_eq!(cursor.text, "aかなb");

        let mut ime = ImeState::default();
        ime.apply_preedit("字", Some((0, 3)));
        cursor.apply_ime_commit(&mut ime);
        assert_eq!(cursor.text, "aかな字b");
        assert!(!ime.active);
    }

    #[test]
    fn virtual_list_and_surface_cache_report_minimal_invalidation() {
        let range = VirtualListState {
            total_items: 100,
            first_visible: 10,
            visible_count: 5,
            overscan: 2,
        }
        .materialized_range();
        assert_eq!(range, 8..17);

        let mut cache = SurfaceRenderCache::new(UiId(42));
        let flags = cache.update_keys(1, 2, 3);
        assert!(flags.layout && flags.paint && flags.semantics);
        let flags = cache.update_keys(1, 9, 3);
        assert!(!flags.layout && flags.paint && !flags.semantics);
        assert_eq!(cache.invalidation_epoch, 2);
    }

    #[test]
    fn preferences_cover_font_scale_and_reduced_motion() {
        let prefs = UiPreferences {
            font_scale: 9.0,
            reduced_motion: true,
            ..UiPreferences::default()
        };
        assert_eq!(prefs.clamped_font_scale(), 1.60);
        assert_eq!(prefs.animation_duration_ms(180), 0);
        assert_eq!(
            DesktopTheme::for_preferences(&prefs, true).mode,
            ThemeMode::Dark
        );
    }

    #[test]
    fn rich_transcript_display_list_virtualizes_runs_and_semantics() {
        let messages = [crate::desktop_rich_text::RichTranscriptMessage::new(
            "assistant-1",
            crate::desktop_rich_text::TranscriptRole::Assistant,
            "```rust\nfn main() {}\n```",
        )];
        let document = crate::desktop_rich_text::build_rich_transcript(
            &messages,
            &crate::desktop_rich_text::RichTranscriptBuildOptions {
                search_query: Some("main".to_string()),
                ..crate::desktop_rich_text::RichTranscriptBuildOptions::default()
            },
        );
        let display_list = build_transcript_display_list(
            &document,
            &DesktopTheme::light(),
            TranscriptDisplayListConfig {
                viewport_lines: 1,
                overscan_lines: 0,
                ..TranscriptDisplayListConfig::default()
            },
        );

        assert_eq!(display_list.commands.len(), 1);
        assert_eq!(display_list.semantic_nodes.len(), 1);
        assert_eq!(display_list.semantic_nodes[0].role, AccessibilityRole::Code);

        let display_list = build_transcript_display_list(
            &document,
            &DesktopTheme::light(),
            TranscriptDisplayListConfig {
                first_visible_line: 1,
                viewport_lines: 1,
                overscan_lines: 0,
                ..TranscriptDisplayListConfig::default()
            },
        );
        let DisplayCommand::Text { runs, .. } = &display_list.commands[0] else {
            panic!("expected text command");
        };
        assert!(runs.iter().any(|run| run.text == "main" && run.attrs.bold));
    }
}
