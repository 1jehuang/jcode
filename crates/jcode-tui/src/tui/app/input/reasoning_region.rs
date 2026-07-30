//! Streaming reasoning region: the live "thinking" block rendered as dim,
//! italic text in the streaming buffer.
//!
//! Extracted from `input.rs`, which is over the code-size budget. Grouped here
//! because these methods share one fragile invariant: `reasoning_partial_len`
//! and `reasoning_block_start` are **byte offsets into
//! `streaming.streaming_text`**, recorded at one point and used to slice the
//! buffer later. If the buffer is replaced in between, those offsets describe
//! nothing, and slicing at a non-boundary offset panics and kills the process
//! (issues #632/#633/#635). Every slice here therefore snaps to a character
//! boundary, and every buffer replacement clears the offsets.

use super::floor_char_boundary;
use crate::tui::app::{App, DisplayMessage};

impl App {
    /// Begin a reasoning region. Reasoning renders as dim, italic text (no
    /// blockquote gutter, no header, no footer). Idempotent while open.
    pub(in crate::tui::app) fn open_reasoning_region(&mut self) {
        if self.reasoning_streaming {
            return;
        }
        // Separate the reasoning block from any prior content with a blank line.
        if !self.streaming.streaming_text.is_empty() {
            if self.streaming.streaming_text.ends_with("\n\n") {
                // already separated
            } else if self.streaming.streaming_text.ends_with('\n') {
                self.append_streaming_text("\n");
            } else {
                self.append_streaming_text("\n\n");
            }
        }
        self.reasoning_streaming = true;
        self.reasoning_pending_line.clear();
        self.reasoning_partial_len = 0;
        // Remember where this reasoning block starts in the stream so `current`
        // mode can later slice it back out in place (without disturbing any
        // preceding answer text) once the model starts answering.
        self.reasoning_block_start = Some(self.streaming.streaming_text.len());
    }

    /// Remove the live partial-reasoning tail (the rendered, not-yet-committed
    /// in-progress line) from the streaming buffer so it can be rebuilt. No-op
    /// when there is no live partial.
    pub(in crate::tui::app) fn strip_reasoning_partial_tail(&mut self) {
        if self.reasoning_partial_len > 0 {
            let new_len = self
                .streaming
                .streaming_text
                .len()
                .saturating_sub(self.reasoning_partial_len);
            // `String::truncate` panics when `new_len` is not a UTF-8 boundary.
            // The tail length is normally exact, but the buffer can be replaced
            // out from under us (reconnect/resume replays a server snapshot), so
            // snap to the nearest boundary at or below `new_len` instead of
            // trusting the recorded length (see issues #632/#633/#635).
            let new_len = floor_char_boundary(&self.streaming.streaming_text, new_len);
            self.streaming.streaming_text.truncate(new_len);
            self.reasoning_partial_len = 0;
        }
    }

    /// Append streamed reasoning text, rendering the in-progress line live so
    /// reasoning trickles in token-by-token (like normal output) rather than one
    /// whole line at a time. Complete lines (terminated by `\n`) are committed as
    /// dim+italic markdown; the trailing partial line is rendered as a live tail
    /// that is re-emitted in place on each delta. The whole-line emphasis run is
    /// preserved (each line is its own `*…*`) so styling never breaks mid-line.
    pub(in crate::tui::app) fn append_reasoning_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.reasoning_streaming {
            self.open_reasoning_region();
        }
        // Drop the previous live tail; we rebuild committed lines + a fresh tail.
        self.strip_reasoning_partial_tail();
        let mut committed = String::new();
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.reasoning_pending_line);
                committed.push_str(&jcode_tui_markdown::reasoning_line_markup(&line));
            } else {
                self.reasoning_pending_line.push(ch);
            }
        }
        if !committed.is_empty() {
            self.streaming.streaming_text.push_str(&committed);
        }
        // Re-append the live tail for the in-progress (partial) line.
        let partial = jcode_tui_markdown::reasoning_partial_markup(&self.reasoning_pending_line);
        self.reasoning_partial_len = partial.len();
        self.streaming.streaming_text.push_str(&partial);
        self.refresh_split_view_if_needed();
    }

    /// Promote the live partial line to a committed line and end the region. The
    /// `_footer` argument is ignored (the "Thought for Xs" footer was removed);
    /// it is kept for call-site compatibility.
    pub(in crate::tui::app) fn close_reasoning_region(&mut self, _footer: Option<String>) {
        if !self.reasoning_streaming {
            return;
        }
        // Replace the live tail with the committed (newline-terminated) line.
        self.strip_reasoning_partial_tail();
        let pending = std::mem::take(&mut self.reasoning_pending_line);
        if !pending.is_empty() {
            self.streaming
                .streaming_text
                .push_str(&jcode_tui_markdown::reasoning_line_markup(&pending));
        }
        self.reasoning_streaming = false;

        // In `current` mode, reasoning is ephemeral: it is never written to the
        // persistent transcript. The closed block is sliced out of the live
        // stream and anchored *in place* as a display-only reasoning message in
        // the transcript flow: it never moves again (no bottom-following, no
        // hoisting), stays readable for the rest of the turn, and is removed
        // when the next user prompt starts a new turn.
        if self.reasoning_current_mode() {
            self.anchor_current_reasoning_block();
            return;
        }

        // Terminate the reasoning block with a blank line so following output
        // renders as a normal paragraph.
        if !self.streaming.streaming_text.ends_with("\n\n") {
            if self.streaming.streaming_text.ends_with('\n') {
                self.streaming.streaming_text.push('\n');
            } else {
                self.streaming.streaming_text.push_str("\n\n");
            }
        }
        self.refresh_split_view_if_needed();
    }

    /// True when the active reasoning-display mode is `current` (live-only,
    /// ephemeral reasoning).
    pub(in crate::tui::app) fn reasoning_current_mode(&self) -> bool {
        matches!(
            crate::config::config().display.reasoning_display(),
            crate::config::ReasoningDisplayMode::Current
        )
    }

    /// Slice the just-closed reasoning block out of `streaming_text` and anchor
    /// it as a display-only reasoning message in the transcript flow, exactly
    /// where it streamed. Used in `current` mode: the trace keeps its position
    /// (content below it can only be appended, never inserted above), so the
    /// thought stays readable and anchored until the next user prompt removes
    /// the turn's traces.
    pub(in crate::tui::app) fn anchor_current_reasoning_block(&mut self) {
        // Same hazard as `strip_reasoning_partial_tail`: this is a byte offset
        // recorded against an earlier state of the buffer, so clamping to the
        // length is not enough. `split_off` panics on a non-boundary offset, so
        // snap it to a character boundary (see issues #632/#633/#635).
        let block_start = self.reasoning_block_start.take().unwrap_or(0);
        let block_start = floor_char_boundary(&self.streaming.streaming_text, block_start);
        // Everything from the block start onward is the reasoning markup. Split it
        // off so the preceding answer text (if any) stays in the live stream.
        let block = self.streaming.streaming_text.split_off(block_start);
        // Drop the separator the open path added before the reasoning block so the
        // surrounding answer text rejoins cleanly.
        while self.streaming.streaming_text.ends_with('\n') {
            self.streaming.streaming_text.pop();
        }
        let block = block.trim_matches('\n').to_string();
        if block.is_empty() {
            self.refresh_split_view_if_needed();
            return;
        }
        // Answer text that streamed *before* the block must commit first so the
        // anchored trace lands after it in the transcript (chronological order).
        if !self.streaming.streaming_text.trim().is_empty() {
            let preceding = self.take_streaming_text();
            let preceding = self.collapse_reasoning_for_commit(preceding);
            if !preceding.trim().is_empty() {
                self.push_display_message(DisplayMessage::assistant(preceding));
            }
        }
        self.turn_reasoning_traces
            .push(crate::tui::app::TurnReasoningTrace {
                display_index: self.display_messages.len(),
                // Snapshot the transcript height when this trace anchors. The trace
                // begins life at the viewport tail; once the transcript grows a
                // full viewport beyond this point the trace is provably off-screen
                // (while tail-following) and can be GC'd without visible motion.
                wrapped_lines_at_anchor: crate::tui::ui::last_total_wrapped_lines(),
            });
        self.push_display_message(DisplayMessage::reasoning(block));
        self.refresh_split_view_if_needed();
    }

    /// Remove the current turn's anchored reasoning traces from the transcript.
    /// Called when the next user prompt is submitted so `current` mode stays
    /// ephemeral across turns: the trace never moves while on screen, it is
    /// simply gone the next time the user acts (a moment when the transcript
    /// reflows anyway).
    pub(in crate::tui::app) fn clear_turn_reasoning_traces(&mut self) {
        if self.turn_reasoning_traces.is_empty() {
            return;
        }
        let traces = std::mem::take(&mut self.turn_reasoning_traces);
        let removed = self.remove_reasoning_trace_messages(traces.iter().map(|t| t.display_index));
        if removed > 0 {
            self.bump_display_messages_version();
            self.refresh_split_view_if_needed();
        }
    }
}
