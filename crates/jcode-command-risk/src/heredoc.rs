//! Line-based heredoc validation for bash commands.
//!
//! Detects and validates `$(cat <<'DELIM'...DELIM)` patterns inside command
//! substitution.  The implementation is a direct port of Claude Code's
//! `isSafeHeredoc` / `stripSafeHeredocSubstitutions` (bashSecurity.ts
//! L288-583) using line-based matching that replicates bash's closing
//! behaviour exactly.

use regex::Regex;

/// Quick heuristic: does the command even look like it has a heredoc in a
/// substitution?  Cheaper than the full scan.
pub fn has_heredoc_substitution(command: &str) -> bool {
    command.contains("$(") && command.contains("<<")
}

/// Match a safe heredoc-in-substitution pattern:
///
/// ```text
/// $(cat <<(-?)[ \t]*(('DELIM') | (\DELIM)))
/// ```
///
/// Group 1: optional dash for `<<-`
/// Group 2: single-quoted delimiter
/// Group 3: backslash-escaped delimiter
fn heredoc_open_pattern() -> Regex {
    // SAFETY: the pattern is a compile-time constant; compilation cannot fail.
    Regex::new(r#"\$\(cat[ \t]*<<(-?)[ \t]*(?:'+([A-Za-z_]\w*)'+|\\([A-Za-z_]\w*))"#).unwrap()
}

/// A validated heredoc span (byte offsets into the original command string).
#[derive(Debug, Clone)]
struct HeredocSpan {
    /// Byte offset where the `$(` begins.
    start: usize,
    /// Byte offset one past the closing `)`.
    end: usize,
}

/// Find every well-formed `$(cat <<'DELIM' ... DELIM)` or `$(cat <<\DELIM ...
/// DELIM)` in `command`, validating each with line-based delimiter matching.
///
/// Returns the spans sorted ascending by `start`, or an empty vec if none are
/// valid.
fn find_valid_heredocs(command: &str) -> Vec<HeredocSpan> {
    let re = heredoc_open_pattern();
    // (start, operator_end, delimiter, is_dash)
    let mut candidates: Vec<(usize, usize, String, bool)> = Vec::new();

    for cap in re.captures_iter(command) {
        let m = cap.get(0).unwrap();
        let delimiter = cap
            .get(2)
            .or_else(|| cap.get(3))
            .map(|d| d.as_str().to_string());
        let Some(delimiter) = delimiter else {
            continue;
        };
        let is_dash = cap.get(1).is_some_and(|d| d.as_str() == "-");
        candidates.push((m.start(), m.end(), delimiter, is_dash));
    }

    let mut verified: Vec<HeredocSpan> = Vec::new();

    for (start, operator_end, delimiter, is_dash) in &candidates {
        // The opening line must end with only horizontal whitespace after the
        // operator (no `; rm -rf /` appended).
        let after_operator = &command[*operator_end..];
        let open_line_end = match after_operator.find('\n') {
            Some(e) => e,
            None => return Vec::new(),
        };
        let open_line_tail = &after_operator[..open_line_end];
        if !open_line_tail.chars().all(|c| c == ' ' || c == '\t') {
            continue;
        }

        let body_start = *operator_end + open_line_end + 1;
        let body = &command[body_start..];
        let body_lines: Vec<&str> = body.split('\n').collect();

        // Find the FIRST matching delimiter line (line-based, not regex).
        let mut closing_line_idx: Option<usize> = None;
        let mut close_paren_line_idx: Option<usize> = None;
        let mut close_paren_col: usize = 0;

        for (i, raw_line) in body_lines.iter().enumerate() {
            let line = if *is_dash {
                raw_line.trim_start_matches('\t')
            } else {
                *raw_line
            };

            // Form 1: delimiter alone on a line, `)` on the NEXT line.
            if line == *delimiter {
                closing_line_idx = Some(i);
                let next_line = match body_lines.get(i + 1) {
                    Some(l) => *l,
                    None => return Vec::new(),
                };
                let trimmed = next_line.trim_start();
                if !trimmed.starts_with(')') {
                    return Vec::new();
                }
                close_paren_line_idx = Some(i + 1);
                let leading = next_line.len() - trimmed.len();
                close_paren_col = leading;
                break;
            }

            // Form 2: `DELIM)` inline (PST_EOFTOKEN).
            if let Some(after_delim) = line.strip_prefix(delimiter.as_str()) {
                let trimmed_after = after_delim.trim_start();
                if trimmed_after.starts_with(')') {
                    closing_line_idx = Some(i);
                    close_paren_line_idx = Some(i);
                    let tab_prefix_len = if *is_dash {
                        raw_line.len() - raw_line.trim_start_matches('\t').len()
                    } else {
                        0
                    };
                    let spaces_before_paren =
                        after_delim.len() - after_delim.trim_start_matches([' ', '\t']).len();
                    close_paren_col = tab_prefix_len + delimiter.len() + spaces_before_paren;
                    break;
                }
                // Starts with delimiter but has other content — reject.
                if trimmed_after.starts_with([')', '}', '|', '&', ';', '<', '>']) {
                    continue;
                }
            }
        }

        let Some(_closing_idx) = closing_line_idx else {
            continue;
        };
        let Some(paren_idx) = close_paren_line_idx else {
            continue;
        };

        // Compute absolute end position (one past `)`).
        let mut end_pos = body_start;
        for line in body_lines.iter().take(paren_idx) {
            end_pos += line.len() + 1;
        }
        end_pos += close_paren_col + 1;

        verified.push(HeredocSpan {
            start: *start,
            end: end_pos,
        });
    }

    // SECURITY: Reject if ANY nested matches exist. The regex finds
    // $(cat <<'X' patterns in raw text without understanding quoted-heredoc
    // semantics. A nested match inside a quoted body is always literal text.
    // Stripping nested ranges corrupts indices: the outer range's end becomes
    // stale, silently dropping any suffix (e.g. `; rm -rf /`). Bail entirely.
    for (i, inner) in verified.iter().enumerate() {
        if verified
            .iter()
            .enumerate()
            .any(|(j, outer)| i != j && inner.start > outer.start && inner.start < outer.end)
        {
            return Vec::new();
        }
    }

    verified
}

/// Strip all validated heredoc substitutions from the command and return the
/// remainder.  Returns `None` if no safe heredoc was found.
fn strip_heredocs(command: &str) -> Option<String> {
    if !has_heredoc_substitution(command) {
        return None;
    }

    let mut spans = find_valid_heredocs(command);
    if spans.is_empty() {
        return None;
    }

    // SECURITY: verify there is a non-whitespace prefix before the first
    // heredoc — the substitution must be in argument position, not command
    // name position (where the heredoc body becomes the command to execute).
    let first_start = spans.iter().map(|s| s.start).min().unwrap_or(0);
    let prefix = &command[..first_start];
    let rest = &command[first_start..];
    // Compute trimmed remainder after all closing parens are accounted for.
    let trimmed_remaining = rest.trim();
    if !trimmed_remaining.is_empty() && prefix.trim().is_empty() {
        return None;
    }

    // SECURITY: remaining text must contain only safe characters (no shell
    // metacharacters that could chain a dangerous command).
    let safe_re = Regex::new(r#"^[a-zA-Z0-9 \t"'.\-/_@=,:+~]*$"#).unwrap();

    // Strip in reverse order so earlier indices stay valid.
    spans.sort_by_key(|a| std::cmp::Reverse(a.start));
    let mut result = command.to_string();
    for span in &spans {
        result.replace_range(span.start..span.end, "");
    }
    if !safe_re.is_match(&result) {
        return None;
    }

    Some(result)
}

/// Checks if a heredoc substitution is "safe" — a `cat` command with a
/// single-quoted or backslash-escaped delimiter (so the body is literal text
/// with no shell expansion).
pub fn is_safe_heredoc(command: &str) -> bool {
    strip_heredocs(command).is_some()
}

/// Detect well-formed `$(cat <<'DELIM'...DELIM)` patterns and return the
/// command with matched heredocs stripped.  Returns `None` if no safe heredoc
/// was found.
pub fn strip_safe_heredoc_substitutions(command: &str) -> Option<String> {
    strip_heredocs(command)
}

#[cfg(test)]
#[path = "heredoc_tests.rs"]
mod heredoc_tests;
