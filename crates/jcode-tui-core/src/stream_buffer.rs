//! Semantic stream buffer - chunks streaming text at natural boundaries

use serde::Serialize;
use std::time::{Duration, Instant};

/// Buffer that accumulates streaming text and flushes at semantic boundaries
pub struct StreamBuffer {
    buffer: String,
    last_flush: Instant,
    timeout: Duration,
    smooth_frame_chars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamBufferMemoryProfile {
    pub buffered_text_bytes: usize,
    pub timeout_ms: u64,
}

impl Default for StreamBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            last_flush: Instant::now(),
            timeout: Duration::from_millis(150),
            smooth_frame_chars: 96,
        }
    }

    /// Push text into buffer, returns chunk to display if boundary found
    pub fn push(&mut self, text: &str) -> Option<String> {
        self.buffer.push_str(text);

        // Find semantic boundary
        if let Some(boundary) = self.find_boundary() {
            return Some(self.drain_prefix(boundary.min(self.smooth_frame_boundary())));
        }

        if self.last_flush.elapsed() >= self.timeout {
            return self.flush_smooth_frame();
        }

        None
    }

    /// Force flush the entire buffer (call on timeout or message end)
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            self.last_flush = Instant::now();
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Flush up to one smooth-render frame worth of text. This is used for
    /// periodic streaming redraws so large provider/SSE bursts are revealed
    /// over a few quick frames instead of popping into the TUI all at once.
    /// Finalization paths should still call [`flush`] to avoid leaving text
    /// buffered at message boundaries.
    pub fn flush_smooth_frame(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            let boundary = self.smooth_frame_boundary().min(self.buffer.len());
            Some(self.drain_prefix(boundary))
        }
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Clear the buffer without returning content
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.last_flush = Instant::now();
    }

    pub fn debug_memory_profile(&self) -> StreamBufferMemoryProfile {
        StreamBufferMemoryProfile {
            buffered_text_bytes: self.buffer.len(),
            timeout_ms: self.timeout.as_millis() as u64,
        }
    }

    fn smooth_frame_boundary(&self) -> usize {
        if self.buffer.chars().count() <= self.smooth_frame_chars {
            return self.buffer.len();
        }
        self.buffer
            .char_indices()
            .map(|(idx, _)| idx)
            .nth(self.smooth_frame_chars)
            .unwrap_or(self.buffer.len())
    }

    fn drain_prefix(&mut self, boundary: usize) -> String {
        let boundary = floor_char_boundary(&self.buffer, boundary);
        let chunk = self.buffer[..boundary].to_string();
        self.buffer = self.buffer[boundary..].to_string();
        self.last_flush = Instant::now();
        chunk
    }

    /// Find a boundary in the buffer (newline-based), returns position after boundary
    fn find_boundary(&self) -> Option<usize> {
        let buf = &self.buffer;

        // Code block start/end (```language or ```)
        if let Some(pos) = buf.find("```") {
            // Find end of the ``` line
            if let Some(newline) = buf[pos..].find('\n') {
                return Some(pos + newline + 1);
            }
        }

        // Any newline - simple and predictable
        if let Some(pos) = buf.find('\n') {
            return Some(pos + 1);
        }

        None
    }
}

fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    index = index.min(s.len());
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_newline_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("First line\nSecond line");
        assert_eq!(result, Some("First line\n".to_string()));
        assert_eq!(buf.buffer, "Second line");
    }

    #[test]
    fn test_code_block_boundary() {
        let mut buf = StreamBuffer::new();
        // Code block marker ``` causes flush to include the whole line
        let result = buf.push("```rust\nfn main() {}");
        assert_eq!(result, Some("```rust\n".to_string()));
    }

    #[test]
    fn test_no_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("partial text without newline");
        assert_eq!(result, None);
        assert_eq!(buf.buffer, "partial text without newline");
    }

    #[test]
    fn test_flush() {
        let mut buf = StreamBuffer::new();
        buf.push("remaining content");
        let result = buf.flush();
        assert_eq!(result, Some("remaining content".to_string()));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_multiple_newlines() {
        let mut buf = StreamBuffer::new();
        // First push returns first line
        let result = buf.push("Line one\nLine two\nLine three");
        assert_eq!(result, Some("Line one\n".to_string()));
        // Second push returns second line
        let result = buf.push("");
        assert_eq!(result, Some("Line two\n".to_string()));
    }

    #[test]
    fn test_smooth_frame_flush_caps_large_chunks() {
        let mut buf = StreamBuffer::new();
        let text = "a".repeat(150);
        assert_eq!(buf.push(&text), None);

        let first = buf.flush_smooth_frame().unwrap();
        assert_eq!(first.len(), 96);
        assert_eq!(buf.buffer.len(), 54);

        let rest = buf.flush().unwrap();
        assert_eq!(rest.len(), 54);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_smooth_frame_flush_respects_utf8_boundaries() {
        let mut buf = StreamBuffer::new();
        let text = "é".repeat(120);
        assert_eq!(buf.push(&text), None);

        let first = buf.flush_smooth_frame().unwrap();
        assert_eq!(first.chars().count(), 96);
        assert!(first.is_char_boundary(first.len()));

        let rest = buf.flush().unwrap();
        assert_eq!(rest.chars().count(), 24);
    }
}
