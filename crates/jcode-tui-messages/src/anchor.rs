//! Content-coordinate viewport anchors.
//!
//! A viewport scroll position stored as a wrapped line index means something
//! only for one specific window width and one specific transcript: a resize
//! reinterprets the index against new content and the reader lands on unrelated
//! messages. An [`Anchor`] stores the position in content coordinates instead
//! (which message, which row inside it) and is resolved against the geometry of
//! the frame being drawn.
//!
//! Identity is the [`MessageBoundary::item_id`] already carried by the frame: a
//! stable id minted when the message entered the transcript. It survives a
//! reflow, a prepend, a removal and a compaction re-sync, so a position names
//! exactly one message. There is no ordinal and no content hash here.

use crate::ItemId;
use crate::prepared::{PreparedChatFrame, PreparedSectionKind};

/// A reader position in content coordinates: the message identified by
/// `item_id`, `row_within_item` rows below its first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub item_id: ItemId,
    pub row_within_item: usize,
}

/// Row range of every message in the frame's flat wrapped-row vector, in
/// transcript order, as `(item_id, start, len)`.
///
/// Boundaries are cumulative within a section, so the absolute start is the
/// section's `line_start` plus the previous boundary's cumulative length.
pub fn message_row_ranges(frame: &PreparedChatFrame) -> Vec<(ItemId, usize, usize)> {
    let mut ranges = Vec::new();
    for section in &frame.sections {
        let mut prev = 0usize;
        for boundary in &section.prepared.message_boundaries {
            let end = boundary.wrapped_len;
            ranges.push((
                boundary.item_id,
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
    for (item_id, start, len) in message_row_ranges(frame) {
        if row >= start && row < start + len {
            return Some(Anchor {
                item_id,
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
    for (item_id, start, len) in message_row_ranges(frame) {
        if item_id != anchor.item_id {
            continue;
        }
        if len == 0 {
            return Some(start.min(max_scroll));
        }
        let row = start + anchor.row_within_item.min(len - 1);
        return Some(row.min(max_scroll));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ItemId;
    use crate::prepared::{MessageBoundary, PreparedMessages, PreparedSectionKind};
    use ratatui::text::Line;
    use std::sync::Arc;

    /// Build a frame whose messages have the given `(item id, wrapped rows at this
    /// width)` pairs, in transcript order.
    fn frame(messages: &[(u64, usize)]) -> PreparedChatFrame {
        let mut boundaries = Vec::new();
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut cumulative = 0usize;
        for (hash, rows) in messages {
            cumulative += rows;
            boundaries.push(MessageBoundary {
                item_id: ItemId(*hash),
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
            vec![
                (ItemId(1), 0, 3),
                (ItemId(2), 3, 1),
                (ItemId(3), 4, 2)
            ]
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
                item_id: ItemId(20),
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
        assert_eq!(anchor.item_id, ItemId(2));
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
            item_id: ItemId(1),
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
                    item_id: ItemId(1),
                    row_within_item: 0
                },
                &f,
                10
            ),
            None
        );
    }
}

/// A viewport position in content coordinates, for any row of a frame.
///
/// [`Anchor`] names a message. Not every scrollable row belongs to one: live
/// streaming output, retained reasoning, and the header are sections with no
/// message boundaries. Those rows still need a content coordinate, or a resize
/// has nothing to resolve against and replays a wrapped row index instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentPos {
    Message(Anchor),
    /// A row inside a section with no message boundaries, identified by the raw
    /// line and column it was wrapped from. A rewrap preserves both.
    Section {
        kind: PreparedSectionKind,
        raw_line: usize,
        column: usize,
    },
}

/// Capture the content position of `row`, message or not.
pub fn content_pos_at_row(frame: &PreparedChatFrame, row: usize) -> Option<ContentPos> {
    if let Some(anchor) = anchor_at_row(frame, row) {
        return Some(ContentPos::Message(anchor));
    }
    let section = frame.sections.iter().find(|section| {
        row >= section.line_start && row < section.line_start + section.prepared.wrapped_lines.len()
    })?;
    // A duplicate kind would resolve into the wrong section; decline instead.
    if frame
        .sections
        .iter()
        .filter(|other| other.kind == section.kind)
        .count()
        != 1
    {
        return None;
    }
    let entry = section
        .prepared
        .wrapped_line_map
        .get(row - section.line_start)?;
    Some(ContentPos::Section {
        kind: section.kind,
        raw_line: entry.raw_line,
        column: entry.start_col,
    })
}

/// Resolve a content position to a row offset in `frame`, clamped to
/// `max_scroll`. `None` keeps the caller's current position.
pub fn resolve_content_pos(
    pos: &ContentPos,
    frame: &PreparedChatFrame,
    max_scroll: usize,
) -> Option<usize> {
    match pos {
        ContentPos::Message(anchor) => resolve(anchor, frame, max_scroll),
        // ponytail: precise inside the section only, and assumes its raw text is
        // unchanged (true for a resize and for append-only streaming). Promote
        // to a public anchor if a bookmark or selection ever needs a coordinate
        // inside live output.
        ContentPos::Section {
            kind,
            raw_line,
            column,
        } => {
            let mut sections = frame
                .sections
                .iter()
                .filter(|section| section.kind == *kind);
            let section = sections.next()?;
            if sections.next().is_some() {
                return None;
            }
            // Last entry starting at or before the column: the row the offset
            // was wrapped into. The captured row is exactly one of these.
            let local = section
                .prepared
                .wrapped_line_map
                .iter()
                .rposition(|entry| entry.raw_line == *raw_line && entry.start_col <= *column)?;
            Some((section.line_start + local).min(max_scroll))
        }
    }
}

#[cfg(test)]
mod section_pos_tests {
    use super::*;
    use crate::WrappedLineMap;
    use crate::prepared::PreparedMessages;
    use ratatui::text::Line;
    use std::sync::Arc;

    /// A section whose `rows` wrapped rows each begin one stride later in a
    /// single raw line, the shape a long streaming blob is prepared in.
    fn section(total: usize, rows: usize) -> Arc<PreparedMessages> {
        let stride = (total / rows).max(1);
        Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from("x"); rows],
            wrapped_plain_lines: Arc::new(Vec::new()),
            wrapped_copy_offsets: Arc::new(Vec::new()),
            raw_plain_lines: Arc::new(vec!["x".repeat(total)]),
            wrapped_line_map: Arc::new(
                (0..rows)
                    .map(|row| WrappedLineMap {
                        raw_line: 0,
                        start_col: row * stride,
                        end_col: (row + 1) * stride,
                    })
                    .collect(),
            ),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
            message_boundaries: Vec::new(),
            mermaid_pending_epoch: None,
        })
    }

    fn frame(kind: PreparedSectionKind, total: usize, rows: usize) -> PreparedChatFrame {
        PreparedChatFrame::from_sections(vec![(kind, section(total, rows))])
    }

    #[test]
    fn section_row_keeps_its_content_through_a_rewrap() {
        // Same 100 columns of text: 4 rows wide, 10 rows narrow. The content at
        // column 50 is row 2 wide and row 5 narrow.
        let wide = frame(PreparedSectionKind::Streaming, 100, 4);
        let narrow = frame(PreparedSectionKind::Streaming, 100, 10);

        let pos = content_pos_at_row(&wide, 2).expect("row 2 is inside the section");
        assert_eq!(
            pos,
            ContentPos::Section {
                kind: PreparedSectionKind::Streaming,
                raw_line: 0,
                column: 50
            }
        );
        assert_eq!(resolve_content_pos(&pos, &narrow, 100), Some(5));
    }

    #[test]
    fn rows_outside_a_section_and_ambiguous_kinds_decline() {
        let wide = frame(PreparedSectionKind::Streaming, 100, 4);
        assert_eq!(content_pos_at_row(&wide, 99), None);

        let doubled = PreparedChatFrame::from_sections(vec![
            (PreparedSectionKind::Streaming, section(100, 4)),
            (PreparedSectionKind::Streaming, section(100, 4)),
        ]);
        assert_eq!(content_pos_at_row(&doubled, 0), None);
        assert_eq!(
            resolve_content_pos(
                &ContentPos::Section {
                    kind: PreparedSectionKind::Streaming,
                    raw_line: 0,
                    column: 0
                },
                &doubled,
                100
            ),
            None
        );
    }
}
