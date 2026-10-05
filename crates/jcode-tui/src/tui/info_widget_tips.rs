use super::*;

const TIP_CYCLE_SECONDS: u64 = 15;
const STATUS_TIP_PERIOD_SECONDS: u64 = 90;
const STATUS_TIP_OFFSET_SECONDS: u64 = 28;
const STATUS_TIP_SHOW_SECONDS: u64 = 12;

struct Tip {
    text: String,
}

fn all_tips() -> Vec<Tip> {
    let mut tips = vec![
        "Ctrl+J / Ctrl+K to jump between user prompts (Cmd+J / Cmd+K on macOS terminals that forward Command)",
        "Ctrl+Shift+J / Ctrl+Shift+K to scroll the chat down and up one line",
        "Ctrl+G to bookmark your scroll position - press again to teleport back",
        "Swarms form automatically when multiple sessions share a repo - they coordinate plans, share context, and track file conflicts",
        "Memories are stored in a graph with semantic embeddings - recall finds related facts even if you use different words",
        "Ambient mode runs background cycles while you're away - maintaining memories, compacting context, and doing proactive work",
        "Ambient cycles can email you a summary and you can reply with directives for the next run",
        "Alt+B moves a long-running tool to the background - the agent continues and can check on it later with the `bg` tool",
        "Most terminals can be configured to copy text on highlight - no Ctrl+C needed. Check your terminal's settings for 'copy on select'",
        "Alt+G (or /diff) cycles diff mode: Off, Inline, Pinned, File. Shift+Tab cycles favorited models. Pinned shows all diffs in a side pane. File shows the full file with changes highlighted, synced to your scroll position",
    ];
    if crate::config::config().features.mermaid {
        tips.insert(3, "```mermaid code blocks render as diagrams");
    }
    // Mac keyboards label this modifier ⌥, not Alt, so rewrite hints there.
    let alt = jcode_tui_core::keybind::alt_label();
    tips.into_iter()
        .map(|text| Tip {
            text: text.replace("Alt+", &format!("{alt}+")),
        })
        .collect()
}

static TIP_STATE: Mutex<Option<(usize, Instant)>> = Mutex::new(None);

fn current_tip(_max_width: usize) -> Tip {
    let tips = all_tips();
    let mut guard = TIP_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let (idx, last) = guard.get_or_insert_with(|| (0, now));

    let should_advance = now.duration_since(*last).as_secs() >= TIP_CYCLE_SECONDS;
    if should_advance {
        *idx = (*idx + 1) % tips.len();
        *last = now;
    }

    let i = *idx % tips.len();
    drop(guard);
    Tip {
        text: tips[i].text.clone(),
    }
}

pub(crate) fn occasional_status_tip(max_width: usize, elapsed_secs: u64) -> Option<String> {
    if max_width < 16 {
        return None;
    }

    let cycle_pos = elapsed_secs % STATUS_TIP_PERIOD_SECONDS;
    let show_until = STATUS_TIP_OFFSET_SECONDS + STATUS_TIP_SHOW_SECONDS;
    if cycle_pos < STATUS_TIP_OFFSET_SECONDS || cycle_pos >= show_until {
        return None;
    }

    let prefix = "💡 ";
    let available = max_width.saturating_sub(prefix.chars().count());
    if available < 12 {
        return None;
    }

    let tip = current_tip(available);
    Some(format!(
        "{}{}",
        prefix,
        truncate_smart(&tip.text, available)
    ))
}

fn wrap_tip_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= width {
            lines.push(remaining.to_string());
            break;
        }
        let mut boundary = width.min(remaining.len());
        while boundary > 0 && !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }
        let split = remaining[..boundary].rfind(' ').unwrap_or(boundary);
        let (line, rest) = remaining.split_at(split);
        lines.push(line.to_string());
        remaining = rest.trim_start();
    }
    lines
}

/// Border layout: `💡 Did you know?` top-left, the wrapped tip in the body.
pub(super) fn render_tips_widget(inner: Rect) -> super::frame::Framed {
    let w = inner.width as usize;
    let tip = current_tip(w);
    let wrapped = wrap_tip_text(&tip.text, w);

    let lines: Vec<Line<'static>> = wrapped
        .into_iter()
        .map(|line_text| {
            Line::from(Span::styled(
                line_text,
                Style::default().fg(rgb(160, 160, 175)),
            ))
        })
        .collect();

    super::frame::Framed::body(lines).title(Line::from(vec![
        Span::styled("💡 ", Style::default().fg(rgb(255, 210, 80))),
        super::frame::label("Did you know?"),
    ]))
}
