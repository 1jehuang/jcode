/// Truncate a string at a valid UTF-8 character boundary.
///
/// Returns a slice of at most `max_bytes` bytes, ending at a valid char boundary.
/// This prevents panics when truncating strings that contain multi-byte characters.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the largest valid char boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub const APPROX_CHARS_PER_TOKEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproxTokenSeverity {
    Normal,
    Warning,
    Danger,
}

/// Estimate token count using jcode's existing chars-per-token heuristic.
pub fn estimate_tokens(s: &str) -> usize {
    s.len() / APPROX_CHARS_PER_TOKEN
}

/// Format a number with ASCII thousands separators.
pub fn format_number(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Format a token count in the compact style used by the TUI.
pub fn format_approx_token_count(tokens: usize) -> String {
    match tokens {
        0..=999 => format!("{} tok", tokens),
        1_000..=9_999 => {
            let whole = tokens / 1_000;
            let tenth = (tokens % 1_000) / 100;
            if tenth == 0 {
                format!("{}k tok", whole)
            } else {
                format!("{}.{}k tok", whole, tenth)
            }
        }
        _ => format!("{}k tok", tokens / 1_000),
    }
}

/// Light severity levels for tool outputs that are unusually large for context.
pub fn approx_tool_output_token_severity(tokens: usize) -> ApproxTokenSeverity {
    if tokens >= 12_000 {
        ApproxTokenSeverity::Danger
    } else if tokens >= 4_000 {
        ApproxTokenSeverity::Warning
    } else {
        ApproxTokenSeverity::Normal
    }
}

/// Extract the payload from an SSE `data:` line.
///
/// The SSE spec allows an optional single space after the colon, so both
/// `data:{...}` and `data: {...}` are valid and should parse identically.
pub fn sse_data_line(line: &str) -> Option<&str> {
    line.strip_prefix("data:")
        .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
}

/// Incremental UTF-8 decoder for byte streams whose chunk boundaries are not
/// guaranteed to fall on character boundaries.
///
/// Providers stream SSE over HTTPS, and a multi-byte character (e.g. a 3-byte
/// CJK codepoint) is routinely split across two TCP chunks. Calling
/// `str::from_utf8` on each chunk in isolation fails in that case, and callers
/// that used `if let Ok(text) = ...` silently dropped the entire chunk, losing
/// text and truncating tool-call arguments. See issue #609.
///
/// This decoder carries the incomplete trailing bytes over to the next chunk so
/// no input is ever discarded. Genuinely invalid sequences are replaced with
/// U+FFFD rather than dropping surrounding good data.
#[derive(Debug, Default)]
pub struct Utf8StreamDecoder {
    carry: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode the next chunk, holding back any incomplete trailing character.
    pub fn decode(&mut self, chunk: &[u8]) -> String {
        let bytes: &[u8] = if self.carry.is_empty() {
            chunk
        } else {
            self.carry.extend_from_slice(chunk);
            &self.carry
        };

        match std::str::from_utf8(bytes) {
            Ok(text) => {
                let out = text.to_string();
                self.carry.clear();
                out
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                // `valid_up_to` is by definition the length of the valid prefix,
                // so this conversion is exact and never actually substitutes.
                let good = String::from_utf8_lossy(&bytes[..valid_up_to]).into_owned();
                let rest = bytes[valid_up_to..].to_vec();
                match error.error_len() {
                    // Truncated but potentially valid: keep it for the next chunk.
                    None => {
                        self.carry = rest;
                        good
                    }
                    // Genuinely invalid bytes: emit a replacement char and keep going
                    // rather than discarding the rest of the chunk.
                    Some(invalid_len) => {
                        self.carry.clear();
                        let mut out = good;
                        out.push('\u{FFFD}');
                        out.push_str(&self.decode(&rest[invalid_len..]));
                        out
                    }
                }
            }
        }
    }

    /// Flush anything still held back when the stream ends. A non-empty result
    /// means the stream ended mid-character, which is upstream truncation.
    pub fn flush(&mut self) -> String {
        if self.carry.is_empty() {
            return String::new();
        }
        let carry = std::mem::take(&mut self.carry);
        String::from_utf8_lossy(&carry).into_owned()
    }

    pub fn has_pending_bytes(&self) -> bool {
        !self.carry.is_empty()
    }
}

/// Split a payload that contains several concatenated JSON objects.
///
/// Some proxies drop the `\n\n` SSE event separator, producing payloads like
/// `{"id":"a"}{"id":"b"}` or `{"id":"a"}data: {"id":"b"}`. `serde_json` rejects
/// those wholesale with "trailing characters", which discarded every object in
/// the blob. Callers should attempt a normal parse first and only fall back to
/// this on failure, so well-formed payloads keep their existing behavior.
///
/// Returns `None` when the payload is not a recoverable concatenation (fewer
/// than two complete objects), leaving the caller's original error intact.
/// A trailing incomplete object is reported separately so the caller can carry
/// it over to the next event instead of losing it.
pub fn split_concatenated_json(payload: &str) -> Option<SplitJson> {
    let bytes = payload.as_bytes();
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    let mut in_string = false;
    let mut escaped = false;

    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(s) = start.take()
                {
                    objects.push(payload[s..=i].to_string());
                }
            }
            _ => {}
        }
    }

    // An unterminated final object: hand the partial text back to the caller.
    let trailing_partial = start.map(|s| payload[s..].to_string());

    if objects.len() < 2 && trailing_partial.is_none() {
        return None;
    }
    if objects.is_empty() && trailing_partial.is_some() {
        return Some(SplitJson {
            objects,
            trailing_partial,
        });
    }
    Some(SplitJson {
        objects,
        trailing_partial,
    })
}

/// Result of [`split_concatenated_json`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitJson {
    /// Complete, brace-balanced JSON objects, in stream order.
    pub objects: Vec<String>,
    /// A trailing object whose braces never closed, if any.
    pub trailing_partial: Option<String>,
}

#[cfg(unix)]
fn read_max_open_files_limits() -> Option<(String, String)> {
    let contents = std::fs::read_to_string("/proc/self/limits").ok()?;
    contents.lines().find_map(|line| {
        let parts: Vec<_> = line.split_whitespace().collect();
        (parts.len() >= 5 && parts[0] == "Max" && parts[1] == "open" && parts[2] == "files")
            .then(|| (parts[3].to_string(), parts[4].to_string()))
    })
}

/// Summarize the current process's file-descriptor usage for debugging reload or
/// connect failures such as EMFILE/`Too many open files`.
pub fn process_fd_diagnostic_snapshot() -> String {
    #[cfg(unix)]
    {
        let pid = std::process::id();
        let fd_dir = std::path::Path::new("/proc/self/fd");
        let mut total = 0usize;
        let mut sockets = 0usize;
        let mut pipes = 0usize;
        let mut anon = 0usize;
        let mut chars = 0usize;
        let mut regs = 0usize;
        let mut dirs = 0usize;
        let mut other = 0usize;

        if let Ok(entries) = std::fs::read_dir(fd_dir) {
            for entry in entries.flatten() {
                total += 1;
                let target = std::fs::read_link(entry.path())
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if target.starts_with("socket:") {
                    sockets += 1;
                } else if target.starts_with("pipe:") {
                    pipes += 1;
                } else if target.starts_with("anon_inode:") {
                    anon += 1;
                } else if target.starts_with("/dev/") {
                    chars += 1;
                } else if target.starts_with('/') {
                    match std::fs::metadata(&target) {
                        Ok(meta) if meta.is_file() => regs += 1,
                        Ok(meta) if meta.is_dir() => dirs += 1,
                        Ok(_) | Err(_) => other += 1,
                    }
                } else {
                    other += 1;
                }
            }
        }

        let (soft_limit, hard_limit) = read_max_open_files_limits()
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        format!(
            "pid={} fds={} soft_limit={} hard_limit={} kinds={{socket:{}, pipe:{}, anon_inode:{}, char:{}, file:{}, dir:{}, other:{}}}",
            pid, total, soft_limit, hard_limit, sockets, pipes, anon, chars, regs, dirs, other
        )
    }

    #[cfg(not(unix))]
    {
        format!(
            "pid={} fd snapshot unsupported on this platform",
            std::process::id()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_multibyte() {
        // "学" is 3 bytes (E5 AD A6)
        let s = "abc学def";
        assert_eq!(truncate_str(s, 3), "abc"); // exactly before 学
        assert_eq!(truncate_str(s, 4), "abc"); // mid-char, back up
        assert_eq!(truncate_str(s, 5), "abc"); // mid-char, back up
        assert_eq!(truncate_str(s, 6), "abc学"); // exactly after 学
    }

    #[test]
    fn test_truncate_emoji() {
        // "🦀" is 4 bytes
        let s = "hi🦀bye";
        assert_eq!(truncate_str(s, 2), "hi");
        assert_eq!(truncate_str(s, 3), "hi"); // mid-emoji
        assert_eq!(truncate_str(s, 5), "hi"); // mid-emoji
        assert_eq!(truncate_str(s, 6), "hi🦀");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate_str("", 10), "");
        assert_eq!(truncate_str("hello", 0), "");
    }

    #[test]
    fn test_truncate_boundary_inside_multibyte_does_not_panic() {
        // Regression for issue #398: a multibyte char ('改', 3 bytes) straddling
        // the byte-200 boundary used to panic with a raw `&s[..200]` slice.
        // 199 ASCII bytes + '改' places the char at bytes 199..202.
        let s = format!("{}改", "a".repeat(199));
        let truncated = truncate_str(&s, 200);
        // Backs up to the previous char boundary (byte 199), never panics.
        assert_eq!(truncated.len(), 199);
        assert!(s.starts_with(truncated));
    }

    #[test]
    fn test_sse_data_line_accepts_optional_space() {
        assert_eq!(sse_data_line("data: {\"ok\":true}"), Some("{\"ok\":true}"));
        assert_eq!(sse_data_line("data:{\"ok\":true}"), Some("{\"ok\":true}"));
        assert_eq!(sse_data_line("event: message"), None);
    }

    #[test]
    fn test_format_number_adds_commas() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(12), "12");
        assert_eq!(format_number(1_234), "1,234");
        assert_eq!(format_number(12_345_678), "12,345,678");
    }

    #[test]
    fn test_format_approx_token_count_compacts_thousands() {
        assert_eq!(format_approx_token_count(999), "999 tok");
        assert_eq!(format_approx_token_count(1_000), "1k tok");
        assert_eq!(format_approx_token_count(1_900), "1.9k tok");
        assert_eq!(format_approx_token_count(10_000), "10k tok");
    }

    #[test]
    fn test_process_fd_diagnostic_snapshot_mentions_pid() {
        let snapshot = process_fd_diagnostic_snapshot();
        assert!(snapshot.contains("pid="));
    }

    #[test]
    fn test_approx_tool_output_token_severity_thresholds() {
        assert_eq!(
            approx_tool_output_token_severity(3_999),
            ApproxTokenSeverity::Normal
        );
        assert_eq!(
            approx_tool_output_token_severity(4_000),
            ApproxTokenSeverity::Warning
        );
        assert_eq!(
            approx_tool_output_token_severity(11_999),
            ApproxTokenSeverity::Warning
        );
        assert_eq!(
            approx_tool_output_token_severity(12_000),
            ApproxTokenSeverity::Danger
        );
    }
}

#[cfg(test)]
#[path = "sse_stream_integrity_tests.rs"]
mod sse_stream_integrity_tests;
