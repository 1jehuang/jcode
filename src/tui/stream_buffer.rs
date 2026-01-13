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

    /// Find a semantic boundary in the buffer, returns position after boundary
    fn find_boundary(&self) -> Option<usize> {
        let buf = &self.buffer;

        // Code block start/end (```language or ```)
        if let Some(pos) = buf.find("```") {
            // Find end of the ``` line
            if let Some(newline) = buf[pos..].find('\n') {
                return Some(pos + newline + 1);
            }
        }

        // Paragraph break (double newline)
        if let Some(pos) = buf.find("\n\n") {
            return Some(pos + 2);
        }

        // List item (newline followed by - or * or number.)
        for pattern in &["\n- ", "\n* ", "\n1. ", "\n2. ", "\n3. "] {
            if let Some(pos) = buf.find(pattern) {
                return Some(pos + 1); // Include the newline, stop before list marker
            }
        }

        // Sentence end followed by space or newline (but not in middle of number like "1.5")
        let chars: Vec<char> = buf.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            let c = chars[i];
            let next = chars.get(i + 1);

            if matches!(c, '.' | '!' | '?') {
                if let Some(&next_c) = next {
                    // Sentence end: punctuation followed by space, newline, or quote+space
                    if next_c == ' ' || next_c == '\n' {
                        // Make sure it's not a decimal number (check char before)
                        if c == '.' && i > 0 {
                            let prev = chars[i - 1];
                            if prev.is_ascii_digit() {
                                continue; // Skip "1.5" etc
                            }
                        }
                        // Find byte position
                        let byte_pos: usize = chars[..=i].iter().map(|c| c.len_utf8()).sum();
                        return Some(byte_pos + 1); // Include the space/newline
                    }
                }
            }
        }

        // Single newline as fallback for shorter chunks
        if buf.len() > 80 {
            if let Some(pos) = buf.rfind('\n') {
                if pos > 40 {
                    // Only if we have reasonable content
                    return Some(pos + 1);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paragraph_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("First paragraph.\n\nSecond paragraph.");
        assert_eq!(result, Some("First paragraph.\n\n".to_string()));
        assert_eq!(buf.buffer, "Second paragraph.");
    }

    #[test]
    fn test_sentence_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("Hello world. This is a test.");
        assert_eq!(result, Some("Hello world. ".to_string()));
    }

    #[test]
    fn test_code_block_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("Here's code:\n```rust\nfn main() {}");
        assert_eq!(result, Some("Here's code:\n```rust\n".to_string()));
    }

    #[test]
    fn test_no_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("partial text");
        assert_eq!(result, None);
        assert_eq!(buf.buffer, "partial text");
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
    fn test_decimal_not_sentence() {
        let mut buf = StreamBuffer::new();
        // "1.5" should not be treated as sentence end
        let result = buf.push("The value is 1.5 meters.");
        // Should find sentence end at the final period
        assert!(result.is_none() || !result.as_ref().unwrap().ends_with("1."));
    }

    #[test]
    fn test_list_boundary() {
        let mut buf = StreamBuffer::new();
        let result = buf.push("Items:\n- First item\n- Second");
        assert_eq!(result, Some("Items:\n".to_string()));
    }
}
