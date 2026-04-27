use super::*;

pub(crate) fn push_panel_title(
    vertices: &mut Vec<Vertex>,
    title: &str,
    rect: Rect,
    size: PhysicalSize<u32>,
) {
    let text = normalize_bitmap_text(title);
    let max_width = (rect.width - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
    push_bitmap_text(
        vertices,
        &text,
        rect.x + PANEL_TITLE_LEFT_PADDING,
        rect.y + PANEL_TITLE_TOP_PADDING,
        BITMAP_TEXT_PIXEL,
        PANEL_TITLE_COLOR,
        size,
        max_width,
    );
}

pub(crate) fn push_panel_contents(
    vertices: &mut Vec<Vertex>,
    surface: &workspace::Surface,
    rect: Rect,
    size: PhysicalSize<u32>,
    expanded: bool,
    scroll_lines: usize,
    draft: Option<&str>,
) {
    push_panel_title(vertices, surface.title.as_str(), rect, size);

    let lines = if expanded && !surface.detail_lines.is_empty() {
        &surface.detail_lines
    } else {
        &surface.body_lines
    };
    let max_width = (rect.width - PANEL_TITLE_LEFT_PADDING * 2.0).max(1.0);
    let mut y = rect.y + PANEL_BODY_TOP_PADDING;
    let line_height = bitmap_text_height(BITMAP_TEXT_PIXEL) + PANEL_BODY_LINE_GAP;
    let max_y = rect.y + rect.height - PANEL_TITLE_TOP_PADDING;
    for line in lines.iter().skip(if expanded { scroll_lines } else { 0 }) {
        let text = normalize_bitmap_text(line);
        let color = if is_panel_section_header(line) {
            PANEL_SECTION_COLOR
        } else {
            PANEL_BODY_COLOR
        };
        for visual_line in wrap_bitmap_text(&text, BITMAP_TEXT_PIXEL, max_width) {
            if y + bitmap_text_height(BITMAP_TEXT_PIXEL) > max_y {
                return;
            }
            push_bitmap_text(
                vertices,
                &visual_line,
                rect.x + PANEL_TITLE_LEFT_PADDING,
                y,
                BITMAP_TEXT_PIXEL,
                color,
                size,
                max_width,
            );
            y += line_height;
        }
    }

    if let Some(draft) = draft {
        let mut draft_y = (rect.y + rect.height - PANEL_BODY_TOP_PADDING).max(y + line_height);
        let draft_text = normalize_bitmap_text(&format!("draft {draft}"));
        for visual_line in wrap_bitmap_text(&draft_text, BITMAP_TEXT_PIXEL, max_width)
            .into_iter()
            .take(2)
        {
            if draft_y + bitmap_text_height(BITMAP_TEXT_PIXEL) > max_y {
                break;
            }
            push_bitmap_text(
                vertices,
                &visual_line,
                rect.x + PANEL_TITLE_LEFT_PADDING,
                draft_y,
                BITMAP_TEXT_PIXEL,
                PANEL_SECTION_COLOR,
                size,
                max_width,
            );
            draft_y += line_height;
        }
    }
}

pub(crate) fn focused_panel_draft(workspace: &Workspace, surface_id: u64) -> Option<String> {
    if workspace.mode == InputMode::Insert
        && workspace.is_focused(surface_id)
        && !workspace.draft.trim().is_empty()
    {
        Some(workspace.draft.trim().to_string())
    } else {
        None
    }
}

pub(crate) fn is_panel_section_header(line: &str) -> bool {
    matches!(
        line,
        "session metadata" | "recent transcript" | "expanded transcript"
    )
}

pub(crate) fn wrap_bitmap_text(text: &str, pixel: f32, max_width: f32) -> Vec<String> {
    let max_chars = ((max_width / bitmap_char_advance(pixel)).floor() as usize).max(1);
    let words = text.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in words {
        if word.chars().count() > max_chars {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            push_wrapped_long_word(&mut lines, word, max_chars);
            continue;
        }

        let separator = usize::from(!current.is_empty());
        if current.chars().count() + separator + word.chars().count() > max_chars {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

pub(crate) fn push_wrapped_long_word(lines: &mut Vec<String>, word: &str, max_chars: usize) {
    let mut chunk = String::new();
    for ch in word.chars() {
        if chunk.chars().count() >= max_chars {
            lines.push(std::mem::take(&mut chunk));
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        lines.push(chunk);
    }
}

pub(crate) fn normalize_bitmap_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_was_space = false;
    for ch in text.chars() {
        let mapped = match ch {
            'a'..='z' => ch.to_ascii_uppercase(),
            'A'..='Z' | '0'..='9' => ch,
            '-' | '/' => ch,
            _ => ' ',
        };
        if mapped == ' ' {
            if !last_was_space {
                normalized.push(mapped);
            }
            last_was_space = true;
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }
    normalized.trim().to_string()
}

pub(crate) fn push_bitmap_text(
    vertices: &mut Vec<Vertex>,
    text: &str,
    x: f32,
    y: f32,
    pixel: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
    max_width: f32,
) {
    let advance = bitmap_char_advance(pixel);
    let mut cursor_x = x;
    for ch in text.chars() {
        if cursor_x + 5.0 * pixel > x + max_width {
            break;
        }
        if let Some(rows) = bitmap_glyph(ch) {
            for (row_index, row) in rows.iter().enumerate() {
                for column in 0..5 {
                    let mask = 1 << (4 - column);
                    if row & mask != 0 {
                        push_rect(
                            vertices,
                            Rect {
                                x: cursor_x + column as f32 * pixel,
                                y: y + row_index as f32 * pixel,
                                width: pixel,
                                height: pixel,
                            },
                            color,
                            size,
                        );
                    }
                }
            }
        }
        cursor_x += advance;
    }
}

pub(crate) fn bitmap_text_width(text: &str, pixel: f32) -> f32 {
    let count = text.chars().count();
    if count == 0 {
        0.0
    } else {
        count as f32 * 5.0 * pixel + count.saturating_sub(1) as f32 * pixel
    }
}

pub(crate) fn bitmap_text_height(pixel: f32) -> f32 {
    7.0 * pixel
}

pub(crate) fn bitmap_char_advance(pixel: f32) -> f32 {
    6.0 * pixel
}

pub(crate) fn bitmap_glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        ' ' => [0; 7],
        _ => return None,
    })
}

pub(crate) fn push_workspace_number(
    vertices: &mut Vec<Vertex>,
    active_workspace: i32,
    status_rect: Rect,
    size: PhysicalSize<u32>,
) {
    let label = active_workspace.to_string();
    let digit_count = label.chars().count() as f32;
    let total_width = digit_count * WORKSPACE_NUMBER_DIGIT_WIDTH
        + (digit_count - 1.0).max(0.0) * WORKSPACE_NUMBER_DIGIT_GAP;
    let mut x = status_rect.x + WORKSPACE_NUMBER_LEFT_PADDING;
    let y = status_rect.y + (status_rect.height - WORKSPACE_NUMBER_DIGIT_HEIGHT) / 2.0;
    if x + total_width > status_rect.x + status_rect.width {
        return;
    }

    for ch in label.chars() {
        match ch {
            '-' => push_workspace_minus(vertices, x, y, size),
            digit if digit.is_ascii_digit() => {
                let digit = digit.to_digit(10).unwrap_or_default() as usize;
                push_workspace_digit(vertices, digit, x, y, size);
            }
            _ => {}
        }
        x += WORKSPACE_NUMBER_DIGIT_WIDTH + WORKSPACE_NUMBER_DIGIT_GAP;
    }
}

pub(crate) fn push_workspace_minus(
    vertices: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    size: PhysicalSize<u32>,
) {
    let thickness = WORKSPACE_NUMBER_STROKE;
    push_rounded_rect(
        vertices,
        Rect {
            x,
            y: y + WORKSPACE_NUMBER_DIGIT_HEIGHT / 2.0 - thickness / 2.0,
            width: WORKSPACE_NUMBER_DIGIT_WIDTH,
            height: thickness,
        },
        thickness / 2.0,
        WORKSPACE_NUMBER_COLOR,
        size,
    );
}

pub(crate) fn push_workspace_digit(
    vertices: &mut Vec<Vertex>,
    digit: usize,
    x: f32,
    y: f32,
    size: PhysicalSize<u32>,
) {
    const DIGIT_SEGMENTS: [[bool; 7]; 10] = [
        [true, true, true, true, true, true, false],
        [false, true, true, false, false, false, false],
        [true, true, false, true, true, false, true],
        [true, true, true, true, false, false, true],
        [false, true, true, false, false, true, true],
        [true, false, true, true, false, true, true],
        [true, false, true, true, true, true, true],
        [true, true, true, false, false, false, false],
        [true, true, true, true, true, true, true],
        [true, true, true, true, false, true, true],
    ];
    let segments = DIGIT_SEGMENTS[digit % DIGIT_SEGMENTS.len()];
    for rect in workspace_digit_segment_rects(x, y)
        .into_iter()
        .zip(segments)
        .filter_map(|(rect, enabled)| enabled.then_some(rect))
    {
        push_rounded_rect(
            vertices,
            rect,
            WORKSPACE_NUMBER_STROKE / 2.0,
            WORKSPACE_NUMBER_COLOR,
            size,
        );
    }
}

pub(crate) fn workspace_digit_segment_rects(x: f32, y: f32) -> [Rect; 7] {
    let w = WORKSPACE_NUMBER_DIGIT_WIDTH;
    let h = WORKSPACE_NUMBER_DIGIT_HEIGHT;
    let t = WORKSPACE_NUMBER_STROKE;
    let vertical_height = (h - t * 3.0) / 2.0;
    [
        Rect {
            x,
            y,
            width: w,
            height: t,
        },
        Rect {
            x: x + w - t,
            y,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x: x + w - t,
            y: y + h / 2.0,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x,
            y: y + h - t,
            width: w,
            height: t,
        },
        Rect {
            x,
            y: y + h / 2.0,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x,
            y,
            width: t,
            height: vertical_height + t,
        },
        Rect {
            x,
            y: y + h / 2.0 - t / 2.0,
            width: w,
            height: t,
        },
    ]
}

pub(crate) fn push_status_preview(
    vertices: &mut Vec<Vertex>,
    workspace: &Workspace,
    active_workspace: i32,
    visible_layout: VisibleColumnLayout,
    status_rect: Rect,
    size: PhysicalSize<u32>,
) {
    let first_lane = active_workspace - STATUS_PREVIEW_LANE_RADIUS;
    let last_lane = active_workspace + STATUS_PREVIEW_LANE_RADIUS;
    let lanes: Vec<StatusPreviewLane> = (first_lane..=last_lane)
        .map(|lane| status_preview_lane(workspace, lane, active_workspace, visible_layout))
        .filter(|lane| !lane.is_empty || lane.is_active)
        .collect();

    if lanes.is_empty() {
        return;
    }

    let full_width = lanes.iter().map(StatusPreviewLane::width).sum::<f32>()
        + STATUS_PREVIEW_GROUP_GAP * lanes.len().saturating_sub(1) as f32;
    let preview_area = inset_rect(
        status_rect,
        STATUS_PREVIEW_SIDE_RESERVE.min(status_rect.width / 4.0),
    );
    let max_width = STATUS_PREVIEW_MAX_WIDTH.min((preview_area.width - 24.0).max(1.0));
    if max_width < 24.0 {
        return;
    }
    let scale = (max_width / full_width).min(1.0);
    let panel_width = (STATUS_PREVIEW_PANEL_WIDTH * scale).max(2.0);
    let panel_gap = (STATUS_PREVIEW_PANEL_GAP * scale).max(1.0);
    let group_gap = (STATUS_PREVIEW_GROUP_GAP * scale).max(4.0);
    let scaled_width = lanes
        .iter()
        .map(|lane| lane.scaled_width(panel_width, panel_gap))
        .sum::<f32>()
        + group_gap * lanes.len().saturating_sub(1) as f32;
    let strip_height = STATUS_PREVIEW_HEIGHT.min((status_rect.height - 8.0).max(1.0));
    let strip_y = status_rect.y + (status_rect.height - strip_height) / 2.0;
    let mut cursor_x = preview_area.x + (preview_area.width - scaled_width) / 2.0;

    for lane in lanes {
        let lane_width = lane.scaled_width(panel_width, panel_gap);
        let lane_rect = Rect {
            x: cursor_x - 3.0,
            y: strip_y - 3.0,
            width: lane_width + 6.0,
            height: strip_height + 6.0,
        };

        if lane.is_active {
            push_rounded_rect(
                vertices,
                lane_rect,
                5.0,
                STATUS_PREVIEW_ACTIVE_GROUP_COLOR,
                size,
            );
        }

        if lane.is_empty {
            push_rounded_rect(
                vertices,
                Rect {
                    x: cursor_x + lane_width / 2.0 - 2.0,
                    y: strip_y + strip_height / 2.0 - 2.0,
                    width: 4.0,
                    height: 4.0,
                },
                2.0,
                STATUS_PREVIEW_EMPTY_FOCUSED_COLOR,
                size,
            );
            cursor_x += lane_width + group_gap;
            continue;
        }

        for surface in workspace
            .surfaces
            .iter()
            .filter(|surface| surface.lane == lane.lane)
        {
            let column_offset = (surface.column - lane.min_column) as f32;
            let surface_x = cursor_x + column_offset * (panel_width + panel_gap);
            let focused = workspace.is_focused(surface.id);
            let color = status_preview_surface_color(surface.color_index, focused, lane.is_active);
            let tick_width = if focused {
                panel_width
            } else {
                panel_width * 0.56
            };
            let tick_x = surface_x + (panel_width - tick_width) / 2.0;
            push_rounded_rect(
                vertices,
                Rect {
                    x: tick_x,
                    y: strip_y,
                    width: tick_width.max(2.0),
                    height: strip_height,
                },
                2.0,
                color,
                size,
            );
        }

        if lane.is_active {
            let viewport_x = cursor_x
                + (visible_layout.first_visible_column - lane.min_column) as f32
                    * (panel_width + panel_gap);
            let viewport_width = visible_layout.visible_columns as f32 * panel_width
                + visible_layout.visible_columns.saturating_sub(1) as f32 * panel_gap;
            push_stroked_rect(
                vertices,
                Rect {
                    x: viewport_x - 1.5,
                    y: strip_y - 2.0,
                    width: (viewport_width + 3.0).min(cursor_x + lane_width - viewport_x + 1.5),
                    height: strip_height + 4.0,
                },
                1.0,
                STATUS_PREVIEW_VIEWPORT_COLOR,
                size,
            );
        }

        cursor_x += lane_width + group_gap;
    }
}

pub(crate) fn status_preview_surface_color(
    color_index: usize,
    focused: bool,
    active_lane: bool,
) -> [f32; 4] {
    let accent = STATUS_PREVIEW_ACCENTS[color_index % STATUS_PREVIEW_ACCENTS.len()];
    let alpha = if focused {
        0.94
    } else if active_lane {
        0.72
    } else {
        0.34
    };
    [accent[0], accent[1], accent[2], alpha]
}

#[derive(Clone, Copy)]
struct StatusPreviewLane {
    lane: i32,
    min_column: i32,
    max_column: i32,
    is_active: bool,
    is_empty: bool,
}

impl StatusPreviewLane {
    fn column_count(&self) -> i32 {
        (self.max_column - self.min_column + 1).max(1)
    }

    fn width(&self) -> f32 {
        self.scaled_width(STATUS_PREVIEW_PANEL_WIDTH, STATUS_PREVIEW_PANEL_GAP)
    }

    fn scaled_width(&self, panel_width: f32, panel_gap: f32) -> f32 {
        let column_count = self.column_count() as f32;
        column_count * panel_width + (column_count - 1.0).max(0.0) * panel_gap
    }
}

pub(crate) fn status_preview_lane(
    workspace: &Workspace,
    lane: i32,
    active_workspace: i32,
    visible_layout: VisibleColumnLayout,
) -> StatusPreviewLane {
    let is_active = lane == active_workspace;
    let viewport_first_column = visible_layout.first_visible_column;
    let viewport_last_column =
        viewport_first_column + visible_layout.visible_columns.saturating_sub(1) as i32;
    let mut min_column = if is_active {
        viewport_first_column
    } else {
        i32::MAX
    };
    let mut max_column = if is_active {
        viewport_last_column
    } else {
        i32::MIN
    };
    let mut is_empty = true;

    for surface in workspace
        .surfaces
        .iter()
        .filter(|surface| surface.lane == lane)
    {
        min_column = min_column.min(surface.column);
        max_column = max_column.max(surface.column);
        is_empty = false;
    }

    if is_empty && !is_active {
        min_column = 0;
        max_column = 0;
    }

    StatusPreviewLane {
        lane,
        min_column,
        max_column,
        is_active,
        is_empty,
    }
}

pub(crate) fn push_surface(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    _color_index: usize,
    focused: bool,
    focus_pulse: f32,
    size: PhysicalSize<u32>,
) {
    let border = if focused {
        FOCUS_RING_COLOR
    } else {
        UNFOCUSED_BORDER_COLOR
    };

    let stroke_width = if focused {
        FOCUSED_BORDER_WIDTH
    } else {
        UNFOCUSED_BORDER_WIDTH
    } + focus_pulse * 2.5;
    push_panel_outline(vertices, rect, stroke_width, border, size);

    if focus_pulse > 0.0 {
        let pulse_rect = inset_rect(rect, -3.0 * focus_pulse);
        push_panel_outline(
            vertices,
            pulse_rect,
            1.0,
            with_alpha(FOCUS_RING_COLOR, 0.32 * focus_pulse),
            size,
        );
    }
}

pub(crate) fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha.clamp(0.0, 1.0);
    color
}

pub(crate) fn push_panel_outline(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    stroke_width: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let stroke_width = stroke_width
        .max(1.0)
        .min(rect.width / 2.0)
        .min(rect.height / 2.0);
    let outer_radius = PANEL_RADIUS.min(rect.width / 2.0).min(rect.height / 2.0);
    let inner = inset_rect(rect, stroke_width);
    let inner_radius = (outer_radius - stroke_width).max(0.0);
    let outer_points = rounded_rect_points(rect, outer_radius);
    let inner_points = rounded_rect_points(inner, inner_radius);

    for index in 0..outer_points.len() {
        let next_index = (index + 1) % outer_points.len();
        push_pixel_triangle(
            vertices,
            outer_points[index],
            outer_points[next_index],
            inner_points[next_index],
            color,
            size,
        );
        push_pixel_triangle(
            vertices,
            outer_points[index],
            inner_points[next_index],
            inner_points[index],
            color,
            size,
        );
    }
}

pub(crate) fn rounded_rect_points(rect: Rect, radius: f32) -> Vec<[f32; 2]> {
    let radius = radius.max(0.0).min(rect.width / 2.0).min(rect.height / 2.0);
    let mut points = Vec::with_capacity((ROUNDED_CORNER_SEGMENTS + 1) * 4);
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + radius,
        radius,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    );
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + rect.height - radius,
        radius,
        0.0,
        std::f32::consts::FRAC_PI_2,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + rect.height - radius,
        radius,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + radius,
        radius,
        std::f32::consts::PI,
        std::f32::consts::PI * 1.5,
    );
    points
}

pub(crate) fn inset_rect(rect: Rect, amount: f32) -> Rect {
    Rect {
        x: rect.x + amount,
        y: rect.y + amount,
        width: (rect.width - amount * 2.0).max(1.0),
        height: (rect.height - amount * 2.0).max(1.0),
    }
}

pub(crate) fn push_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    push_gradient_rect(vertices, rect, color, color, color, color, size);
}

pub(crate) fn push_stroked_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    stroke_width: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let stroke_width = stroke_width.max(1.0).min(rect.width).min(rect.height);
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: stroke_width,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y + rect.height - stroke_width,
            width: rect.width,
            height: stroke_width,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x,
            y: rect.y,
            width: stroke_width,
            height: rect.height,
        },
        color,
        size,
    );
    push_rect(
        vertices,
        Rect {
            x: rect.x + rect.width - stroke_width,
            y: rect.y,
            width: stroke_width,
            height: rect.height,
        },
        color,
        size,
    );
}

pub(crate) fn push_rounded_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    radius: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let radius = radius.max(0.0).min(rect.width / 2.0).min(rect.height / 2.0);
    if radius <= 0.5 {
        push_rect(vertices, rect, color, size);
        return;
    }

    let center = [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0];
    let mut points = Vec::with_capacity((ROUNDED_CORNER_SEGMENTS + 1) * 4);
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + radius,
        radius,
        -std::f32::consts::FRAC_PI_2,
        0.0,
    );
    append_arc_points(
        &mut points,
        rect.x + rect.width - radius,
        rect.y + rect.height - radius,
        radius,
        0.0,
        std::f32::consts::FRAC_PI_2,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + rect.height - radius,
        radius,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
    );
    append_arc_points(
        &mut points,
        rect.x + radius,
        rect.y + radius,
        radius,
        std::f32::consts::PI,
        std::f32::consts::PI * 1.5,
    );

    for index in 0..points.len() {
        let next_index = (index + 1) % points.len();
        push_pixel_triangle(
            vertices,
            center,
            points[index],
            points[next_index],
            color,
            size,
        );
    }
}

pub(crate) fn append_arc_points(
    points: &mut Vec<[f32; 2]>,
    center_x: f32,
    center_y: f32,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
) {
    for step in 0..=ROUNDED_CORNER_SEGMENTS {
        let t = step as f32 / ROUNDED_CORNER_SEGMENTS as f32;
        let angle = start_angle + (end_angle - start_angle) * t;
        points.push([
            center_x + radius * angle.cos(),
            center_y + radius * angle.sin(),
        ]);
    }
}

pub(crate) fn push_pixel_triangle(
    vertices: &mut Vec<Vertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    vertices.extend_from_slice(&[
        Vertex {
            position: pixel_to_ndc(a, size),
            color,
        },
        Vertex {
            position: pixel_to_ndc(b, size),
            color,
        },
        Vertex {
            position: pixel_to_ndc(c, size),
            color,
        },
    ]);
}

pub(crate) fn pixel_to_ndc(point: [f32; 2], size: PhysicalSize<u32>) -> [f32; 2] {
    let width = size.width.max(1) as f32;
    let height = size.height.max(1) as f32;
    [point[0] / width * 2.0 - 1.0, 1.0 - point[1] / height * 2.0]
}

pub(crate) fn push_gradient_rect(
    vertices: &mut Vec<Vertex>,
    rect: Rect,
    top_left_color: [f32; 4],
    bottom_left_color: [f32; 4],
    bottom_right_color: [f32; 4],
    top_right_color: [f32; 4],
    size: PhysicalSize<u32>,
) {
    let width = size.width.max(1) as f32;
    let height = size.height.max(1) as f32;
    let left = rect.x / width * 2.0 - 1.0;
    let right = (rect.x + rect.width) / width * 2.0 - 1.0;
    let top = 1.0 - rect.y / height * 2.0;
    let bottom = 1.0 - (rect.y + rect.height) / height * 2.0;

    vertices.extend_from_slice(&[
        Vertex {
            position: [left, top],
            color: top_left_color,
        },
        Vertex {
            position: [left, bottom],
            color: bottom_left_color,
        },
        Vertex {
            position: [right, bottom],
            color: bottom_right_color,
        },
        Vertex {
            position: [left, top],
            color: top_left_color,
        },
        Vertex {
            position: [right, bottom],
            color: bottom_right_color,
        },
        Vertex {
            position: [right, top],
            color: top_right_color,
        },
    ]);
}

pub(crate) fn non_zero_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}
