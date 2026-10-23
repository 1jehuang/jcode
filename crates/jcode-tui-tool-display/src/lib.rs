use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Map provider-side tool names to internal display names.
/// Mirrors Registry::resolve_tool_name so TUI surfaces show friendly names.
pub fn resolve_display_tool_name(name: &str) -> &str {
    match name {
        "communicate" => "swarm",
        "discover_tools" => "integration_tools",
        "task" | "task_runner" => "subagent",
        "shell_exec" => "bash",
        "file_read" => "read",
        "file_write" => "write",
        "file_edit" => "edit",
        "file_glob" => "glob",
        "file_grep" => "grep",
        "todo_read" | "todo_write" | "todoread" | "todowrite" => "todo",
        other => other,
    }
}

pub fn canonical_tool_name(name: &str) -> &str {
    match name {
        "communicate" => "swarm",
        "discover_tools" => "integration_tools",
        "Write" => "write",
        "Edit" => "edit",
        "MultiEdit" => "multiedit",
        "Patch" => "patch",
        "ApplyPatch" => "apply_patch",
        other => other,
    }
}

pub fn is_edit_tool_name(name: &str) -> bool {
    matches!(
        canonical_tool_name(name),
        "write" | "edit" | "multiedit" | "patch" | "apply_patch" | "replace"
    )
}

/// Canonical name refined by input shape, for diff and summary rendering.
/// `edit` with an `edits` array renders like the old `multiedit`, and
/// `apply_patch` carrying a unified diff renders like the old `patch`.
pub fn edit_render_name<'a>(name: &'a str, input: &serde_json::Value) -> &'a str {
    match canonical_tool_name(name) {
        "edit" if input.get("edits").is_some_and(|edits| edits.is_array()) => "multiedit",
        "apply_patch"
            if input
                .get("patch_text")
                .and_then(|text| text.as_str())
                .is_some_and(|text| {
                    !text.contains("*** Begin Patch")
                        && text.lines().any(|line| line.starts_with("+++ "))
                }) =>
        {
            "patch"
        }
        other => other,
    }
}

/// A compact diagnostic, independent of transport wrappers and UI truncation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFailureSummary {
    pub reason: String,
    pub exit_code: Option<i32>,
}

fn exit_code(line: &str) -> Option<i32> {
    let lower = line.to_ascii_lowercase();
    let rest = [
        "exit status:",
        "exit code:",
        "exit code ",
        "exited with code ",
        "exited code ",
        "exited code",
    ]
    .iter()
    .find_map(|marker| lower.find(marker).map(|at| &line[at + marker.len()..]))?;
    let number = rest
        .trim_start()
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .next()?;
    number.parse().ok()
}

fn strip_wrappers(mut line: &str) -> (&str, u8) {
    let mut severity = 0;
    loop {
        line = line.trim();
        let lower = line.to_ascii_lowercase();
        if let Some(prefix) = ["error:", "failed:", "fatal:"]
            .iter()
            .find(|prefix| lower.starts_with(**prefix))
        {
            line = &line[prefix.len()..];
            severity = severity.max(if *prefix == "fatal:" { 65 } else { 50 });
            continue;
        }
        if let Some(rest) = line.strip_prefix('✗') {
            line = rest;
            severity = severity.max(50);
            continue;
        }
        if let Some(rest) = line.strip_prefix('[')
            && let Some((tag, tail)) = rest.split_once(']')
            && !tag.is_empty()
            && tag.chars().all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
            // Uppercase errno and compiler codes are content, not transport labels.
            && !tag.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            line = tail;
            continue;
        }
        return (line.trim(), severity);
    }
}

fn display_prefix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut used = 0usize;
    let mut end = 0usize;
    for (idx, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        used += cw;
        end = idx + ch.len_utf8();
    }
    &s[..end]
}

fn display_suffix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut used = 0usize;
    let mut start = s.len();
    for (idx, ch) in s.char_indices().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + cw > max_width {
            break;
        }
        used += cw;
        start = idx;
    }
    &s[start..]
}

pub fn truncate_middle_display(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let remaining = max_width.saturating_sub(1);
    let head = remaining / 2 + remaining % 2;
    let tail = remaining / 2;
    format!(
        "{}…{}",
        display_prefix_by_width(s, head),
        display_suffix_by_width(s, tail)
    )
}

/// Remove terminal control sequences for classification and compact display only.
/// The caller's original tool output remains untouched.
fn without_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for ch in chars.by_ref() {
                    if ('@'..='~').contains(&ch) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(ch) = chars.next() {
                    if ch == '\u{7}' || (ch == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Extract the strongest diagnostic across the entire output, without modifying
/// raw output or applying display-width limits. Wrappers and exit summaries are
/// weaker evidence than diagnostics and explicit cause chains.
pub fn parse_tool_failure(content: &str) -> Option<ToolFailureSummary> {
    let mut best: Option<(u8, String)> = None;
    let mut code = None;
    let mut cause_chain = false;
    let mut panic_location: Option<String> = None;
    let normalized = without_ansi(content);
    for raw in normalized.lines() {
        let raw = raw.trim_end();
        // Only a standalone timing record is metadata. A diagnostic may
        // legitimately contain the same text and must not be truncated.
        if raw.trim_start().starts_with("[tool timing:") && raw.ends_with(']') {
            continue;
        }
        let (mut line, marked) = strip_wrappers(raw);
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let is_exit = lower.starts_with("exit code:")
            || lower.starts_with("--- command finished with exit code:")
            || lower.starts_with("command exited")
            || lower.starts_with("process exited")
            || lower.starts_with("process didn't exit successfully");
        if is_exit
            && let Some(value) = exit_code(line)
            && value != 0
        {
            code = Some(value);
        }
        let mut score = marked;
        if let Some((tag, _)) = line.strip_prefix('[').and_then(|rest| rest.split_once(']'))
            && tag.starts_with('E')
            && tag.len() > 1
            && tag
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            score = 80;
        }
        if lower.starts_with("error[e") && line.contains("]:") {
            line = &line[5..];
            score = 80;
        }
        if lower.starts_with("caused by:") {
            cause_chain = true;
            line = line[10..].trim();
            score = 70;
        } else if cause_chain && raw.starts_with(char::is_whitespace) {
            if let Some((index, rest)) = line.split_once(':')
                && index.chars().all(|c| c.is_ascii_digit())
            {
                line = rest.trim();
            }
            score = 70;
        } else {
            cause_chain = false;
        }
        let mut panic_reason = None;
        if lower.starts_with("thread '") && lower.contains("panicked at") {
            panic_location = line
                .split_once("panicked at ")
                .and_then(|(_, location)| location.strip_suffix(':').map(str::to_string));
            score = 60;
        } else if let Some(location) = panic_location.take() {
            if !lower.starts_with("note:")
                && !lower.starts_with("stack backtrace:")
                && !lower.starts_with("backtrace:")
                && !line.starts_with(|c: char| c.is_ascii_digit())
            {
                panic_reason = Some(format!("{location}: {line}"));
                score = 85;
            }
        }
        if let Some((kind, _)) = line.split_once(':')
            && (kind.ends_with("Error") || kind.ends_with("Exception"))
            && kind
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        {
            score = 80;
        }
        if lower.contains("compile terminated by signal") {
            score = 60;
        }
        if lower == "status: failed" || lower == "failed to start" || lower == "terminated" {
            score = 20;
        }
        if is_exit
            || lower.starts_with("could not compile ")
            || lower.starts_with("aborting due to ")
        {
            score = if marked > 0 && !(is_exit && code.is_some()) {
                10
            } else {
                0
            };
        }
        let (reason, _) = strip_wrappers(line);
        if score > 0
            && !reason.is_empty()
            && best.as_ref().is_none_or(|(previous, _)| {
                score > *previous || (cause_chain && score == *previous)
            })
        {
            best = Some((score, panic_reason.unwrap_or_else(|| reason.to_string())));
        }
    }
    let reason = best
        .filter(|(score, _)| *score > 10 || code.is_none())
        .map(|(_, reason)| reason)
        .or_else(|| code.map(|code| format!("exit {code}")))?;
    Some(ToolFailureSummary {
        reason,
        exit_code: code,
    })
}

pub fn concise_tool_error_summary(content: &str) -> Option<String> {
    parse_tool_failure(content).map(|failure| failure.reason)
}

pub fn tool_output_looks_failed(content: &str) -> bool {
    parse_tool_failure(content).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_edit_tool_names() {
        assert_eq!(canonical_tool_name("ApplyPatch"), "apply_patch");
        assert!(is_edit_tool_name("MultiEdit"));
        assert!(!is_edit_tool_name("read"));
    }

    #[test]
    fn summarizes_tool_errors() {
        assert_eq!(
            concise_tool_error_summary("Error: missing field `command`").as_deref(),
            Some("missing field `command`")
        );
        assert_eq!(
            concise_tool_error_summary("--- Command finished with exit code: 2 ---").as_deref(),
            Some("exit 2")
        );
    }

    #[test]
    fn structural_failure_regressions() {
        for (input, reason, code) in [
            (
                "Error: [selfdev] Error: source metadata was for /old/path",
                "source metadata was for /old/path",
                None,
            ),
            (
                "Error: error: Error: Unexpected string",
                "Unexpected string",
                None,
            ),
            (
                "[stderr] warning: unused variable\n[stderr] error[E0308]: mismatched types",
                "[E0308]: mismatched types",
                None,
            ),
            (
                "Command exited code101\nError: /src/main.rs: incorrect argument",
                "/src/main.rs: incorrect argument",
                Some(101),
            ),
            (
                "error: could not compile `demo` due to 1 previous error\nerror[E0308]: mismatched types",
                "[E0308]: mismatched types",
                None,
            ),
            ("Error: Command exited with code 101", "exit 101", Some(101)),
            ("Exit code: 2", "exit 2", Some(2)),
            (
                "Error: request failed\nCaused by:\n    0: opening /tmp/data\n    1: permission denied",
                "permission denied",
                None,
            ),
            (
                "thread 'main' panicked at src/main.rs:4:2:\nmissing configuration /etc/demo",
                "src/main.rs:4:2: missing configuration /etc/demo",
                None,
            ),
            (
                "Traceback (most recent call last):\n  File \"main.py\", line 3\nValueError: invalid value",
                "ValueError: invalid value",
                None,
            ),
            (
                "[tool timing: duration=3ms]\nError: [read] Failed: unavailable",
                "unavailable",
                None,
            ),
        ] {
            assert_eq!(
                parse_tool_failure(input),
                Some(ToolFailureSummary {
                    reason: reason.into(),
                    exit_code: code
                }),
                "{input}"
            );
            assert!(tool_output_looks_failed(input));
            assert_eq!(concise_tool_error_summary(input).as_deref(), Some(reason));
        }
    }

    #[test]
    fn preserves_full_diagnostics_and_ignores_success() {
        let reason = format!("/a/long/path: {}", "detail ".repeat(100));
        assert_eq!(
            concise_tool_error_summary(&format!("Error: {reason}")),
            Some(reason.trim_end().into())
        );
        for output in [
            "",
            "Exit code: 0",
            "Command exited code0",
            "[stderr] warning: unused variable",
            "[read] completed successfully",
            "There was an error: in the example",
            "[tool timing: duration=3ms]",
            "Finished successfully\n0 errors, 0 failures",
        ] {
            assert_eq!(parse_tool_failure(output), None, "{output}");
            assert!(!tool_output_looks_failed(output));
        }
    }

    #[test]
    fn preserves_codes_and_normalizes_ansi() {
        for (raw, expected) in [
            (
                "Error: [read] [ENOENT] /tmp/missing",
                "[ENOENT] /tmp/missing",
            ),
            ("[stderr] [EACCES] /private/data", "[EACCES] /private/data"),
            (
                "\u{1b}[31mError:\u{1b}[0m [selfdev] Error: denied",
                "denied",
            ),
            (
                "\u{1b}[1;31merror[E0308]\u{1b}[0m: mismatched types",
                "[E0308]: mismatched types",
            ),
            (
                "Error: expected literal [tool timing: duration=3ms] here",
                "expected literal [tool timing: duration=3ms] here",
            ),
            (
                "Error: expected literal [tool timing: duration=3ms]",
                "expected literal [tool timing: duration=3ms]",
            ),
            (
                "\u{1b}]8;;https://example.invalid\u{1b}\\Error: denied\u{1b}]8;;\u{1b}\\",
                "denied",
            ),
        ] {
            assert_eq!(
                concise_tool_error_summary(raw).as_deref(),
                Some(expected),
                "{raw:?}"
            );
            assert!(tool_output_looks_failed(raw));
        }
        assert!(!tool_output_looks_failed(
            "\u{1b}[32m[stderr] warning: unused\u{1b}[0m"
        ));
    }

    #[test]
    fn panic_notes_and_backtraces_are_not_causes() {
        let header = "thread 'main' panicked at src/main.rs:42:9:";
        for suffix in [
            "note: run with RUST_BACKTRACE=1\nstack backtrace:\n   0: demo::main",
            "stack backtrace:\n   0: demo::main",
        ] {
            assert_eq!(
                concise_tool_error_summary(&format!("{header}\n{suffix}")).as_deref(),
                Some(header)
            );
        }
        assert_eq!(
            concise_tool_error_summary(&format!(
                "{header}\nactual cause\nnote: run with RUST_BACKTRACE=1"
            ))
            .as_deref(),
            Some("src/main.rs:42:9: actual cause")
        );
        let inline = "thread 'main' panicked at src/main.rs:42:9: actual cause";
        assert_eq!(
            concise_tool_error_summary(&format!("{inline}\nnote: run with RUST_BACKTRACE=1"))
                .as_deref(),
            Some(inline)
        );
    }

    #[test]
    fn exit_only_is_canonical_even_with_boilerplate() {
        for raw in [
            "Error: Command exited with code 101",
            "error: could not compile `demo` due to 1 previous error\nExit code: 101",
            "\u{1b}[31mExit code: 101\u{1b}[0m",
        ] {
            assert_eq!(
                parse_tool_failure(raw),
                Some(ToolFailureSummary {
                    reason: "exit 101".into(),
                    exit_code: Some(101)
                })
            );
        }
    }

    #[test]
    fn fatal_diagnostics_outrank_generic_errors() {
        let reason = "loose object abc123 is corrupt";
        for (output, exit_code) in [
            ("fatal: loose object abc123 is corrupt", None),
            (
                "error: object file .git/objects/ab/c123 is empty\nfatal: loose object abc123 is corrupt",
                None,
            ),
            (
                "Error: Command exited with code 128\nerror: object file .git/objects/ab/c123 is empty\n[stderr] fatal: loose object abc123 is corrupt",
                Some(128),
            ),
            (
                "Error: [stderr] FATAL: loose object abc123 is corrupt",
                None,
            ),
        ] {
            assert_eq!(
                parse_tool_failure(output),
                Some(ToolFailureSummary {
                    reason: reason.into(),
                    exit_code
                })
            );
            assert!(tool_output_looks_failed(output));
        }
    }

    #[test]
    fn detects_failed_tool_output() {
        assert!(tool_output_looks_failed("Status: failed"));
        assert!(tool_output_looks_failed("Exit code: 1"));
        assert!(tool_output_looks_failed(
            "✗ demo.txt: failed to find expected lines"
        ));
        assert!(tool_output_looks_failed(
            "[apply_patch] ✗ demo.txt: failed to find expected lines"
        ));
        assert!(!tool_output_looks_failed("Exit code: 0"));
        assert!(!tool_output_looks_failed("completed successfully"));
    }
}
