use super::*;

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
