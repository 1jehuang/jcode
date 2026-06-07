use super::*;

const ISSUE_BROWSER_WIDE_MIN_WIDTH: u32 = 1180;
const ISSUE_BROWSER_MEDIUM_MIN_WIDTH: u32 = 860;
const ISSUE_BROWSER_PANEL_RADIUS: f32 = 12.0;
const ISSUE_BROWSER_ROW_HEIGHT: f32 = 70.0;
const ISSUE_BROWSER_PANEL_PADDING: f32 = 14.0;
const ISSUE_BROWSER_CARD_BACKGROUND: [f32; 4] = [0.970, 0.984, 1.000, 0.58];
const ISSUE_BROWSER_CARD_BORDER: [f32; 4] = [0.080, 0.170, 0.360, 0.16];
const ISSUE_BROWSER_SELECTED_ROW: [f32; 4] = [0.155, 0.360, 0.860, 0.16];
const ISSUE_BROWSER_ACTIVE_ROW: [f32; 4] = [0.045, 0.530, 0.330, 0.16];
const ISSUE_BROWSER_TEXT: [f32; 4] = [0.060, 0.078, 0.115, 0.96];
const ISSUE_BROWSER_MUTED_TEXT: [f32; 4] = [0.245, 0.285, 0.360, 0.84];
const ISSUE_BROWSER_ACCENT: [f32; 4] = [0.090, 0.315, 0.900, 0.70];
const ISSUE_BROWSER_P0: [f32; 4] = [0.900, 0.140, 0.160, 0.82];
const ISSUE_BROWSER_P1: [f32; 4] = [0.950, 0.520, 0.120, 0.82];
const ISSUE_BROWSER_P2: [f32; 4] = [0.120, 0.520, 0.820, 0.78];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IssueBrowserLayoutMode {
    Hidden,
    Wide,
    Medium,
    Narrow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IssueBrowserLayout {
    pub(crate) mode: IssueBrowserLayoutMode,
    pub(crate) list: Option<Rect>,
    pub(crate) preview: Option<Rect>,
    pub(crate) chat: Rect,
}

impl IssueBrowserLayout {
    pub(crate) fn hidden(size: PhysicalSize<u32>) -> Self {
        Self {
            mode: IssueBrowserLayoutMode::Hidden,
            list: None,
            preview: None,
            chat: Rect {
                x: 0.0,
                y: 0.0,
                width: size.width.max(1) as f32,
                height: size.height.max(1) as f32,
            },
        }
    }

    pub(crate) fn visible(self) -> bool {
        self.mode != IssueBrowserLayoutMode::Hidden
    }

    pub(crate) fn chat_size(self) -> PhysicalSize<u32> {
        workspace_panel_size(self.chat)
    }
}

pub(crate) fn issue_browser_layout(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
) -> IssueBrowserLayout {
    if !app.issue_browser_visible() {
        return IssueBrowserLayout::hidden(size);
    }

    let width = size.width.max(1) as f32;
    let height = size.height.max(1) as f32;
    if size.width < ISSUE_BROWSER_MEDIUM_MIN_WIDTH {
        return IssueBrowserLayout {
            mode: IssueBrowserLayoutMode::Narrow,
            list: None,
            preview: None,
            chat: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
        };
    }

    let content_top = OUTER_PADDING;
    let content_height = (height - OUTER_PADDING * 2.0).max(1.0);
    if size.width >= ISSUE_BROWSER_WIDE_MIN_WIDTH {
        let list_width = (width * 0.215).clamp(252.0, 328.0);
        let preview_width = (width * 0.285).clamp(332.0, 468.0);
        let chat_x = OUTER_PADDING + list_width + GAP + preview_width + GAP;
        let chat_width = (width - chat_x - OUTER_PADDING).max(360.0);
        return IssueBrowserLayout {
            mode: IssueBrowserLayoutMode::Wide,
            list: Some(Rect {
                x: OUTER_PADDING,
                y: content_top,
                width: list_width,
                height: content_height,
            }),
            preview: Some(Rect {
                x: OUTER_PADDING + list_width + GAP,
                y: content_top,
                width: preview_width,
                height: content_height,
            }),
            chat: Rect {
                x: chat_x,
                y: 0.0,
                width: chat_width,
                height,
            },
        };
    }

    let browser_width = (width * 0.43).clamp(326.0, 430.0);
    let list_height = (content_height * 0.46).clamp(220.0, content_height - 220.0);
    let preview_y = content_top + list_height + GAP;
    let chat_x = OUTER_PADDING + browser_width + GAP;
    IssueBrowserLayout {
        mode: IssueBrowserLayoutMode::Medium,
        list: Some(Rect {
            x: OUTER_PADDING,
            y: content_top,
            width: browser_width,
            height: list_height,
        }),
        preview: Some(Rect {
            x: OUTER_PADDING,
            y: preview_y,
            width: browser_width,
            height: (height - preview_y - OUTER_PADDING).max(1.0),
        }),
        chat: Rect {
            x: chat_x,
            y: 0.0,
            width: (width - chat_x).max(360.0),
            height,
        },
    }
}

fn push_issue_browser_shell(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    layout: IssueBrowserLayout,
    size: PhysicalSize<u32>,
) {
    if !layout.visible() {
        return;
    }
    if let Some(list_rect) = layout.list {
        push_issue_list_panel(vertices, app, list_rect, size);
    }
    if let Some(preview_rect) = layout.preview {
        push_issue_preview_panel(vertices, app, preview_rect, size);
    }
    match app.side_panel().focus {
        single_session::DesktopSidePanelFocus::IssueList => {
            if let Some(rect) = layout.list {
                push_rounded_rect_border(
                    vertices,
                    rect,
                    ISSUE_BROWSER_PANEL_RADIUS,
                    2.0,
                    [0.090, 0.315, 0.900, 0.34],
                    size,
                );
            }
        }
        single_session::DesktopSidePanelFocus::IssuePreview => {
            if let Some(rect) = layout.preview {
                push_rounded_rect_border(
                    vertices,
                    rect,
                    ISSUE_BROWSER_PANEL_RADIUS,
                    2.0,
                    [0.090, 0.315, 0.900, 0.34],
                    size,
                );
            }
        }
        single_session::DesktopSidePanelFocus::Chat => {
            push_rounded_rect_border(
                vertices,
                layout.chat,
                ISSUE_BROWSER_PANEL_RADIUS,
                2.0,
                [0.090, 0.315, 0.900, 0.20],
                size,
            );
        }
    }
}

fn issue_priority_color(priority: &str) -> [f32; 4] {
    match priority {
        "P0" => ISSUE_BROWSER_P0,
        "P1" => ISSUE_BROWSER_P1,
        _ => ISSUE_BROWSER_P2,
    }
}

fn push_issue_panel_frame(vertices: &mut Vec<Vertex>, rect: Rect, size: PhysicalSize<u32>) {
    push_rounded_rect(
        vertices,
        rect,
        ISSUE_BROWSER_PANEL_RADIUS,
        ISSUE_BROWSER_CARD_BACKGROUND,
        size,
    );
    push_rounded_rect_border(
        vertices,
        rect,
        ISSUE_BROWSER_PANEL_RADIUS,
        1.4,
        ISSUE_BROWSER_CARD_BORDER,
        size,
    );
}

fn push_issue_browser_text(
    vertices: &mut Vec<Vertex>,
    text: &str,
    x: f32,
    y: f32,
    color: [f32; 4],
    size: PhysicalSize<u32>,
    max_width: f32,
) {
    push_bitmap_text(
        vertices,
        &normalize_bitmap_text(text),
        x,
        y,
        BITMAP_TEXT_PIXEL,
        color,
        size,
        max_width.max(1.0),
    );
}

fn push_issue_list_panel(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    rect: Rect,
    size: PhysicalSize<u32>,
) {
    let browser = &app.side_panel().github_issues;
    push_issue_panel_frame(vertices, rect, size);
    let x = rect.x + ISSUE_BROWSER_PANEL_PADDING;
    let max_width = (rect.width - ISSUE_BROWSER_PANEL_PADDING * 2.0).max(1.0);
    push_issue_browser_text(
        vertices,
        "GitHub Issues",
        x,
        rect.y + 12.0,
        ISSUE_BROWSER_TEXT,
        size,
        max_width,
    );
    push_issue_browser_text(
        vertices,
        &browser.repo,
        x,
        rect.y + 34.0,
        ISSUE_BROWSER_MUTED_TEXT,
        size,
        max_width,
    );
    push_issue_browser_text(
        vertices,
        &browser.filter_label,
        x,
        rect.y + 54.0,
        ISSUE_BROWSER_MUTED_TEXT,
        size,
        max_width,
    );
    let sync_label = app.side_panel().github_issue_sync.label();
    if let Some(sync_label) = sync_label.as_deref() {
        push_issue_browser_text(
            vertices,
            sync_label,
            x,
            rect.y + 74.0,
            ISSUE_BROWSER_ACCENT,
            size,
            max_width,
        );
    }
    let guidance_label = app.side_panel().github_issue_sync.guidance();
    if let Some(guidance_label) = guidance_label.as_deref() {
        push_issue_browser_text(
            vertices,
            guidance_label,
            x,
            rect.y + 94.0,
            ISSUE_BROWSER_MUTED_TEXT,
            size,
            max_width,
        );
    }

    let mut row_y = rect.y
        + if guidance_label.is_some() {
            124.0
        } else if sync_label.is_some() {
            104.0
        } else {
            84.0
        };
    let max_y = rect.y + rect.height - ISSUE_BROWSER_PANEL_PADDING;
    if browser.issues.is_empty() {
        push_issue_browser_text(
            vertices,
            "No cached issues yet. Authenticate gh, then press r or Ctrl+R to sync.",
            x,
            row_y,
            ISSUE_BROWSER_MUTED_TEXT,
            size,
            max_width,
        );
        return;
    }
    for (index, issue) in browser.issues.iter().enumerate().skip(browser.list_scroll) {
        if row_y + ISSUE_BROWSER_ROW_HEIGHT > max_y {
            break;
        }
        let row = Rect {
            x: rect.x + 8.0,
            y: row_y,
            width: rect.width - 16.0,
            height: ISSUE_BROWSER_ROW_HEIGHT - 6.0,
        };
        if issue.state == single_session::GitHubIssueVisualState::Active {
            push_rounded_rect(vertices, row, 8.0, ISSUE_BROWSER_ACTIVE_ROW, size);
        } else if index == browser.selected
            || issue.state == single_session::GitHubIssueVisualState::Selected
        {
            push_rounded_rect(vertices, row, 8.0, ISSUE_BROWSER_SELECTED_ROW, size);
        }
        push_rounded_rect(
            vertices,
            Rect {
                x: row.x + 5.0,
                y: row.y + 6.0,
                width: 4.0,
                height: row.height - 12.0,
            },
            2.0,
            issue_priority_color(&issue.priority),
            size,
        );
        let text_x = row.x + 16.0;
        push_issue_browser_text(
            vertices,
            &format!("{} #{}", issue.priority, issue.number),
            text_x,
            row.y + 8.0,
            issue_priority_color(&issue.priority),
            size,
            row.width - 24.0,
        );
        push_issue_browser_text(
            vertices,
            &issue.title,
            text_x,
            row.y + 28.0,
            ISSUE_BROWSER_TEXT,
            size,
            row.width - 24.0,
        );
        push_issue_browser_text(
            vertices,
            &format!(
                "{} · {} comments · {}",
                issue.labels.join(","),
                issue.comments,
                issue.age
            ),
            text_x,
            row.y + 49.0,
            ISSUE_BROWSER_MUTED_TEXT,
            size,
            row.width - 24.0,
        );
        row_y += ISSUE_BROWSER_ROW_HEIGHT;
    }
}

fn push_issue_preview_panel(
    vertices: &mut Vec<Vertex>,
    app: &SingleSessionApp,
    rect: Rect,
    size: PhysicalSize<u32>,
) {
    let browser = &app.side_panel().github_issues;
    push_issue_panel_frame(vertices, rect, size);
    let x = rect.x + ISSUE_BROWSER_PANEL_PADDING;
    let max_width = (rect.width - ISSUE_BROWSER_PANEL_PADDING * 2.0).max(1.0);
    let Some(issue) = browser.selected_issue() else {
        push_issue_browser_text(
            vertices,
            "No issue selected",
            x,
            rect.y + 14.0,
            ISSUE_BROWSER_TEXT,
            size,
            max_width,
        );
        return;
    };

    push_issue_browser_text(
        vertices,
        &format!("Preview #{}", issue.number),
        x,
        rect.y + 12.0,
        ISSUE_BROWSER_TEXT,
        size,
        max_width,
    );
    push_issue_browser_text(
        vertices,
        &issue.title,
        x,
        rect.y + 36.0,
        ISSUE_BROWSER_TEXT,
        size,
        max_width,
    );
    push_issue_browser_text(
        vertices,
        &format!(
            "{} · {} · {} comments",
            issue.priority,
            issue.labels.join(","),
            issue.comments
        ),
        x,
        rect.y + 60.0,
        ISSUE_BROWSER_MUTED_TEXT,
        size,
        max_width,
    );

    let action = Rect {
        x,
        y: rect.y + 86.0,
        width: (max_width * 0.58).clamp(150.0, max_width),
        height: 24.0,
    };
    push_rounded_rect(vertices, action, 7.0, [0.090, 0.315, 0.900, 0.13], size);
    push_rounded_rect_border(
        vertices,
        action,
        7.0,
        1.2,
        [0.090, 0.315, 0.900, 0.28],
        size,
    );
    push_issue_browser_text(
        vertices,
        "Enter · investigate",
        action.x + 10.0,
        action.y + 6.0,
        ISSUE_BROWSER_ACCENT,
        size,
        action.width - 18.0,
    );

    let mut y = rect.y + 126.0;
    let line_height = bitmap_text_height(BITMAP_TEXT_PIXEL) + 8.0;
    let max_y = rect.y + rect.height - ISSUE_BROWSER_PANEL_PADDING;
    push_issue_browser_text(
        vertices,
        "Priority rationale",
        x,
        y,
        ISSUE_BROWSER_ACCENT,
        size,
        max_width,
    );
    y += line_height;
    for line in wrap_bitmap_text(&issue.priority_reason, BITMAP_TEXT_PIXEL, max_width) {
        if y + line_height > max_y {
            return;
        }
        push_issue_browser_text(
            vertices,
            &line,
            x,
            y,
            ISSUE_BROWSER_MUTED_TEXT,
            size,
            max_width,
        );
        y += line_height;
    }
    y += 8.0;
    push_issue_browser_text(
        vertices,
        "Issue body",
        x,
        y,
        ISSUE_BROWSER_ACCENT,
        size,
        max_width,
    );
    y += line_height;
    for line in issue.body_lines.iter().skip(browser.preview_scroll) {
        for visual_line in wrap_bitmap_text(line, BITMAP_TEXT_PIXEL, max_width) {
            if y + line_height > max_y {
                return;
            }
            push_issue_browser_text(
                vertices,
                &visual_line,
                x,
                y,
                ISSUE_BROWSER_TEXT,
                size,
                max_width,
            );
            y += line_height;
        }
        y += 4.0;
    }
    y += 8.0;
    push_issue_browser_text(
        vertices,
        "Recent comments",
        x,
        y,
        ISSUE_BROWSER_ACCENT,
        size,
        max_width,
    );
    y += line_height;
    for line in &issue.comment_lines {
        for visual_line in wrap_bitmap_text(line, BITMAP_TEXT_PIXEL, max_width) {
            if y + line_height > max_y {
                return;
            }
            push_issue_browser_text(
                vertices,
                &visual_line,
                x,
                y,
                ISSUE_BROWSER_MUTED_TEXT,
                size,
                max_width,
            );
            y += line_height;
        }
        y += 4.0;
    }
}

pub(crate) fn compose_single_session_issue_browser_vertices(
    app: &SingleSessionApp,
    layout: IssueBrowserLayout,
    child_vertices: &[Vertex],
    child_size: PhysicalSize<u32>,
    size: PhysicalSize<u32>,
) -> Vec<Vertex> {
    let mut vertices = Vec::with_capacity(child_vertices.len() + 1024);
    push_gradient_rect(
        &mut vertices,
        Rect {
            x: 0.0,
            y: 0.0,
            width: size.width.max(1) as f32,
            height: size.height.max(1) as f32,
        },
        BACKGROUND_TOP_LEFT,
        BACKGROUND_BOTTOM_LEFT,
        BACKGROUND_BOTTOM_RIGHT,
        BACKGROUND_TOP_RIGHT,
        size,
    );
    push_issue_browser_shell(&mut vertices, app, layout, size);
    append_child_vertices_to_parent_with_opacity(
        &mut vertices,
        child_vertices,
        child_size,
        layout.chat,
        size,
        1.0,
    );
    vertices
}
