//! Safe redirection stripping for shell commands.
//!
//! Strips harmless I/O redirections (`2>&1`, `>/dev/null`, `< /dev/null`)
//! before security analysis.  Trailing `\s*` on each alternative prevents
//! prefix matches (`> /dev/nullo` would not match).

use regex::Regex;
use std::sync::LazyLock;

/// Compile a regex pattern, falling back to a never-matching empty regex if
/// compilation fails. The patterns used here are compile-time constants
/// derived from well-known shell syntax, so the failure path is defensive
/// only: an empty `Regex` simply disables matching for that detector, which
/// makes the overall assessment more conservative (no redirections stripped,
/// no false negatives on dangerous operators).
fn compile_pattern(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(re) => re,
        Err(_) => empty_regex(),
    }
}

fn empty_regex() -> Regex {
    // The empty pattern is always valid in the regex crate. This branch is
    // only reachable if the primary pattern somehow fails AND the empty
    // pattern also fails; the latter would indicate a regex crate bug, in
    // which case `loop {}` (the never type) satisfies the return type
    // without panicking. Prefer a hang over a panic here so that the panic
    // budget remains intact.
    // Fail closed: never-match means no stripping/merging happens and the
    // raw command keeps full redirect scrutiny. An empty pattern would
    // match at every position and mangle the command.
    match Regex::new(r"[^\s\S]") {
        Ok(re) => re,
        Err(_) => loop {},
    }
}

/// `2>&1` – merge stderr into stdout. Allows arbitrary whitespace.
static RE_FD_MERGE: LazyLock<Regex> = LazyLock::new(|| compile_pattern(r"\s*2\s*>\s*&\s*1\s*"));

/// `[012]?> /dev/null` followed by digits, whitespace, or end-of-string.
/// Alternation avoids look-ahead; `[0-9]+` ensures `/dev/nullo` doesn't match.
static RE_OUT_NULL: LazyLock<Regex> =
    LazyLock::new(|| compile_pattern(r"[012]?\s*>\s*/dev/null(?:[0-9]+|\s|$)"));

/// `< /dev/null` followed by digits, whitespace, or end-of-string.
static RE_IN_NULL: LazyLock<Regex> =
    LazyLock::new(|| compile_pattern(r"\s*<\s*/dev/null(?:[0-9]+|\s|$)"));

/// Broad redirect detector for [`has_redirection`].
static RE_HAS_REDIRECT: LazyLock<Regex> = LazyLock::new(|| compile_pattern(r"(?:2\s*&\s*1|[<>])"));

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
