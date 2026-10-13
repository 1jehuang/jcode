//! Safe redirection stripping for shell commands.
//!
//! Strips harmless I/O redirections (`2>&1`, `>/dev/null`, `< /dev/null`)
//! before security analysis.  Trailing `\s*` on each alternative prevents
//! prefix matches (`> /dev/nullo` would not match).

use regex::Regex;
use std::sync::LazyLock;

/// `2>&1` – merge stderr into stdout. Allows arbitrary whitespace.
static RE_FD_MERGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*2\s*>\s*&\s*1\s*").expect("valid"));

/// `[012]?> /dev/null` followed by digits, whitespace, or end-of-string.
/// Alternation avoids look-ahead; `[0-9]+` ensures `/dev/nullo` doesn't match.
static RE_OUT_NULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[012]?\s*>\s*/dev/null(?:[0-9]+|\s|$)").expect("valid"));

/// `< /dev/null` followed by digits, whitespace, or end-of-string.
static RE_IN_NULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*<\s*/dev/null(?:[0-9]+|\s|$)").expect("valid"));

/// Broad redirect detector for [`has_redirection`].
static RE_HAS_REDIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:2\s*&\s*1|[<>])").expect("valid"));

/// Strip safe, harmless redirections from a command string.
///
/// Removes: `2>&1`, `>/dev/null` (optionally `1>` or `2>`), `< /dev/null`,
/// and numbered variants (`>/dev/null1`).  Each requires surrounding
/// whitespace or end-of-string to avoid prefix matches.
pub fn strip_safe_redirections(command: &str) -> String {
    let result = command.to_string();
    let result = RE_FD_MERGE.replace_all(&result, " ");
    let result = RE_OUT_NULL.replace_all(&result, "");
    let result = RE_IN_NULL.replace_all(&result, "");
    collapse_whitespace(&result)
}

/// Return `true` if the command contains any shell redirection operator.
pub fn has_redirection(command: &str) -> bool {
    RE_HAS_REDIRECT.is_match(command)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_ascii_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fd_merge() {
        assert_eq!(strip_safe_redirections("echo hi 2>&1"), "echo hi");
    }

    #[test]
    fn strip_fd_merge_extra_spaces() {
        assert_eq!(strip_safe_redirections("echo hi  2  >  &  1"), "echo hi");
    }

    #[test]
    fn strip_output_null() {
        assert_eq!(strip_safe_redirections("cmd > /dev/null"), "cmd");
    }

    #[test]
    fn strip_output_null_no_space_before() {
        // >/dev/null is valid bash redirect; strip it.
        assert_eq!(strip_safe_redirections("echo>/dev/null"), "echo");
    }

    #[test]
    fn strip_fd1_and_fd2_output_null() {
        assert_eq!(strip_safe_redirections("cmd 1>/dev/null"), "cmd");
        assert_eq!(strip_safe_redirections("cmd 2>/dev/null"), "cmd");
    }

    #[test]
    fn strip_input_null() {
        assert_eq!(strip_safe_redirections("cat < /dev/null"), "cat");
    }

    #[test]
    fn strip_numbered_null() {
        assert_eq!(strip_safe_redirections("cmd > /dev/null1"), "cmd");
        assert_eq!(strip_safe_redirections("cmd 2>/dev/null99"), "cmd");
    }

    #[test]
    fn does_not_strip_prefix_path() {
        assert_eq!(
            strip_safe_redirections("echo hi > /dev/nullo"),
            "echo hi > /dev/nullo"
        );
        assert_eq!(
            strip_safe_redirections("cmd 2>/dev/nulls"),
            "cmd 2>/dev/nulls"
        );
    }

    #[test]
    fn multiple_redirections() {
        assert_eq!(
            strip_safe_redirections("cmd 2>/dev/null 2>&1 < /dev/null"),
            "cmd"
        );
    }

    #[test]
    fn no_redirections_unchanged() {
        assert_eq!(
            strip_safe_redirections("echo hello world"),
            "echo hello world"
        );
    }

    #[test]
    fn empty_and_only_redirects() {
        assert_eq!(strip_safe_redirections(""), "");
        assert_eq!(strip_safe_redirections("2>&1 > /dev/null"), "");
    }

    #[test]
    fn detect_redirects() {
        assert!(has_redirection("echo hi > file.txt"));
        assert!(has_redirection("cat < input.txt"));
        assert!(has_redirection("cmd 2>&1"));
        assert!(!has_redirection("echo hello"));
        assert!(!has_redirection(""));
    }
}
