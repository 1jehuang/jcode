use super::*;

/// Compute (and cache) the auxiliary raw-line-map data for an assistant message.
/// `cached` is the already-rendered (and possibly cached) display lines for the
/// message; `align` and `centered` describe the current display mode.
pub(super) fn assistant_aux_data(
    msg: &DisplayMessage,
    cached: &[Line<'static>],
    content_width: u16,
    centered: bool,
    diff_mode: crate::config::DiffDisplayMode,
    align: ratatui::layout::Alignment,
) -> std::sync::Arc<AssistantAuxData> {
    let build = || {
        let content_lines =
            markdown::render_markdown_with_width(&msg.content, Some(content_width as usize));
        let content_line_count = content_lines.len().min(cached.len());
        let logical_plain_lines: Vec<String> =
            if content_prefers_display_as_logical_lines(&msg.content) {
                cached
                    .iter()
                    .take(content_line_count)
                    .map(ui::line_plain_text)
                    .collect()
            } else {
                markdown::render_markdown(&msg.content)
                    .into_iter()
                    .map(|line| ui::line_plain_text(&align_if_unset(line, align)))
                    .collect()
            };
        AssistantAuxData {
            content_line_count,
            logical_plain_lines,
        }
    };

    if cfg!(test) {
        return std::sync::Arc::new(build());
    }

    let key = AssistantAuxKey {
        message_hash: msg.stable_cache_hash(),
        content_len: msg.content.len(),
        content_width,
        centered,
        diff_mode,
        cached_len: cached.len(),
    };

    {
        let cache = match assistant_aux_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(data) = cache.entries.get(&key) {
            return data.clone();
        }
    }

    let data = std::sync::Arc::new(build());
    let mut cache = match assistant_aux_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    if cache.entries.insert(key.clone(), data.clone()).is_none() {
        cache.order.push_back(key);
        while cache.order.len() > ASSISTANT_AUX_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    }
    data
}
