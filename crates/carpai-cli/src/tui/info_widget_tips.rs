use super::*;

const TIP_CYCLE_SECONDS: u64 = 15;
const STATUS_TIP_PERIOD_SECONDS: u64 = 90;
const STATUS_TIP_OFFSET_SECONDS: u64 = 28;
const STATUS_TIP_SHOW_SECONDS: u64 = 12;

struct Tip {
    text: String,
}

fn all_tips() -> Vec<Tip> {
    [
        "Ctrl+J / Ctrl+K to scroll chat up and down (Cmd+J / Cmd+K on macOS terminals that forward Command)",
        "Ctrl+[ / Ctrl+] to jump between user prompts",
        "Ctrl+G to bookmark your scroll position — press again to teleport back",
        "```mermaid code blocks render as diagrams",
        "Swarms form automatically when multiple sessions share a repo — they coordinate plans, share context, and track file conflicts",
        "Memories are stored in a graph with semantic embeddings — recall finds related facts even if you use different words",
        "Ambient mode runs background cycles while you're away — maintaining memories, compacting context, and doing proactive work",
        "Ambient cycles can email you a summary and you can reply with directives for the next run",
        "Alt+B moves a long-running tool to the background — the agent continues and can check on it later with the `bg` tool",
        "Most terminals can be configured to copy text on highlight — no Ctrl+C needed. Check your terminal's settings for 'copy on select'",
        "Shift+Tab cycles diff mode: Off, Inline, Pinned, File. Pinned shows all diffs in a side pane. File shows the full file with changes highlighted, synced to your scroll position",
    ]
    .iter()
    .map(|t| Tip {
        text: t.to_string(),
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

pub(super) fn tips_widget_height(inner_width: usize) -> u16 {
    let effective_w = inner_width.saturating_sub(2);
    let tip = current_tip(effective_w);
    let lines = wrap_tip_text(&tip.text, effective_w);
    1 + lines.len() as u16
}

pub(super) fn render_tips_widget(inner: Rect) -> Vec<Line<'static>> {
    let w = inner.width.saturating_sub(2) as usize;
    let tip = current_tip(w);
    let wrapped = wrap_tip_text(&tip.text, w);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("💡 ", Style::default().fg(rgb(255, 210, 80))),
        Span::styled(
            "Did you know?",
            Style::default()
                .fg(rgb(200, 200, 210))
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    for line_text in wrapped {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line_text, Style::default().fg(rgb(160, 160, 175))),
        ]));
    }

    lines
}
