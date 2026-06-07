//! Text buffer construction, shaping, styled segments, rich-line conversion,
//! font-family selection, and color/attribute mapping for single_session_render.

use super::*;

pub(super) fn single_session_text_buffer(
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
) -> Buffer {
    single_session_text_buffer_with_family(
        font_system,
        text,
        SINGLE_SESSION_FONT_FAMILY,
        font_size,
        line_height,
        width,
        height,
    )
}

pub(super) fn single_session_text_buffer_with_family(
    font_system: &mut FontSystem,
    text: &str,
    family: &'static str,
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    buffer.set_wrap(font_system, Wrap::Word);
    buffer.set_text(
        font_system,
        text,
        Attrs::new().family(Family::Name(family)),
        desktop_text_shaping(text),
    );
    buffer.shape_until_scroll(font_system);
    buffer
}

pub(super) fn single_session_styled_text_buffer(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
    wrap: Wrap,
) -> Buffer {
    single_session_styled_text_buffer_with_opacity(
        font_system,
        lines,
        font_size,
        line_height,
        width,
        height,
        wrap,
        1.0,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn single_session_styled_text_buffer_with_opacity(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
    wrap: Wrap,
    opacity: f32,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    buffer.set_wrap(font_system, wrap);
    let segments = single_session_styled_text_segments_with_opacity(lines, opacity);
    // Inline span geometry uses glyphon cursors with byte offsets, and the
    // glyphon `highlight()` API used to position inline-code/math pills only
    // works on Advanced-shaped buffers. So any line carrying inline spans must be
    // Advanced-shaped regardless of script. Advanced shaping is also required for
    // text containing complex scripts, combining marks, or joiner sequences.
    //
    // The expensive case on real transcripts was emoji-rich *prose* lines (no
    // inline spans): standalone pictographic emoji render identically under Basic
    // and Advanced shaping, so `char_needs_advanced_shaping` no longer escalates
    // for them. That keeps the visible-window reshape on every scroll frame cheap
    // while preserving correct pill geometry for code/math spans.
    let shaping = if lines.iter().any(|line| !line.inline_spans.is_empty())
        || segments
            .iter()
            .any(|(text, _)| text_needs_advanced_shaping(text))
    {
        Shaping::Advanced
    } else {
        Shaping::Basic
    };
    buffer.set_rich_text(font_system, segments.iter().copied(), shaping);
    buffer.shape_until_scroll(font_system);
    buffer
}

pub(super) fn desktop_text_shaping(text: &str) -> Shaping {
    if text_needs_advanced_shaping(text) {
        Shaping::Advanced
    } else {
        Shaping::Basic
    }
}

pub(super) fn text_needs_advanced_shaping(text: &str) -> bool {
    text.chars().any(char_needs_advanced_shaping)
}

pub(super) fn char_needs_advanced_shaping(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        // Combining marks and joiners.
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
            | 0x200C..=0x200D
            // Scripts where shaping, bidi, or syllable reordering matter.
            | 0x0590..=0x08FF
            | 0x0900..=0x0DFF
            | 0x1780..=0x18AF
            // Regional indicators combine into flag emoji (pairs need shaping).
            | 0x1F1E6..=0x1F1FF
    )
    // Note: standalone pictographic emoji and symbols (e.g. 🔄 ⬜ → ✓) render
    // identically under Basic and Advanced shaping (single fallback glyph each),
    // so they intentionally do NOT force Advanced shaping here. Advanced shaping
    // is several times more expensive and is the dominant per-frame cost when
    // scrolling emoji-rich transcripts. Only sequences that actually depend on
    // ligature/joiner shaping (variation selectors, ZWJ, regional-indicator flag
    // pairs) escalate, which the ranges above already cover.
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn single_session_styled_text_segments(
    lines: &[SingleSessionStyledLine],
) -> Vec<(&str, Attrs<'static>)> {
    single_session_styled_text_segments_with_opacity(lines, 1.0)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn single_session_styled_text_segments_with_opacity(
    lines: &[SingleSessionStyledLine],
    opacity: f32,
) -> Vec<(&str, Attrs<'static>)> {
    let mut segments = Vec::new();
    let total_user_turns = lines
        .iter()
        .filter(|line| line.style == SingleSessionLineStyle::User)
        .count();
    for (index, line) in lines.iter().enumerate() {
        if !line.text.is_empty() {
            if line.style == SingleSessionLineStyle::User {
                push_user_prompt_segments(&mut segments, &line.text, total_user_turns);
            } else if line.style == SingleSessionLineStyle::Tool {
                push_tool_line_segments(&mut segments, &line.text);
            } else if push_assistant_markdown_inline_segments(&mut segments, line) {
                // Markdown prose can mix display fonts with inline code/math, emphasis,
                // strong text, strike-through spans, and task/list markers. Segmenting
                // here keeps rendered text clean while giving each semantic run a
                // distinct font, weight, style, or color.
            } else {
                segments.push((
                    line.text.as_str(),
                    single_session_style_attrs_for_text(line.style, &line.text),
                ));
            }
        }
        if index + 1 < lines.len() {
            segments.push((
                "\n",
                single_session_style_attrs(SingleSessionLineStyle::Blank),
            ));
        }
    }
    if segments.is_empty() {
        segments.push((
            "",
            single_session_style_attrs(SingleSessionLineStyle::Blank),
        ));
    }
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity < 0.999 {
        for (_, attrs) in &mut segments {
            *attrs = text_attrs_with_opacity(*attrs, opacity);
        }
    }
    segments
}

pub(super) fn text_attrs_with_opacity(mut attrs: Attrs<'static>, opacity: f32) -> Attrs<'static> {
    let Some(color) = attrs.color_opt else {
        return attrs;
    };
    let (r, g, b, a) = color.as_rgba_tuple();
    attrs.color_opt = Some(TextColor::rgba(
        r,
        g,
        b,
        (a as f32 * opacity).round().clamp(0.0, 255.0) as u8,
    ));
    attrs
}

pub(super) fn push_assistant_markdown_inline_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a SingleSessionStyledLine,
) -> bool {
    if !single_session_line_style_supports_markdown_inline_segments(line.style) {
        return false;
    }

    if let Some(marker) = assistant_markdown_list_marker_span(&line.text) {
        if marker.prefix_start > 0 {
            push_assistant_markdown_inline_range(segments, line, 0, marker.prefix_start, false);
        }
        if marker.marker_start > marker.prefix_start {
            push_assistant_markdown_inline_range(
                segments,
                line,
                marker.prefix_start,
                marker.marker_start,
                false,
            );
        }
        segments.push((
            &line.text[marker.marker_start..marker.marker_end],
            single_session_inline_color_attrs_for_text(
                line.style,
                &line.text[marker.marker_start..marker.marker_end],
                marker.color,
            ),
        ));
        push_assistant_markdown_inline_range(
            segments,
            line,
            marker.marker_end,
            line.text.len(),
            false,
        );
        return true;
    }

    push_assistant_markdown_inline_range(segments, line, 0, line.text.len(), true)
}

pub(super) fn single_session_line_style_supports_markdown_inline_segments(
    style: SingleSessionLineStyle,
) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
            | SingleSessionLineStyle::AssistantLink
            | SingleSessionLineStyle::AssistantMedia
    )
}

pub(super) fn push_assistant_markdown_inline_range<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a SingleSessionStyledLine,
    start: usize,
    end: usize,
    require_semantic_span: bool,
) -> bool {
    if start >= end {
        return false;
    }

    let inline_spans = clipped_inline_spans_for_range(&line.inline_spans, start, end);
    if inline_spans.is_empty() && require_semantic_span {
        return false;
    }

    if inline_spans.is_empty() {
        let text = &line.text[start..end];
        segments.push((text, single_session_style_attrs_for_text(line.style, text)));
        return true;
    }

    let force_main_font = inline_spans.iter().any(|span| {
        matches!(
            span.kind,
            SingleSessionInlineSpanKind::Code | SingleSessionInlineSpanKind::Math
        )
    });

    let mut boundaries = Vec::with_capacity(inline_spans.len().saturating_mul(2) + 2);
    boundaries.push(start);
    boundaries.push(end);
    for span in &inline_spans {
        boundaries.push(span.start);
        boundaries.push(span.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    for window in boundaries.windows(2) {
        let segment_start = window[0];
        let segment_end = window[1];
        if segment_start >= segment_end {
            continue;
        }
        let text = &line.text[segment_start..segment_end];
        let active_kinds =
            active_inline_span_kinds_for_range(&inline_spans, segment_start, segment_end);
        segments.push((
            text,
            assistant_inline_markdown_run_attrs(line.style, text, &active_kinds, force_main_font),
        ));
    }
    true
}

pub(super) fn clipped_inline_spans_for_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpan> {
    inline_spans
        .iter()
        .filter_map(|span| {
            let span_start = span.start.max(start);
            let span_end = span.end.min(end);
            (span_start < span_end).then_some(SingleSessionInlineSpan {
                start: span_start,
                end: span_end,
                kind: span.kind,
            })
        })
        .collect()
}

pub(super) fn active_inline_span_kinds_for_range(
    inline_spans: &[SingleSessionInlineSpan],
    start: usize,
    end: usize,
) -> Vec<SingleSessionInlineSpanKind> {
    inline_spans
        .iter()
        .filter_map(|span| (span.start <= start && end <= span.end).then_some(span.kind))
        .collect()
}

pub(super) fn assistant_inline_markdown_run_attrs(
    style: SingleSessionLineStyle,
    text: &str,
    kinds: &[SingleSessionInlineSpanKind],
    force_main_font: bool,
) -> Attrs<'static> {
    if kinds.iter().any(|kind| {
        matches!(
            kind,
            SingleSessionInlineSpanKind::Code | SingleSessionInlineSpanKind::Math
        )
    }) {
        return single_session_style_attrs(SingleSessionLineStyle::Code);
    }

    let mut attrs = if force_main_font {
        single_session_style_attrs_for_family(style, SINGLE_SESSION_FONT_FAMILY)
    } else {
        single_session_style_attrs_for_text(style, text)
    };
    if kinds.contains(&SingleSessionInlineSpanKind::Strike) {
        attrs = attrs.color(text_color(MARKDOWN_STRIKE_TEXT_COLOR));
    }
    if kinds.contains(&SingleSessionInlineSpanKind::Strong) {
        attrs = attrs.weight(glyphon::Weight::BOLD);
    }
    if kinds.contains(&SingleSessionInlineSpanKind::Emphasis) {
        attrs = attrs.style(glyphon::Style::Italic);
    }
    attrs
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rich_line_text_segments(line: &RichLine) -> Vec<(&str, Attrs<'static>)> {
    let base_style = rich_line_style_to_single_session_style(line.style);
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
        return vec![(
            &line.text,
            single_session_style_attrs_for_text(base_style, &line.text),
        )];
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

    let mut segments = Vec::new();
    for window in boundaries.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }
        let text = &line.text[start..end];
        let active = valid_spans
            .iter()
            .filter_map(|span| (span.start <= start && end <= span.end).then_some(&span.style))
            .collect::<Vec<_>>();
        segments.push((text, rich_span_attrs(base_style, text, &active)));
    }
    segments
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rich_line_style_to_single_session_style(
    style: RichLineStyle,
) -> SingleSessionLineStyle {
    match style {
        RichLineStyle::User => SingleSessionLineStyle::User,
        RichLineStyle::Assistant => SingleSessionLineStyle::Assistant,
        RichLineStyle::AssistantHeading => SingleSessionLineStyle::AssistantHeading,
        RichLineStyle::AssistantQuote => SingleSessionLineStyle::AssistantQuote,
        RichLineStyle::AssistantTable => SingleSessionLineStyle::AssistantTable,
        RichLineStyle::CodeHeader => SingleSessionLineStyle::CodeHeader,
        RichLineStyle::Code => SingleSessionLineStyle::Code,
        RichLineStyle::ToolHeader | RichLineStyle::ToolOutput | RichLineStyle::ToolMetadata => {
            SingleSessionLineStyle::Tool
        }
        RichLineStyle::System => SingleSessionLineStyle::Status,
        RichLineStyle::Meta => SingleSessionLineStyle::Meta,
        RichLineStyle::MediaPlaceholder => SingleSessionLineStyle::AssistantMedia,
    }
}

pub(super) fn rich_span_attrs(
    base_style: SingleSessionLineStyle,
    text: &str,
    styles: &[&RichSpanStyle],
) -> Attrs<'static> {
    let mut attrs = single_session_style_attrs_for_text(base_style, text);
    for style in styles {
        match style {
            RichSpanStyle::InlineCode => {
                attrs = single_session_style_attrs(SingleSessionLineStyle::Code);
            }
            RichSpanStyle::Link { .. } => {
                attrs = attrs.color(single_session_line_color(
                    SingleSessionLineStyle::AssistantLink,
                ));
            }
            RichSpanStyle::Emphasis => {
                attrs = attrs.style(glyphon::Style::Italic);
            }
            RichSpanStyle::Strong => {
                attrs = attrs.weight(glyphon::Weight::BOLD);
            }
            RichSpanStyle::Strike => {
                attrs = attrs.color(text_color(MARKDOWN_STRIKE_TEXT_COLOR));
            }
            RichSpanStyle::Syntax(kind) => {
                attrs = attrs.color(text_color(rich_syntax_token_color(*kind)));
            }
            RichSpanStyle::Ansi(style) => {
                if let Some(color) = rich_ansi_foreground(*style) {
                    attrs = attrs.color(text_color(color));
                }
                if style.bold {
                    attrs = attrs.weight(glyphon::Weight::BOLD);
                }
                if style.italic {
                    attrs = attrs.style(glyphon::Style::Italic);
                }
            }
            RichSpanStyle::SearchMatch => {
                attrs = attrs
                    .color(text_color(STATUS_TEXT_ACCENT_COLOR))
                    .weight(glyphon::Weight::BOLD);
            }
        }
    }
    attrs
}

pub(super) fn rich_syntax_token_color(kind: SyntaxTokenKind) -> [f32; 4] {
    match kind {
        SyntaxTokenKind::Keyword => [0.350, 0.145, 0.640, 1.0],
        SyntaxTokenKind::String => [0.020, 0.360, 0.190, 1.0],
        SyntaxTokenKind::Number => [0.490, 0.250, 0.035, 1.0],
        SyntaxTokenKind::Comment => [0.320, 0.350, 0.420, 0.95],
        SyntaxTokenKind::Function => [0.000, 0.255, 0.430, 1.0],
        SyntaxTokenKind::Type => [0.225, 0.215, 0.620, 1.0],
        SyntaxTokenKind::Punctuation => [0.270, 0.290, 0.340, 0.98],
        SyntaxTokenKind::Plain => CODE_TEXT_COLOR,
    }
}

pub(super) fn rich_ansi_foreground(style: AnsiStyle) -> Option<[f32; 4]> {
    let color = if style.inverse {
        style.background.or(style.foreground)
    } else {
        style.foreground
    }?;
    Some(match color {
        AnsiColor::Black => [0.040, 0.045, 0.055, 1.0],
        AnsiColor::Red => [0.560, 0.070, 0.095, 1.0],
        AnsiColor::Green => [0.035, 0.360, 0.220, 1.0],
        AnsiColor::Yellow => [0.520, 0.360, 0.055, 1.0],
        AnsiColor::Blue => [0.045, 0.265, 0.640, 1.0],
        AnsiColor::Magenta => [0.410, 0.145, 0.580, 1.0],
        AnsiColor::Cyan => [0.000, 0.330, 0.430, 1.0],
        AnsiColor::White => [0.700, 0.720, 0.770, 1.0],
        AnsiColor::BrightBlack => [0.320, 0.345, 0.405, 1.0],
        AnsiColor::BrightRed => [0.780, 0.110, 0.145, 1.0],
        AnsiColor::BrightGreen => [0.025, 0.500, 0.275, 1.0],
        AnsiColor::BrightYellow => [0.700, 0.500, 0.080, 1.0],
        AnsiColor::BrightBlue => [0.090, 0.360, 0.850, 1.0],
        AnsiColor::BrightMagenta => [0.560, 0.190, 0.760, 1.0],
        AnsiColor::BrightCyan => [0.000, 0.460, 0.580, 1.0],
        AnsiColor::BrightWhite => [0.900, 0.915, 0.945, 1.0],
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AssistantMarkdownListMarkerSpan {
    prefix_start: usize,
    marker_start: usize,
    marker_end: usize,
    color: [f32; 4],
}

pub(super) fn assistant_markdown_list_marker_span(
    text: &str,
) -> Option<AssistantMarkdownListMarkerSpan> {
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with("│ ") {
            index += "│ ".len();
        } else if rest.starts_with("  ") {
            index += "  ".len();
        } else {
            break;
        }
    }

    let rest = &text[index..];
    let (marker_len, color) = if rest.starts_with("✓ ") {
        ("✓ ".len(), MARKDOWN_TASK_DONE_COLOR)
    } else if rest.starts_with("☐ ") {
        ("☐ ".len(), MARKDOWN_TASK_OPEN_COLOR)
    } else if rest.starts_with("• ") || rest.starts_with("◦ ") || rest.starts_with("▪ ") {
        (
            rest.chars().take(2).map(char::len_utf8).sum(),
            MARKDOWN_LIST_MARKER_COLOR,
        )
    } else if let Some(marker_len) = ordered_list_marker_len(rest) {
        (marker_len, MARKDOWN_LIST_MARKER_COLOR)
    } else {
        return None;
    };

    Some(AssistantMarkdownListMarkerSpan {
        prefix_start: 0,
        marker_start: index,
        marker_end: index + marker_len,
        color,
    })
}

pub(super) fn ordered_list_marker_len(text: &str) -> Option<usize> {
    let mut digit_bytes = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digit_bytes += ch.len_utf8();
        } else {
            break;
        }
    }
    if digit_bytes == 0 || !text[digit_bytes..].starts_with(". ") {
        return None;
    }
    Some(digit_bytes + ". ".len())
}

pub(super) fn single_session_inline_color_attrs_for_text(
    style: SingleSessionLineStyle,
    text: &str,
    color: [f32; 4],
) -> Attrs<'static> {
    let family = single_session_font_family_for_text(style, text);
    Attrs::new()
        .family(Family::Name(family))
        .color(text_color(color))
}

pub(super) fn push_user_prompt_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a str,
    total_user_turns: usize,
) {
    let Some((number, text)) = line.split_once("  ") else {
        segments.push((
            line,
            single_session_style_attrs(SingleSessionLineStyle::User),
        ));
        return;
    };
    let Ok(turn) = number.parse::<usize>() else {
        segments.push((
            line,
            single_session_style_attrs(SingleSessionLineStyle::User),
        ));
        return;
    };

    segments.push((
        number,
        single_session_color_attrs(user_prompt_number_color_for_distance(
            total_user_turns.saturating_add(1).saturating_sub(turn),
        )),
    ));
    segments.push((
        "› ",
        single_session_color_attrs(text_color(USER_PROMPT_ACCENT_COLOR)),
    ));
    segments.push((
        text,
        single_session_style_attrs(SingleSessionLineStyle::User),
    ));
}

pub(super) fn push_tool_line_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    line: &'a str,
) {
    let trimmed = line.trim_start_matches(' ');
    let indent_len = line.len().saturating_sub(trimmed.len());
    if indent_len > 0 {
        segments.push((
            &line[..indent_len],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
    }

    if trimmed.is_empty() {
        return;
    }

    if push_tool_widget_segments(segments, trimmed) {
        return;
    }

    let Some((icon, icon_text, mut rest)) = split_tool_line_icon(trimmed) else {
        segments.push((
            trimmed,
            single_session_color_attrs(text_color(TOOL_DETAIL_TEXT_COLOR)),
        ));
        return;
    };

    segments.push((
        icon_text,
        single_session_color_attrs(text_color(tool_icon_text_color(icon))),
    ));

    let rest_indent_len = rest
        .char_indices()
        .find(|(_, ch)| *ch != ' ')
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    if rest_indent_len > 0 {
        segments.push((
            &rest[..rest_indent_len],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        rest = &rest[rest_indent_len..];
    }

    push_tool_header_segments(segments, rest);
}

pub(super) fn push_tool_widget_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    text: &'a str,
) -> bool {
    if text.starts_with('╭') || text.starts_with('╰') {
        segments.push((
            text,
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        return true;
    }

    if text.starts_with('│') && text.ends_with('│') && text.len() >= '│'.len_utf8() * 2 {
        let border_len = '│'.len_utf8();
        let content_start = border_len;
        let content_end = text.len().saturating_sub(border_len);
        let content = &text[content_start..content_end];
        let visible_content_end = content.trim_end_matches(' ').len();

        segments.push((
            &text[..content_start],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        if visible_content_end > 0 {
            segments.push((
                &content[..visible_content_end],
                single_session_color_attrs(text_color(TOOL_DETAIL_TEXT_COLOR)),
            ));
        }
        if visible_content_end < content.len() {
            segments.push((
                &content[visible_content_end..],
                single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
            ));
        }
        segments.push((
            &text[content_end..],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        return true;
    }

    false
}

pub(super) fn split_tool_line_icon(text: &str) -> Option<(char, &str, &str)> {
    let mut chars = text.char_indices();
    let (_, icon) = chars.next()?;
    if !matches!(icon, '✓' | '✕' | '●' | '○' | '▸' | '•') {
        return None;
    }
    let icon_end = chars.next().map(|(index, _)| index).unwrap_or(text.len());
    Some((icon, &text[..icon_end], &text[icon_end..]))
}

pub(super) fn push_tool_header_segments<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    text: &'a str,
) {
    const TOOL_SEPARATOR: &str = " · ";

    if text.is_empty() {
        return;
    }

    let mut remaining = text;
    let mut part_index = 0usize;
    while let Some(separator_index) = remaining.find(TOOL_SEPARATOR) {
        let part = &remaining[..separator_index];
        push_tool_header_part_segment(segments, part, part_index);
        let separator_end = separator_index + TOOL_SEPARATOR.len();
        segments.push((
            &remaining[separator_index..separator_end],
            single_session_color_attrs(text_color(TOOL_MUTED_TEXT_COLOR)),
        ));
        remaining = &remaining[separator_end..];
        part_index += 1;
    }

    push_tool_header_part_segment(segments, remaining, part_index);
}

pub(super) fn push_tool_header_part_segment<'a>(
    segments: &mut Vec<(&'a str, Attrs<'static>)>,
    part: &'a str,
    part_index: usize,
) {
    if part.is_empty() {
        return;
    }
    let color = match part_index {
        0 => TOOL_TEXT_COLOR,
        1 => tool_state_text_color(part).unwrap_or(TOOL_MUTED_TEXT_COLOR),
        _ => TOOL_DETAIL_TEXT_COLOR,
    };
    segments.push((part, single_session_color_attrs(text_color(color))));
}

pub(super) fn tool_icon_text_color(icon: char) -> [f32; 4] {
    match icon {
        '✓' => TOOL_SUCCESS_TEXT_COLOR,
        '✕' => TOOL_FAILED_TEXT_COLOR,
        '●' => TOOL_RUNNING_TEXT_COLOR,
        '○' => TOOL_PENDING_TEXT_COLOR,
        '▸' | '•' => TOOL_TEXT_COLOR,
        _ => TOOL_DETAIL_TEXT_COLOR,
    }
}

pub(super) fn tool_state_text_color(state: &str) -> Option<[f32; 4]> {
    match state.trim().to_ascii_lowercase().as_str() {
        "done" | "success" | "succeeded" | "passed" => Some(TOOL_SUCCESS_TEXT_COLOR),
        "failed" | "failure" | "error" | "errored" => Some(TOOL_FAILED_TEXT_COLOR),
        "running" | "executing" | "active" => Some(TOOL_RUNNING_TEXT_COLOR),
        "preparing" | "pending" | "queued" | "waiting" => Some(TOOL_PENDING_TEXT_COLOR),
        _ => None,
    }
}

pub(super) fn single_session_style_attrs(style: SingleSessionLineStyle) -> Attrs<'static> {
    single_session_style_attrs_for_family(style, single_session_font_family_for_style(style))
}

pub(super) fn single_session_style_attrs_for_text(
    style: SingleSessionLineStyle,
    text: &str,
) -> Attrs<'static> {
    let family = single_session_font_family_for_text(style, text);
    single_session_style_attrs_for_family(style, family)
}

pub(super) fn single_session_font_family_for_text(
    style: SingleSessionLineStyle,
    text: &str,
) -> &'static str {
    if matches!(
        style,
        SingleSessionLineStyle::User | SingleSessionLineStyle::UserContinuation
    ) {
        return single_session_user_font_family();
    }

    if assistant_text_should_use_handwriting_font(style, text) {
        return single_session_assistant_font_family();
    }

    SINGLE_SESSION_FONT_FAMILY
}

pub(super) fn single_session_font_family_for_style(style: SingleSessionLineStyle) -> &'static str {
    if matches!(
        style,
        SingleSessionLineStyle::User | SingleSessionLineStyle::UserContinuation
    ) {
        single_session_user_font_family()
    } else if assistant_style_can_use_handwriting_font(style) {
        single_session_assistant_font_family()
    } else {
        SINGLE_SESSION_FONT_FAMILY
    }
}

pub(super) fn single_session_style_attrs_for_family(
    style: SingleSessionLineStyle,
    family: &'static str,
) -> Attrs<'static> {
    Attrs::new()
        .family(Family::Name(family))
        .color(single_session_line_color(style))
}

pub(super) fn text_contains_symbol_glyphs(text: &str) -> bool {
    !text.is_ascii()
}

pub(super) fn assistant_style_can_use_handwriting_font(style: SingleSessionLineStyle) -> bool {
    matches!(
        style,
        SingleSessionLineStyle::Assistant
            | SingleSessionLineStyle::AssistantHeading
            | SingleSessionLineStyle::AssistantQuote
    )
}

pub(super) fn assistant_text_should_use_handwriting_font(
    style: SingleSessionLineStyle,
    text: &str,
) -> bool {
    assistant_style_can_use_handwriting_font(style)
        && !text.trim().is_empty()
        && !text_contains_symbol_glyphs(text)
        && !text_contains_urlish_token(text)
        && !text_contains_codeish_token(text)
        && !text_has_dense_punctuation(text)
}

pub(super) fn text_contains_urlish_token(text: &str) -> bool {
    text.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| matches!(ch, ',' | '.' | ')' | ']' | '}'));
        token.starts_with("http://")
            || token.starts_with("https://")
            || token.starts_with("www.")
            || token.contains("://")
            || token.contains('@')
            || (token.contains('.')
                && token.rsplit_once('.').is_some_and(|(_, suffix)| {
                    suffix.len() >= 2 && suffix.chars().all(|ch| ch.is_ascii_alphabetic())
                }))
    })
}

pub(super) fn text_contains_codeish_token(text: &str) -> bool {
    const CODE_MARKERS: &[&str] = &[
        "`", "```", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "</", "/>",
    ];
    if CODE_MARKERS.iter().any(|marker| text.contains(marker)) {
        return true;
    }
    text.split_whitespace().any(|token| {
        token
            .chars()
            .any(|ch| matches!(ch, '{' | '}' | '[' | ']' | ';' | '$' | '\\'))
            || (token.contains('/') && token.chars().any(|ch| ch.is_ascii_alphabetic()))
            || token
                .split('_')
                .nth(1)
                .is_some_and(|_| token.chars().any(|ch| ch.is_ascii_alphabetic()))
    })
}

pub(super) fn text_has_dense_punctuation(text: &str) -> bool {
    let mut punctuation = 0_usize;
    let mut non_space = 0_usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        non_space += 1;
        if ch.is_ascii_punctuation() && !matches!(ch, '.' | ',' | '!' | '?' | ':' | '-') {
            punctuation += 1;
        }
    }
    non_space > 0 && punctuation * 4 > non_space
}

pub(super) fn single_session_color_attrs(color: TextColor) -> Attrs<'static> {
    Attrs::new()
        .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
        .color(color)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn user_prompt_number_color(turn: usize) -> TextColor {
    user_prompt_number_color_for_distance(turn.saturating_sub(1))
}

pub(super) fn user_prompt_number_color_for_distance(distance: usize) -> TextColor {
    // Match the TUI prompt-number effect: recent prompts start in a softened
    // rainbow and older prompts exponentially decay toward gray.
    const RAINBOW: [[f32; 3]; 7] = [
        [1.000, 0.314, 0.314],
        [1.000, 0.627, 0.314],
        [1.000, 0.902, 0.314],
        [0.314, 0.863, 0.392],
        [0.314, 0.784, 0.863],
        [0.392, 0.549, 1.000],
        [0.706, 0.392, 1.000],
    ];
    const GRAY: [f32; 3] = [0.314, 0.314, 0.314];

    let decay = (-0.4 * distance as f32).exp();
    let rainbow = RAINBOW[distance.min(RAINBOW.len() - 1)];
    text_color([
        rainbow[0] * decay + GRAY[0] * (1.0 - decay),
        rainbow[1] * decay + GRAY[1] * (1.0 - decay),
        rainbow[2] * decay + GRAY[2] * (1.0 - decay),
        1.0,
    ])
}

pub(crate) fn single_session_line_color(style: SingleSessionLineStyle) -> TextColor {
    text_color(single_session_line_rgba(style))
}

pub(super) fn single_session_line_rgba(style: SingleSessionLineStyle) -> [f32; 4] {
    match style {
        SingleSessionLineStyle::Assistant => ASSISTANT_TEXT_COLOR,
        SingleSessionLineStyle::AssistantHeading => ASSISTANT_HEADING_TEXT_COLOR,
        SingleSessionLineStyle::AssistantQuote => ASSISTANT_QUOTE_TEXT_COLOR,
        SingleSessionLineStyle::AssistantTable => ASSISTANT_TABLE_TEXT_COLOR,
        SingleSessionLineStyle::AssistantLink | SingleSessionLineStyle::AssistantMedia => {
            ASSISTANT_LINK_TEXT_COLOR
        }
        SingleSessionLineStyle::CodeHeader => META_TEXT_COLOR,
        SingleSessionLineStyle::Code => CODE_TEXT_COLOR,
        SingleSessionLineStyle::User => USER_TEXT_COLOR,
        SingleSessionLineStyle::UserContinuation => USER_CONTINUATION_TEXT_COLOR,
        SingleSessionLineStyle::Tool => TOOL_TEXT_COLOR,
        SingleSessionLineStyle::Meta | SingleSessionLineStyle::Blank => META_TEXT_COLOR,
        SingleSessionLineStyle::Status => STATUS_TEXT_ACCENT_COLOR,
        SingleSessionLineStyle::Error => ERROR_TEXT_COLOR,
        SingleSessionLineStyle::OverlayTitle => PANEL_TITLE_COLOR,
        SingleSessionLineStyle::Overlay => OVERLAY_TEXT_COLOR,
        SingleSessionLineStyle::OverlaySelection => OVERLAY_SELECTION_TEXT_COLOR,
    }
}
