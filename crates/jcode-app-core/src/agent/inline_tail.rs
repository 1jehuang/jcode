//! Rolling live-output tail for inline swarm workers.
//!
//! The coordinator's inline gallery/dock renders a small viewport of each
//! worker's recent activity. Streaming only the in-progress assistant text
//! had two failure modes:
//!
//! 1. Workers spend most wall-clock time inside tool calls, during which no
//!    text streams, so the viewport froze on stale prose for the duration.
//! 2. `text_content` resets on every API call, so the viewport blanked at
//!    each turn/continuation boundary.
//!
//! [`InlineTailBuffer`] fixes both: it keeps a rolling, capped buffer of
//! *committed* activity lines (finished text segments and tool markers) that
//! survives across turns, plus the current *live* streaming text segment.
//! Tool executions are interleaved as `⚙ name · summary` markers, updated in
//! place with a duration or error state on completion.

use std::collections::VecDeque;

/// Max committed lines retained (matches the gallery viewport budget).
const MAX_LINES: usize = 14;
/// Max total characters in the rendered tail (bus payload cap).
const MAX_CHARS: usize = 1400;
/// Max characters of a tool marker's input summary.
const MAX_SUMMARY_CHARS: usize = 60;

/// Rolling tail of a worker's recent activity: committed lines (text +
/// tool markers) plus the live in-progress assistant text.
#[derive(Debug, Default)]
pub(crate) struct InlineTailBuffer {
    /// Finished activity lines, oldest first, capped to [`MAX_LINES`].
    committed: VecDeque<String>,
    /// In-progress assistant text for the current stream (replaced wholesale
    /// on every delta, committed at message end, discarded on rollback).
    live: String,
    /// Whether the last committed line is an in-flight tool marker that
    /// [`Self::finish_tool`] should update in place.
    pending_tool_marker: bool,
}

impl InlineTailBuffer {
    /// Replace the live streaming text with the accumulated `text` so far.
    pub(crate) fn set_live(&mut self, text: &str) {
        self.live.clear();
        self.live.push_str(text);
    }

    /// Commit the live text into the rolling buffer (end of a message) and
    /// clear it. Empty/whitespace-only live text is discarded.
    pub(crate) fn commit_live(&mut self) {
        let live = std::mem::take(&mut self.live);
        for line in live.lines().filter(|l| !l.trim().is_empty()) {
            self.push_committed(line.to_string());
        }
    }

    /// Discard the live text (mid-stream retry rollback replays from the top).
    pub(crate) fn clear_live(&mut self) {
        self.live.clear();
    }

    /// Record a tool execution starting. Any live text is committed first so
    /// ordering in the tail matches what actually happened.
    pub(crate) fn start_tool(&mut self, name: &str, input: &serde_json::Value) {
        self.commit_live();
        let summary = tool_marker_summary(name, input);
        let marker = if summary.is_empty() {
            format!("⚙ {name}")
        } else {
            format!("⚙ {name} · {summary}")
        };
        self.push_committed(marker);
        self.pending_tool_marker = true;
    }

    /// Record the in-flight tool finishing, updating its marker in place with
    /// a duration (and error flag). If the marker was already evicted by
    /// buffer pressure, this is a no-op.
    pub(crate) fn finish_tool(&mut self, elapsed_secs: f64, is_error: bool) {
        if !self.pending_tool_marker {
            return;
        }
        self.pending_tool_marker = false;
        if let Some(last) = self.committed.back_mut() {
            let status = if is_error { " ✗" } else { "" };
            last.push_str(&format!(" ({}){status}", humanize_secs(elapsed_secs)));
        }
    }

    /// Render the tail for the bus: committed lines then live lines, bounded
    /// to the last [`MAX_LINES`] lines / [`MAX_CHARS`] chars.
    pub(crate) fn render(&self) -> String {
        let live_lines = self
            .live
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>();
        let mut lines: Vec<&str> = self
            .committed
            .iter()
            .map(String::as_str)
            .chain(live_lines)
            .collect();
        if lines.len() > MAX_LINES {
            lines.drain(..lines.len() - MAX_LINES);
        }
        let mut tail = lines.join("\n");
        if tail.len() > MAX_CHARS {
            let start = floor_char_boundary(&tail, tail.len() - MAX_CHARS);
            tail = tail[start..].to_string();
        }
        tail
    }

    fn push_committed(&mut self, line: String) {
        // A new committed line supersedes any pending in-place marker update
        // ordering (finish_tool only touches the true last line).
        self.pending_tool_marker = false;
        self.committed.push_back(line);
        while self.committed.len() > MAX_LINES {
            self.committed.pop_front();
        }
    }
}

/// Compact one-line summary of a tool's input for the activity marker.
/// Prefers the model-provided `intent`, then well-known per-tool fields.
fn tool_marker_summary(name: &str, input: &serde_json::Value) -> String {
    let raw = jcode_message_types::ToolCall::intent_from_input(input)
        .or_else(|| {
            let field = match name {
                "bash" => "command",
                "read" | "write" => "file_path",
                "edit" | "multiedit" => "file_path",
                "replace" => "pattern",
                "agentgrep" | "websearch" => "query",
                "webfetch" => "url",
                "task" | "subagent" => "description",
                _ => return None,
            };
            input
                .get(field)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > MAX_SUMMARY_CHARS {
        let mut out: String = flat.chars().take(MAX_SUMMARY_CHARS - 1).collect();
        out.push('…');
        out
    } else {
        flat
    }
}

/// "3s" / "2m10s" style duration for tool markers.
fn humanize_secs(secs: f64) -> String {
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{}s", secs as u64)
    } else {
        let total = secs as u64;
        format!("{}m{}s", total / 60, total % 60)
    }
}

/// Largest byte index `<= index` that is a UTF-8 char boundary in `text`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut boundary = index;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_text_renders_and_survives_commit() {
        let mut tail = InlineTailBuffer::default();
        tail.set_live("thinking about the fix\nsecond line");
        assert_eq!(tail.render(), "thinking about the fix\nsecond line");
        tail.commit_live();
        // Next stream starts blank but the previous output is retained.
        tail.set_live("");
        assert_eq!(tail.render(), "thinking about the fix\nsecond line");
    }

    #[test]
    fn tool_markers_interleave_and_complete_in_place() {
        let mut tail = InlineTailBuffer::default();
        tail.set_live("Let me check the render pipeline.");
        tail.start_tool(
            "bash",
            &serde_json::json!({"command": "cargo build --profile selfdev"}),
        );
        let mid = tail.render();
        assert!(
            mid.contains("Let me check the render pipeline."),
            "live text must commit before the marker: {mid}"
        );
        assert!(
            mid.contains("⚙ bash · cargo build --profile selfdev"),
            "{mid}"
        );

        tail.finish_tool(47.2, false);
        assert!(tail.render().contains("(47s)"), "{}", tail.render());

        tail.start_tool("edit", &serde_json::json!({"file_path": "src/ui.rs"}));
        tail.finish_tool(0.3, true);
        let done = tail.render();
        assert!(done.contains("⚙ edit · src/ui.rs (0.3s) ✗"), "{done}");
    }

    #[test]
    fn marker_prefers_intent_over_raw_input() {
        let mut tail = InlineTailBuffer::default();
        tail.start_tool(
            "bash",
            &serde_json::json!({"command": "x", "intent": "run the ui tests"}),
        );
        assert!(tail.render().contains("⚙ bash · run the ui tests"));
    }

    #[test]
    fn rollback_discards_live_but_keeps_committed() {
        let mut tail = InlineTailBuffer::default();
        tail.start_tool("read", &serde_json::json!({"file_path": "a.rs"}));
        tail.finish_tool(0.1, false);
        tail.set_live("partial output that gets replayed");
        tail.clear_live();
        let out = tail.render();
        assert!(out.contains("⚙ read"), "{out}");
        assert!(!out.contains("partial output"), "{out}");
    }

    #[test]
    fn caps_lines_and_chars_and_summary_length() {
        let mut tail = InlineTailBuffer::default();
        for i in 0..40 {
            tail.set_live(&format!("line number {i}"));
            tail.commit_live();
        }
        let out = tail.render();
        assert!(out.lines().count() <= MAX_LINES);
        assert!(out.contains("line number 39"));
        assert!(!out.contains("line number 0\n"));

        let huge = "x".repeat(5000);
        tail.set_live(&huge);
        assert!(tail.render().len() <= MAX_CHARS);

        let mut tail = InlineTailBuffer::default();
        let long_cmd = "cargo test ".repeat(30);
        tail.start_tool("bash", &serde_json::json!({ "command": long_cmd }));
        let line = tail.render();
        assert!(line.chars().count() < 80, "summary must truncate: {line}");
        assert!(line.contains('…'), "{line}");
    }

    #[test]
    fn finish_without_pending_marker_is_noop() {
        let mut tail = InlineTailBuffer::default();
        tail.set_live("hello");
        tail.commit_live();
        tail.finish_tool(1.0, false);
        assert_eq!(tail.render(), "hello");
        // Committing new lines invalidates a pending marker: finish after
        // commit must not append a duration to unrelated text.
        let mut tail = InlineTailBuffer::default();
        tail.start_tool("bash", &serde_json::json!({"command": "ls"}));
        tail.set_live("output line");
        tail.commit_live();
        tail.finish_tool(2.0, false);
        let out = tail.render();
        assert!(!out.contains("output line (2.0s)"), "{out}");
    }
}
