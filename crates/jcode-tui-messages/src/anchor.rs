//! Content-coordinate viewport anchors.
//!
//! A viewport scroll position stored as a wrapped line index means something
//! only for one specific window width and one specific transcript: a resize
//! reinterprets the index against new content and the reader lands on unrelated
//! messages. An [`Anchor`] stores the position in content coordinates instead
//! (which message, which row inside it) and is resolved against the geometry of
//! the frame being drawn.
//!
//! Identity is the [`MessageBoundary::msg_hash`] already carried by the frame,
//! plus an occurrence index to disambiguate messages with identical content.
//! A content hash survives a reflow, and the occurrence index only matters for
//! exact duplicates.

use crate::prepared::PreparedChatFrame;

/// A reader position in content coordinates: the `occurrence`-th message whose
/// content hash is `msg_hash`, `row_within_item` rows below its first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub msg_hash: u64,
    pub occurrence: usize,
    pub row_within_item: usize,
}

/// Row range of every message in the frame's flat wrapped-row vector, in
/// transcript order, as `(msg_hash, start, len)`.
///
/// Boundaries are cumulative within a section, so the absolute start is the
/// section's `line_start` plus the previous boundary's cumulative length.
pub fn message_row_ranges(frame: &PreparedChatFrame) -> Vec<(u64, usize, usize)> {
    let mut ranges = Vec::new();
    for section in &frame.sections {
        let mut prev = 0usize;
        for boundary in &section.prepared.message_boundaries {
            let end = usize::from(boundary.wrapped_len);
            ranges.push((
                boundary.msg_hash,
                section.line_start + prev,
                end.saturating_sub(prev),
            ));
            prev = end;
        }
    }
    ranges
}

/// Capture the anchor for `row`, or `None` when the row is outside every
/// message (e.g. trailing blank rows, or a frame without boundaries).
pub fn anchor_at_row(frame: &PreparedChatFrame, row: usize) -> Option<Anchor> {
    let ranges = message_row_ranges(frame);
    let mut occurrences: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for (msg_hash, start, len) in ranges {
        let occurrence = occurrences.entry(msg_hash).or_insert(0);
        let this_occurrence = *occurrence;
        *occurrence += 1;
        if row >= start && row < start + len {
            return Some(Anchor {
                msg_hash,
                occurrence: this_occurrence,
                row_within_item: row - start,
            });
        }
    }
    None
}

/// Resolve `anchor` to a row offset in `frame`, clamped to the message's new
/// height and to the scrollable range.
///
/// Returns `None` when the anchored message is gone (pruned or compacted away)
/// or the frame carries no boundaries, so the caller can keep its current
/// position instead of jumping.
pub fn resolve(anchor: &Anchor, frame: &PreparedChatFrame, max_scroll: usize) -> Option<usize> {
    let mut seen = 0usize;
    for (msg_hash, start, len) in message_row_ranges(frame) {
        if msg_hash != anchor.msg_hash {
            continue;
        }
        if seen == anchor.occurrence {
            if len == 0 {
                return Some(start.min(max_scroll));
            }
            let row = start + anchor.row_within_item.min(len - 1);
            return Some(row.min(max_scroll));
        }
        seen += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepared::{MessageBoundary, PreparedMessages, PreparedSectionKind};
    use ratatui::text::Line;
    use std::sync::Arc;

    /// Build a frame whose messages have the given `(hash, wrapped rows at this
    /// width)` pairs, in transcript order.
    fn frame(messages: &[(u64, usize)]) -> PreparedChatFrame {
        let mut boundaries = Vec::new();
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut cumulative = 0usize;
        for (hash, rows) in messages {
            cumulative += rows;
            boundaries.push(MessageBoundary {
                msg_hash: *hash,
                wrapped_len: cumulative,
                raw_len: 0,
                user_prompt_len: 0,
            });
            for _ in 0..*rows {
                lines.push(Line::from("x"));
            }
        }
        PreparedChatFrame::from_sections(vec![(
            PreparedSectionKind::Body,
            Arc::new(PreparedMessages {
                wrapped_lines: lines,
                wrapped_plain_lines: Arc::new(Vec::new()),
                wrapped_copy_offsets: Arc::new(Vec::new()),
                raw_plain_lines: Arc::new(Vec::new()),
                wrapped_line_map: Arc::new(Vec::new()),
                wrapped_user_indices: Vec::new(),
                wrapped_user_prompt_starts: Vec::new(),
                wrapped_user_prompt_ends: Vec::new(),
                user_prompt_texts: Vec::new(),
                image_regions: Vec::new(),
                edit_tool_ranges: Vec::new(),
                copy_targets: Vec::new(),
                message_boundaries: boundaries,
                mermaid_pending_epoch: None,
            }),
        )])
    }

    #[test]
    fn ranges_tile_the_row_vector() {
        let f = frame(&[(1, 3), (2, 1), (3, 2)]);
        assert_eq!(
            message_row_ranges(&f),
            vec![(1, 0, 3), (2, 3, 1), (3, 4, 2)]
        );
        assert_eq!(message_row_ranges(&f).len(), 3);
        assert_eq!(f.total_wrapped_lines(), 6);
    }

    #[test]
    fn anchor_round_trips_within_a_message() {
        let f = frame(&[(10, 4), (20, 2)]);
        let anchor = anchor_at_row(&f, 5).expect("row 5 is inside the second message");
        assert_eq!(
            anchor,
            Anchor {
                msg_hash: 20,
                occurrence: 0,
                row_within_item: 1
            }
        );
        assert_eq!(resolve(&anchor, &f, 100), Some(5));
    }

    #[test]
    fn anchor_survives_a_reflow() {
        // Same messages, taller wrap (narrower window).
        let wide = frame(&[(1, 2), (2, 3), (3, 1)]);
        let narrow = frame(&[(1, 5), (2, 8), (3, 3)]);

        // Anchor two rows into the second message at the wide width.
        let anchor = anchor_at_row(&wide, 3).expect("row 3 is in message 2");
        assert_eq!(anchor.msg_hash, 2);
        assert_eq!(anchor.row_within_item, 1);

        // At the narrow width the same message starts at row 5.
        assert_eq!(resolve(&anchor, &narrow, 100), Some(6));
    }

    #[test]
    fn row_within_item_clamps_when_a_message_shrinks() {
        let wide = frame(&[(7, 10)]);
        let narrow = frame(&[(7, 2)]);
        let anchor = anchor_at_row(&wide, 9).expect("last row");
        assert_eq!(anchor.row_within_item, 9);
        assert_eq!(resolve(&anchor, &narrow, 100), Some(1));
    }

    #[test]
    fn duplicate_hashes_are_disambiguated_by_occurrence() {
        // Two identical messages: the anchor must land on the right one.
        let f = frame(&[(5, 2), (5, 2)]);
        let second = anchor_at_row(&f, 3).expect("row 3 is in the second copy");
        assert_eq!(second.occurrence, 1);
        assert_eq!(resolve(&second, &f, 100), Some(3));

        let first = anchor_at_row(&f, 0).expect("row 0 is in the first copy");
        assert_eq!(first.occurrence, 0);
        assert_eq!(resolve(&first, &f, 100), Some(0));
    }

    #[test]
    fn removed_message_resolves_to_none() {
        let before = frame(&[(1, 2), (2, 2)]);
        let after = frame(&[(1, 2)]);
        let anchor = anchor_at_row(&before, 3).expect("row 3 is in message 2");
        assert_eq!(resolve(&anchor, &after, 100), None);
    }

    #[test]
    fn resolved_row_is_clamped_to_max_scroll() {
        let f = frame(&[(1, 50)]);
        let anchor = Anchor {
            msg_hash: 1,
            occurrence: 0,
            row_within_item: 40,
        };
        assert_eq!(resolve(&anchor, &f, 12), Some(12));
    }

    #[test]
    fn frames_without_boundaries_yield_no_anchor() {
        let f = frame(&[]);
        assert_eq!(message_row_ranges(&f), Vec::new());
        assert_eq!(anchor_at_row(&f, 0), None);
        assert_eq!(
            resolve(
                &Anchor {
                    msg_hash: 1,
                    occurrence: 0,
                    row_within_item: 0
                },
                &f,
                10
            ),
            None
        );
    }
}
