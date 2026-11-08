use super::*;

pub(crate) fn prompt_preview_lines(
    number: usize,
    text: &str,
    width: u16,
    align: ratatui::layout::Alignment,
) -> Vec<Line<'static>> {
    // The pinned preview is rebuilt every frame with the same prompt; memoize
    // the last result (keyed on every input, including theme colors) so the
    // grapheme walk over the whole prompt runs only when something changes.
    type Key = (usize, u64, usize, u16, ratatui::layout::Alignment, [Color; 4]);
    thread_local! {
        static MEMO: std::cell::RefCell<Option<(Key, Vec<Line<'static>>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let text_hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    };
    let key: Key = (
        number,
        text_hash,
        text.len(),
        width,
        align,
        [user_bg(), dim_color(), user_color(), user_text()],
    );
    if let Some(hit) = MEMO.with(|m| {
        m.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, lines)| lines.clone())
    }) {
        return hit;
    }
    let lines = prompt_preview_lines_uncached(number, text, width, align);
    MEMO.with(|m| *m.borrow_mut() = Some((key, lines.clone())));
    lines
}

fn prompt_preview_lines_uncached(
    number: usize,
    text: &str,
    width: u16,
    align: ratatui::layout::Alignment,
) -> Vec<Line<'static>> {
    use unicode_segmentation::UnicodeSegmentation;
    let text = text.trim().replace('\n', " ");
    if text.is_empty() {
        return Vec::new();
    }
    let number = number.to_string();
    let prefix = number.len() + 2;
    let content_width = usize::from(width).saturating_sub(prefix + 2);
    let style = Style::default().dim().bg(user_bg());
    let mut lines = Vec::with_capacity(2);
    let long = UnicodeWidthStr::width(text.as_str()) > content_width;
    let (head, tail) = if long {
        let budget = usize::from(width.saturating_sub(1)).saturating_sub(prefix + 4);
        let mut used = 0;
        let head: String = text
            .graphemes(true)
            .take_while(|g| {
                used += UnicodeWidthStr::width(*g);
                used <= budget
            })
            .collect();
        used = 0;
        let tail: Vec<_> = text
            .graphemes(true)
            .rev()
            .take_while(|g| {
                used += UnicodeWidthStr::width(*g);
                used <= budget
            })
            .collect();
        (
            format!("{} ...", head.trim_end()),
            Some(format!(
                "... {}",
                tail.into_iter().rev().collect::<String>().trim_start()
            )),
        )
    } else {
        (text, None)
    };
    lines.push(
        Line::from(vec![
            Span::styled(number, style.fg(dim_color())),
            Span::styled("› ", style.fg(user_color())),
            Span::styled(head, style.fg(user_text())),
        ])
        .alignment(align),
    );
    if let Some(tail) = tail {
        lines.push(
            Line::from(vec![
                Span::styled(" ".repeat(prefix), style),
                Span::styled(tail, style.fg(user_text())),
            ])
            .alignment(align),
        );
    }
    lines
        .into_iter()
        .map(|line| {
            let mut clipped = super::super::truncate_line_with_ellipsis_to_width(
                &line,
                usize::from(width.saturating_sub(1)),
            );
            clipped.alignment = Some(align);
            clipped
        })
        .collect()
}

pub(super) fn compute_prompt_preview_line_count(
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    scroll: usize,
    area_width: u16,
    prompt_number_offset: usize,
) -> u16 {
    if prompt_history_expanded() {
        return expanded_prompt_preview_items(
            wrapped_user_prompt_starts,
            user_prompt_texts,
            scroll,
            prompt_number_offset,
        )
        .len() as u16;
    }
    let last_offscreen = lower_bound(wrapped_user_prompt_starts, scroll).checked_sub(1);
    let Some(prompt_order) = last_offscreen else {
        return 0;
    };
    let Some(prompt_text) = user_prompt_texts.get(prompt_order) else {
        return 0;
    };
    let prompt_text = prompt_text.trim();
    if prompt_text.is_empty() {
        return 0;
    }
    let num_str = format!("{}", prompt_number_offset + prompt_order + 1);
    let prefix_len = num_str.len() + 2;
    let content_width = area_width.saturating_sub(prefix_len as u16 + 2) as usize;
    let text_flat = prompt_text.replace('\n', " ");
    let display_width = UnicodeWidthStr::width(text_flat.as_str());
    if display_width > content_width { 2 } else { 1 }
}

/// Number of previous prompts shown when the expanded prompt history (Alt+E) is on.
pub(super) const EXPANDED_PROMPT_PREVIEW_COUNT: usize = 6;

static PROMPT_HISTORY_EXPANDED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn prompt_history_expanded() -> bool {
    PROMPT_HISTORY_EXPANDED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Toggle the expanded previous-prompt band. Returns the new state.
pub(crate) fn toggle_prompt_history_expanded() -> bool {
    !PROMPT_HISTORY_EXPANDED.fetch_xor(true, std::sync::atomic::Ordering::Relaxed)
}

/// Last few offscreen prompts, oldest first, as `(number, flattened text)`.
pub(super) fn expanded_prompt_preview_items(
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    scroll: usize,
    prompt_number_offset: usize,
) -> Vec<(String, String)> {
    let offscreen = lower_bound(wrapped_user_prompt_starts, scroll).min(user_prompt_texts.len());
    let mut items: Vec<(String, String)> = (0..offscreen)
        .rev()
        .filter_map(|idx| {
            let text = user_prompt_texts[idx].trim();
            (!text.is_empty()).then(|| {
                (
                    format!("{}", prompt_number_offset + idx + 1),
                    text.split_whitespace().collect::<Vec<_>>().join(" "),
                )
            })
        })
        .take(EXPANDED_PROMPT_PREVIEW_COUNT)
        .collect();
    items.reverse();
    items
}

pub(super) fn compute_max_scroll_with_prompt_preview(
    total_lines: usize,
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    area: Rect,
    pinned_todo_lines: u16,
    prompt_number_offset: usize,
) -> usize {
    let mut max_scroll =
        total_lines.saturating_sub(area.height.saturating_sub(pinned_todo_lines) as usize);
    let preview_enabled = crate::config::config().display.prompt_preview;
    if max_scroll == 0 || (!preview_enabled && pinned_todo_lines == 0) {
        return max_scroll;
    }

    for _ in 0..4 {
        let prompt_preview_lines = if preview_enabled {
            compute_prompt_preview_line_count(
                wrapped_user_prompt_starts,
                user_prompt_texts,
                max_scroll,
                area.width,
                prompt_number_offset,
            )
        } else {
            0
        };
        let content_height =
            area.height
                .saturating_sub(prompt_preview_lines + pinned_todo_lines) as usize;
        let adjusted = total_lines.saturating_sub(content_height);
        if adjusted == max_scroll {
            break;
        }
        max_scroll = adjusted;
    }

    max_scroll
}
