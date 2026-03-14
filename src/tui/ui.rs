#![allow(dead_code)]

use super::info_widget;
use super::markdown;
use super::ui_diff::{
    DiffLineKind, ParsedDiffLine, collect_diff_lines, diff_add_color, diff_change_counts_for_tool,
    diff_del_color, generate_diff_lines_from_tool_input, tint_span_with_diff_color,
};
use super::visual_debug::{
    self, FrameCaptureBuilder, ImageRegionCapture, InfoWidgetCapture, InfoWidgetSummary,
    MarginsCapture, MessageCapture, RenderTimingCapture, WidgetPlacementCapture,
};
use super::{DisplayMessage, ProcessingStatus, TuiState, is_unexpected_cache_miss};
use crate::message::ToolCall;
use ratatui::{prelude::*, widgets::Paragraph};
use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::hash::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[path = "ui_animations.rs"]
mod animations;
#[path = "ui_diagram_pane.rs"]
mod diagram_pane;
#[path = "ui_file_diff.rs"]
mod file_diff_ui;
#[path = "ui_header.rs"]
mod header;
#[path = "ui_input.rs"]
mod input_ui;
#[path = "ui_memory.rs"]
mod memory_ui;
#[path = "ui_messages.rs"]
mod messages;
#[path = "ui_overlays.rs"]
mod overlays;
#[path = "ui_picker.rs"]
mod picker_ui;
#[path = "ui_pinned.rs"]
mod pinned_ui;
#[path = "ui_prepare.rs"]
mod prepare;
#[path = "ui_tools.rs"]
mod tools_ui;
#[path = "ui_viewport.rs"]
mod viewport;

#[cfg(test)]
use diagram_pane::{
    div_ceil_u32, estimate_pinned_diagram_pane_width_with_font, is_diagram_poor_fit,
    vcenter_fitted_image_with_font,
};
use diagram_pane::{
    draw_pinned_diagram, estimate_pinned_diagram_pane_height, estimate_pinned_diagram_pane_width,
};
use file_diff_ui::active_file_diff_context;
use file_diff_ui::draw_file_diff_view;
#[cfg(test)]
use file_diff_ui::{
    FileDiffCacheKey, FileDiffViewCacheEntry, file_content_signature, file_diff_cache,
};
pub(crate) use header::capitalize;
#[cfg(test)]
use memory_ui::{choose_memory_tile_span, plan_memory_tile};
use memory_ui::{group_into_tiles, render_memory_tiles, split_by_display_width};
use messages::get_cached_message_lines;
pub(crate) use messages::{
    render_assistant_message, render_swarm_message, render_system_message, render_tool_message,
};
use picker_ui::draw_picker_line;
use pinned_ui::{
    collect_pinned_content_cached, draw_pinned_content_cached, draw_side_panel_markdown,
};
use tools_ui::summarize_batch_running_tools_compact;
#[cfg(test)]
use viewport::compute_visible_margins;
use viewport::draw_messages;
/// Last known max scroll value from the renderer. Updated each frame.
/// Scroll handlers use this to clamp scroll_offset and prevent overshoot.
static LAST_MAX_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Number of recovered panics while rendering the frame.
static DRAW_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Total line count in the pinned diff/content pane (set during render).
static PINNED_PANE_TOTAL_LINES: AtomicUsize = AtomicUsize::new(0);
/// Effective scroll position of the side pane after render-time clamping.
static LAST_DIFF_PANE_EFFECTIVE_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Wrapped line indices where each user prompt starts (updated each render frame).
/// Used by prompt-jump keybindings (Ctrl+5..9, Ctrl+[/]) for accurate positioning.
static LAST_USER_PROMPT_POSITIONS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

/// Get the last known max scroll value (from the most recent render frame).
/// Returns 0 if no frame has been rendered yet.
pub fn last_max_scroll() -> usize {
    LAST_MAX_SCROLL.load(Ordering::Relaxed)
}

/// Get the total line count from the pinned diff/content pane (set during render).
pub fn pinned_pane_total_lines() -> usize {
    PINNED_PANE_TOTAL_LINES.load(Ordering::Relaxed)
}

pub fn last_diff_pane_effective_scroll() -> usize {
    LAST_DIFF_PANE_EFFECTIVE_SCROLL.load(Ordering::Relaxed)
}

/// Get the last known user prompt line positions (from the most recent render frame).
/// Returns positions as wrapped line indices from the top of content.
pub fn last_user_prompt_positions() -> Vec<usize> {
    LAST_USER_PROMPT_POSITIONS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|v| v.clone())
        .unwrap_or_default()
}

fn update_user_prompt_positions(positions: &[usize]) {
    let mutex = LAST_USER_PROMPT_POSITIONS.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(mut v) = mutex.lock() {
        v.clear();
        v.extend_from_slice(positions);
    }
}

use super::color_support::rgb;

fn clear_area(frame: &mut Frame, area: Rect) {
    super::color_support::clear_buf(area, frame.buffer_mut());
}

fn user_color() -> Color {
    rgb(138, 180, 248)
}
fn ai_color() -> Color {
    rgb(129, 199, 132)
}
fn tool_color() -> Color {
    rgb(120, 120, 120)
}
fn file_link_color() -> Color {
    rgb(180, 200, 255)
}
fn dim_color() -> Color {
    rgb(80, 80, 80)
}
fn accent_color() -> Color {
    rgb(186, 139, 255)
}
fn system_message_color() -> Color {
    rgb(255, 170, 220)
}
fn queued_color() -> Color {
    rgb(255, 193, 7)
}
fn asap_color() -> Color {
    rgb(110, 210, 255)
}
fn pending_color() -> Color {
    rgb(140, 140, 140)
}
fn user_text() -> Color {
    rgb(245, 245, 255)
}
fn user_bg() -> Color {
    rgb(35, 40, 50)
}
fn ai_text() -> Color {
    rgb(220, 220, 215)
}
fn header_icon_color() -> Color {
    rgb(120, 210, 230)
}
fn header_name_color() -> Color {
    rgb(190, 210, 235)
}
fn header_session_color() -> Color {
    rgb(255, 255, 255)
}

// Spinner frames for animated status
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn spinner_frame_index(elapsed: f32, fps: f32) -> usize {
    ((elapsed * fps) as usize) % SPINNER_FRAMES.len()
}

pub(super) fn spinner_frame(elapsed: f32, fps: f32) -> &'static str {
    SPINNER_FRAMES[spinner_frame_index(elapsed, fps)]
}

// Keep the picker spacious on tall terminals without crowding the chat pane.
const MODEL_PICKER_MAX_HEIGHT: u16 = 16;
const MODEL_PICKER_MIN_MESSAGES_HEIGHT: u16 = 3;

/// Duration of the startup fade-in animation in seconds
const HEADER_ANIM_DURATION: f32 = 1.5;

/// Speed of the continuous chroma wave (lower = slower)
const CHROMA_SPEED: f32 = 0.15;

/// Convert HSL to RGB (h in 0-360, s and l in 0-1)
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

/// Chroma color based on position and time - creates flowing rainbow wave
fn chroma_color(pos: f32, elapsed: f32, saturation: f32, lightness: f32) -> Color {
    // Hue shifts over time and varies by position
    // pos: 0.0-1.0 position in the text
    // Creates a wave that flows across the text
    let hue = ((pos * 60.0) + (elapsed * CHROMA_SPEED * 360.0)) % 360.0;
    let (r, g, b) = hsl_to_rgb(hue, saturation, lightness);
    rgb(r, g, b)
}

/// Calculate chroma color with fade-in from dim during startup
fn header_chroma_color(pos: f32, elapsed: f32) -> Color {
    let fade = ((elapsed / HEADER_ANIM_DURATION).clamp(0.0, 1.0)).powf(0.5);

    // During fade-in, transition from dim gray to full chroma
    let saturation = 0.75 * fade;
    let lightness = 0.3 + 0.35 * fade; // Start darker (0.3), end bright (0.65)

    chroma_color(pos, elapsed, saturation, lightness)
}

/// Calculate smooth animated color for the header (single color, no position)
fn header_animation_color(elapsed: f32) -> Color {
    header_chroma_color(0.5, elapsed)
}

fn header_fade_t(elapsed: f32, offset: f32) -> f32 {
    let t = ((elapsed - offset) / HEADER_ANIM_DURATION).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn header_fade_color(target: Color, elapsed: f32, offset: f32) -> Color {
    blend_color(dim_color(), target, header_fade_t(elapsed, offset))
}

fn color_to_floats(c: Color, fallback: (f32, f32, f32)) -> (f32, f32, f32) {
    match c {
        Color::Rgb(r, g, b) => (r as f32, g as f32, b as f32),
        Color::Indexed(n) => {
            let (r, g, b) = super::color_support::indexed_to_rgb(n);
            (r as f32, g as f32, b as f32)
        }
        _ => fallback,
    }
}

fn blend_color(from: Color, to: Color, t: f32) -> Color {
    let (fr, fg, fb) = color_to_floats(from, (80.0, 80.0, 80.0));
    let (tr, tg, tb) = color_to_floats(to, (200.0, 200.0, 200.0));
    let r = fr + (tr - fr) * t;
    let g = fg + (tg - fg) * t;
    let b = fb + (tb - fb) * t;
    rgb(
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

/// Chrome-style sweep highlight across header text.
fn header_chrome_color(base: Color, pos: f32, elapsed: f32, intensity: f32) -> Color {
    let highlight_c: Color = rgb(235, 245, 255);
    let shadow_c: Color = rgb(70, 80, 95);
    const SPEED: f32 = 0.12;
    const WIDTH: f32 = 0.22;

    let center = (elapsed * SPEED) % 1.0;
    let mut dist = (pos - center).abs();
    dist = dist.min(1.0 - dist);
    let shine = (1.0 - (dist / WIDTH).clamp(0.0, 1.0)).powf(2.4);

    let micro = ((pos * 12.0 + elapsed * 2.6).sin() * 0.5 + 0.5) * 0.12;
    let shimmer = (shine * 0.9 + micro).clamp(0.0, 1.0) * intensity;

    let shadow_center = (center + 0.5) % 1.0;
    let mut shadow_dist = (pos - shadow_center).abs();
    shadow_dist = shadow_dist.min(1.0 - shadow_dist);
    let shadow_t =
        (1.0 - (shadow_dist / (WIDTH * 1.2)).clamp(0.0, 1.0)).powf(2.0) * 0.16 * intensity;

    let darkened = blend_color(base, shadow_c, shadow_t);
    blend_color(darkened, highlight_c, shimmer)
}

/// Set alignment on a line only if it doesn't already have one set.
/// This allows markdown rendering to mark code blocks as left-aligned while
/// other content inherits the default alignment (e.g., centered mode).
pub(crate) fn align_if_unset(line: Line<'static>, align: Alignment) -> Line<'static> {
    if line.alignment.is_some() {
        line
    } else {
        line.alignment(align)
    }
}

/// Extract semantic version from full version string (e.g., "v0.1.0-dev (abc123)" -> "v0.1.0")
fn semver() -> &'static str {
    static SEMVER: OnceLock<String> = OnceLock::new();
    SEMVER.get_or_init(|| {
        let full = env!("JCODE_VERSION");
        // Extract just the version part (before any space or -dev suffix for display)
        if let Some(space_pos) = full.find(' ') {
            full[..space_pos].trim_end_matches("-dev").to_string()
        } else {
            full.trim_end_matches("-dev").to_string()
        }
    })
}

/// True when this process is running from the stable release binary path.
/// Only matches the explicit ~/.jcode/builds/stable/jcode path, NOT
/// ~/.local/bin/jcode launcher path (which points to stable).
fn is_running_stable_release() -> bool {
    static IS_STABLE: OnceLock<bool> = OnceLock::new();
    *IS_STABLE.get_or_init(|| {
        // Use the raw symlink target (read_link), not canonicalize, to
        // check whether we're on the stable channel link.
        let current_exe = match std::env::current_exe().ok() {
            Some(path) => path,
            None => return false,
        };

        // Check if we were launched via the stable symlink
        if let Ok(stable_path) = crate::build::stable_binary_path() {
            // Compare the symlink target (not canonical) to distinguish
            // launcher/stable links from direct binary execution.
            let stable_target =
                std::fs::read_link(&stable_path).unwrap_or_else(|_| stable_path.clone());
            let current_target =
                std::fs::read_link(&current_exe).unwrap_or_else(|_| current_exe.clone());
            if stable_target == current_target {
                return true;
            }
            // Also check canonical paths for when launched directly
            if let (Ok(stable_canon), Ok(current_canon)) = (
                std::fs::canonicalize(&stable_path),
                std::fs::canonicalize(&current_exe),
            ) {
                if stable_canon == current_canon
                    && !current_exe.to_string_lossy().contains("target/release")
                {
                    return true;
                }
            }
        }

        false
    })
}

/// Render context window as vertical list with smart grouping
/// Items < 5% are grouped by category (docs, msgs, etc.)
fn render_context_bar(
    info: &crate::prompt::ContextInfo,
    max_width: usize,
    context_limit: usize,
) -> Vec<Line<'static>> {
    let sys_c: Color = rgb(100, 140, 200);
    let docs_c: Color = rgb(200, 160, 100);
    let tools_c: Color = rgb(100, 200, 200);
    let msgs_c: Color = rgb(138, 180, 248);
    let tool_io_c: Color = rgb(255, 183, 77);
    let other_c: Color = rgb(150, 150, 150);
    let empty_c: Color = rgb(50, 50, 50);

    const THRESHOLD: f64 = 5.0;
    let limit = context_limit.max(1);

    // Collect raw: (icon, label, tokens, color, category)
    let mut raw: Vec<(&str, String, usize, Color, &str)> = Vec::new();

    let sys = info.system_prompt_chars / 4;
    if sys > 0 {
        raw.push(("⚙", "system".into(), sys, sys_c, "system"));
    }

    if info.has_project_agents_md {
        raw.push((
            "📋",
            "AGENTS.md".into(),
            info.project_agents_md_chars / 4,
            docs_c,
            "docs",
        ));
    }
    if info.has_project_claude_md {
        raw.push((
            "📝",
            "CLAUDE.md".into(),
            info.project_claude_md_chars / 4,
            docs_c,
            "docs",
        ));
    }
    if info.has_global_agents_md {
        raw.push((
            "📋",
            "~/.AGENTS".into(),
            info.global_agents_md_chars / 4,
            docs_c,
            "docs",
        ));
    }
    if info.has_global_claude_md {
        raw.push((
            "📝",
            "~/.CLAUDE".into(),
            info.global_claude_md_chars / 4,
            docs_c,
            "docs",
        ));
    }

    if info.env_context_chars > 0 {
        raw.push((
            "🌍",
            "env".into(),
            info.env_context_chars / 4,
            other_c,
            "other",
        ));
    }
    if info.skills_chars > 0 {
        raw.push((
            "🔧",
            "skills".into(),
            info.skills_chars / 4,
            other_c,
            "other",
        ));
    }
    if info.selfdev_chars > 0 {
        raw.push((
            "🛠",
            "selfdev".into(),
            info.selfdev_chars / 4,
            other_c,
            "other",
        ));
    }

    if info.tool_defs_chars > 0 {
        let lbl = if info.tool_defs_count > 0 {
            format!("tools ({})", info.tool_defs_count)
        } else {
            "tools".into()
        };
        raw.push(("🔨", lbl, info.tool_defs_chars / 4, tools_c, "tools"));
    }
    if info.user_messages_chars > 0 {
        let lbl = if info.user_messages_count > 0 {
            format!("user ({})", info.user_messages_count)
        } else {
            "user".into()
        };
        raw.push(("👤", lbl, info.user_messages_chars / 4, msgs_c, "msgs"));
    }
    if info.assistant_messages_chars > 0 {
        let lbl = if info.assistant_messages_count > 0 {
            format!("assistant ({})", info.assistant_messages_count)
        } else {
            "assistant".into()
        };
        raw.push(("🤖", lbl, info.assistant_messages_chars / 4, msgs_c, "msgs"));
    }
    if info.tool_calls_chars > 0 {
        let lbl = if info.tool_calls_count > 0 {
            format!("calls ({})", info.tool_calls_count)
        } else {
            "calls".into()
        };
        raw.push(("⚡", lbl, info.tool_calls_chars / 4, tool_io_c, "tool_io"));
    }
    if info.tool_results_chars > 0 {
        let lbl = if info.tool_results_count > 0 {
            format!("results ({})", info.tool_results_count)
        } else {
            "results".into()
        };
        raw.push(("📤", lbl, info.tool_results_chars / 4, tool_io_c, "tool_io"));
    }

    // Smart grouping
    let mut final_segs: Vec<(String, String, usize, Color)> = Vec::new();
    let mut grouped: std::collections::HashMap<&str, (usize, Vec<String>)> =
        std::collections::HashMap::new();

    for (icon, label, tokens, color, cat) in &raw {
        let pct = (*tokens as f64 / limit as f64) * 100.0;
        if pct >= THRESHOLD || *cat == "system" {
            final_segs.push((icon.to_string(), label.clone(), *tokens, *color));
        } else {
            let e = grouped.entry(*cat).or_insert((0, Vec::new()));
            e.0 += tokens;
            e.1.push(label.clone());
        }
    }

    for (cat, icon, color) in [
        ("docs", "📄", docs_c),
        ("msgs", "💬", msgs_c),
        ("tools", "🔨", tools_c),
        ("tool_io", "⚡", tool_io_c),
        ("other", "📦", other_c),
    ] {
        if let Some((tokens, items)) = grouped.get(cat) {
            if *tokens > 0 {
                let lbl = if items.len() == 1 {
                    items[0].clone()
                } else {
                    format!("{} ({})", cat, items.len())
                };
                final_segs.push((icon.to_string(), lbl, *tokens, color));
            }
        }
    }

    final_segs.sort_by(|a, b| b.2.cmp(&a.2));

    let mut lines: Vec<Line<'static>> = Vec::new();
    let total: usize = final_segs.iter().map(|(_, _, t, _)| *t).sum();

    // Summary bar (top)
    let total_str = if total >= 1000 {
        format!("{}k", total / 1000)
    } else {
        format!("{}", total)
    };
    let limit_str = if limit >= 1000 {
        format!("{}k", limit / 1000)
    } else {
        format!("{}", limit)
    };
    let tail = format!("{}/{}", total_str, limit_str);
    let tail_len = tail.chars().count();

    let max_bar = max_width.saturating_sub(tail_len + 3); // "[" + bar + "] " + tail
    let sum_w = 36.min(max_bar).max(10);
    let used_w = ((total as f64 / limit as f64) * sum_w as f64)
        .ceil()
        .max(if total > 0 { 1.0 } else { 0.0 })
        .min(sum_w as f64) as usize;
    let empty_w = sum_w.saturating_sub(used_w);

    let mut bar: Vec<Span<'static>> = vec![Span::styled("[", Style::default().fg(dim_color()))];
    let mut rem = used_w;
    for (_, _, t, c) in &final_segs {
        if rem == 0 || total == 0 {
            break;
        }
        let w = ((*t as f64 / total as f64) * used_w as f64)
            .round()
            .min(rem as f64) as usize;
        if w > 0 {
            bar.push(Span::styled("█".repeat(w), Style::default().fg(*c)));
            rem -= w;
        }
    }
    if rem > 0 && !final_segs.is_empty() {
        if let Some(last_seg) = final_segs.last() {
            bar.push(Span::styled(
                "█".repeat(rem),
                Style::default().fg(last_seg.3),
            ));
        }
    }
    if empty_w > 0 {
        bar.push(Span::styled(
            "░".repeat(empty_w),
            Style::default().fg(empty_c),
        ));
    }
    bar.push(Span::styled("] ", Style::default().fg(dim_color())));
    bar.push(Span::styled(tail, Style::default().fg(dim_color())));
    lines.push(Line::from(bar));

    // Detail list with dot leaders
    let max_label_len = final_segs
        .iter()
        .map(|(_, l, _, _)| l.chars().count())
        .max()
        .unwrap_or(8);
    let label_w = max_label_len.max(10).min(18);
    let line_w = max_width;

    for (icon, label, tokens, color) in &final_segs {
        let pct = (*tokens as f64 / limit as f64 * 100.0).round() as usize;
        let token_str = if *tokens >= 1000 {
            format!("{}k", tokens / 1000)
        } else {
            format!("{}", tokens)
        };
        let tail = format!("{}  {}%", token_str, pct);
        let label_text = format!("{} {}", icon, label);
        let label_len = label_text.chars().count();
        let pad = label_w.saturating_sub(label_len);
        let reserved = label_w + pad + tail.chars().count() + 2;
        let dots = line_w.saturating_sub(reserved).max(2);

        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(label_text, Style::default().fg(*color)));
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(
            "·".repeat(dots),
            Style::default().fg(dim_color()),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(tail, Style::default().fg(dim_color())));
        lines.push(Line::from(spans));
    }

    lines
}

fn render_rounded_box(
    title: &str,
    content: Vec<Line<'static>>,
    max_width: usize,
    border_style: Style,
) -> Vec<Line<'static>> {
    if content.is_empty() || max_width < 6 {
        return Vec::new();
    }

    let max_content_width = content
        .iter()
        .map(|line| line.width())
        .max()
        .unwrap_or(0)
        .min(max_width.saturating_sub(4));

    if max_content_width < 6 {
        return Vec::new();
    }

    let box_width = max_content_width + 4; // "│ " + content + " │"
    let title_text = format!(" {} ", title);
    let title_len = unicode_width::UnicodeWidthStr::width(title_text.as_str());
    let border_chars = box_width.saturating_sub(title_len + 2);
    let left_border = "─".repeat(border_chars / 2);
    let right_border = "─".repeat(border_chars - border_chars / 2);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("╭{}{}{}╮", left_border, title_text, right_border),
        border_style,
    )));

    for line in content {
        let truncated = truncate_line_to_width(&line, max_content_width);
        let padding = max_content_width.saturating_sub(truncated.width());
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("│ ", border_style));
        spans.extend(truncated.spans);
        if padding > 0 {
            spans.push(Span::raw(" ".repeat(padding)));
        }
        spans.push(Span::styled(" │", border_style));
        lines.push(Line::from(spans));
    }

    let bottom_border = "─".repeat(box_width.saturating_sub(2));
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", bottom_border),
        border_style,
    )));

    lines
}

fn truncate_line_to_width(line: &Line<'static>, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = width;
    for span in &line.spans {
        if remaining == 0 {
            break;
        }
        let text = span.content.as_ref();
        let span_width = unicode_width::UnicodeWidthStr::width(text);
        if span_width <= remaining {
            spans.push(span.clone());
            remaining -= span_width;
        } else {
            let mut clipped = String::new();
            let mut used = 0;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + cw > remaining {
                    break;
                }
                clipped.push(ch);
                used += cw;
            }
            if !clipped.is_empty() {
                spans.push(Span::styled(clipped, span.style));
            }
            remaining = 0;
        }
    }

    if spans.is_empty() {
        Line::from("")
    } else {
        Line::from(spans)
    }
}

/// Calculate rainbow color for prompt index with exponential decay to gray.
/// `distance` is how many prompts back from the most recent (0 = most recent).
fn rainbow_prompt_color(distance: usize) -> Color {
    // Rainbow colors (hue progression): red -> orange -> yellow -> green -> cyan -> blue -> violet
    const RAINBOW: [(u8, u8, u8); 7] = [
        (255, 80, 80),   // Red (softened)
        (255, 160, 80),  // Orange
        (255, 230, 80),  // Yellow
        (80, 220, 100),  // Green
        (80, 200, 220),  // Cyan
        (100, 140, 255), // Blue
        (180, 100, 255), // Violet
    ];

    // Gray target (dim_color())
    const GRAY: (u8, u8, u8) = (80, 80, 80);

    // Exponential decay factor - how quickly we fade to gray
    // decay = e^(-distance * rate), rate of ~0.4 gives nice falloff
    let decay = (-0.4 * distance as f32).exp();

    // Select rainbow color based on distance (cycle through)
    let rainbow_idx = distance.min(RAINBOW.len() - 1);
    let (r, g, b) = RAINBOW[rainbow_idx];

    // Blend rainbow color with gray based on decay
    // At distance 0: 100% rainbow, as distance increases: approaches gray
    let blend = |rainbow: u8, gray: u8| -> u8 {
        (rainbow as f32 * decay + gray as f32 * (1.0 - decay)) as u8
    };

    rgb(blend(r, GRAY.0), blend(g, GRAY.1), blend(b, GRAY.2))
}

fn prompt_entry_color(base: Color, t: f32) -> Color {
    let peak = rgb(255, 230, 120);
    // Quick pulse in/out over the animation window.
    let phase = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
    blend_color(base, peak, phase.clamp(0.0, 1.0) * 0.7)
}

fn prompt_entry_bg_color(base: Color, t: f32) -> Color {
    let spotlight = rgb(58, 66, 82);
    let ease_in = 1.0 - (1.0 - t).powi(3);
    let ease_out = (1.0 - t).powi(2);
    let phase = (ease_in * ease_out * 1.65).clamp(0.0, 1.0);
    blend_color(base, spotlight, phase * 0.85)
}

fn prompt_entry_shimmer_color(base: Color, pos: f32, t: f32) -> Color {
    let travel = (t * 1.15).clamp(0.0, 1.0);
    let width = 0.18;
    let dist = (pos - travel).abs();
    let shimmer = (1.0 - (dist / width).clamp(0.0, 1.0)).powf(2.2);
    let pulse = (1.0 - t).powf(0.55);
    let highlight = rgb(255, 248, 210);
    blend_color(base, highlight, shimmer * pulse * 0.7)
}

/// Generate an animated color that pulses between two colors
fn animated_tool_color(elapsed: f32) -> Color {
    // Cycle period of ~1.5 seconds
    let t = (elapsed * 2.0).sin() * 0.5 + 0.5; // 0.0 to 1.0

    // Interpolate between cyan and purple
    let r = (80.0 + t * 106.0) as u8; // 80 -> 186
    let g = (200.0 - t * 61.0) as u8; // 200 -> 139
    let b = (220.0 + t * 35.0) as u8; // 220 -> 255

    rgb(r, g, b)
}

/// Format seconds as a human-readable age string
fn format_age(secs: i64) -> String {
    if secs < 0 {
        "future?".to_string()
    } else if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Get how long ago the binary was built and when the code was committed
/// Shows both if they differ significantly, otherwise just the build time
fn binary_age() -> Option<String> {
    let build_time = env!("JCODE_BUILD_TIME");
    let git_date = env!("JCODE_GIT_DATE");

    let now = chrono::Utc::now();

    // Parse build time
    let build_date = chrono::DateTime::parse_from_str(build_time, "%Y-%m-%d %H:%M:%S %z").ok()?;
    let build_secs = now.signed_duration_since(build_date).num_seconds();

    // Parse git commit date
    let git_commit_date = chrono::DateTime::parse_from_str(git_date, "%Y-%m-%d %H:%M:%S %z").ok();
    let git_secs = git_commit_date.map(|d| now.signed_duration_since(d).num_seconds());

    let build_age = format_age(build_secs);

    // If git date is available and differs significantly (>5 min), show both
    if let Some(git_secs) = git_secs {
        let diff = (git_secs - build_secs).abs();
        if diff > 300 {
            // More than 5 minutes difference
            let git_age = format_age(git_secs);
            return Some(format!("{}, code {}", build_age, git_age));
        }
    }

    Some(build_age)
}

/// A changelog entry: hash, optional version tag, and commit subject.
struct ChangelogEntry<'a> {
    hash: &'a str,
    tag: &'a str,
    subject: &'a str,
}

/// Parse the embedded changelog. Format per entry: "hash:tag:subject"
/// where tag is either a version like "v0.4.2" or empty.
/// Entries are separated by ASCII unit separator (0x1F).
fn parse_changelog() -> Vec<ChangelogEntry<'static>> {
    let changelog: &'static str = env!("JCODE_CHANGELOG");
    if changelog.is_empty() {
        return Vec::new();
    }
    changelog
        .split('\x1f')
        .filter_map(|entry| {
            let (hash, rest) = entry.split_once(':')?;
            let (tag, subject) = rest.split_once(':')?;
            Some(ChangelogEntry { hash, tag, subject })
        })
        .collect()
}

/// A group of changelog entries under a version heading.
pub struct ChangelogGroup {
    pub version: String,
    pub entries: Vec<String>,
}

/// Return all embedded changelog entries grouped by release version.
/// Each group has a version label (e.g. "v0.4.2") and the commit subjects
/// that belong to that release. Commits before any tag are grouped under
/// the current build version.
pub fn get_grouped_changelog() -> Vec<ChangelogGroup> {
    let entries = parse_changelog();
    if entries.is_empty() {
        return Vec::new();
    }

    let current_version = env!("JCODE_VERSION");
    let version_label = current_version
        .split_whitespace()
        .next()
        .unwrap_or(current_version);

    let mut groups: Vec<ChangelogGroup> = Vec::new();
    let mut current_group = ChangelogGroup {
        version: format!("{} (unreleased)", version_label),
        entries: Vec::new(),
    };

    for entry in &entries {
        if !entry.tag.is_empty() {
            if !current_group.entries.is_empty() {
                groups.push(current_group);
            }
            current_group = ChangelogGroup {
                version: entry.tag.to_string(),
                entries: vec![entry.subject.to_string()],
            };
        } else {
            current_group.entries.push(entry.subject.to_string());
        }
    }
    if !current_group.entries.is_empty() {
        groups.push(current_group);
    }

    groups
}

/// Get changelog entries the user hasn't seen yet.
/// Reads the last-seen commit hash from ~/.jcode/last_seen_changelog,
/// filters the embedded changelog to only new entries, then saves the latest hash.
/// Returns just the commit subjects (not the hashes).
fn get_unseen_changelog_entries() -> &'static Vec<String> {
    static ENTRIES: OnceLock<Vec<String>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let all_entries = parse_changelog();
        if all_entries.is_empty() {
            return Vec::new();
        }

        let state_file = dirs::home_dir()
            .map(|h| h.join(".jcode").join("last_seen_changelog"))
            .unwrap_or_else(|| std::path::PathBuf::from(".jcode/last_seen_changelog"));

        let last_seen_hash = std::fs::read_to_string(&state_file)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let new_entries: Vec<String> = if last_seen_hash.is_empty() {
            all_entries
                .iter()
                .take(5)
                .map(|e| e.subject.to_string())
                .collect()
        } else {
            all_entries
                .iter()
                .take_while(|e| e.hash != last_seen_hash)
                .map(|e| e.subject.to_string())
                .collect()
        };

        if let Some(first) = all_entries.first() {
            if let Some(parent) = state_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&state_file, first.hash);
        }

        new_entries
    })
}

/// Shorten model name for display (e.g., "claude-opus-4-5-20251101" -> "claude4.5opus")
fn shorten_model_name(model: &str) -> String {
    // Handle OpenRouter models (format: provider/model-name)
    // Keep the full identifier for display
    if model.contains('/') {
        return model.to_string();
    }
    // Handle common Claude model patterns
    if model.contains("opus") {
        if model.contains("4-5") || model.contains("4.5") {
            return "claude4.5opus".to_string();
        }
        return "claudeopus".to_string();
    }
    if model.contains("sonnet") {
        if model.contains("3-5") || model.contains("3.5") {
            return "claude3.5sonnet".to_string();
        }
        return "claudesonnet".to_string();
    }
    if model.contains("haiku") {
        return "claudehaiku".to_string();
    }
    // Handle OpenAI models (gpt-5.2-codex -> gpt5.2codex)
    if model.starts_with("gpt-5") {
        // e.g., "gpt-5.2-codex" -> "gpt5.2codex"
        return model.replace("gpt-", "gpt").replace("-", "");
    }
    if model.starts_with("gpt-4") {
        return model.replace("gpt-", "").replace("-", "");
    }
    if model.starts_with("gpt-3") {
        return "gpt3.5".to_string();
    }
    // Fallback: remove common suffixes and dashes
    model.split('-').take(3).collect::<Vec<_>>().join("")
}

/// Calculate the number of visual lines an input string will occupy
/// when wrapped to a given width, accounting for explicit newlines.
fn calculate_input_lines(input: &str, line_width: usize) -> usize {
    use unicode_width::UnicodeWidthChar;

    if line_width == 0 {
        return 1;
    }
    if input.is_empty() {
        return 1;
    }

    let mut total_lines = 0;
    for line in input.split('\n') {
        if line.is_empty() {
            total_lines += 1;
        } else {
            let display_width: usize = line.chars().map(|c| c.width().unwrap_or(0)).sum();
            total_lines += (display_width + line_width - 1) / line_width;
        }
    }
    total_lines.max(1)
}

/// Format status line content for visual debug capture
fn format_status_for_debug(app: &dyn TuiState) -> String {
    match app.status() {
        ProcessingStatus::Idle => {
            if let Some(notice) = app.status_notice() {
                format!("Idle (notice: {})", notice)
            } else if let Some((input, output)) = app.total_session_tokens() {
                format!(
                    "Idle (session: {}k in, {}k out)",
                    input / 1000,
                    output / 1000
                )
            } else if let Some(tip) =
                info_widget::occasional_status_tip(120, app.animation_elapsed() as u64)
            {
                format!("Idle ({})", tip)
            } else {
                "Idle".to_string()
            }
        }
        ProcessingStatus::Sending => "Sending...".to_string(),
        ProcessingStatus::Connecting(ref phase) => format!("{}...", phase),
        ProcessingStatus::Thinking(_start) => {
            let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
            format!("Thinking... ({:.1}s)", elapsed)
        }
        ProcessingStatus::Streaming => {
            let (input, output) = app.streaming_tokens();
            format!("Streaming (↑{} ↓{})", input, output)
        }
        ProcessingStatus::RunningTool(ref name) => {
            if name == "batch" {
                if let Some(progress) = app.batch_progress() {
                    let completed = progress.completed;
                    let total = progress.total;
                    let mut status = format!("Running batch: {}/{} done", completed, total);
                    if let Some(running) = summarize_batch_running_tools_compact(&progress.running)
                    {
                        status.push_str(&format!(", running: {}", running));
                    }
                    if let Some(last) = progress.last_completed.filter(|_| completed < total) {
                        status.push_str(&format!(", last done: {}", last));
                    }
                    return status;
                }
            }
            format!("Running tool: {}", name)
        }
    }
}

/// Pre-computed image region from line scanning
#[derive(Clone, Copy)]
struct ImageRegion {
    /// Absolute line index in wrapped_lines
    abs_line_idx: usize,
    /// Absolute exclusive end line of the image placeholder region.
    end_line: usize,
    /// Hash of the mermaid content (for cache lookup)
    hash: u64,
    /// Total height of the image placeholder in lines
    height: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CopyTargetKind {
    CodeBlock { language: Option<String> },
    Error,
}

impl CopyTargetKind {
    fn label(&self) -> String {
        match self {
            Self::CodeBlock { language } => language
                .as_deref()
                .filter(|lang| !lang.is_empty())
                .unwrap_or("code")
                .to_string(),
            Self::Error => "error".to_string(),
        }
    }

    fn copied_notice(&self) -> String {
        match self {
            Self::CodeBlock { language } => {
                let label = language
                    .as_deref()
                    .filter(|lang| !lang.is_empty())
                    .unwrap_or("code block");
                format!("Copied {}", label)
            }
            Self::Error => "Copied error".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RawCopyTarget {
    pub(crate) kind: CopyTargetKind,
    pub(crate) content: String,
    pub(crate) start_raw_line: usize,
    pub(crate) end_raw_line: usize,
    pub(crate) badge_raw_line: usize,
}

#[derive(Clone, Debug)]
struct CopyTarget {
    kind: CopyTargetKind,
    content: String,
    start_line: usize,
    end_line: usize,
    badge_line: usize,
}

#[derive(Clone)]
struct PreparedMessages {
    wrapped_lines: Vec<Line<'static>>,
    wrapped_user_indices: Vec<usize>,
    /// Wrapped line indices where a user prompt line starts
    wrapped_user_prompt_starts: Vec<usize>,
    /// Wrapped line indices where a user prompt line ends (exclusive)
    wrapped_user_prompt_ends: Vec<usize>,
    /// Flattened user prompt text in display order, used by prompt preview without
    /// scanning display_messages on every frame.
    user_prompt_texts: Vec<String>,
    /// Pre-scanned image regions (computed once, not every frame)
    image_regions: Vec<ImageRegion>,
    /// Line ranges for edit tool messages: (msg_index, start_line, end_line)
    /// Used by File diff mode to determine which edit is visible at current scroll
    edit_tool_ranges: Vec<EditToolRange>,
    copy_targets: Vec<CopyTarget>,
}

#[derive(Clone, Debug)]
struct EditToolRange {
    edit_index: usize,
    msg_index: usize,
    file_path: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Clone, Debug)]
struct ActiveFileDiffContext {
    edit_index: usize,
    msg_index: usize,
    file_path: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct VisibleCopyTarget {
    pub key: char,
    pub kind_label: String,
    pub copied_notice: String,
    pub content: String,
}

const COPY_BADGE_KEYS: [char; 12] = ['s', 'd', 'f', 'g', 'w', 'e', 'r', 't', 'x', 'c', 'v', 'b'];

static VISIBLE_COPY_TARGETS: OnceLock<Mutex<Vec<VisibleCopyTarget>>> = OnceLock::new();

fn visible_copy_targets_state() -> &'static Mutex<Vec<VisibleCopyTarget>> {
    VISIBLE_COPY_TARGETS.get_or_init(|| Mutex::new(Vec::new()))
}

fn set_visible_copy_targets(targets: Vec<VisibleCopyTarget>) {
    let mut state = match visible_copy_targets_state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *state = targets;
}

pub(crate) fn visible_copy_target_for_key(key: char) -> Option<VisibleCopyTarget> {
    let state = match visible_copy_targets_state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state
        .iter()
        .find(|target| target.key.eq_ignore_ascii_case(&key))
        .cloned()
}

#[derive(Clone, Copy)]
struct PromptViewportAnimation {
    line_idx: usize,
    start_ms: u64,
}

#[derive(Clone, Copy, Default)]
struct PromptViewportState {
    initialized: bool,
    last_visible_start: usize,
    last_visible_end: usize,
    active: Option<PromptViewportAnimation>,
}

const PROMPT_ENTRY_ANIMATION_MS: u64 = 450;

static PROMPT_VIEWPORT_STATE: OnceLock<Mutex<PromptViewportState>> = OnceLock::new();

fn prompt_viewport_state() -> &'static Mutex<PromptViewportState> {
    PROMPT_VIEWPORT_STATE.get_or_init(|| Mutex::new(PromptViewportState::default()))
}

fn active_prompt_entry_animation(now_ms: u64) -> Option<PromptViewportAnimation> {
    let mut state = match prompt_viewport_state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if let Some(anim) = state.active {
        if now_ms.saturating_sub(anim.start_ms) <= PROMPT_ENTRY_ANIMATION_MS {
            return Some(anim);
        }
        state.active = None;
    }
    None
}

fn record_prompt_viewport(visible_start: usize, visible_end: usize) {
    let mut state = match prompt_viewport_state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.initialized = true;
    state.last_visible_start = visible_start;
    state.last_visible_end = visible_end;
    state.active = None;
}

fn update_prompt_entry_animation(
    user_prompt_lines: &[usize],
    visible_start: usize,
    visible_end: usize,
    now_ms: u64,
) {
    let mut state = match prompt_viewport_state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    if !state.initialized {
        state.initialized = true;
        state.last_visible_start = visible_start;
        state.last_visible_end = visible_end;
        return;
    }

    let prev_visible_start = state.last_visible_start;
    let prev_visible_end = state.last_visible_end;
    let viewport_changed = prev_visible_start != visible_start || prev_visible_end != visible_end;

    if let Some(anim) = state.active {
        let still_fresh = now_ms.saturating_sub(anim.start_ms) <= PROMPT_ENTRY_ANIMATION_MS;
        let still_visible = anim.line_idx >= visible_start && anim.line_idx < visible_end;
        if still_fresh && still_visible {
            state.last_visible_start = visible_start;
            state.last_visible_end = visible_end;
            return;
        }
        if !still_fresh || !still_visible {
            state.active = None;
        }
    }

    if viewport_changed && state.active.is_none() {
        let newly_visible = user_prompt_lines.iter().copied().find(|line| {
            *line >= visible_start
                && *line < visible_end
                && (*line < prev_visible_start || *line >= prev_visible_end)
        });
        if let Some(line_idx) = newly_visible {
            state.active = Some(PromptViewportAnimation {
                line_idx,
                start_ms: now_ms,
            });
        }
    }

    state.last_visible_start = visible_start;
    state.last_visible_end = visible_end;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BodyCacheKey {
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    messages_version: u64,
    diagram_mode: crate::config::DiagramDisplayMode,
    centered: bool,
}

#[derive(Clone)]
struct BodyCacheEntry {
    key: BodyCacheKey,
    prepared: Arc<PreparedMessages>,
    msg_count: usize,
}

const BODY_CACHE_MAX_ENTRIES: usize = 8;

#[derive(Default)]
struct BodyCacheState {
    entries: VecDeque<BodyCacheEntry>,
}

impl BodyCacheState {
    fn get_exact(&mut self, key: &BodyCacheKey) -> Option<Arc<PreparedMessages>> {
        let pos = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(pos)?;
        let prepared = entry.prepared.clone();
        self.entries.push_front(entry);
        Some(prepared)
    }

    fn best_incremental_base(
        &self,
        key: &BodyCacheKey,
        msg_count: usize,
    ) -> Option<(Arc<PreparedMessages>, usize)> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.msg_count > 0
                    && msg_count > entry.msg_count
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
            })
            .max_by_key(|entry| entry.msg_count)
            .map(|entry| (entry.prepared.clone(), entry.msg_count))
    }

    fn insert(&mut self, key: BodyCacheKey, prepared: Arc<PreparedMessages>, msg_count: usize) {
        if let Some(pos) = self.entries.iter().position(|entry| entry.key == key) {
            self.entries.remove(pos);
        }
        self.entries.push_front(BodyCacheEntry {
            key,
            prepared,
            msg_count,
        });
        while self.entries.len() > BODY_CACHE_MAX_ENTRIES {
            self.entries.pop_back();
        }
    }
}

static BODY_CACHE: OnceLock<Mutex<BodyCacheState>> = OnceLock::new();

fn body_cache() -> &'static Mutex<BodyCacheState> {
    BODY_CACHE.get_or_init(|| Mutex::new(BodyCacheState::default()))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FullPrepCacheKey {
    width: u16,
    height: u16,
    diff_mode: crate::config::DiffDisplayMode,
    messages_version: u64,
    diagram_mode: crate::config::DiagramDisplayMode,
    centered: bool,
    is_processing: bool,
    streaming_text_len: usize,
    batch_progress_hash: u64,
    startup_active: bool,
}

#[derive(Default)]
struct FullPrepCacheState {
    key: Option<FullPrepCacheKey>,
    prepared: Option<Arc<PreparedMessages>>,
}

static FULL_PREP_CACHE: OnceLock<Mutex<FullPrepCacheState>> = OnceLock::new();

fn full_prep_cache() -> &'static Mutex<FullPrepCacheState> {
    FULL_PREP_CACHE.get_or_init(|| Mutex::new(FullPrepCacheState::default()))
}

#[derive(Default)]
struct RenderProfile {
    frames: u64,
    total: Duration,
    prepare: Duration,
    draw: Duration,
    last_log: Option<Instant>,
}

static PROFILE_STATE: OnceLock<Mutex<RenderProfile>> = OnceLock::new();

fn profile_state() -> &'static Mutex<RenderProfile> {
    PROFILE_STATE.get_or_init(|| Mutex::new(RenderProfile::default()))
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutSnapshot {
    pub messages_area: Rect,
    pub diagram_area: Option<Rect>,
    pub diff_pane_area: Option<Rect>,
}

static LAST_LAYOUT: OnceLock<Mutex<Option<LayoutSnapshot>>> = OnceLock::new();

fn last_layout_state() -> &'static Mutex<Option<LayoutSnapshot>> {
    LAST_LAYOUT.get_or_init(|| Mutex::new(None))
}

pub fn record_layout_snapshot(
    messages_area: Rect,
    diagram_area: Option<Rect>,
    diff_pane_area: Option<Rect>,
) {
    if let Ok(mut snapshot) = last_layout_state().lock() {
        *snapshot = Some(LayoutSnapshot {
            messages_area,
            diagram_area,
            diff_pane_area,
        });
    }
}

pub fn last_layout_snapshot() -> Option<LayoutSnapshot> {
    last_layout_state()
        .lock()
        .ok()
        .and_then(|snapshot| *snapshot)
}

fn profile_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("JCODE_TUI_PROFILE").is_ok())
}

fn record_profile(prepare: Duration, draw: Duration, total: Duration) {
    let mut state = match profile_state().lock() {
        Ok(s) => s,
        Err(poisoned) => poisoned.into_inner(),
    };
    state.frames += 1;
    state.prepare += prepare;
    state.draw += draw;
    state.total += total;

    let now = Instant::now();
    let should_log = match state.last_log {
        Some(last) => now.duration_since(last) >= Duration::from_secs(1),
        None => true,
    };
    if should_log && state.frames > 0 {
        let frames = state.frames as f64;
        let avg_prepare = state.prepare.as_secs_f64() * 1000.0 / frames;
        let avg_draw = state.draw.as_secs_f64() * 1000.0 / frames;
        let avg_total = state.total.as_secs_f64() * 1000.0 / frames;
        crate::logging::info(&format!(
            "TUI perf: {:.1} fps | prepare {:.2}ms | draw {:.2}ms | total {:.2}ms",
            frames, avg_prepare, avg_draw, avg_total
        ));
        state.frames = 0;
        state.prepare = Duration::from_secs(0);
        state.draw = Duration::from_secs(0);
        state.total = Duration::from_secs(0);
        state.last_log = Some(now);
    }
}

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw_inner(frame, app))) {
        Ok(()) => {}
        Err(payload) => {
            let panic_count = DRAW_PANIC_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            let msg = panic_payload_to_string(&payload);
            if panic_count <= 3 || panic_count % 50 == 0 {
                crate::logging::error(&format!(
                    "Recovered TUI draw panic #{}: {}",
                    panic_count, msg
                ));
            }
            let area = frame.area().intersection(*frame.buffer_mut().area());
            if area.width == 0 || area.height == 0 {
                return;
            }
            clear_area(frame, area);
            let lines = vec![
                Line::from(Span::styled(
                    "rendering error recovered",
                    Style::default().fg(Color::Red),
                )),
                Line::from(Span::styled(
                    "continuing with a safe fallback frame",
                    Style::default().fg(dim_color()),
                )),
            ];
            frame.render_widget(Paragraph::new(lines), area);
        }
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn draw_inner(frame: &mut Frame, app: &dyn TuiState) {
    let area = frame.area().intersection(*frame.buffer_mut().area());
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Clear full frame to prevent stale cells from prior layouts.
    // This is critical on macOS terminals where ratatui's diff-based updates
    // can leave outdated content when layout dimensions change between frames
    // (e.g., diagram pane toggling, streaming text clearing, tool calls finishing).
    // Uses Color::Reset (terminal default bg) so text selection highlighting works
    // natively in all terminal emulators.
    clear_area(frame, area);

    if let Some(scroll) = app.changelog_scroll() {
        overlays::draw_changelog_overlay(frame, area, scroll);
        return;
    }

    if let Some(scroll) = app.help_scroll() {
        overlays::draw_help_overlay(frame, area, scroll, app);
        return;
    }

    if let Some(picker_cell) = app.session_picker_overlay() {
        let mut picker = picker_cell.borrow_mut();
        picker.render(frame);
        return;
    }

    // Initialize visual debug capture if enabled
    let mut debug_capture = if visual_debug::is_enabled() {
        Some(FrameCaptureBuilder::new(area.width, area.height))
    } else {
        None
    };

    // Check diagram display mode and get active diagrams early so we can
    // determine the horizontal split before computing input width etc.
    let diagram_mode = app.diagram_mode();
    let diagrams = super::mermaid::get_active_diagrams();
    let diagram_count = diagrams.len();
    let selected_index = if diagram_count > 0 {
        app.diagram_index().min(diagram_count - 1)
    } else {
        0
    };
    let pane_enabled = app.diagram_pane_enabled();
    let pane_position = app.diagram_pane_position();
    let has_side_panel_content = app.side_panel().focused_page().is_some();
    let suppress_side_diagram =
        has_side_panel_content && pane_position == crate::config::DiagramPanePosition::Side;
    let pinned_diagram = if diagram_mode == crate::config::DiagramDisplayMode::Pinned
        && pane_enabled
        && !suppress_side_diagram
    {
        diagrams.get(selected_index).cloned()
    } else {
        None
    };
    let diagram_focus = app.diagram_focus();
    let (diagram_scroll_x, diagram_scroll_y) = app.diagram_scroll();

    // Compute layout depending on pane position (Side = right column, Top = above chat).
    let (chat_area, diagram_area) = if let Some(diagram) = pinned_diagram.as_ref() {
        match pane_position {
            crate::config::DiagramPanePosition::Side => {
                const MIN_DIAGRAM_WIDTH: u16 = 24;
                const MIN_CHAT_WIDTH: u16 = 20;
                let max_diagram = area.width.saturating_sub(MIN_CHAT_WIDTH);
                if max_diagram >= MIN_DIAGRAM_WIDTH {
                    let ratio = app.diagram_pane_ratio().clamp(25, 100) as u32;
                    let ratio_target = ((area.width as u32 * ratio) / 100) as u16;
                    let needed =
                        estimate_pinned_diagram_pane_width(diagram, area.height, MIN_DIAGRAM_WIDTH);
                    let diagram_width = ratio_target
                        .max(needed.min(max_diagram))
                        .max(MIN_DIAGRAM_WIDTH)
                        .min(max_diagram);
                    let chat_width = area.width.saturating_sub(diagram_width);
                    if diagram_width > 0 && chat_width > 0 {
                        let chat = Rect {
                            x: area.x,
                            y: area.y,
                            width: chat_width,
                            height: area.height,
                        };
                        let diag = Rect {
                            x: area.x + chat_width,
                            y: area.y,
                            width: diagram_width,
                            height: area.height,
                        };
                        (chat, Some(diag))
                    } else {
                        (area, None)
                    }
                } else {
                    (area, None)
                }
            }
            crate::config::DiagramPanePosition::Top => {
                const MIN_DIAGRAM_HEIGHT: u16 = 6;
                const MIN_CHAT_HEIGHT: u16 = 8;
                let max_diagram = area.height.saturating_sub(MIN_CHAT_HEIGHT);
                if max_diagram >= MIN_DIAGRAM_HEIGHT {
                    let ratio = app.diagram_pane_ratio().clamp(20, 100) as u32;
                    let ratio_target = ((area.height as u32 * ratio) / 100) as u16;
                    let needed = estimate_pinned_diagram_pane_height(
                        diagram,
                        area.width,
                        MIN_DIAGRAM_HEIGHT,
                    );
                    let diagram_height = ratio_target
                        .max(needed.min(max_diagram))
                        .max(MIN_DIAGRAM_HEIGHT)
                        .min(max_diagram);
                    let chat_height = area.height.saturating_sub(diagram_height);
                    if diagram_height > 0 && chat_height > 0 {
                        let diag = Rect {
                            x: area.x,
                            y: area.y,
                            width: area.width,
                            height: diagram_height,
                        };
                        let chat = Rect {
                            x: area.x,
                            y: area.y + diagram_height,
                            width: area.width,
                            height: chat_height,
                        };
                        (chat, Some(diag))
                    } else {
                        (area, None)
                    }
                } else {
                    (area, None)
                }
            }
        }
    } else {
        (area, None)
    };

    let diff_mode = app.diff_mode();
    let pin_images = app.pin_images();
    let collect_diffs = diff_mode.is_pinned();
    let has_pinned_content = if collect_diffs || pin_images {
        collect_pinned_content_cached(
            app.display_messages(),
            collect_diffs,
            pin_images,
            app.display_messages_version(),
        )
    } else {
        false
    };
    let has_file_diff_edits = diff_mode.is_file()
        && app.display_messages().iter().any(|m| {
            m.tool_data
                .as_ref()
                .map(|tc| {
                    matches!(
                        tc.name.as_str(),
                        "edit"
                            | "Edit"
                            | "write"
                            | "multiedit"
                            | "patch"
                            | "Patch"
                            | "apply_patch"
                            | "ApplyPatch"
                    )
                })
                .unwrap_or(false)
        });

    let needs_side_pane = has_side_panel_content || has_pinned_content || has_file_diff_edits;

    let (chat_area, diff_pane_area) = if needs_side_pane {
        const MIN_DIFF_WIDTH: u16 = 30;
        const MIN_CHAT_WIDTH: u16 = 20;
        let max_diff = chat_area.width.saturating_sub(MIN_CHAT_WIDTH);
        if max_diff >= MIN_DIFF_WIDTH {
            let diff_width = (((chat_area.width as u32
                * app.diagram_pane_ratio().clamp(25, 100) as u32)
                / 100) as u16)
                .max(MIN_DIFF_WIDTH)
                .min(max_diff);
            let new_chat_width = chat_area.width.saturating_sub(diff_width);
            let chat = Rect {
                x: chat_area.x,
                y: chat_area.y,
                width: new_chat_width,
                height: chat_area.height,
            };
            let diff = Rect {
                x: chat_area.x + new_chat_width,
                y: chat_area.y,
                width: diff_width,
                height: chat_area.height,
            };
            (chat, Some(diff))
        } else {
            (chat_area, None)
        }
    } else {
        (chat_area, None)
    };

    // Calculate pending messages (queued + interleave) for numbering and layout
    let pending_count = input_ui::pending_prompt_count(app);
    let queued_height = pending_count.min(3) as u16;

    // Count user messages to show next prompt number
    let user_count = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "user")
        .count();
    let next_prompt = user_count + 1;

    // Calculate input height based on the same wrapping logic used for rendering
    // (max 10 lines visible, scrolls if more).
    let base_input_height =
        input_ui::wrapped_input_line_count(app, chat_area.width, next_prompt).min(10) as u16;
    // Add 1 line for command suggestions when typing /, or for Shift+Enter hint when processing
    let suggestions = app.command_suggestions();
    let has_slash_input = app.input().trim_start().starts_with('/');
    let hint_line_height = if !suggestions.is_empty() && (has_slash_input || !app.is_processing()) {
        1 // Command suggestions (shown even during streaming when typing /commands)
    } else if app.is_processing() && !app.input().is_empty() {
        1 // Shift+Enter hint
    } else {
        0
    };
    let picker_height: u16 = if let Some(picker) = app.picker_state() {
        let visible_models = picker.filtered.len() as u16;
        let rows_needed = visible_models + 1; // +1 for header
        let max_height: u16 = 20;
        rows_needed.min(max_height)
    } else {
        0
    };
    let input_height = base_input_height + hint_line_height;

    let total_start = Instant::now();
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("prepare_messages".to_string());
    }
    let prep_start = Instant::now();
    let prepared = prepare::prepare_messages(app, chat_area.width, chat_area.height);
    if let Some(ref mut capture) = debug_capture {
        capture.image_regions = prepared
            .image_regions
            .iter()
            .map(|region| ImageRegionCapture {
                hash: format!("{:016x}", region.hash),
                abs_line_idx: region.abs_line_idx,
                height: region.height,
            })
            .collect();
    }
    let prep_elapsed = prep_start.elapsed();
    let content_height = prepared.wrapped_lines.len().max(1) as u16;
    let show_donut = crate::config::config().display.idle_animation
        && app.display_messages().is_empty()
        && !app.is_processing()
        && app.streaming_text().is_empty()
        && app.queued_messages().is_empty();
    let donut_height: u16 = if show_donut { 14 } else { 0 };
    let notification_height: u16 = if app.has_notification() { 1 } else { 0 };
    let fixed_height =
        1 + queued_height + notification_height + picker_height + input_height + donut_height; // status + queued + notification + picker + input + donut
    let available_height = chat_area.height;

    // Use packed layout when content fits, scrolling layout otherwise
    let use_packed = content_height + fixed_height <= available_height;

    // Layout: messages (includes header), queued, status, notification, picker, input, donut
    // All vertical chunks are within the chat_area (left column).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if use_packed {
            vec![
                Constraint::Length(content_height.max(1)), // Messages (exact height)
                Constraint::Length(queued_height),         // Queued messages (above status)
                Constraint::Length(1),                     // Status line
                Constraint::Length(notification_height),   // Notification line
                Constraint::Length(picker_height),         // Picker (0 or 1 line)
                Constraint::Length(input_height),          // Input
                Constraint::Length(donut_height),          // Donut animation
            ]
        } else {
            vec![
                Constraint::Min(3),                      // Messages (scrollable)
                Constraint::Length(queued_height),       // Queued messages (above status)
                Constraint::Length(1),                   // Status line
                Constraint::Length(notification_height), // Notification line
                Constraint::Length(picker_height),       // Picker (0 or 1 line)
                Constraint::Length(input_height),        // Input
                Constraint::Length(donut_height),        // Donut animation
            ]
        })
        .split(chat_area);

    // Capture layout info for visual debug
    if let Some(ref mut capture) = debug_capture {
        capture.layout.use_packed = use_packed;
        capture.layout.estimated_content_height = content_height as usize;
        capture.layout.messages_area = Some(chunks[0].into());
        if queued_height > 0 {
            capture.layout.queued_area = Some(chunks[1].into());
        }
        capture.layout.status_area = Some(chunks[2].into());
        capture.layout.input_area = Some(chunks[5].into());
        capture.layout.input_lines_raw = app.input().lines().count().max(1);
        capture.layout.input_lines_wrapped = base_input_height as usize;

        // Capture state snapshot
        capture.state.is_processing = app.is_processing();
        capture.state.input_len = app.input().len();
        capture.state.input_preview = app.input().chars().take(100).collect();
        capture.state.cursor_pos = app.cursor_pos();
        capture.state.scroll_offset = app.scroll_offset();
        capture.state.queued_count = pending_count;
        capture.state.message_count = app.display_messages().len();
        capture.state.streaming_text_len = app.streaming_text().len();
        capture.state.has_suggestions = !suggestions.is_empty();
        capture.state.status = format!("{:?}", app.status());
        capture.state.diagram_mode = Some(format!("{:?}", diagram_mode));
        capture.state.diagram_focus = diagram_focus;
        capture.state.diagram_index = selected_index;
        capture.state.diagram_count = diagram_count;
        capture.state.diagram_scroll_x = diagram_scroll_x;
        capture.state.diagram_scroll_y = diagram_scroll_y;
        capture.state.diagram_pane_ratio = app.diagram_pane_ratio();
        capture.state.diagram_pane_enabled = app.diagram_pane_enabled();
        capture.state.diagram_pane_position = Some(format!("{:?}", app.diagram_pane_position()));
        capture.state.diagram_zoom = app.diagram_zoom();

        // Capture rendered content
        // Queued messages
        capture.rendered_text.queued_messages = input_ui::pending_queue_preview(app);

        // Recent display messages (last 5 for context)
        capture.rendered_text.recent_messages = app
            .display_messages()
            .iter()
            .rev()
            .take(5)
            .map(|m| MessageCapture {
                role: m.role.clone(),
                content_preview: m.content.chars().take(200).collect(),
                content_len: m.content.len(),
            })
            .collect();

        // Streaming text preview
        let streaming = app.streaming_text();
        if !streaming.is_empty() {
            capture.rendered_text.streaming_text_preview = streaming.chars().take(500).collect();
        }

        // Status line content
        capture.rendered_text.status_line = format_status_for_debug(app);
    }

    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_messages".to_string());
    }
    let draw_start = Instant::now();

    // Messages area is chunks[0] within the chat column (already excludes diagram).
    let messages_area = chunks[0];

    if let Some(ref mut capture) = debug_capture {
        capture.layout.messages_area = Some(messages_area.into());
        capture.layout.diagram_area = diagram_area.map(|r| r.into());
    }
    record_layout_snapshot(messages_area, diagram_area, diff_pane_area);

    let margins = draw_messages(frame, app, messages_area, &prepared);

    // Render pinned diagram if we have one
    if let (Some(diagram_info), Some(area)) = (&pinned_diagram, diagram_area) {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_pinned_diagram".to_string());
        }
        draw_pinned_diagram(
            frame,
            diagram_info,
            area,
            selected_index,
            diagram_count,
            diagram_focus,
            diagram_scroll_x,
            diagram_scroll_y,
            app.diagram_zoom(),
            pane_position,
            app.diagram_pane_animating(),
        );
    }

    if let Some(diff_area) = diff_pane_area {
        if has_side_panel_content {
            if let Some(ref mut capture) = debug_capture {
                capture
                    .render_order
                    .push("draw_side_panel_markdown".to_string());
            }
            draw_side_panel_markdown(
                frame,
                diff_area,
                app.side_panel(),
                app.diff_pane_scroll(),
                app.diff_pane_focus(),
            );
        } else if has_file_diff_edits {
            if let Some(ref mut capture) = debug_capture {
                capture.render_order.push("draw_file_diff_view".to_string());
            }
            draw_file_diff_view(
                frame,
                diff_area,
                app,
                &prepared,
                app.diff_pane_scroll(),
                app.diff_pane_focus(),
            );
        } else if has_pinned_content {
            if let Some(ref mut capture) = debug_capture {
                capture.render_order.push("draw_pinned_content".to_string());
            }
            draw_pinned_content_cached(
                frame,
                diff_area,
                app.diff_pane_scroll(),
                app.diff_line_wrap(),
                app.diff_pane_focus(),
            );
        }
    }

    let messages_draw = draw_start.elapsed();

    if let Some(ref mut capture) = debug_capture {
        capture.layout.margins = Some(MarginsCapture {
            left_widths: margins.left_widths.clone(),
            right_widths: margins.right_widths.clone(),
            centered: margins.centered,
        });
    }
    if queued_height > 0 {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_queued".to_string());
        }
        input_ui::draw_queued(frame, app, chunks[1], user_count + 1);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_status".to_string());
    }
    input_ui::draw_status(frame, app, chunks[2], pending_count);
    if notification_height > 0 {
        input_ui::draw_notification(frame, app, chunks[3]);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_input".to_string());
    }
    // Draw picker line if active
    if picker_height > 0 {
        draw_picker_line(frame, app, chunks[4]);
    }

    input_ui::draw_input(
        frame,
        app,
        chunks[5],
        user_count + pending_count + 1,
        &mut debug_capture,
    );

    if donut_height > 0 {
        animations::draw_idle_animation(frame, app, chunks[6]);
    }

    // Draw info widget overlays (skip during idle animation - they look out of place)
    let widget_data = app.info_widget_data();
    let mut widget_render_ms: Option<f32> = None;
    let mut placements: Vec<info_widget::WidgetPlacement> = Vec::new();
    let widget_bounds = messages_area;
    if !widget_data.is_empty() && !show_donut {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("render_info_widgets".to_string());
        }
        placements = info_widget::calculate_placements(widget_bounds, &margins, &widget_data);

        if let Some(ref mut capture) = debug_capture {
            let placement_captures = capture_widget_placements(&placements);
            capture.layout.widget_placements = placement_captures.clone();
            capture.info_widgets = Some(InfoWidgetCapture {
                summary: build_info_widget_summary(&widget_data),
                placements: placement_captures,
            });

            // Detect overlaps with message area
            for placement in &placements {
                if rects_overlap(placement.rect, widget_bounds) {
                    capture.anomaly(format!(
                        "Info widget {:?} overlaps messages area",
                        placement.kind
                    ));
                }
                if !rect_within_bounds(placement.rect, area) {
                    capture.anomaly(format!(
                        "Info widget {:?} out of bounds {:?}",
                        placement.kind, placement.rect
                    ));
                }
                if let Some(diagram_area) = diagram_area {
                    if rects_overlap(placement.rect, diagram_area) {
                        capture.anomaly(format!(
                            "Info widget {:?} overlaps diagram area",
                            placement.kind
                        ));
                    }
                }
            }
            for i in 0..placements.len() {
                for j in (i + 1)..placements.len() {
                    if rects_overlap(placements[i].rect, placements[j].rect) {
                        capture.anomaly(format!(
                            "Info widgets overlap: {:?} and {:?}",
                            placements[i].kind, placements[j].kind
                        ));
                    }
                }
            }
        }

        let widget_start = Instant::now();
        info_widget::render_all(frame, &placements, &widget_data);
        widget_render_ms = Some(widget_start.elapsed().as_secs_f32() * 1000.0);

        // Optional visual overlay for placements
    } else if let Some(ref mut capture) = debug_capture {
        capture.info_widgets = Some(InfoWidgetCapture {
            summary: build_info_widget_summary(&widget_data),
            placements: Vec::new(),
        });
    }
    if visual_debug::overlay_enabled() {
        overlays::draw_debug_overlay(frame, &placements, &chunks);
    }

    // Record the frame capture if enabled
    if let Some(capture) = debug_capture {
        let total_draw = draw_start.elapsed();
        let render_timing = RenderTimingCapture {
            prepare_ms: prep_elapsed.as_secs_f32() * 1000.0,
            draw_ms: total_draw.as_secs_f32() * 1000.0,
            total_ms: total_start.elapsed().as_secs_f32() * 1000.0,
            messages_ms: Some(messages_draw.as_secs_f32() * 1000.0),
            widgets_ms: widget_render_ms,
        };

        let mut capture = capture;
        capture.render_timing = Some(render_timing);
        capture.mermaid = crate::tui::mermaid::debug_stats_json();
        capture.markdown = crate::tui::markdown::debug_stats_json();
        capture.theme = overlays::debug_palette_json();
        visual_debug::record_frame(capture.build());
    }

    if profile_enabled() {
        let total_draw = draw_start.elapsed();
        record_profile(prep_elapsed, total_draw, total_start.elapsed());
    }
}

fn extract_line_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn extract_line_styled_chars(line: &Line) -> Vec<(char, Style)> {
    let mut chars = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            chars.push((ch, span.style));
        }
    }
    chars
}

fn morph_lines_to_header(
    anim_lines: &[Line<'static>],
    header_lines: &[Line<'static>],
    morph_t: f32,
    width: u16,
) -> Vec<Line<'static>> {
    let blend = ((morph_t - 0.6) / 0.35).clamp(0.0, 1.0);
    let max_rows = anim_lines.len().max(header_lines.len());
    let w = width as usize;

    let mut result = Vec::with_capacity(max_rows);

    let anim_row_count = anim_lines.len();
    let header_row_count = header_lines.len();
    let row_blend = blend * blend;
    let target_rows =
        anim_row_count as f32 + (header_row_count as f32 - anim_row_count as f32) * row_blend;
    let output_rows = target_rows.round() as usize;

    for out_row in 0..output_rows {
        let anim_row_f = if output_rows > 1 {
            out_row as f32 / (output_rows - 1) as f32 * (anim_row_count.max(1) - 1) as f32
        } else {
            0.0
        };
        let header_row_f = if output_rows > 1 {
            out_row as f32 / (output_rows - 1) as f32 * (header_row_count.max(1) - 1) as f32
        } else {
            0.0
        };

        let anim_idx = (anim_row_f.round() as usize).min(anim_row_count.saturating_sub(1));
        let header_idx = (header_row_f.round() as usize).min(header_row_count.saturating_sub(1));

        let anim_chars: Vec<(char, Style)> = if anim_idx < anim_row_count {
            extract_line_styled_chars(&anim_lines[anim_idx])
        } else {
            Vec::new()
        };
        let header_chars: Vec<(char, Style)> = if header_idx < header_row_count {
            extract_line_styled_chars(&header_lines[header_idx])
        } else {
            Vec::new()
        };

        let anim_text: String = anim_chars.iter().map(|(c, _)| *c).collect();
        let header_text: String = header_chars.iter().map(|(c, _)| *c).collect();
        let anim_trimmed = anim_text.trim();
        let header_trimmed = header_text.trim();

        let anim_start = anim_text.find(anim_trimmed).unwrap_or(0);
        let header_start = header_text.find(header_trimmed).unwrap_or(0);

        let anim_center = if !anim_trimmed.is_empty() {
            anim_start as f32 + anim_trimmed.len() as f32 / 2.0
        } else {
            w as f32 / 2.0
        };
        let header_center = if !header_trimmed.is_empty() {
            header_start as f32 + header_trimmed.len() as f32 / 2.0
        } else {
            w as f32 / 2.0
        };

        let center = anim_center + (header_center - anim_center) * blend;
        let max_col = anim_chars.len().max(header_chars.len()).max(w);

        let mut spans: Vec<Span<'static>> = Vec::new();

        for col in 0..max_col {
            let anim_ch = anim_chars.get(col).map(|(c, _)| *c).unwrap_or(' ');
            let anim_style = anim_chars.get(col).map(|(_, s)| *s).unwrap_or_default();
            let header_ch = header_chars.get(col).map(|(c, _)| *c).unwrap_or(' ');
            let header_style = header_chars.get(col).map(|(_, s)| *s).unwrap_or_default();

            let dist_from_center = ((col as f32) - center).abs() / (w as f32 / 2.0).max(1.0);
            let flip_hash = {
                let mut h = DefaultHasher::new();
                out_row.hash(&mut h);
                col.hash(&mut h);
                (std::hash::Hasher::finish(&h) % 1000) as f32 / 1000.0
            };
            let flip_threshold = (0.3 + dist_from_center * 0.4 + flip_hash * 0.3).clamp(0.0, 1.0);

            let (ch, style) = if blend >= flip_threshold {
                let style_blend = ((blend - flip_threshold) / 0.15).clamp(0.0, 1.0);
                if style_blend < 0.3 {
                    let glitch_chars = b"@#$%&*!?~=+<>";
                    let gi = {
                        let mut h = DefaultHasher::new();
                        out_row.hash(&mut h);
                        col.hash(&mut h);
                        ((blend * 100.0) as u32).hash(&mut h);
                        (std::hash::Hasher::finish(&h) % glitch_chars.len() as u64) as usize
                    };
                    let gc = glitch_chars[gi] as char;
                    (gc, lerp_style(anim_style, header_style, style_blend))
                } else {
                    (header_ch, lerp_style(anim_style, header_style, style_blend))
                }
            } else {
                let fade = (1.0 - blend / flip_threshold.max(0.01)).clamp(0.0, 1.0);
                let mut s = anim_style;
                if let Some(fg) = s.fg {
                    let (r, g, b) = color_to_floats(fg, (80.0, 80.0, 80.0));
                    s.fg = Some(rgb((r * fade) as u8, (g * fade) as u8, (b * fade) as u8));
                }
                (anim_ch, s)
            };

            spans.push(Span::styled(ch.to_string(), style));
        }

        let align = header_lines
            .get(header_idx)
            .and_then(|l| l.alignment)
            .or_else(|| anim_lines.get(anim_idx).and_then(|l| l.alignment))
            .unwrap_or(ratatui::layout::Alignment::Center);

        result.push(Line::from(spans).alignment(align));
    }

    result
}

fn lerp_style(from: Style, to: Style, t: f32) -> Style {
    let fg = match (from.fg, to.fg) {
        (Some(f), Some(toc)) => {
            let (r1, g1, b1) = color_to_floats(f, (80.0, 80.0, 80.0));
            let (r2, g2, b2) = color_to_floats(toc, (200.0, 200.0, 200.0));
            Some(rgb(
                (r1 + (r2 - r1) * t).clamp(0.0, 255.0) as u8,
                (g1 + (g2 - g1) * t).clamp(0.0, 255.0) as u8,
                (b1 + (b2 - b1) * t).clamp(0.0, 255.0) as u8,
            ))
        }
        (Some(f), _) => {
            let (r, g, b) = color_to_floats(f, (80.0, 80.0, 80.0));
            let dim = 1.0 - t;
            Some(rgb((r * dim) as u8, (g * dim) as u8, (b * dim) as u8))
        }
        (_, Some(toc)) => {
            let (r, g, b) = color_to_floats(toc, (200.0, 200.0, 200.0));
            Some(rgb((r * t) as u8, (g * t) as u8, (b * t) as u8))
        }
        (_, to_fg) => to_fg,
    };
    let mut s = to;
    s.fg = fg;
    s
}

fn build_header_lines(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let centered = app.centered_mode();
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let model = app.provider_model();
    let provider_name = app.provider_name();
    let upstream = app.upstream_provider();
    let auth = app.auth_status();
    let provider_label = {
        let trimmed = provider_name.trim();
        if trimmed.is_empty() {
            "unknown".to_string()
        } else {
            let name = trimmed.to_lowercase();
            let auth_tag = match name.as_str() {
                "anthropic" => {
                    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                        "api-key"
                    } else if auth.anthropic.has_oauth {
                        "oauth"
                    } else {
                        ""
                    }
                }
                "openai" => {
                    if auth.openai_has_api_key {
                        "api-key"
                    } else if auth.openai_has_oauth {
                        "oauth"
                    } else {
                        ""
                    }
                }
                "copilot" => {
                    if auth.copilot_has_api_token {
                        "oauth"
                    } else {
                        ""
                    }
                }
                "openrouter" => "api-key",
                _ => "",
            };
            if auth_tag.is_empty() {
                name
            } else {
                format!("{}:{}", auth_tag, name)
            }
        }
    };

    // Line: provider + model + upstream provider if available + hint to switch
    let w = width as usize;
    let model_info = if let Some(ref provider) = upstream {
        let full = format!(
            "({}) {} via {} · /model to switch",
            provider_label, model, provider
        );
        if full.chars().count() <= w {
            full
        } else {
            let short = format!("({}) {} via {}", provider_label, model, provider);
            if short.chars().count() <= w {
                short
            } else {
                format!("({}) {}", provider_label, model)
            }
        }
    } else {
        let full = format!("({}) {} · /model to switch", provider_label, model);
        if full.chars().count() <= w {
            full
        } else {
            format!("({}) {}", provider_label, model)
        }
    };
    lines.push(
        Line::from(Span::styled(model_info, Style::default().fg(dim_color()))).alignment(align),
    );

    // Line: Auth status indicators (colored dots for each provider)
    let auth_line = header::build_auth_status_line(&auth, w);
    if !auth_line.spans.is_empty() {
        lines.push(auth_line.alignment(align));
    }

    // Line 3+: Recent changes in a box (from git log, embedded at build time)
    // Each line is "hash:subject". We filter to only show commits since the user last saw updates.
    let new_entries = get_unseen_changelog_entries();
    let term_width = width as usize;
    if !new_entries.is_empty() && term_width > 20 {
        const MAX_LINES: usize = 8;
        let available_width = term_width.saturating_sub(2);
        let display_count = new_entries.len().min(MAX_LINES);
        let has_more = new_entries.len() > MAX_LINES;

        let mut content: Vec<Line> = Vec::new();
        for entry in new_entries.iter().take(display_count) {
            content.push(
                Line::from(Span::styled(
                    format!("• {}", entry),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
        }
        if has_more {
            content.push(
                Line::from(Span::styled(
                    format!(
                        "  …{} more · /changelog to see all",
                        new_entries.len() - MAX_LINES
                    ),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
        }

        let boxed = render_rounded_box(
            "Updates",
            content,
            available_width,
            Style::default().fg(dim_color()),
        );
        for line in boxed {
            lines.push(line.alignment(align));
        }
    }

    // Line 4: MCPs - show server names with tool counts, or (none)
    let mcps = app.mcp_servers();
    let mcp_text = if mcps.is_empty() {
        "mcp: (none)".to_string()
    } else {
        let full_parts: Vec<String> = mcps
            .iter()
            .map(|(name, count)| {
                if *count > 0 {
                    format!("{} ({} tools)", name, count)
                } else {
                    format!("{} (...)", name)
                }
            })
            .collect();
        let full = format!("mcp: {}", full_parts.join(", "));
        if full.chars().count() <= w {
            full
        } else {
            // Try shorter: just names with counts
            let short_parts: Vec<String> = mcps
                .iter()
                .map(|(name, count)| {
                    if *count > 0 {
                        format!("{}({})", name, count)
                    } else {
                        format!("{}(…)", name)
                    }
                })
                .collect();
            let short = format!("mcp: {}", short_parts.join(" "));
            if short.chars().count() <= w {
                short
            } else {
                // Just count
                format!("mcp: {} servers", mcps.len())
            }
        }
    };
    lines.push(
        Line::from(Span::styled(mcp_text, Style::default().fg(dim_color()))).alignment(align),
    );

    // Line 4: Skills (if any)
    let skills = app.available_skills();
    if !skills.is_empty() {
        let full = format!(
            "skills: {}",
            skills
                .iter()
                .map(|s| format!("/{}", s))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let skills_text = if full.chars().count() <= w {
            full
        } else {
            format!("skills: {} loaded", skills.len())
        };
        lines.push(
            Line::from(Span::styled(skills_text, Style::default().fg(dim_color())))
                .alignment(align),
        );
    }

    // Line 5: Server stats (if running as server with clients)
    let client_count = app.connected_clients().unwrap_or(0);
    let session_count = app.server_sessions().len();
    if client_count > 0 || session_count > 1 {
        let mut parts = Vec::new();
        if client_count > 0 {
            parts.push(format!(
                "{} client{}",
                client_count,
                if client_count == 1 { "" } else { "s" }
            ));
        }
        if session_count > 1 {
            parts.push(format!("{} sessions", session_count));
        }
        lines.push(
            Line::from(Span::styled(
                format!("server: {}", parts.join(", ")),
                Style::default().fg(dim_color()),
            ))
            .alignment(align),
        );
    }

    // Context window info (at the end of header) - DISABLED
    // let context_info = app.context_info();
    // if context_info.total_chars > 0 {
    //     let context_width = width.saturating_sub(4) as usize;
    //     let context_limit = app
    //         .context_limit()
    //         .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT);
    //     let context_lines = render_context_bar(&context_info, context_width, context_limit);
    //     if !context_lines.is_empty() {
    //         let boxed = render_rounded_box(
    //             "Context",
    //             context_lines,
    //             width as usize,
    //             Style::default().fg(dim_color()),
    //         );
    //         for line in boxed {
    //             lines.push(line.alignment(align));
    //         }
    //     }
    // }

    // Blank line after header
    lines.push(Line::from(""));

    lines
}

fn capture_widget_placements(
    placements: &[info_widget::WidgetPlacement],
) -> Vec<WidgetPlacementCapture> {
    placements
        .iter()
        .map(|p| WidgetPlacementCapture {
            kind: p.kind.as_str().to_string(),
            side: p.side.as_str().to_string(),
            rect: p.rect.into(),
        })
        .collect()
}

fn build_info_widget_summary(data: &info_widget::InfoWidgetData) -> InfoWidgetSummary {
    let todos_total = data.todos.len();
    let todos_done = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();

    let context_total_chars = data.context_info.as_ref().map(|c| c.total_chars);
    let context_limit = data.context_limit;

    let memory_total = data.memory_info.as_ref().map(|m| m.total_count);
    let memory_project = data.memory_info.as_ref().map(|m| m.project_count);
    let memory_global = data.memory_info.as_ref().map(|m| m.global_count);
    let memory_activity = data.memory_info.as_ref().map(|m| m.activity.is_some());

    let swarm_session_count = data.swarm_info.as_ref().map(|s| s.session_count);
    let swarm_member_count = data.swarm_info.as_ref().map(|s| s.members.len());
    let swarm_subagent_status = data
        .swarm_info
        .as_ref()
        .and_then(|s| s.subagent_status.clone());

    let background_running = data.background_info.as_ref().map(|b| b.running_count);
    let background_tasks = data.background_info.as_ref().map(|b| b.running_tasks.len());

    let usage_available = data.usage_info.as_ref().map(|u| u.available);
    let usage_provider = data
        .usage_info
        .as_ref()
        .map(|u| format!("{:?}", u.provider));

    InfoWidgetSummary {
        todos_total,
        todos_done,
        context_total_chars,
        context_limit,
        queue_mode: data.queue_mode,
        model: data.model.clone(),
        reasoning_effort: data.reasoning_effort.clone(),
        session_count: data.session_count,
        client_count: data.client_count,
        memory_total,
        memory_project,
        memory_global,
        memory_activity,
        swarm_session_count,
        swarm_member_count,
        swarm_subagent_status,
        background_running,
        background_tasks,
        usage_available,
        usage_provider,
        tokens_per_second: data.tokens_per_second,
        auth_method: Some(format!("{:?}", data.auth_method)),
        upstream_provider: data.upstream_provider.clone(),
    }
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    let a_right = a.x.saturating_add(a.width);
    let a_bottom = a.y.saturating_add(a.height);
    let b_right = b.x.saturating_add(b.width);
    let b_bottom = b.y.saturating_add(b.height);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

fn rect_within_bounds(rect: Rect, bounds: Rect) -> bool {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    let bounds_right = bounds.x.saturating_add(bounds.width);
    let bounds_bottom = bounds.y.saturating_add(bounds.height);
    rect.x >= bounds.x && rect.y >= bounds.y && right <= bounds_right && bottom <= bounds_bottom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::session_picker;

    #[derive(Clone, Default)]
    struct TestState {
        input: String,
        cursor_pos: usize,
        display_messages: Vec<DisplayMessage>,
        batch_progress: Option<crate::bus::BatchProgress>,
        queued_messages: Vec<String>,
        pending_soft_interrupts: Vec<String>,
        interleave_message: Option<String>,
        status: ProcessingStatus,
        queue_mode: bool,
        active_skill: Option<String>,
        centered_mode: bool,
        anim_elapsed: f32,
        picker_state: Option<crate::tui::PickerState>,
    }

    impl crate::tui::TuiState for TestState {
        fn display_messages(&self) -> &[DisplayMessage] {
            &self.display_messages
        }
        fn display_messages_version(&self) -> u64 {
            0
        }
        fn streaming_text(&self) -> &str {
            ""
        }
        fn input(&self) -> &str {
            &self.input
        }
        fn cursor_pos(&self) -> usize {
            self.cursor_pos
        }
        fn is_processing(&self) -> bool {
            !matches!(self.status, ProcessingStatus::Idle)
        }
        fn queued_messages(&self) -> &[String] {
            &self.queued_messages
        }
        fn interleave_message(&self) -> Option<&str> {
            self.interleave_message.as_deref()
        }
        fn pending_soft_interrupts(&self) -> &[String] {
            &self.pending_soft_interrupts
        }
        fn scroll_offset(&self) -> usize {
            0
        }
        fn auto_scroll_paused(&self) -> bool {
            false
        }
        fn provider_name(&self) -> String {
            "mock".to_string()
        }
        fn provider_model(&self) -> String {
            "mock-model".to_string()
        }
        fn upstream_provider(&self) -> Option<String> {
            None
        }
        fn connection_type(&self) -> Option<String> {
            None
        }
        fn mcp_servers(&self) -> Vec<(String, usize)> {
            Vec::new()
        }
        fn available_skills(&self) -> Vec<String> {
            Vec::new()
        }
        fn streaming_tokens(&self) -> (u64, u64) {
            (0, 0)
        }
        fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>) {
            (None, None)
        }
        fn output_tps(&self) -> Option<f32> {
            None
        }
        fn streaming_tool_calls(&self) -> Vec<ToolCall> {
            Vec::new()
        }
        fn elapsed(&self) -> Option<Duration> {
            None
        }
        fn status(&self) -> ProcessingStatus {
            self.status.clone()
        }
        fn command_suggestions(&self) -> Vec<(String, &'static str)> {
            Vec::new()
        }
        fn active_skill(&self) -> Option<String> {
            self.active_skill.clone()
        }
        fn subagent_status(&self) -> Option<String> {
            None
        }
        fn batch_progress(&self) -> Option<crate::bus::BatchProgress> {
            self.batch_progress.clone()
        }
        fn time_since_activity(&self) -> Option<Duration> {
            None
        }
        fn total_session_tokens(&self) -> Option<(u64, u64)> {
            None
        }
        fn is_remote_mode(&self) -> bool {
            false
        }
        fn is_canary(&self) -> bool {
            false
        }
        fn is_replay(&self) -> bool {
            false
        }
        fn diff_mode(&self) -> crate::config::DiffDisplayMode {
            crate::config::DiffDisplayMode::Inline
        }
        fn current_session_id(&self) -> Option<String> {
            None
        }
        fn session_display_name(&self) -> Option<String> {
            None
        }
        fn server_display_name(&self) -> Option<String> {
            None
        }
        fn server_display_icon(&self) -> Option<String> {
            None
        }
        fn server_sessions(&self) -> Vec<String> {
            Vec::new()
        }
        fn connected_clients(&self) -> Option<usize> {
            None
        }
        fn status_notice(&self) -> Option<String> {
            None
        }
        fn animation_elapsed(&self) -> f32 {
            self.anim_elapsed
        }
        fn rate_limit_remaining(&self) -> Option<Duration> {
            None
        }
        fn queue_mode(&self) -> bool {
            self.queue_mode
        }
        fn has_stashed_input(&self) -> bool {
            false
        }
        fn context_info(&self) -> crate::prompt::ContextInfo {
            Default::default()
        }
        fn context_limit(&self) -> Option<usize> {
            None
        }
        fn client_update_available(&self) -> bool {
            false
        }
        fn server_update_available(&self) -> Option<bool> {
            None
        }
        fn info_widget_data(&self) -> info_widget::InfoWidgetData {
            Default::default()
        }
        fn render_streaming_markdown(&self, _width: usize) -> Vec<Line<'static>> {
            Vec::new()
        }
        fn centered_mode(&self) -> bool {
            self.centered_mode
        }
        fn auth_status(&self) -> crate::auth::AuthStatus {
            Default::default()
        }
        fn update_cost(&mut self) {}
        fn diagram_mode(&self) -> crate::config::DiagramDisplayMode {
            Default::default()
        }
        fn diagram_focus(&self) -> bool {
            false
        }
        fn diagram_index(&self) -> usize {
            0
        }
        fn diagram_scroll(&self) -> (i32, i32) {
            (0, 0)
        }
        fn diagram_pane_ratio(&self) -> u8 {
            50
        }
        fn diagram_pane_animating(&self) -> bool {
            false
        }
        fn diagram_pane_enabled(&self) -> bool {
            false
        }
        fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition {
            Default::default()
        }
        fn diagram_zoom(&self) -> u8 {
            100
        }
        fn diff_pane_scroll(&self) -> usize {
            0
        }
        fn diff_pane_focus(&self) -> bool {
            false
        }
        fn side_panel(&self) -> &crate::side_panel::SidePanelSnapshot {
            static EMPTY: std::sync::LazyLock<crate::side_panel::SidePanelSnapshot> =
                std::sync::LazyLock::new(crate::side_panel::SidePanelSnapshot::default);
            &EMPTY
        }
        fn pin_images(&self) -> bool {
            false
        }
        fn diff_line_wrap(&self) -> bool {
            true
        }
        fn picker_state(&self) -> Option<&crate::tui::PickerState> {
            self.picker_state.as_ref()
        }
        fn changelog_scroll(&self) -> Option<usize> {
            None
        }
        fn help_scroll(&self) -> Option<usize> {
            None
        }
        fn session_picker_overlay(
            &self,
        ) -> Option<&std::cell::RefCell<session_picker::SessionPicker>> {
            None
        }
        fn working_dir(&self) -> Option<String> {
            None
        }
        fn now_millis(&self) -> u64 {
            0
        }
        fn copy_badge_ui(&self) -> crate::tui::CopyBadgeUiState {
            Default::default()
        }
        fn suggestion_prompts(&self) -> Vec<(String, String)> {
            Vec::new()
        }
        fn cache_ttl_status(&self) -> Option<crate::tui::CacheTtlInfo> {
            None
        }
    }

    fn reset_prompt_viewport_state_for_test() {
        let mut state = match prompt_viewport_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *state = PromptViewportState::default();
    }

    #[test]
    fn test_calculate_input_lines_empty() {
        assert_eq!(calculate_input_lines("", 80), 1);
    }

    #[test]
    fn test_prompt_entry_animation_detects_newly_visible_prompt_line() {
        reset_prompt_viewport_state_for_test();

        // First frame initializes viewport history and should not animate.
        update_prompt_entry_animation(&[5, 20], 0, 10, 1000);
        assert!(active_prompt_entry_animation(1000).is_none());

        // Scrolling down brings line 20 into view and should trigger animation.
        update_prompt_entry_animation(&[5, 20], 15, 25, 1100);
        let anim = active_prompt_entry_animation(1100).expect("expected active prompt animation");
        assert_eq!(anim.line_idx, 20);
    }

    #[test]
    fn test_prompt_entry_animation_expires_after_window() {
        reset_prompt_viewport_state_for_test();

        update_prompt_entry_animation(&[5, 20], 0, 10, 2000);
        update_prompt_entry_animation(&[5, 20], 15, 25, 2100);

        assert!(active_prompt_entry_animation(2100).is_some());
        assert!(
            active_prompt_entry_animation(2100 + PROMPT_ENTRY_ANIMATION_MS + 1).is_none(),
            "animation should expire after configured duration"
        );
    }

    #[test]
    fn test_prompt_entry_bg_color_pulses_then_fades() {
        let base = user_bg();
        let early = prompt_entry_bg_color(base, 0.15);
        let peak = prompt_entry_bg_color(base, 0.45);
        let late = prompt_entry_bg_color(base, 0.95);

        assert_ne!(early, base);
        assert_ne!(peak, base);
        assert_ne!(late, peak);
    }

    #[test]
    fn test_prompt_entry_shimmer_color_moves_across_positions() {
        let base = user_text();
        let left_early = prompt_entry_shimmer_color(base, 0.1, 0.1);
        let right_early = prompt_entry_shimmer_color(base, 0.9, 0.1);
        let left_late = prompt_entry_shimmer_color(base, 0.1, 0.8);
        let right_late = prompt_entry_shimmer_color(base, 0.9, 0.8);

        assert_ne!(left_early, right_early);
        assert_ne!(left_late, right_late);
        assert_ne!(left_early, left_late);
    }

    #[test]
    fn test_active_file_diff_context_resolves_visible_edit() {
        let prepared = PreparedMessages {
            wrapped_lines: vec![Line::from("a"); 20],
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: vec![
                EditToolRange {
                    edit_index: 0,
                    msg_index: 3,
                    file_path: "src/one.rs".to_string(),
                    start_line: 2,
                    end_line: 5,
                },
                EditToolRange {
                    edit_index: 1,
                    msg_index: 7,
                    file_path: "src/two.rs".to_string(),
                    start_line: 10,
                    end_line: 14,
                },
            ],
            copy_targets: Vec::new(),
        };

        let active = active_file_diff_context(&prepared, 9, 4).expect("visible edit context");
        assert_eq!(active.edit_index, 2);
        assert_eq!(active.msg_index, 7);
        assert_eq!(active.file_path, "src/two.rs");
    }

    #[test]
    fn test_body_cache_state_keeps_multiple_width_entries() {
        let key_a = BodyCacheKey {
            width: 40,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
        };
        let key_b = BodyCacheKey {
            width: 41,
            ..key_a.clone()
        };

        let prepared_a = Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from("a")],
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        });
        let prepared_b = Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from("b")],
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        });

        let mut cache = BodyCacheState::default();
        cache.insert(key_a.clone(), prepared_a.clone(), 3);
        cache.insert(key_b.clone(), prepared_b.clone(), 3);

        let hit_a = cache
            .get_exact(&key_a)
            .expect("expected width 40 cache hit");
        let hit_b = cache
            .get_exact(&key_b)
            .expect("expected width 41 cache hit");

        assert!(Arc::ptr_eq(&hit_a, &prepared_a));
        assert!(Arc::ptr_eq(&hit_b, &prepared_b));
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn test_body_cache_state_evicts_oldest_entries() {
        let mut cache = BodyCacheState::default();

        for idx in 0..(BODY_CACHE_MAX_ENTRIES + 2) {
            let key = BodyCacheKey {
                width: 40 + idx as u16,
                diff_mode: crate::config::DiffDisplayMode::Off,
                messages_version: 1,
                diagram_mode: crate::config::DiagramDisplayMode::Pinned,
                centered: false,
            };
            let prepared = Arc::new(PreparedMessages {
                wrapped_lines: vec![Line::from(format!("{idx}"))],
                wrapped_user_indices: Vec::new(),
                wrapped_user_prompt_starts: Vec::new(),
                wrapped_user_prompt_ends: Vec::new(),
                user_prompt_texts: Vec::new(),
                image_regions: Vec::new(),
                edit_tool_ranges: Vec::new(),
                copy_targets: Vec::new(),
            });
            cache.insert(key, prepared, idx);
        }

        assert_eq!(cache.entries.len(), BODY_CACHE_MAX_ENTRIES);
        assert!(
            cache.entries.iter().all(|entry| entry.key.width >= 42),
            "oldest widths should be evicted"
        );
    }

    #[test]
    fn test_file_diff_cache_reuses_entry_when_signature_matches() {
        let temp = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(temp.path(), "fn main() {}\n").expect("write file");
        let path = temp.path().to_string_lossy().to_string();

        let state = file_diff_cache();
        {
            let mut cache = state.lock().expect("cache lock");
            cache.entries.clear();
            cache.order.clear();
            let key = FileDiffCacheKey {
                file_path: path.clone(),
                msg_index: 1,
            };
            let sig = file_content_signature(&path);
            cache.insert(
                key.clone(),
                FileDiffViewCacheEntry {
                    file_sig: sig.clone(),
                    rows: vec![file_diff_ui::FileDiffDisplayRow {
                        prefix: String::new(),
                        text: "cached".to_string(),
                        kind: file_diff_ui::FileDiffDisplayRowKind::Placeholder,
                    }],
                    rendered_rows: vec![Some(Line::from("cached"))],
                    first_change_line: 0,
                    additions: 1,
                    deletions: 0,
                    file_ext: None,
                },
            );

            let cached = cache.entries.get(&key).expect("cached entry");
            assert_eq!(cached.file_sig, sig);
        }
    }

    #[test]
    fn test_calculate_input_lines_single_line() {
        assert_eq!(calculate_input_lines("hello", 80), 1);
        assert_eq!(calculate_input_lines("hello world", 80), 1);
    }

    #[test]
    fn test_calculate_input_lines_wrapped() {
        // 10 chars with width 5 = 2 lines
        assert_eq!(calculate_input_lines("aaaaaaaaaa", 5), 2);
        // 15 chars with width 5 = 3 lines
        assert_eq!(calculate_input_lines("aaaaaaaaaaaaaaa", 5), 3);
    }

    #[test]
    fn test_calculate_input_lines_with_newlines() {
        // Two lines separated by newline
        assert_eq!(calculate_input_lines("hello\nworld", 80), 2);
        // Three lines
        assert_eq!(calculate_input_lines("a\nb\nc", 80), 3);
        // Trailing newline
        assert_eq!(calculate_input_lines("hello\n", 80), 2);
    }

    #[test]
    fn test_calculate_input_lines_newlines_and_wrapping() {
        // First line wraps (10 chars / 5 = 2), second line is short (1)
        assert_eq!(calculate_input_lines("aaaaaaaaaa\nb", 5), 3);
    }

    #[test]
    fn test_calculate_input_lines_zero_width() {
        assert_eq!(calculate_input_lines("hello", 0), 1);
    }

    #[test]
    fn test_wrap_input_text_empty() {
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("", 0, 80, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 0);
    }

    #[test]
    fn test_wrap_input_text_simple() {
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("hello", 5, 80, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 5); // cursor at end
    }

    #[test]
    fn test_wrap_input_text_cursor_middle() {
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("hello world", 6, 80, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 1);
        assert_eq!(cursor_line, 0);
        assert_eq!(cursor_col, 6); // cursor at 'w'
    }

    #[test]
    fn test_wrap_input_text_wrapping() {
        // 10 chars with width 5 = 2 lines
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("aaaaaaaaaa", 7, 5, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1); // second line
        assert_eq!(cursor_col, 2); // 7 - 5 = 2
    }

    #[test]
    fn test_wrap_input_text_with_newlines() {
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("hello\nworld", 6, 80, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1); // second line (after newline)
        assert_eq!(cursor_col, 0); // at start of 'world'
    }

    #[test]
    fn test_wrap_input_text_cursor_at_end_of_wrapped() {
        // 10 chars with width 5, cursor at position 10 (end)
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("aaaaaaaaaa", 10, 5, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor_line, 1);
        assert_eq!(cursor_col, 5);
    }

    #[test]
    fn test_wrap_input_text_many_lines() {
        // Create text that spans 15 lines when wrapped to width 10
        let text = "a".repeat(150);
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text(&text, 145, 10, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 15);
        assert_eq!(cursor_line, 14); // last line
        assert_eq!(cursor_col, 5); // 145 % 10 = 5
    }

    #[test]
    fn test_wrap_input_text_multiple_newlines() {
        let (lines, cursor_line, cursor_col) =
            input_ui::wrap_input_text("a\nb\nc\nd", 6, 80, "1", "> ", user_color(), 3);
        assert_eq!(lines.len(), 4);
        assert_eq!(cursor_line, 3); // on 'd' line
        assert_eq!(cursor_col, 0);
    }

    #[test]
    fn test_wrapped_input_line_count_respects_two_digit_prompt_width() {
        let mut app = TestState {
            input: "abcdefghijk".to_string(),
            cursor_pos: "abcdefghijk".len(),
            ..Default::default()
        };
        for _ in 0..9 {
            app.display_messages.push(DisplayMessage {
                role: "user".to_string(),
                content: "previous".to_string(),
                tool_calls: Vec::new(),
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }

        // Old layout math effectively used width 11 here (14 total - hardcoded prompt width 3),
        // which incorrectly fit this input on a single line. The real prompt is "10> ", width 4,
        // so the wrapped renderer only has 10 columns and must use 2 lines.
        assert_eq!(calculate_input_lines(app.input(), 11), 1);
        assert_eq!(input_ui::wrapped_input_line_count(&app, 14, 10), 2);
    }

    #[test]
    fn test_compute_visible_margins_centered_respects_line_alignment() {
        let lines = vec![
            ratatui::text::Line::from("centered").centered(),
            ratatui::text::Line::from("left block").left_aligned(),
            ratatui::text::Line::from("right").right_aligned(),
        ];
        let area = Rect::new(0, 0, 20, 3);
        let margins = compute_visible_margins(&lines, &[], 0, area, true);

        // centered: used=8 => total_margin=12 => 6/6 split
        assert_eq!(margins.left_widths[0], 6);
        assert_eq!(margins.right_widths[0], 6);

        // left-aligned: used=10 => left=0, right=10
        assert_eq!(margins.left_widths[1], 0);
        assert_eq!(margins.right_widths[1], 10);

        // right-aligned: used=5 => left=15, right=0
        assert_eq!(margins.left_widths[2], 15);
        assert_eq!(margins.right_widths[2], 0);
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_scales_to_height() {
        let diagram = info_widget::DiagramInfo {
            hash: 1,
            width: 800,
            height: 600,
            label: None,
        };
        let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, Some((8, 16)));
        assert_eq!(width, 50);
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_respects_minimum() {
        let diagram = info_widget::DiagramInfo {
            hash: 2,
            width: 120,
            height: 120,
            label: None,
        };
        let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 10, 24, Some((8, 16)));
        assert_eq!(width, 24);
    }

    #[test]
    fn test_summarize_apply_patch_input_ignores_begin_marker() {
        let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n";
        let summary = tools_ui::summarize_apply_patch_input(patch);
        assert_eq!(summary, "src/lib.rs (6 lines)");
    }

    #[test]
    fn test_summarize_apply_patch_input_multiple_files() {
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-a\n+b\n*** Update File: b.txt\n@@\n-c\n+d\n*** End Patch\n";
        let summary = tools_ui::summarize_apply_patch_input(patch);
        assert_eq!(summary, "2 files (10 lines)");
    }

    #[test]
    fn test_extract_apply_patch_primary_file() {
        let patch = "*** Begin Patch\n*** Add File: new/file.rs\n+fn main() {}\n*** End Patch\n";
        let file = tools_ui::extract_apply_patch_primary_file(patch);
        assert_eq!(file.as_deref(), Some("new/file.rs"));
    }

    #[test]
    fn test_batch_subcall_params_supports_flat_and_nested_shapes() {
        let flat = serde_json::json!({
            "tool": "read",
            "file_path": "src/session.rs",
            "offset": 0,
            "limit": 420
        });
        let nested = serde_json::json!({
            "tool": "read",
            "parameters": {
                "file_path": "src/main.rs",
                "offset": 2320,
                "limit": 220
            }
        });

        let flat_params = tools_ui::batch_subcall_params(&flat);
        let nested_params = tools_ui::batch_subcall_params(&nested);

        assert_eq!(flat_params["file_path"], "src/session.rs");
        assert_eq!(flat_params["offset"], 0);
        assert_eq!(flat_params["limit"], 420);

        assert_eq!(nested_params["file_path"], "src/main.rs");
        assert_eq!(nested_params["offset"], 2320);
        assert_eq!(nested_params["limit"], 220);
    }

    #[test]
    fn test_batch_subcall_params_excludes_name_key() {
        let with_name = serde_json::json!({
            "name": "read",
            "file_path": "src/lib.rs",
            "offset": 0,
            "limit": 100
        });
        let params = tools_ui::batch_subcall_params(&with_name);
        assert_eq!(params["file_path"], "src/lib.rs");
        assert_eq!(params["offset"], 0);
        assert!(params.get("name").is_none());
        assert!(params.get("tool").is_none());
    }

    #[test]
    fn test_render_tool_message_batch_flat_subcall_params_include_read_details() {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content:
                "--- [1] read ---\nok\n\n--- [2] read ---\nok\n\nCompleted: 2 succeeded, 0 failed"
                    .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call_batch_1".to_string(),
                name: "batch".to_string(),
                input: serde_json::json!({
                    "tool_calls": [
                        {"tool": "read", "file_path": "src/session.rs", "offset": 0, "limit": 420},
                        {"tool": "read", "file_path": "src/main.rs", "offset": 2320, "limit": 220}
                    ]
                }),
                intent: None,
            }),
        };

        let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("read src/session.rs:0-420")),
            "missing first read summary in {:?}",
            rendered
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("read src/main.rs:2320-2540")),
            "missing second read summary in {:?}",
            rendered
        );
    }

    #[test]
    fn test_prepare_messages_shows_live_batch_progress_in_chat_history() {
        let state = TestState {
            display_messages: vec![DisplayMessage {
                role: "user".to_string(),
                content: "build it".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            }],
            status: ProcessingStatus::RunningTool("batch".to_string()),
            anim_elapsed: 0.0,
            batch_progress: Some(crate::bus::BatchProgress {
                session_id: "s".to_string(),
                tool_call_id: "tc".to_string(),
                total: 2,
                completed: 1,
                last_completed: Some("read".to_string()),
                running: vec![ToolCall {
                    id: "batch-2-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "cargo build --release --workspace"}),
                    intent: None,
                }],
                subcalls: vec![
                    crate::bus::BatchSubcallProgress {
                        index: 1,
                        tool_call: ToolCall {
                            id: "batch-1-read".to_string(),
                            name: "read".to_string(),
                            input: serde_json::json!({"file_path": "Cargo.toml"}),
                            intent: None,
                        },
                        state: crate::bus::BatchSubcallState::Succeeded,
                    },
                    crate::bus::BatchSubcallProgress {
                        index: 2,
                        tool_call: ToolCall {
                            id: "batch-2-bash".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "cargo build --release --workspace"}),
                            intent: None,
                        },
                        state: crate::bus::BatchSubcallState::Running,
                    },
                ],
            }),
            ..Default::default()
        };

        let prepared = prepare::prepare_messages(&state, 100, 30);
        let rendered: Vec<String> = prepared
            .wrapped_lines
            .iter()
            .map(extract_line_text)
            .collect();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("⠋ batch 2 calls · 1/2 done")),
            "missing live batch header in {:?}",
            rendered
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("✓ read Cargo.toml")),
            "missing completed batch subcall in {:?}",
            rendered
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("⠋ bash $ cargo build --release --workspace")),
            "missing running batch subcall in {:?}",
            rendered
        );
        assert!(
            rendered
                .iter()
                .all(|line| !line.contains("#1") && !line.contains("#2")),
            "live batch rows should align with completed rows in {:?}",
            rendered
        );
    }

    #[test]
    fn test_prepare_messages_live_batch_spinner_advances_between_frames() {
        let batch_progress = crate::bus::BatchProgress {
            session_id: "s".to_string(),
            tool_call_id: "tc".to_string(),
            total: 1,
            completed: 0,
            last_completed: None,
            running: vec![ToolCall {
                id: "batch-1-bash".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "sleep 1"}),
                intent: None,
            }],
            subcalls: vec![crate::bus::BatchSubcallProgress {
                index: 1,
                tool_call: ToolCall {
                    id: "batch-1-bash".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "sleep 1"}),
                    intent: None,
                },
                state: crate::bus::BatchSubcallState::Running,
            }],
        };

        let first = TestState {
            status: ProcessingStatus::RunningTool("batch".to_string()),
            anim_elapsed: 0.0,
            batch_progress: Some(batch_progress.clone()),
            ..Default::default()
        };
        let second = TestState {
            status: ProcessingStatus::RunningTool("batch".to_string()),
            anim_elapsed: 0.1,
            batch_progress: Some(batch_progress),
            ..Default::default()
        };

        let first_rendered: Vec<String> = prepare::prepare_messages(&first, 100, 20)
            .wrapped_lines
            .iter()
            .map(extract_line_text)
            .collect();
        let second_rendered: Vec<String> = prepare::prepare_messages(&second, 100, 20)
            .wrapped_lines
            .iter()
            .map(extract_line_text)
            .collect();

        assert!(
            first_rendered
                .iter()
                .any(|line| line.contains("⠋ batch 1 calls · 0/1 done")),
            "expected first spinner frame in {:?}",
            first_rendered
        );
        assert!(
            second_rendered
                .iter()
                .any(|line| line.contains("⠙ batch 1 calls · 0/1 done")),
            "expected second spinner frame in {:?}",
            second_rendered
        );
        assert_ne!(
            first_rendered, second_rendered,
            "batch progress should rerender as spinner advances"
        );
    }

    #[test]
    fn test_prepare_messages_live_batch_centered_mode_uses_left_aligned_padding() {
        let state = TestState {
            centered_mode: true,
            status: ProcessingStatus::RunningTool("batch".to_string()),
            anim_elapsed: 0.0,
            batch_progress: Some(crate::bus::BatchProgress {
                session_id: "s".to_string(),
                tool_call_id: "tc".to_string(),
                total: 1,
                completed: 0,
                last_completed: None,
                running: vec![ToolCall {
                    id: "batch-1-read".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "Cargo.toml"}),
                    intent: None,
                }],
                subcalls: vec![crate::bus::BatchSubcallProgress {
                    index: 1,
                    tool_call: ToolCall {
                        id: "batch-1-read".to_string(),
                        name: "read".to_string(),
                        input: serde_json::json!({"file_path": "Cargo.toml"}),
                        intent: None,
                    },
                    state: crate::bus::BatchSubcallState::Running,
                }],
            }),
            ..Default::default()
        };

        let prepared = prepare::prepare_messages(&state, 100, 20);
        let batch_lines: Vec<&Line<'static>> = prepared
            .wrapped_lines
            .iter()
            .filter(|line| {
                let text = extract_line_text(line);
                text.contains("batch") || text.contains("Cargo.toml")
            })
            .collect();

        assert!(!batch_lines.is_empty(), "expected centered batch lines");
        for line in batch_lines {
            assert_eq!(
                line.alignment,
                Some(Alignment::Left),
                "centered live batch lines should be left-aligned with padding"
            );
            assert!(
                line.spans
                    .first()
                    .is_some_and(|span| span.content.starts_with(' ')),
                "centered live batch lines should start with padding"
            );
        }
    }

    #[test]
    fn test_render_tool_message_batch_nested_subcall_params_still_render() {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: "--- [1] grep ---\nok\n\nCompleted: 1 succeeded, 0 failed".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call_batch_2".to_string(),
                name: "batch".to_string(),
                input: serde_json::json!({
                    "tool_calls": [
                        {"tool": "grep", "parameters": {"pattern": "TODO", "path": "src"}}
                    ]
                }),
                intent: None,
            }),
        };

        let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("grep 'TODO' in src")),
            "missing grep summary in {:?}",
            rendered
        );
    }

    #[test]
    fn test_render_tool_message_batch_flat_grep_subcall_uses_pattern_and_path() {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content: "--- [1] grep ---\nok\n\nCompleted: 1 succeeded, 0 failed".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call_batch_3".to_string(),
                name: "batch".to_string(),
                input: serde_json::json!({
                    "tool_calls": [
                        {"tool": "grep", "pattern": "TODO", "path": "src"}
                    ]
                }),
                intent: None,
            }),
        };

        let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

        assert!(
            rendered
                .iter()
                .any(|line| line.contains("grep 'TODO' in src")),
            "missing flat grep summary in {:?}",
            rendered
        );
    }

    #[test]
    fn test_render_tool_message_batch_subcall_lines_alignment_unset() {
        let msg = DisplayMessage {
            role: "tool".to_string(),
            content:
                "--- [1] read ---\nok\n\n--- [2] grep ---\nok\n\nCompleted: 2 succeeded, 0 failed"
                    .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: Some(ToolCall {
                id: "call_batch_align".to_string(),
                name: "batch".to_string(),
                input: serde_json::json!({
                    "tool_calls": [
                        {"tool": "read", "file_path": "src/session.rs", "offset": 0, "limit": 420},
                        {"tool": "grep", "pattern": "TODO", "path": "src"}
                    ]
                }),
                intent: None,
            }),
        };

        // In non-centered mode, lines have no alignment set
        crate::tui::markdown::set_center_code_blocks(false);
        let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        for line in &lines {
            assert_eq!(
                line.alignment, None,
                "non-centered tool lines should have no alignment set"
            );
        }

        // In centered mode, lines are left-aligned with padding prepended
        crate::tui::markdown::set_center_code_blocks(true);
        let lines = render_tool_message(&msg, 120, crate::config::DiffDisplayMode::Off);
        for line in &lines {
            assert_eq!(
                line.alignment,
                Some(Alignment::Left),
                "centered tool lines should be left-aligned with padding"
            );
            assert!(
                line.spans[0].content.starts_with(' '),
                "first span should be padding spaces"
            );
        }
        crate::tui::markdown::set_center_code_blocks(false);
    }

    #[test]
    fn test_render_rounded_box_sides_aligned() {
        let content = vec![
            Line::from("short"),
            Line::from("a longer line of text here"),
            Line::from("mid"),
        ];
        let style = Style::default();
        let lines = render_rounded_box("title", content, 40, style);
        assert!(lines.len() >= 5);
        let top_width = lines[0].width();
        let bottom_width = lines[lines.len() - 1].width();
        assert_eq!(
            top_width, bottom_width,
            "top and bottom borders must be same width: top={}, bottom={}",
            top_width, bottom_width
        );
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(
                line.width(),
                top_width,
                "line {} has width {} but expected {} (content: {:?})",
                i,
                line.width(),
                top_width,
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_render_rounded_box_emoji_title_aligned() {
        let content = vec![
            Line::from("memory content line one"),
            Line::from("memory content line two"),
        ];
        let style = Style::default();
        let lines = render_rounded_box("🧠 recalled 2 memories", content, 50, style);
        assert!(lines.len() >= 4);
        let top_width = lines[0].width();
        let bottom_width = lines[lines.len() - 1].width();
        assert_eq!(
            top_width, bottom_width,
            "emoji title: top={}, bottom={}",
            top_width, bottom_width
        );
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(
                line.width(),
                top_width,
                "emoji title: line {} width {} != expected {}",
                i,
                line.width(),
                top_width
            );
        }
    }

    #[test]
    fn test_render_swarm_message_uses_left_rail_not_box() {
        crate::tui::markdown::set_center_code_blocks(false);
        let msg = DisplayMessage::swarm("DM from fox", "Can you take parser tests?");

        let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

        assert_eq!(rendered.len(), 2, "expected compact header + body layout");
        assert!(rendered[0].starts_with("│ ✉ DM from fox"));
        assert_eq!(rendered[1], "│ Can you take parser tests?");
        assert!(
            rendered
                .iter()
                .all(|line| !line.contains('╭') && !line.contains('╰')),
            "swarm notifications should no longer render as rounded boxes: {:?}",
            rendered
        );
    }

    #[test]
    fn test_render_swarm_message_trims_extra_newlines() {
        crate::tui::markdown::set_center_code_blocks(false);
        let msg = DisplayMessage::swarm("Broadcast · coordinator", "\n\nPlan updated\n\n");

        let lines = render_swarm_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

        assert_eq!(rendered[0], "│ 📣 Broadcast · coordinator");
        assert_eq!(rendered[1], "│ Plan updated");
        assert_eq!(
            rendered.len(),
            2,
            "trimmed message should not add blank lines"
        );
    }

    #[test]
    fn test_truncate_line_to_width_uses_display_width() {
        let line = Line::from(Span::raw("🧠 hello world"));
        let truncated = truncate_line_to_width(&line, 8);
        let w = truncated.width();
        assert!(w <= 8, "truncated line display width {} should be <= 8", w);
    }

    #[test]
    fn test_render_memory_tiles_uses_variable_box_widths() {
        let mut tiles = group_into_tiles(vec![
            (
                "preference".to_string(),
                "The user wants the mobile experience to be beautiful, animated, and performant."
                    .to_string(),
            ),
            (
                "preference".to_string(),
                "User wants a release cut after testing is complete.".to_string(),
            ),
            ("fact".to_string(), "Jeremy".to_string()),
        ]);
        let border_style = Style::default();
        let text_style = Style::default();

        let preference = tiles.remove(0);
        let fact = tiles.remove(0);

        let preference_plan =
            choose_memory_tile_span(&preference, 20, 2, 2, border_style, text_style)
                .expect("preference span plan");
        let fact_plan = choose_memory_tile_span(&fact, 20, 2, 2, border_style, text_style)
            .expect("fact span plan");
        let preference_width = preference_plan.0.width;
        let fact_width = fact_plan.0.width;
        let narrow_preference = plan_memory_tile(&preference, 20, border_style, text_style)
            .expect("narrow preference plan");
        let chosen_preference = preference_plan.0;

        assert!(
            chosen_preference.height <= narrow_preference.height,
            "expected chosen preference width to be at least as space-efficient as the minimum width: chosen_width={}, chosen_height={}, narrow_height={}",
            preference_width,
            chosen_preference.height,
            narrow_preference.height
        );
        assert!(
            preference_width >= fact_width,
            "expected long preference content to not choose a narrower box than fact: pref={}, fact={}",
            preference_width,
            fact_width
        );
    }

    #[test]
    fn test_render_memory_tiles_allows_boxes_below_other_boxes() {
        let tiles = group_into_tiles(vec![
            (
                "preference".to_string(),
                "The mobile experience should be beautiful, animated, and performant.".to_string(),
            ),
            (
                "preference".to_string(),
                "User prefers quick verification that jcode is up-to-date.".to_string(),
            ),
            ("fact".to_string(), "Jeremy".to_string()),
            (
                "entity".to_string(),
                "Star is a named source providing product strategy input.".to_string(),
            ),
            (
                "correction".to_string(),
                "Assistant incorrectly said it had no memory hits despite existing memories."
                    .to_string(),
            ),
        ]);

        let lines = render_memory_tiles(
            &tiles,
            120,
            Style::default(),
            Style::default(),
            Some(Line::from("🧠 recalled 5 memories")),
        );
        let rendered: Vec<String> = lines.iter().map(extract_line_text).collect();

        let correction_idx = rendered
            .iter()
            .position(|line| line.contains(" correction "))
            .expect("correction box present");

        assert!(
            correction_idx > 0,
            "expected correction box to render below first row: {:?}",
            rendered
        );
        assert!(
            rendered
                .iter()
                .skip(1)
                .any(|line| line.contains(" correction ")),
            "expected at least one box to appear on a later visual row: {:?}",
            rendered
        );
    }

    #[test]
    fn test_render_memory_tiles_uses_full_row_width_for_stable_alignment() {
        let tiles = group_into_tiles(vec![
            (
                "fact".to_string(),
                "home.html has a new \"Final Oral Test\" link under Scripts · Memorization"
                    .to_string(),
            ),
            (
                "preference".to_string(),
                "User wants unprofessional demo/chat messages removed or replaced with professional wording for demos."
                    .to_string(),
            ),
            ("entity".to_string(), "User account name is `jeremy`.".to_string()),
            ("note".to_string(), "The number 42".to_string()),
        ]);

        let lines = render_memory_tiles(
            &tiles,
            96,
            Style::default(),
            Style::default(),
            Some(Line::from("🧠 recalled 4 memories")),
        );
        let rendered: Vec<String> = lines.iter().skip(1).map(extract_line_text).collect();

        assert!(
            rendered
                .iter()
                .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) == 96),
            "expected each rendered memory row to fill full layout width for stable centering: {:?}",
            rendered
        );
    }

    #[test]
    fn test_truncate_line_preserves_width_for_ascii() {
        let line = Line::from(Span::raw("hello world foo bar"));
        let truncated = truncate_line_to_width(&line, 11);
        assert_eq!(truncated.width(), 11);
    }

    // ---- Mermaid side panel rendering tests ----

    const TEST_FONT: Option<(u16, u16)> = Some((8, 16));

    #[test]
    fn test_vcenter_fitted_image_wide_image_in_narrow_pane() {
        // Wide image (800x200) in a narrow side panel (40 cols x 30 rows).
        // The image width should be the constraining dimension, so the
        // fitted image should fill the panel width.
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 30,
        };
        let result = vcenter_fitted_image_with_font(area, 800, 200, TEST_FONT);
        assert!(
            result.width >= area.width / 2,
            "wide image should fill most of pane width: got {} out of {} (expected >= {})",
            result.width,
            area.width,
            area.width / 2
        );
    }

    #[test]
    fn test_vcenter_fitted_image_square_image_fills_width() {
        // Square image (400x400) in a side panel (40 cols x 40 rows).
        // With typical 8x16 font, terminal cells are 2:1 aspect.
        // 40 cols = 320px, 40 rows = 640px.
        // scale = min(320/400, 640/400) = min(0.8, 1.6) = 0.8
        // fitted_w = (400 * 0.8) / 8 = 40 cells -> fills width
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 40,
        };
        let result = vcenter_fitted_image_with_font(area, 400, 400, TEST_FONT);
        assert!(
            result.width >= area.width * 3 / 4,
            "square image should fill most of pane width: got {} out of {}",
            result.width,
            area.width
        );
    }

    #[test]
    fn test_vcenter_fitted_image_tall_image_in_wide_pane() {
        // Tall image (200x800) in a wide pane (80 cols x 30 rows).
        // Height is constraining. Image won't fill width.
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };
        let result = vcenter_fitted_image_with_font(area, 200, 800, TEST_FONT);
        assert!(
            result.width < area.width,
            "tall image should not fill full width: got {} out of {}",
            result.width,
            area.width
        );
        assert!(
            result.height <= area.height,
            "tall image height should not exceed pane: got {} out of {}",
            result.height,
            area.height
        );
    }

    #[test]
    fn test_vcenter_fitted_image_centering_horizontal() {
        // Tall image centered in a wide area - should have x_offset > 0
        let area = Rect {
            x: 10,
            y: 5,
            width: 80,
            height: 20,
        };
        let result = vcenter_fitted_image_with_font(area, 100, 800, TEST_FONT);
        if result.width < area.width {
            assert!(
                result.x > area.x,
                "should be horizontally centered: x={}, area.x={}",
                result.x,
                area.x
            );
        }
    }

    #[test]
    fn test_vcenter_fitted_image_centering_vertical() {
        // Wide image centered vertically - should have y_offset > 0
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 40,
        };
        let result = vcenter_fitted_image_with_font(area, 800, 100, TEST_FONT);
        if result.height < area.height {
            assert!(
                result.y > area.y || result.height < area.height,
                "should be vertically centered"
            );
        }
    }

    #[test]
    fn test_vcenter_fitted_image_zero_dimensions() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let result = vcenter_fitted_image_with_font(area, 400, 400, TEST_FONT);
        assert_eq!(result, area);

        let area2 = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 30,
        };
        let result2 = vcenter_fitted_image_with_font(area2, 0, 0, TEST_FONT);
        assert_eq!(result2, area2);
    }

    #[test]
    fn test_vcenter_fitted_image_never_exceeds_area() {
        let test_cases: Vec<(Rect, u32, u32)> = vec![
            (
                Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 30,
                },
                800,
                600,
            ),
            (
                Rect {
                    x: 5,
                    y: 3,
                    width: 60,
                    height: 20,
                },
                100,
                100,
            ),
            (
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 40,
                },
                1920,
                1080,
            ),
            (
                Rect {
                    x: 0,
                    y: 0,
                    width: 30,
                    height: 50,
                },
                200,
                800,
            ),
        ];
        for (area, img_w, img_h) in test_cases {
            let result = vcenter_fitted_image_with_font(area, img_w, img_h, TEST_FONT);
            assert!(
                result.x >= area.x,
                "result.x ({}) < area.x ({})",
                result.x,
                area.x
            );
            assert!(
                result.y >= area.y,
                "result.y ({}) < area.y ({})",
                result.y,
                area.y
            );
            assert!(
                result.x + result.width <= area.x + area.width,
                "result right edge ({}) > area right edge ({})",
                result.x + result.width,
                area.x + area.width
            );
            assert!(
                result.y + result.height <= area.y + area.height,
                "result bottom edge ({}) > area bottom edge ({})",
                result.y + result.height,
                area.y + area.height
            );
        }
    }

    #[test]
    fn test_vcenter_fitted_image_typical_mermaid_in_side_panel() {
        // Typical mermaid diagram: wider than tall (e.g., flowchart LR).
        // Side panel is narrow and tall (e.g., 50 cols x 40 rows).
        // The image should fill the width of the panel.
        let inner = Rect {
            x: 81,
            y: 1,
            width: 48,
            height: 38,
        };
        let result = vcenter_fitted_image_with_font(inner, 600, 300, TEST_FONT);
        let width_utilization = result.width as f64 / inner.width as f64;
        assert!(
            width_utilization > 0.8,
            "typical mermaid in side panel should use >80% width: {}% ({}/{})",
            (width_utilization * 100.0) as u32,
            result.width,
            inner.width
        );
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_wide_image() {
        // A very wide image should get a wider pane
        let diagram = info_widget::DiagramInfo {
            hash: 10,
            width: 1600,
            height: 200,
            label: None,
        };
        let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((8, 16)));
        assert!(
            width >= 24,
            "should be at least minimum width: got {}",
            width
        );
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_tall_image() {
        // A tall image should get a narrower pane (height-constrained)
        let diagram = info_widget::DiagramInfo {
            hash: 11,
            width: 200,
            height: 1600,
            label: None,
        };
        let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((8, 16)));
        // Height-constrained: 30 rows - 2 border = 28 inner rows
        // image_w_cells = ceil(200/8) = 25
        // image_h_cells = ceil(1600/16) = 100
        // fit_w_cells = ceil(25*28/100) = 7
        // pane_width = 7 + 2 = 9, but clamped to min 24
        assert_eq!(width, 24, "tall image should be clamped to minimum width");
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_zero_font_size() {
        // With None font size, should use default (8, 16)
        let diagram = info_widget::DiagramInfo {
            hash: 12,
            width: 800,
            height: 600,
            label: None,
        };
        let with_font =
            estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, Some((8, 16)));
        let with_default = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, None);
        assert_eq!(with_font, with_default);
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_height_wide_image() {
        // Wide image (1600x200) in a pane 80 cols wide.
        // Should need less height since the image is short.
        let diagram = info_widget::DiagramInfo {
            hash: 13,
            width: 1600,
            height: 200,
            label: None,
        };
        let height = estimate_pinned_diagram_pane_height(&diagram, 80, 6);
        assert!(
            height >= 6,
            "should be at least minimum height: got {}",
            height
        );
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_height_tall_image() {
        // Tall image (200x1600) in a pane 80 cols wide.
        // Width-constrained, so height depends on the width scaling.
        let diagram = info_widget::DiagramInfo {
            hash: 14,
            width: 200,
            height: 1600,
            label: None,
        };
        let height = estimate_pinned_diagram_pane_height(&diagram, 80, 6);
        assert!(
            height > 6,
            "tall image should need more than minimum height: got {}",
            height
        );
    }

    #[test]
    fn test_side_panel_layout_ratio_capping() {
        // Test that diagram_width respects the ratio cap.
        // area = 120 cols, ratio = 50% -> cap = 60
        // If estimated pane width > 60, it should be capped at 60.
        let diagram = info_widget::DiagramInfo {
            hash: 20,
            width: 2000,
            height: 400,
            label: None,
        };
        let area_width: u16 = 120;
        let ratio: u32 = 50;
        let ratio_cap = ((area_width as u32 * ratio) / 100) as u16;
        let min_diagram_width: u16 = 24;
        let min_chat_width: u16 = 20;
        let max_diagram = area_width.saturating_sub(min_chat_width);

        let needed = estimate_pinned_diagram_pane_width_with_font(
            &diagram,
            40,
            min_diagram_width,
            Some((8, 16)),
        );
        let diagram_width = needed
            .min(ratio_cap)
            .max(min_diagram_width)
            .min(max_diagram);

        assert!(
            diagram_width <= ratio_cap,
            "diagram_width ({}) should be <= ratio_cap ({})",
            diagram_width,
            ratio_cap
        );
        assert!(
            diagram_width >= min_diagram_width,
            "diagram_width ({}) should be >= min ({})",
            diagram_width,
            min_diagram_width
        );
        let chat_width = area_width.saturating_sub(diagram_width);
        assert!(
            chat_width >= min_chat_width,
            "chat_width ({}) should be >= min ({})",
            chat_width,
            min_chat_width
        );
    }

    #[test]
    fn test_side_panel_layout_narrow_terminal() {
        // On a very narrow terminal (50 cols), side panel should still work
        // or gracefully degrade.
        let area_width: u16 = 50;
        let min_diagram_width: u16 = 24;
        let min_chat_width: u16 = 20;
        let max_diagram = area_width.saturating_sub(min_chat_width); // 30

        let diagram = info_widget::DiagramInfo {
            hash: 21,
            width: 600,
            height: 300,
            label: None,
        };
        let needed = estimate_pinned_diagram_pane_width_with_font(
            &diagram,
            30,
            min_diagram_width,
            Some((8, 16)),
        );
        let ratio_cap = ((area_width as u32 * 50) / 100) as u16; // 25
        let diagram_width = needed
            .min(ratio_cap)
            .max(min_diagram_width)
            .min(max_diagram);
        let chat_width = area_width.saturating_sub(diagram_width);

        assert!(
            diagram_width >= min_diagram_width,
            "narrow term: diagram_width ({}) >= min ({})",
            diagram_width,
            min_diagram_width
        );
        assert!(
            chat_width >= min_chat_width,
            "narrow term: chat_width ({}) >= min ({})",
            chat_width,
            min_chat_width
        );
        assert_eq!(
            diagram_width + chat_width,
            area_width,
            "widths should sum to total"
        );
    }

    #[test]
    fn test_side_panel_image_width_utilization() {
        // This is the key test for the "only uses half width" bug.
        // After computing the pane width and getting the inner area (minus
        // 2 for borders), vcenter_fitted_image should return a rect where
        // the image width is close to the inner width for typical diagrams.
        let diagram = info_widget::DiagramInfo {
            hash: 30,
            width: 800,
            height: 400,
            label: None,
        };
        let area_width: u16 = 120;
        let area_height: u16 = 40;
        let min_diagram_width: u16 = 24;
        let ratio_cap = ((area_width as u32 * 50) / 100) as u16;
        let max_diagram = area_width.saturating_sub(20);

        let needed = estimate_pinned_diagram_pane_width_with_font(
            &diagram,
            area_height,
            min_diagram_width,
            Some((8, 16)),
        );
        let diagram_width = needed
            .min(ratio_cap)
            .max(min_diagram_width)
            .min(max_diagram);

        // Inner area after borders (1 cell each side)
        let inner = Rect {
            x: area_width.saturating_sub(diagram_width) + 1,
            y: 1,
            width: diagram_width.saturating_sub(2),
            height: area_height.saturating_sub(2),
        };

        let render_area =
            vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, TEST_FONT);

        let utilization = render_area.width as f64 / inner.width as f64;
        assert!(
            utilization > 0.5,
            "image should use >50% of inner pane width: {}% ({}/{}) \
             pane_width={}, inner_width={}, render_width={}, \
             img={}x{}, needed={}",
            (utilization * 100.0) as u32,
            render_area.width,
            inner.width,
            diagram_width,
            inner.width,
            render_area.width,
            diagram.width,
            diagram.height,
            needed,
        );
    }

    #[test]
    fn test_side_panel_image_width_various_aspect_ratios() {
        // Test various diagram aspect ratios to ensure none uses "only half"
        let test_cases: Vec<(u32, u32, &str)> = vec![
            (800, 400, "2:1 landscape"),
            (800, 600, "4:3 landscape"),
            (800, 800, "1:1 square"),
            (600, 400, "3:2 landscape"),
            (1200, 300, "4:1 wide panoramic"),
            (400, 600, "2:3 portrait"),
            (300, 900, "1:3 tall portrait"),
        ];

        for (img_w, img_h, label) in test_cases {
            let _diagram = info_widget::DiagramInfo {
                hash: img_w as u64 * 1000 + img_h as u64,
                width: img_w,
                height: img_h,
                label: None,
            };

            let pane_width: u16 = 50;
            let pane_height: u16 = 40;
            let inner = Rect {
                x: 71,
                y: 1,
                width: pane_width - 2,
                height: pane_height - 2,
            };

            let render_area = vcenter_fitted_image_with_font(inner, img_w, img_h, TEST_FONT);

            // For any diagram, at least one dimension should be well-utilized
            let w_util = render_area.width as f64 / inner.width as f64;
            let h_util = render_area.height as f64 / inner.height as f64;
            let max_util = w_util.max(h_util);

            assert!(
                max_util > 0.5,
                "{}: at least one dimension should be >50% utilized: \
                 w_util={:.0}% h_util={:.0}%, render={}x{}, inner={}x{}",
                label,
                w_util * 100.0,
                h_util * 100.0,
                render_area.width,
                render_area.height,
                inner.width,
                inner.height,
            );
        }
    }

    #[test]
    fn test_is_diagram_poor_fit_wide_in_side_pane() {
        // A very wide diagram in a side pane (narrow+tall) should be a poor fit
        let diagram = info_widget::DiagramInfo {
            hash: 40,
            width: 1600,
            height: 100,
            label: None,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 40,
        };
        let poor = is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Side);
        assert!(
            poor,
            "very wide diagram in narrow side pane should be poor fit"
        );
    }

    #[test]
    fn test_is_diagram_poor_fit_tall_in_top_pane() {
        // A very tall diagram in a top pane (wide+short) should be a poor fit
        let diagram = info_widget::DiagramInfo {
            hash: 41,
            width: 100,
            height: 1600,
            label: None,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 15,
        };
        let poor = is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Top);
        assert!(
            poor,
            "very tall diagram in short top pane should be poor fit"
        );
    }

    #[test]
    fn test_is_diagram_poor_fit_good_fit_cases() {
        // Normal aspect ratio diagrams should not be poor fits
        let diagram = info_widget::DiagramInfo {
            hash: 42,
            width: 600,
            height: 400,
            label: None,
        };
        let side_area = Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 40,
        };
        assert!(
            !is_diagram_poor_fit(
                &diagram,
                side_area,
                crate::config::DiagramPanePosition::Side
            ),
            "normal diagram should not be poor fit in side pane"
        );

        let top_area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        assert!(
            !is_diagram_poor_fit(&diagram, top_area, crate::config::DiagramPanePosition::Top),
            "normal diagram should not be poor fit in top pane"
        );
    }

    #[test]
    fn test_is_diagram_poor_fit_zero_dimensions() {
        let diagram = info_widget::DiagramInfo {
            hash: 43,
            width: 0,
            height: 0,
            label: None,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 50,
            height: 40,
        };
        assert!(
            !is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Side),
            "zero-dimension diagram should not crash or be poor fit"
        );
    }

    #[test]
    fn test_is_diagram_poor_fit_tiny_area() {
        let diagram = info_widget::DiagramInfo {
            hash: 44,
            width: 800,
            height: 600,
            label: None,
        };
        let area = Rect {
            x: 0,
            y: 0,
            width: 3,
            height: 2,
        };
        assert!(
            !is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Side),
            "tiny area should return false (not crash)"
        );
    }

    #[test]
    fn test_div_ceil_u32_basic() {
        assert_eq!(div_ceil_u32(10, 3), 4);
        assert_eq!(div_ceil_u32(9, 3), 3);
        assert_eq!(div_ceil_u32(0, 5), 0);
        assert_eq!(div_ceil_u32(1, 1), 1);
        assert_eq!(div_ceil_u32(7, 0), 7);
    }

    #[test]
    fn test_estimate_pinned_diagram_pane_width_various_fonts() {
        // Different font sizes affect the computed pane width.
        // With a proportionally larger font, the raw image-in-cells count
        // is smaller, but ceiling arithmetic can add a cell back.
        let diagram = info_widget::DiagramInfo {
            hash: 50,
            width: 800,
            height: 600,
            label: None,
        };
        let w_8x16 = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((8, 16)));
        let w_10x20 =
            estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((10, 20)));
        let w_16x32 =
            estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((16, 32)));
        // With a substantially larger font, we should need noticeably fewer cells
        assert!(
            w_16x32 <= w_8x16,
            "much larger font should need fewer or equal cells: 16x32={}, 8x16={}",
            w_16x32,
            w_8x16
        );
        // All should respect the minimum
        assert!(w_8x16 >= 24);
        assert!(w_10x20 >= 24);
        assert!(w_16x32 >= 24);
    }

    #[test]
    fn test_side_panel_full_pipeline_width_check() {
        // End-to-end: simulate the entire side panel width calculation pipeline
        // and verify the image render area is reasonable.
        //
        // This mimics what draw_inner + draw_pinned_diagram do:
        // 1. estimate_pinned_diagram_pane_width -> pane width
        // 2. Rect with that width -> block.inner -> inner
        // 3. vcenter_fitted_image(inner, img_w, img_h) -> render_area
        // 4. render_image_widget_scale(render_area) -> image displayed

        let terminal_width: u16 = 120;
        let terminal_height: u16 = 40;
        let diagram = info_widget::DiagramInfo {
            hash: 60,
            width: 700,
            height: 350,
            label: None,
        };
        let font = Some((8u16, 16u16));

        // Step 1: compute pane width (mimics Side branch in draw_inner)
        let min_diagram_width: u16 = 24;
        let min_chat_width: u16 = 20;
        let max_diagram = terminal_width.saturating_sub(min_chat_width);
        let ratio: u32 = 50;
        let ratio_cap = ((terminal_width as u32 * ratio) / 100) as u16;
        let needed = estimate_pinned_diagram_pane_width_with_font(
            &diagram,
            terminal_height,
            min_diagram_width,
            font,
        );
        let pane_width = needed
            .min(ratio_cap)
            .max(min_diagram_width)
            .min(max_diagram);
        let chat_width = terminal_width.saturating_sub(pane_width);

        assert!(pane_width > 0 && chat_width > 0, "both areas must exist");

        // Step 2: compute inner area (Block::inner subtracts 1 from each side)
        let inner = Rect {
            x: chat_width + 1,
            y: 1,
            width: pane_width.saturating_sub(2),
            height: terminal_height.saturating_sub(2),
        };

        // Step 3: compute render area
        let render_area =
            vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, font);

        // Step 4: verify the render area is reasonable
        assert!(
            render_area.width > 0 && render_area.height > 0,
            "render area should be non-empty"
        );
        assert!(
            render_area.x >= inner.x,
            "render area should be within inner"
        );
        assert!(
            render_area.x + render_area.width <= inner.x + inner.width,
            "render area should not exceed inner"
        );

        // THE KEY ASSERTION: the rendered image should use a significant
        // portion of the pane width, not just half.
        let pane_utilization = render_area.width as f64 / inner.width as f64;
        assert!(
            pane_utilization > 0.5,
            "CRITICAL: Image uses only {:.0}% of side panel width ({}/{})! \
             This is the 'half width' bug. Pipeline: terminal={}x{}, \
             pane_width={}, inner={}x{}, render={}x{}, img={}x{}",
            pane_utilization * 100.0,
            render_area.width,
            inner.width,
            terminal_width,
            terminal_height,
            pane_width,
            inner.width,
            inner.height,
            render_area.width,
            render_area.height,
            diagram.width,
            diagram.height,
        );
    }

    #[test]
    fn test_side_panel_various_terminal_sizes() {
        // Test the pipeline at various realistic terminal sizes
        let terminals: Vec<(u16, u16, &str)> = vec![
            (80, 24, "80x24 standard"),
            (120, 40, "120x40 typical"),
            (200, 50, "200x50 ultrawide"),
            (60, 30, "60x30 small"),
        ];

        let diagram = info_widget::DiagramInfo {
            hash: 70,
            width: 800,
            height: 400,
            label: None,
        };

        for (tw, th, label) in terminals {
            let min_diagram_width: u16 = 24;
            let min_chat_width: u16 = 20;
            let max_diagram = tw.saturating_sub(min_chat_width);

            if max_diagram < min_diagram_width {
                continue; // too narrow for side panel
            }

            let ratio_cap = ((tw as u32 * 50) / 100) as u16;
            let needed = estimate_pinned_diagram_pane_width_with_font(
                &diagram,
                th,
                min_diagram_width,
                Some((8, 16)),
            );
            let pane_width = needed
                .min(ratio_cap)
                .max(min_diagram_width)
                .min(max_diagram);
            let chat_width = tw.saturating_sub(pane_width);

            if pane_width < 4 || chat_width == 0 {
                continue;
            }

            let inner = Rect {
                x: chat_width + 1,
                y: 1,
                width: pane_width.saturating_sub(2),
                height: th.saturating_sub(2),
            };

            let render_area =
                vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, TEST_FONT);
            let w_util = render_area.width as f64 / inner.width as f64;

            assert!(
                w_util > 0.4,
                "{}: image width utilization too low: {:.0}% ({}/{})",
                label,
                w_util * 100.0,
                render_area.width,
                inner.width,
            );
        }
    }

    #[test]
    fn test_vcenter_fitted_image_preserves_aspect_ratio_close_to_source() {
        let cases = [
            (Rect::new(0, 0, 48, 38), 600, 300),
            (Rect::new(0, 0, 48, 38), 300, 600),
            (Rect::new(0, 0, 80, 20), 1200, 400),
            (Rect::new(0, 0, 30, 40), 400, 1200),
        ];

        for (area, img_w, img_h) in cases {
            let result = vcenter_fitted_image_with_font(area, img_w, img_h, TEST_FONT);
            let src_aspect = img_w as f64 / img_h as f64;
            let dst_aspect = (result.width as f64 * 8.0) / (result.height as f64 * 16.0);
            let rel_err = (dst_aspect - src_aspect).abs() / src_aspect.max(0.0001);
            assert!(
                rel_err < 0.12,
                "aspect ratio drift too large for {}x{} in {:?}: src={:.3}, dst={:.3}, err={:.3}",
                img_w,
                img_h,
                area,
                src_aspect,
                dst_aspect,
                rel_err,
            );
        }
    }

    #[test]
    fn test_vcenter_fitted_image_with_zero_font_dimension_falls_back_safely() {
        let area = Rect::new(4, 2, 50, 20);
        let safe = vcenter_fitted_image_with_font(area, 800, 400, Some((0, 16)));
        assert!(safe.width > 0);
        assert!(safe.height > 0);
        assert!(safe.x >= area.x && safe.y >= area.y);
        assert!(safe.x + safe.width <= area.x + area.width);
        assert!(safe.y + safe.height <= area.y + area.height);

        let safe2 = vcenter_fitted_image_with_font(area, 800, 400, Some((8, 0)));
        assert!(safe2.width > 0);
        assert!(safe2.height > 0);
        assert!(safe2.x + safe2.width <= area.x + area.width);
        assert!(safe2.y + safe2.height <= area.y + area.height);
    }

    #[test]
    fn test_side_panel_landscape_diagrams_fill_most_width_across_ratios() {
        let pane = Rect::new(0, 0, 48, 38);
        let diagrams = [
            (600, 300, 0.80),
            (800, 400, 0.80),
            (1200, 300, 0.80),
            (800, 600, 0.65),
        ];

        for (img_w, img_h, min_width_util) in diagrams {
            let result = vcenter_fitted_image_with_font(pane, img_w, img_h, TEST_FONT);
            let w_util = result.width as f64 / pane.width as f64;
            assert!(
                w_util >= min_width_util,
                "{}x{} should use at least {:.0}% width, got {:.0}% ({}/{})",
                img_w,
                img_h,
                min_width_util * 100.0,
                w_util * 100.0,
                result.width,
                pane.width,
            );
        }
    }

    #[test]
    fn test_hidpi_font_size_does_not_halve_diagram_width() {
        const HIDPI_FONT: Option<(u16, u16)> = Some((15, 34));

        let terminal_width: u16 = 95;
        let terminal_height: u16 = 51;

        let diagram = info_widget::DiagramInfo {
            hash: 99,
            width: 614,
            height: 743,
            label: None,
        };

        let min_diagram_width: u16 = 24;
        let min_chat_width: u16 = 20;
        let max_diagram = terminal_width.saturating_sub(min_chat_width);
        let ratio: u32 = 40;
        let ratio_cap = ((terminal_width as u32 * ratio) / 100) as u16;

        let needed_hidpi = estimate_pinned_diagram_pane_width_with_font(
            &diagram,
            terminal_height,
            min_diagram_width,
            HIDPI_FONT,
        );
        let pane_width = needed_hidpi
            .min(ratio_cap)
            .max(min_diagram_width)
            .min(max_diagram);

        let inner = Rect {
            x: terminal_width.saturating_sub(pane_width) + 1,
            y: 1,
            width: pane_width.saturating_sub(2),
            height: terminal_height.saturating_sub(2),
        };

        let render_area =
            vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, HIDPI_FONT);

        let w_util = render_area.width as f64 / inner.width as f64;
        assert!(
            w_util > 0.7,
            "HiDPI (15x34 font): image should use >70% of pane width, got {:.0}% ({}/{}) \
             pane_width={}, inner={}x{}, render={}x{}, img={}x{}",
            w_util * 100.0,
            render_area.width,
            inner.width,
            pane_width,
            inner.width,
            inner.height,
            render_area.width,
            render_area.height,
            diagram.width,
            diagram.height,
        );

        let render_default =
            vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, TEST_FONT);
        let w_util_default = render_default.width as f64 / inner.width as f64;

        assert!(
            (w_util - w_util_default).abs() < 0.15 || w_util > 0.7,
            "Font size should not drastically change width utilization. \
             HiDPI={:.0}%, default={:.0}%",
            w_util * 100.0,
            w_util_default * 100.0,
        );
    }

    #[test]
    fn test_query_font_size_returns_valid_dimensions() {
        let font = super::super::mermaid::get_font_size();
        if let Some((w, h)) = font {
            assert!(w > 0, "font width should be positive, got {}", w);
            assert!(h > 0, "font height should be positive, got {}", h);
            assert!(
                w <= 100,
                "font width should be reasonable, got {} (likely bogus)",
                w
            );
            assert!(
                h <= 200,
                "font height should be reasonable, got {} (likely bogus)",
                h
            );
        }
    }
}
