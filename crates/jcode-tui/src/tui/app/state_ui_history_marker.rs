//! Marker parsing and in-flight fencing for lazily loaded history windows.
use super::{CompactedHistoryLazyState, DisplayMessage};
use std::time::Instant;

#[derive(Default)]
pub(super) struct RecentHistoryPagingState {
    /// Request id, display generation and target size of the in-flight page.
    pub request: Option<(u64, u64, usize)>,
    pub retry_at: Option<Instant>,
}

pub(super) fn parse_compacted_history_marker(
    message: &DisplayMessage,
) -> Option<CompactedHistoryLazyState> {
    if message.role != "system" {
        return None;
    }
    let rest = message
        .content
        .strip_prefix("Earlier conversation compacted - ")
        .or_else(|| {
            message
                .content
                .strip_prefix(crate::protocol::RECENT_HISTORY_MARKER_PREFIX)
        })?;
    if let Some(rest) = rest.strip_prefix("showing all ") {
        let (total, _) = parse_leading_usize(rest)?;
        return Some(CompactedHistoryLazyState {
            total_messages: total,
            visible_messages: total,
            ..Default::default()
        });
    }
    let (first, after_first) = parse_leading_usize(rest)?;
    if let Some(showing) = after_first.strip_prefix(" older historical messages hidden. Showing ") {
        let (visible, after_visible) = parse_leading_usize(showing)?;
        let after_visible = after_visible.strip_prefix(" of ")?;
        let (total, _) = parse_leading_usize(after_visible)?;
        return Some(CompactedHistoryLazyState {
            total_messages: total,
            visible_messages: visible,
            remaining_messages: first,
            ..Default::default()
        });
    }
    after_first
        .starts_with(" historical messages hidden")
        .then_some(CompactedHistoryLazyState {
            total_messages: first,
            remaining_messages: first,
            ..Default::default()
        })
}

fn parse_leading_usize(text: &str) -> Option<(usize, &str)> {
    let end = text
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()?;
    let Ok(value) = text[..end].parse() else {
        return None;
    };
    Some((value, &text[end..]))
}
