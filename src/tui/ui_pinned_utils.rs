use super::*;
use crate::tui::mermaid;
use std::collections::VecDeque;
use std::hash::Hasher as _;

/// Format tokens compactly (1.2M, 45K, 123)
pub(super) fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

pub(super) fn format_usage_line(tokens_str: String, cache_status: Option<String>) -> String {
    let mut parts = Vec::new();
    if !tokens_str.is_empty() {
        parts.push(tokens_str);
    }
    if let Some(cache) = cache_status {
        parts.push(cache);
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" • ")
    }
}

pub(super) fn format_cache_status(
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
) -> Option<String> {
    match (cache_read_tokens, cache_creation_tokens) {
        (Some(read), _) if read > 0 => {
            let k = read / 1000;
            if k > 0 {
                Some(format!("⚡{}k cached", k))
            } else {
                Some(format!("⚡{} cached", read))
            }
        }
        (_, Some(created)) if created > 0 => {
            let k = created / 1000;
            if k > 0 {
                Some(format!("💾{}k stored", k))
            } else {
                Some(format!("💾{} stored", created))
            }
        }
        _ => None,
    }
}

pub(super) fn lru_touch<K: PartialEq>(order: &mut VecDeque<K>, key: &K) {
    if let Some(pos) = order.iter().position(|existing| existing == key) {
        order.remove(pos);
    }
}

pub(super) fn side_panel_content_signature(page: &crate::side_panel::SidePanelPage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    page.id.hash(&mut hasher);
    page.file_path.hash(&mut hasher);
    page.source.as_str().hash(&mut hasher);
    page.updated_at_ms.hash(&mut hasher);
    page.content.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn estimate_side_panel_pane_area(
    terminal_width: u16,
    terminal_height: u16,
    ratio_percent: u8,
) -> Option<Rect> {
    const MIN_DIFF_WIDTH: u16 = 30;
    const MIN_CHAT_WIDTH: u16 = 20;

    let max_diff = terminal_width.saturating_sub(MIN_CHAT_WIDTH);
    if max_diff < MIN_DIFF_WIDTH || terminal_height < 3 {
        return None;
    }

    let diff_width = (((terminal_width as u32 * ratio_percent.clamp(25, 100) as u32) / 100) as u16)
        .max(MIN_DIFF_WIDTH)
        .min(max_diff);
    Some(Rect::new(0, 0, diff_width, terminal_height))
}

pub(super) fn compact_image_label(label: &str) -> String {
    if label.contains('/') {
        return label
            .rsplit('/')
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("/");
    }
    label.to_string()
}

pub(super) fn div_ceil_u32_local(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        return value;
    }
    value.saturating_add(divisor - 1) / divisor
}

pub(super) fn estimate_inline_image_rows(
    img_w: u32,
    img_h: u32,
    pane_width: u16,
    pane_height: u16,
) -> u16 {
    let inner_width = pane_width.max(1) as u32;
    let (cell_w, cell_h) = mermaid::get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;
    let width_px = inner_width.saturating_mul(cell_w);
    let scaled_height_px = div_ceil_u32_local(img_h.max(1).saturating_mul(width_px), img_w.max(1));
    let rows = div_ceil_u32_local(scaled_height_px, cell_h)
        .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS as u32)
        .min(pane_height.max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS) as u32);
    rows as u16
}
