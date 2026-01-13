//! Semantic stream buffer - chunks streaming text at natural boundaries

use std::time::{Duration, Instant};

/// Buffer that accumulates streaming text and flushes at semantic boundaries
pub struct StreamBuffer {
    buffer: String,
    last_flush: Instant,
    timeout: Duration,
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
        }
    }

    /// Push text into buffer, returns chunk to display if boundary found
    pub fn push(&mut self, text: &str) -> Option<String> {
        self.buffer.push_str(text);

        // Find semantic boundary
        if let Some(boundary) = self.find_boundary() {
            let chunk = self.buffer[..boundary].to_string();
            self.buffer = self.buffer[boundary..].to_string();
            self.last_flush = Instant::now();
            return Some(chunk);
        }

        None
    }

    /// Check if buffer should be flushed due to timeout
    pub fn should_flush(&self) -> bool {
        !self.buffer.is_empty() && self.last_flush.elapsed() > self.timeout
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

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Clear the buffer without returning content
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.last_flush = Instant::now();
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
}
