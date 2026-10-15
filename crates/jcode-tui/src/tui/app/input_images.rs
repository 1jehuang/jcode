//! Keep composer attachment payloads, numbered markers, and undo in sync.

use super::App;
use std::sync::Arc;

/// Locate `[image N]` attachment markers in composer text. Returns
/// `(start, end, N)` byte spans for every marker whose number refers to one of
/// the `image_count` attached images; anything else typed by hand is ignored.
fn image_marker_spans(text: &str, image_count: usize) -> Vec<(usize, usize, usize)> {
    const PREFIX: &str = "[image ";
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(PREFIX) {
        let start = from + rel;
        let digits_start = start + PREFIX.len();
        let digits_len = text[digits_start..]
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let end = digits_start + digits_len;
        if digits_len > 0 && text[end..].starts_with(']') {
            if let Ok(n) = text[digits_start..end].parse::<usize>()
                && (1..=image_count).contains(&n)
            {
                spans.push((start, end + 1, n));
            }
            from = end + 1;
        } else {
            from = start + 1;
        }
    }
    spans
}

// Adjacent text-only undo entries share their potentially large image payloads.
pub(super) type InputUndoSnapshot = (String, usize, Arc<Vec<(String, String)>>);

impl App {
    /// Attachment markers are atomic when deleting text. Even deleting part of
    /// one removes its payload, rather than leaving an invisible attachment.
    pub(super) fn remove_input_range(&mut self, mut start: usize, mut end: usize) -> String {
        if start == end {
            return String::new();
        }
        let mut removed = vec![false; self.pending_images.len()];
        for (at, marker_end, number) in image_marker_spans(&self.input, self.pending_images.len()) {
            if at < end && marker_end > start {
                start = start.min(at);
                end = end.max(marker_end);
                removed[number - 1] = true;
            }
        }
        let text = self.input.drain(start..end).collect();
        self.cursor_pos = start;
        self.reconcile_input_images_with_removed(&removed);
        text
    }

    /// Inserting inside a marker can damage it too. Never retain a payload
    /// whose numbered marker is no longer present after an edit.
    pub(super) fn reconcile_input_images(&mut self) {
        self.reconcile_input_images_with_removed(&[]);
    }

    fn reconcile_input_images_with_removed(&mut self, removed: &[bool]) {
        if self.pending_images.is_empty() {
            return;
        }
        let spans = image_marker_spans(&self.input, self.pending_images.len());
        let mut retained = vec![false; self.pending_images.len()];
        for &(_, _, number) in &spans {
            retained[number - 1] = !removed.get(number - 1).copied().unwrap_or(false);
        }
        if retained.iter().all(|keep| *keep) {
            return;
        }
        let mut next = 0;
        let renumbered: Vec<usize> = retained
            .iter()
            .map(|keep| {
                if *keep {
                    next += 1;
                    next
                } else {
                    0
                }
            })
            .collect();
        // Rewrite original spans backwards, so a newly assigned marker cannot
        // be mistaken for another original image. Preserve the caret even when
        // a two-digit image number before it shrinks to one digit.
        for (start, end, number) in spans.into_iter().rev() {
            let replacement = match renumbered[number - 1] {
                0 => String::new(),
                number => format!("[image {number}]"),
            };
            if self.cursor_pos >= end {
                self.cursor_pos = self.cursor_pos - (end - start) + replacement.len();
            } else if self.cursor_pos > start {
                self.cursor_pos = start + replacement.len();
            }
            self.input.replace_range(start..end, &replacement);
        }
        let mut index = 0;
        self.pending_images.retain(|_| {
            let keep = retained[index];
            index += 1;
            keep
        });
    }
}
