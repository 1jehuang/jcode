use super::*;
use std::collections::VecDeque;

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
