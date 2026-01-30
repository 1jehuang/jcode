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
}
