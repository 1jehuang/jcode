use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionTextKey {
    pub(crate) size: (u32, u32),
    pub(crate) title: String,
    pub(crate) version: String,
    pub(crate) body: Vec<SingleSessionStyledLine>,
    pub(crate) draft: String,
    pub(crate) status: String,
}

pub(crate) fn build_single_session_vertices(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    focus_pulse: f32,
) -> Vec<Vertex> {
    let width = size.width as f32;
    let height = size.height as f32;
    let mut vertices = Vec::new();

    push_gradient_rect(
        &mut vertices,
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        },
        BACKGROUND_TOP_LEFT,
        BACKGROUND_BOTTOM_LEFT,
        BACKGROUND_BOTTOM_RIGHT,
        BACKGROUND_TOP_RIGHT,
        size,
    );

    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: width.max(1.0),
        height: height.max(1.0),
    };
    let surface = single_session_surface(app.session.as_ref());
    push_surface(
        &mut vertices,
        rect,
        surface.color_index,
        true,
        focus_pulse,
        size,
    );

    push_single_session_composer_card(&mut vertices, size);
    push_single_session_transcript_cards(&mut vertices, app, size);
    push_single_session_selection(&mut vertices, app, size);

    vertices
}

fn push_single_session_composer_card(vertices: &mut Vec<Vertex>, size: PhysicalSize<u32>) {
    let draft_top = single_session_draft_top(size);
    let x = PANEL_TITLE_LEFT_PADDING - 8.0;
    let y = draft_top - SINGLE_SESSION_STATUS_GAP - 8.0;
    let width = (size.width as f32 - x * 2.0).max(1.0);
    let height = (size.height as f32 - y - PANEL_TITLE_TOP_PADDING).max(1.0);
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    push_rounded_rect(
        vertices,
        rect,
        PANEL_RADIUS,
        COMPOSER_CARD_BACKGROUND_COLOR,
        size,
    );
    push_panel_outline(vertices, rect, 1.0, COMPOSER_CARD_BORDER_COLOR, size);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionTranscriptCardRun {
    pub(crate) line: usize,
    pub(crate) line_count: usize,
    pub(crate) style: SingleSessionLineStyle,
}

fn push_single_session_transcript_cards(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let visible_lines = single_session_visible_styled_body(app, size);
    let width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0 + 12.0).max(1.0);

    for run in single_session_transcript_card_runs(&visible_lines) {
        let Some(color) = single_session_line_card_color(run.style) else {
            continue;
        };
        push_rounded_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING - 6.0,
                y: PANEL_BODY_TOP_PADDING + run.line as f32 * line_height + 3.0,
                width,
                height: (run.line_count as f32 * line_height - 6.0).max(1.0),
            },
            7.0,
            color,
            size,
        );
    }
}

pub(crate) fn single_session_transcript_card_runs(
    lines: &[SingleSessionStyledLine],
) -> Vec<SingleSessionTranscriptCardRun> {
    let mut runs = Vec::new();
    let mut current: Option<SingleSessionTranscriptCardRun> = None;

    for (line, styled_line) in lines.iter().enumerate() {
        if single_session_line_card_color(styled_line.style).is_none() {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            continue;
        }

        match &mut current {
            Some(run) if run.style == styled_line.style && run.line + run.line_count == line => {
                run.line_count += 1;
            }
            Some(run) => {
                runs.push(*run);
                current = Some(SingleSessionTranscriptCardRun {
                    line,
                    line_count: 1,
                    style: styled_line.style,
                });
            }
            None => {
                current = Some(SingleSessionTranscriptCardRun {
                    line,
                    line_count: 1,
                    style: styled_line.style,
                });
            }
        }
    }

    if let Some(run) = current {
        runs.push(run);
    }
    runs
}

fn single_session_line_card_color(style: SingleSessionLineStyle) -> Option<[f32; 4]> {
    match style {
        SingleSessionLineStyle::Code => Some(CODE_BLOCK_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantQuote => Some(QUOTE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::AssistantTable => Some(TABLE_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::Tool => Some(TOOL_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::Error => Some(ERROR_CARD_BACKGROUND_COLOR),
        SingleSessionLineStyle::OverlaySelection => Some(OVERLAY_SELECTION_BACKGROUND_COLOR),
        _ => None,
    }
}

fn push_single_session_selection(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let char_width = single_session_body_char_width();
    let visible_lines = single_session_visible_body(app, size);
    for segment in app.selection_segments(&visible_lines) {
        let selected_columns = segment
            .end_column
            .saturating_sub(segment.start_column)
            .max(1);
        push_rect(
            vertices,
            Rect {
                x: PANEL_TITLE_LEFT_PADDING - 2.0 + segment.start_column as f32 * char_width,
                y: PANEL_BODY_TOP_PADDING + segment.line as f32 * line_height,
                width: selected_columns as f32 * char_width + 4.0,
                height: line_height,
            },
            SELECTION_HIGHLIGHT_COLOR,
            size,
        );
    }
}

pub(crate) fn push_single_session_caret(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    draft_buffer: Option<&Buffer>,
) {
    let caret = draft_buffer
        .and_then(|buffer| glyphon_draft_caret_position(app, buffer, size))
        .unwrap_or_else(|| approximate_draft_caret_position(app, size));

    push_rect(
        vertices,
        Rect {
            x: caret.x,
            y: caret.y,
            width: SINGLE_SESSION_CARET_WIDTH,
            height: caret.height,
        },
        SINGLE_SESSION_CARET_COLOR,
        size,
    );
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaretPosition {
    pub(crate) x: f32,
    pub(crate) y: f32,
    height: f32,
}

pub(crate) fn glyphon_draft_caret_position(
    app: &SingleSessionApp,
    draft_buffer: &Buffer,
    size: PhysicalSize<u32>,
) -> Option<CaretPosition> {
    let typography = single_session_typography();
    let target = app.composer_cursor_line_byte_index();
    let target_line = target.0;
    let target_index = target.1;
    let mut fallback = None;

    for run in draft_buffer.layout_runs() {
        if run.line_i != target_line {
            continue;
        }
        let y = single_session_draft_top(size) + run.line_top;
        let height = typography.code_size * 1.12;
        if run.glyphs.is_empty() {
            return Some(CaretPosition {
                x: PANEL_TITLE_LEFT_PADDING,
                y,
                height,
            });
        }

        let first = run.glyphs.first()?;
        let last = run.glyphs.last()?;
        let mut run_position = CaretPosition {
            x: PANEL_TITLE_LEFT_PADDING + last.x + last.w,
            y,
            height,
        };
        if target_index <= first.start {
            run_position.x = PANEL_TITLE_LEFT_PADDING + first.x;
            return Some(run_position);
        }
        for glyph in run.glyphs {
            if target_index <= glyph.start {
                run_position.x = PANEL_TITLE_LEFT_PADDING + glyph.x;
                return Some(run_position);
            }
            if target_index <= glyph.end {
                run_position.x = PANEL_TITLE_LEFT_PADDING + glyph.x + glyph.w;
                return Some(run_position);
            }
        }
        if target_index >= first.start && target_index >= last.end {
            fallback = Some(run_position);
        }
    }

    fallback
}

fn approximate_draft_caret_position(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> CaretPosition {
    let typography = single_session_typography();
    let line_height = typography.code_size * typography.code_line_height;
    let draft_top = single_session_draft_top(size);
    let (cursor_line, cursor_column) = app.draft_cursor_line_col();
    let char_width = typography.code_size * 0.58;
    let prompt_column = if cursor_line == 0 {
        app.composer_prompt().chars().count()
    } else {
        0
    };
    let x = PANEL_TITLE_LEFT_PADDING
        + ((prompt_column + cursor_column) as f32 * char_width)
            .min((size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(0.0));
    let y = draft_top + cursor_line as f32 * line_height;
    CaretPosition {
        x,
        y,
        height: typography.code_size * 1.12,
    }
}

pub(crate) fn single_session_draft_top(size: PhysicalSize<u32>) -> f32 {
    (size.height as f32 - SINGLE_SESSION_DRAFT_TOP_OFFSET).max(112.0)
}

#[cfg(test)]
pub(crate) fn single_session_text_buffers(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    let key = single_session_text_key(app, size);
    single_session_text_buffers_from_key(&key, size, font_system)
}

#[cfg(test)]
pub(crate) fn single_session_text_key(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> SingleSessionTextKey {
    single_session_text_key_for_tick(app, size, 0)
}

pub(crate) fn single_session_text_key_for_tick(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    tick: u64,
) -> SingleSessionTextKey {
    SingleSessionTextKey {
        size: (size.width, size.height),
        title: app.header_title(),
        version: desktop_header_version_label(),
        body: single_session_visible_styled_body(app, size),
        draft: visualize_composer_whitespace(&app.composer_text()),
        status: app.composer_status_line_for_tick(tick),
    }
}

pub(crate) fn single_session_text_buffers_from_key(
    key: &SingleSessionTextKey,
    size: PhysicalSize<u32>,
    font_system: &mut FontSystem,
) -> Vec<Buffer> {
    let typography = single_session_typography();
    let content_width = (size.width as f32 - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);

    let prompt_height =
        (size.height as f32 - single_session_draft_top(size) - SINGLE_SESSION_STATUS_GAP - 18.0)
            .max(typography.code_size * typography.code_line_height * 2.0);

    vec![
        single_session_text_buffer(
            font_system,
            &key.title,
            typography.title_size,
            typography.title_size * typography.meta_line_height,
            content_width,
            48.0,
        ),
        single_session_styled_text_buffer(
            font_system,
            &key.body,
            typography.body_size,
            typography.body_size * typography.body_line_height,
            content_width,
            (size.height as f32 - 150.0).max(1.0),
        ),
        single_session_text_buffer(
            font_system,
            &key.draft,
            typography.code_size,
            typography.code_size * typography.code_line_height,
            content_width,
            prompt_height,
        ),
        single_session_text_buffer(
            font_system,
            &key.status,
            typography.meta_size,
            typography.meta_size * typography.meta_line_height,
            content_width,
            28.0,
        ),
        single_session_text_buffer(
            font_system,
            &key.version,
            typography.meta_size,
            typography.meta_size * typography.meta_line_height,
            content_width,
            24.0,
        ),
    ]
}

pub(crate) fn single_session_visible_body(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> Vec<String> {
    single_session_visible_styled_body(app, size)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

pub(crate) fn single_session_visible_styled_body(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> Vec<SingleSessionStyledLine> {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let available_height =
        (single_session_draft_top(size) - PANEL_BODY_TOP_PADDING - 12.0).max(line_height);
    let visible_lines = ((available_height / line_height).floor() as usize).max(1);
    let lines = app.body_styled_lines();
    if lines.len() <= visible_lines {
        return lines;
    }

    let max_scroll = lines.len().saturating_sub(visible_lines);
    let scroll = app.body_scroll_lines.min(max_scroll);
    let end = lines.len().saturating_sub(scroll);
    let start = end.saturating_sub(visible_lines);
    lines[start..end].to_vec()
}

pub(crate) fn single_session_body_line_at_y(size: PhysicalSize<u32>, y: f32) -> Option<usize> {
    let typography = single_session_typography();
    let line_height = typography.body_size * typography.body_line_height;
    let draft_top = single_session_draft_top(size);
    if y < PANEL_BODY_TOP_PADDING || y >= draft_top - 12.0 {
        return None;
    }
    Some(((y - PANEL_BODY_TOP_PADDING) / line_height).floor() as usize)
}

pub(crate) fn single_session_body_point_at_position(
    size: PhysicalSize<u32>,
    x: f32,
    y: f32,
    lines: &[String],
) -> Option<SelectionPoint> {
    let line = single_session_body_line_at_y(size, y)?;
    let text = lines.get(line)?;
    Some(SelectionPoint {
        line,
        column: single_session_body_column_at_x(x, text),
    })
}

pub(crate) fn single_session_body_column_at_x(x: f32, line: &str) -> usize {
    let char_count = line.chars().count();
    if x <= PANEL_TITLE_LEFT_PADDING {
        return 0;
    }
    let raw = ((x - PANEL_TITLE_LEFT_PADDING) / single_session_body_char_width()).round();
    raw.max(0.0).min(char_count as f32) as usize
}

pub(crate) fn single_session_body_char_width() -> f32 {
    let typography = single_session_typography();
    typography.body_size * 0.58
}

fn single_session_text_buffer(
    font_system: &mut FontSystem,
    text: &str,
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
        Attrs::new().family(Family::Name(SINGLE_SESSION_FONT_FAMILY)),
        Shaping::Basic,
    );
    buffer.shape_until_scroll(font_system);
    buffer
}

fn single_session_styled_text_buffer(
    font_system: &mut FontSystem,
    lines: &[SingleSessionStyledLine],
    font_size: f32,
    line_height: f32,
    width: f32,
    height: f32,
) -> Buffer {
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, line_height));
    buffer.set_size(font_system, width, height);
    let segments = single_session_styled_text_segments(lines);
    buffer.set_rich_text(
        font_system,
        segments
            .iter()
            .map(|(text, style)| (text.as_str(), single_session_line_attrs(*style))),
        Shaping::Basic,
    );
    buffer.shape_until_scroll(font_system);
    buffer
}

fn single_session_styled_text_segments(
    lines: &[SingleSessionStyledLine],
) -> Vec<(String, SingleSessionLineStyle)> {
    let mut segments = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.text.is_empty() {
            segments.push((line.text.clone(), line.style));
        }
        if index + 1 < lines.len() {
            segments.push(("\n".to_string(), SingleSessionLineStyle::Blank));
        }
    }
    if segments.is_empty() {
        segments.push((String::new(), SingleSessionLineStyle::Blank));
    }
    segments
}

fn single_session_line_attrs(style: SingleSessionLineStyle) -> Attrs<'static> {
    Attrs::new()
        .family(Family::Name(SINGLE_SESSION_FONT_FAMILY))
        .color(single_session_line_color(style))
}

pub(crate) fn single_session_line_color(style: SingleSessionLineStyle) -> TextColor {
    text_color(match style {
        SingleSessionLineStyle::Assistant => ASSISTANT_TEXT_COLOR,
        SingleSessionLineStyle::AssistantHeading => ASSISTANT_HEADING_TEXT_COLOR,
        SingleSessionLineStyle::AssistantQuote => ASSISTANT_QUOTE_TEXT_COLOR,
        SingleSessionLineStyle::AssistantTable => ASSISTANT_TABLE_TEXT_COLOR,
        SingleSessionLineStyle::AssistantLink => ASSISTANT_LINK_TEXT_COLOR,
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
    })
}

pub(crate) fn single_session_text_areas(
    buffers: &[Buffer],
    size: PhysicalSize<u32>,
) -> Vec<TextArea<'_>> {
    if buffers.len() < 5 {
        return Vec::new();
    }

    let left = PANEL_TITLE_LEFT_PADDING;
    let right = size.width.saturating_sub(PANEL_TITLE_LEFT_PADDING as u32) as i32;
    let bottom = size.height.saturating_sub(PANEL_TITLE_TOP_PADDING as u32) as i32;
    let draft_top = single_session_draft_top(size);
    let version_left = (size.width as f32 * 0.42).max(left + 220.0);

    vec![
        TextArea {
            buffer: &buffers[0],
            left,
            top: PANEL_TITLE_TOP_PADDING,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right,
                bottom: 64,
            },
            default_color: text_color(PANEL_TITLE_COLOR),
        },
        TextArea {
            buffer: &buffers[4],
            left: version_left,
            top: PANEL_TITLE_TOP_PADDING + 3.0,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right,
                bottom: 64,
            },
            default_color: text_color(META_TEXT_COLOR),
        },
        TextArea {
            buffer: &buffers[1],
            left,
            top: PANEL_BODY_TOP_PADDING,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right,
                bottom: draft_top as i32 - 12,
            },
            default_color: text_color(ASSISTANT_TEXT_COLOR),
        },
        TextArea {
            buffer: &buffers[3],
            left,
            top: draft_top - SINGLE_SESSION_STATUS_GAP,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: (draft_top - SINGLE_SESSION_STATUS_GAP) as i32,
                right,
                bottom: draft_top as i32,
            },
            default_color: text_color(PANEL_SECTION_COLOR),
        },
        TextArea {
            buffer: &buffers[2],
            left,
            top: draft_top,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: draft_top as i32,
                right,
                bottom,
            },
            default_color: text_color(PANEL_SECTION_COLOR),
        },
    ]
}

fn visualize_composer_whitespace(text: &str) -> String {
    text.to_string()
}

pub(crate) fn desktop_header_version_label() -> String {
    let version = option_env!("JCODE_DESKTOP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    let binary = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unknown binary".to_string());
    format!("{binary} · {version}")
}

fn text_color(color: [f32; 4]) -> TextColor {
    TextColor::rgba(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}
