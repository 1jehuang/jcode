use super::*;
use crate::tui::app::suggested_replies::{SuggestedRepliesView, SuggestedReplyFocus};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate `text` to at most `max_cols` terminal display columns (`UnicodeWidthStr`),
/// appending `…` only when the string actually exceeds `max_cols`.
/// Prevents multi-byte UTF-8 strings (such as Russian/Cyrillic, where 1 char = 2 bytes)
/// from being cut at half the available terminal width.
pub(crate) fn truncate_to_display_width(text: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_cols {
        return text.to_string();
    }
    let target = max_cols.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > target {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[derive(Clone, Debug)]
struct SuggestedReplyHitRegion {
    rect: Rect,
    target: SuggestedReplyFocus,
}

#[cfg(not(test))]
static SUGGESTED_REPLY_HIT_REGIONS: OnceLock<Mutex<Vec<SuggestedReplyHitRegion>>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_SUGGESTED_REPLY_HIT_REGIONS: std::cell::RefCell<Vec<SuggestedReplyHitRegion>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn set_suggested_reply_hit_regions(regions: Vec<SuggestedReplyHitRegion>) {
    #[cfg(test)]
    {
        TEST_SUGGESTED_REPLY_HIT_REGIONS.with(|slot| *slot.borrow_mut() = regions);
        return;
    }
    #[cfg(not(test))]
    {
        let mutex = SUGGESTED_REPLY_HIT_REGIONS.get_or_init(|| Mutex::new(Vec::new()));
        let mut guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
        *guard = regions;
    }
}

pub(crate) fn clear_suggested_reply_hit_regions() {
    set_suggested_reply_hit_regions(Vec::new());
}

pub(crate) fn suggested_reply_target_at(column: u16, row: u16) -> Option<SuggestedReplyFocus> {
    #[cfg(test)]
    let regions = TEST_SUGGESTED_REPLY_HIT_REGIONS.with(|slot| slot.borrow().clone());
    #[cfg(not(test))]
    let regions = SUGGESTED_REPLY_HIT_REGIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    regions.into_iter().find_map(|item| {
        (column >= item.rect.x
            && column < item.rect.x.saturating_add(item.rect.width)
            && row >= item.rect.y
            && row < item.rect.y.saturating_add(item.rect.height))
        .then_some(item.target)
    })
}

fn inline_view_display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(crate) fn inline_ui_height(app: &dyn TuiState) -> u16 {
    match app.inline_ui_state() {
        Some(crate::tui::InlineUiStateRef::Interactive(picker)) => {
            // Model choices replace the command suggestions, without reflowing chat.
            if picker.kind == crate::tui::PickerKind::Model {
                return 0;
            }
            let visible_rows = picker.filtered.len() as u16;
            let rows_needed = visible_rows + 1 + 2; // header + rounded border
            rows_needed.min(20)
        }
        Some(crate::tui::InlineUiStateRef::View(view)) => {
            let visible_rows = view.lines.len().max(1) as u16;
            let rows_needed = visible_rows + 1 + 2; // header + rounded border
            rows_needed.min(10)
        }
        None => {
            if let Some(view) = app.suggested_replies_view() {
                let default_rows = u16::from(view.response.default_action.is_some());
                let secondary_rows = view.response.secondary_suggestions.len() as u16;
                default_rows + secondary_rows
            } else {
                0
            }
        }
    }
}

pub(super) fn draw_inline_ui(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    match app.inline_ui_state() {
        Some(crate::tui::InlineUiStateRef::Interactive(picker))
            if picker.kind != crate::tui::PickerKind::Model =>
        {
            clear_suggested_reply_hit_regions();
            super::inline_interactive_ui::draw_inline_interactive(frame, app, area)
        }
        Some(crate::tui::InlineUiStateRef::View(view)) => {
            clear_suggested_reply_hit_regions();
            draw_inline_view(frame, app, view, area)
        }
        None => {
            if let Some(view) = app.suggested_replies_view() {
                draw_suggested_replies_panel(frame, &view, area);
            } else {
                clear_suggested_reply_hit_regions();
            }
        }
        _ => {
            clear_suggested_reply_hit_regions();
        }
    }
}

fn pill_caps() -> (&'static str, &'static str) {
    if cfg!(test) {
        return ("\u{25D6}", "\u{25D7}");
    }
    if let Ok(tp) = std::env::var("TERM_PROGRAM") {
        let tp = tp.to_ascii_lowercase();
        if tp == "ghostty" || tp == "apple_terminal" || tp == "vscode" {
            return ("\u{2590}", "\u{258C}");
        }
    }
    if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok() || std::env::var("GHOSTTY_BIN_DIR").is_ok() {
        return ("\u{2590}", "\u{258C}");
    }
    ("\u{25D6}", "\u{25D7}")
}

pub(crate) fn draw_suggested_replies_panel(
    frame: &mut Frame,
    view: &SuggestedRepliesView,
    area: Rect,
) {
    if area.width < 12 || area.height == 0 {
        clear_suggested_reply_hit_regions();
        return;
    }

    let (left_cap, right_cap) = pill_caps();
    let mut hit_regions = Vec::new();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_row = area.y;

    if let Some(default_action) = view.response.default_action.as_ref() {
        let is_focused = matches!(view.focus, Some(SuggestedReplyFocus::DefaultAction));
        let bg = if is_focused {
            rgb(44, 66, 98)
        } else {
            rgb(30, 38, 54)
        };
        let fg = if is_focused {
            rgb(245, 250, 255)
        } else {
            rgb(205, 218, 238)
        };
        let key_fg = rgb(255, 213, 128);
        let conf_pct = view
            .response
            .confidence_for_text(default_action)
            .or_else(|| view.response.top_confidence())
            .map(|c| (c * 100.0).round() as u32)
            .unwrap_or(75);
        let explorer_tag = view
            .explorer_remaining_ms
            .map(|ms| format!(" · auto {}ms [Esc]", ms))
            .unwrap_or_default();

        let prefix = if is_focused {
            " ★ [Enter ↵] ▸ ".to_string()
        } else {
            " ★ [Tab / ↓] ▸ ".to_string()
        };
        let suffix = if view.show_confidence {
            format!(" ({}%{}) ", conf_pct, explorer_tag)
        } else if !explorer_tag.is_empty() {
            format!(" ({}) ", explorer_tag.trim_start_matches(" · "))
        } else {
            " ".to_string()
        };
        let budget = (area.width as usize)
            .saturating_sub(
                UnicodeWidthStr::width(prefix.as_str())
                    + UnicodeWidthStr::width(suffix.as_str())
                    + 2,
            )
            .max(8);
        let label = truncate_to_display_width(default_action, budget);

        let mut body_style = Style::default().fg(fg).bg(bg);
        if is_focused {
            body_style = body_style.add_modifier(Modifier::BOLD);
        }
        let spans = vec![
            Span::styled(left_cap, Style::default().fg(bg)),
            Span::styled(prefix.clone(), Style::default().fg(key_fg).bg(bg).bold()),
            Span::styled(label.clone(), body_style),
            Span::styled(suffix.clone(), Style::default().fg(dim_color()).bg(bg)),
            Span::styled(right_cap, Style::default().fg(bg)),
        ];
        let total_w = (UnicodeWidthStr::width(prefix.as_str())
            + UnicodeWidthStr::width(label.as_str())
            + UnicodeWidthStr::width(suffix.as_str())
            + 2)
            .min(area.width as usize) as u16;
        hit_regions.push(SuggestedReplyHitRegion {
            rect: Rect {
                x: area.x,
                y: current_row,
                width: total_w,
                height: 1,
            },
            target: SuggestedReplyFocus::DefaultAction,
        });
        lines.push(Line::from(spans));
        current_row = current_row.saturating_add(1);
    }

    if !view.response.secondary_suggestions.is_empty() {
        let max_y = area.y.saturating_add(area.height);
        for (idx, text) in view.response.secondary_suggestions.iter().enumerate() {
            if current_row >= max_y {
                break;
            }
            let conf_suffix = if view.show_confidence {
                view.response
                    .confidence_for_text(text)
                    .map(|c| format!(" ({}%)", (c * 100.0).round() as u32))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let prefix_num = format!("{}. ", idx + 1);
            let max_pill_cols = (area.width as usize)
                .saturating_sub(
                    UnicodeWidthStr::width(prefix_num.as_str())
                        + UnicodeWidthStr::width(conf_suffix.as_str())
                        + 4,
                )
                .max(8);
            let short = truncate_to_display_width(text, max_pill_cols);
            let pill_label = format!("{prefix_num}{short}{conf_suffix}");
            let pill_width = (UnicodeWidthStr::width(pill_label.as_str()) + 4)
                .min(area.width as usize) as u16;
            let is_focused =
                matches!(view.focus, Some(SuggestedReplyFocus::Secondary(i)) if i == idx);
            let (fill, text_fg) = if is_focused {
                (rgb(96, 152, 224), rgb(18, 22, 30))
            } else {
                (rgb(48, 52, 62), rgb(182, 188, 198))
            };
            let cap_style = Style::default().fg(fill);
            let mut pill_style = Style::default().fg(text_fg).bg(fill);
            if is_focused {
                pill_style = pill_style.add_modifier(Modifier::BOLD);
            }
            let row_spans = vec![
                Span::styled(left_cap, cap_style),
                Span::styled(format!(" {pill_label} "), pill_style),
                Span::styled(right_cap, cap_style),
            ];

            hit_regions.push(SuggestedReplyHitRegion {
                rect: Rect {
                    x: area.x,
                    y: current_row,
                    width: pill_width,
                    height: 1,
                },
                target: SuggestedReplyFocus::Secondary(idx),
            });
            lines.push(Line::from(row_spans));
            current_row = current_row.saturating_add(1);
        }
    }

    set_suggested_reply_hit_regions(hit_regions);
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_inline_view(
    frame: &mut Frame,
    app: &dyn TuiState,
    view: &crate::tui::InlineViewState,
    area: Rect,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height <= 2 || width <= 2 {
        return;
    }

    let mut content_width = inline_view_display_width(view.title.as_str());
    if let Some(status) = view.status.as_ref() {
        content_width = content_width.max(inline_view_display_width(status.as_str()) + 2);
    }
    for line in &view.lines {
        content_width = content_width.max(inline_view_display_width(line.as_str()));
    }
    let content_width = content_width.min(width.saturating_sub(2)).max(1);
    let outer_width = content_width.saturating_add(2).min(width);
    let horizontal_offset = if app.centered_mode() {
        area.width.saturating_sub(outer_width as u16) / 2
    } else {
        0
    };
    let render_area = Rect {
        x: area.x + horizontal_offset,
        y: area.y,
        width: outer_width as u16,
        height: area.height,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(85, 85, 110)))
        .style(Style::default().bg(rgb(18, 18, 26)));
    frame.render_widget(block.clone(), render_area);

    let inner = block.inner(render_area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut header_spans = vec![Span::styled(
        view.title.clone(),
        Style::default().fg(Color::White).bold(),
    )];
    if let Some(status) = view.status.as_ref() {
        header_spans.push(Span::styled(
            format!("  {}", status),
            Style::default().fg(dim_color()).italic(),
        ));
    }
    lines.push(Line::from(header_spans));

    for line in &view.lines {
        lines.push(Line::from(Span::styled(
            line.clone(),
            Style::default().fg(rgb(200, 200, 220)),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
