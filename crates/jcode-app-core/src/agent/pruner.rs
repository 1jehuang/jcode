//! SWE-Pruner Stage 1 pilot: deterministic line scorer (PILOT-RESULTS gate).
//!
//! Pre-offload display-copy line filter (PLAN.md section 3). The masking
//! offload path keeps the FULL original bytes; this scorer only prunes the
//! send-view copy. Unwired from `apply_tool_result_clearing` during the
//! pilot: scoring is exercised by the frozen-corpus gate test only.
//!
//! Rules (per line, in order):
//! 1. Error lines (`error|fail|panic|assert|traceback`, case-insensitive)
//!    are ALWAYS kept whole (PLAN section 4, never truncated).
//! 2. Blank / whitespace-only lines are dropped.
//! 3. Non-error mega-lines (> `MEGA_CHARS` chars) are truncated to
//!    `MEGA_KEEP` chars plus a length marker.
//! 4. Import-block runs (>= `IMPORT_RUN` consecutive import-ish lines) keep
//!    the first `IMPORT_KEEP`.
//! 5. Repeat runs (>= `REPEAT_RUN` consecutive identical full lines, each >=
//!    `REPEAT_MIN_CHARS` chars) keep the first `REPEAT_KEEP`.
//! 6. `read`/`grep`-family only: non-first lines with zero token overlap
//!    against the ToolUse-derived hint set are dropped. Test-runner
//!    outputs get repeat-collapsing only, never content drops.
//! 7. Single dropped lines between two kept lines are healed (kept), matching
//!    the paper's single-line-gap healing.
//! 8. Dropped runs become one `(filtered N lines)` marker each (paper format).
//!
//! Fail open: scorer error or zero kept lines returns the input unchanged.

use std::collections::HashSet;
use std::time::Instant;

/// Minimum token length for hint/overlap tokens.
const MIN_TOK_LEN: usize = 3;
/// Non-error lines longer than this are truncated, not dropped.
const MEGA_CHARS: usize = 600;
/// Kept prefix length for truncated mega-lines.
const MEGA_KEEP: usize = 200;
/// Consecutive import-ish lines that form a prunable block.
const IMPORT_RUN: usize = 3;
/// Kept head lines of an import block.
const IMPORT_KEEP: usize = 2;
/// Consecutive identical lines that form a prunable repeat run.
const REPEAT_RUN: usize = 3;
/// Kept head lines of a repeat run.
const REPEAT_KEEP: usize = 2;
/// Lines shorter than this never participate in repeat-run collapsing
/// (avoids noise-collapsing `}` / `---` / blank-ish separators).
const REPEAT_MIN_CHARS: usize = 20;

/// Stopwords excluded from hint/overlap token sets.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "are", "was", "were", "have", "has",
    "had", "not", "but", "you", "your", "all", "any", "can", "will", "would", "should", "there",
    "their", "they", "them", "then", "than", "into", "over", "under", "when", "where", "which",
    "while", "about", "after", "before", "between", "withs",
];

/// Per-item scoring outcome (PLAN section 6 record shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneOutcome {
    /// Pruned send-view text (original on fail-open).
    pub text: String,
    /// Total dropped lines replaced by `(filtered N lines)` markers.
    pub filtered_lines: usize,
    /// Scorer wall time in microseconds.
    pub scorer_us: u128,
}

/// Tool families the Stage 1 scorer handles. Unknown families bypass
/// (same backward-compat rule as the paper's missing-hint bypass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrunerFamily {
    Read,
    Grep,
    TestBash,
}

/// Classify a tool name into a prunable family; `None` bypasses.
pub fn classify_tool(tool_name: &str) -> Option<PrunerFamily> {
    let lower = tool_name.to_lowercase();
    if lower.contains("read") || lower.contains("cat") {
        Some(PrunerFamily::Read)
    } else if lower.contains("grep") || lower.contains("search") {
        Some(PrunerFamily::Grep)
    } else if lower == "bash"
        || lower.contains("test")
        || lower.contains("cargo")
        || lower.contains("exec")
        || lower.contains("shell")
    {
        Some(PrunerFamily::TestBash)
    } else {
        None
    }
}

/// Case-insensitive `error|fail|panic|assert|traceback` line match
/// (PLAN section 4 policy; always kept, never truncated).
pub fn is_error_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let needle = |w: &[u8]| {
        bytes
            .windows(w.len())
            .any(|win| win.eq_ignore_ascii_case(w))
    };
    needle(b"error")
        || needle(b"fail")
        || needle(b"panic")
        || needle(b"assert")
        || needle(b"traceback")
}

/// Import-ish line head (PLAN section 4 boilerplate-import-block signal).
fn is_import_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("use ")
        || t.starts_with("import ")
        || t.starts_with("#include")
        || t.starts_with("require(")
        || (t.starts_with("from ") && t.contains(" import "))
        || (t.starts_with("export ") && t.contains(" from "))
}

fn is_stopword(tok: &str) -> bool {
    STOPWORDS.contains(&tok)
}

fn add_terms(set: &mut HashSet<String>, text: &str) {
    // ASCII-alphanumeric runs, lowercased (prototype parity: TOK_RE [a-z0-9]+).
    // Byte-walk is safe: every boundary byte is non-alphanumeric ASCII, so
    // token slices always land on char boundaries.
    let bytes = text.as_bytes();
    let mut start: Option<usize> = None;
    for (i, b) in bytes.iter().enumerate() {
        if b.is_ascii_alphanumeric() {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            let tok = text[s..i].to_lowercase();
            if tok.len() >= MIN_TOK_LEN && !is_stopword(&tok) {
                set.insert(tok);
            }
        }
    }
    if let Some(s) = start.take() {
        let tok = text[s..].to_lowercase();
        if tok.len() >= MIN_TOK_LEN && !is_stopword(&tok) {
            set.insert(tok);
        }
    }
}

/// Deterministic goal-hint term set derived from the ToolUse block
/// (PLAN section 4: tool name + grep pattern / read path / command argv,
/// never agent-generated).
pub fn hint_terms(
    tool_name: &str,
    file_path: &str,
    query: &str,
    command: &str,
    extra: &str,
) -> HashSet<String> {
    let mut set = HashSet::new();
    add_terms(&mut set, tool_name);
    add_terms(&mut set, &file_path.replace(['/', '.'], " "));
    add_terms(&mut set, query);
    add_terms(&mut set, command);
    add_terms(&mut set, extra);
    set
}

fn line_terms(line: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    add_terms(&mut set, line);
    set
}

/// Score one tool result. Returns the pruned copy plus stats; fails open
/// to the original string on any degenerate outcome.
pub fn prune_tool_result(
    family: PrunerFamily,
    tool_name: &str,
    file_path: &str,
    query: &str,
    command: &str,
    extra_hint: &str,
    content: &str,
) -> PruneOutcome {
    let started = Instant::now();
    let outcome = prune_inner(family, tool_name, file_path, query, command, extra_hint, content);
    let scorer_us = started.elapsed().as_micros();
    PruneOutcome {
        text: outcome.0,
        filtered_lines: outcome.1,
        scorer_us,
    }
}

fn prune_inner(
    family: PrunerFamily,
    tool_name: &str,
    file_path: &str,
    query: &str,
    command: &str,
    extra_hint: &str,
    content: &str,
) -> (String, usize) {
    let lines: Vec<&str> = content.split('\n').collect();
    let n = lines.len();
    let mut keep = vec![true; n];
    let mut truncated: Vec<Option<String>> = vec![None; n];

    for (i, line) in lines.iter().enumerate() {
        if is_error_line(line) {
            continue;
        }
        if line.trim().is_empty() {
            keep[i] = false;
            continue;
        }
        if line.chars().count() > MEGA_CHARS {
            let prefix: String = line.chars().take(MEGA_KEEP).collect();
            let rest = line.chars().count() - MEGA_KEEP;
            truncated[i] = Some(format!("{prefix}... (truncated {rest} chars)"));
        }
    }

    // Import-block runs.
    let mut i = 0;
    while i < n {
        if keep[i] && truncated[i].is_none() && is_import_line(lines[i]) {
            let mut j = i;
            while j < n && keep[j] && truncated[j].is_none() && is_import_line(lines[j]) {
                j += 1;
            }
            if j - i >= IMPORT_RUN {
                for k in (i + IMPORT_KEEP)..j {
                    keep[k] = false;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // Repeat runs (identical full lines, length floor, error lines exempt).
    let mut i = 0;
    while i < n {
        if keep[i]
            && truncated[i].is_none()
            && lines[i].chars().count() >= REPEAT_MIN_CHARS
            && !is_error_line(lines[i])
        {
            let mut j = i + 1;
            while j < n
                && keep[j]
                && truncated[j].is_none()
                && lines[j].chars().count() >= REPEAT_MIN_CHARS
                && !is_error_line(lines[j])
                && lines[j] == lines[i]
            {
                j += 1;
            }
            if j - i >= REPEAT_RUN {
                for k in (i + REPEAT_KEEP)..j {
                    keep[k] = false;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // Zero-overlap drop for read/grep families only (never test output).
    if family == PrunerFamily::Read || family == PrunerFamily::Grep {
        let hint = hint_terms(tool_name, file_path, query, command, extra_hint);
        for (i, line) in lines.iter().enumerate() {
            if i == 0 || !keep[i] || truncated[i].is_some() || is_error_line(line) {
                continue;
            }
            if line_terms(line).is_disjoint(&hint) {
                keep[i] = false;
            }
        }
    }

    // Heal single-line gaps.
    for i in 1..n.saturating_sub(1) {
        if !keep[i] && keep[i - 1] && keep[i + 1] {
            keep[i] = true;
        }
    }

    if !keep.iter().any(|k| *k) {
        return (content.to_string(), 0);
    }
    let mut out: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    let mut filtered_total = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            if dropped > 0 {
                out.push(format!("(filtered {dropped} lines)"));
                filtered_total += dropped;
                dropped = 0;
            }
            match &truncated[i] {
                Some(t) => out.push(t.clone()),
                None => out.push((*line).to_string()),
            }
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        out.push(format!("(filtered {dropped} lines)"));
        filtered_total += dropped;
    }
    if filtered_total == 0 && truncated.iter().all(|t| t.is_none()) {
        return (content.to_string(), 0);
    }
    (out.join("\n"), filtered_total)
}

/// Metering-harness counting rule: `tokens_for_text` = ceiling(chars/4),
/// multibyte-aware (chars, not bytes).
pub fn tokens_for_text(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_always_kept_whole() {
        let content = "ok line\nFAILED: something broke badly\nanother ok";
        let out = prune_tool_result(PrunerFamily::TestBash, "bash", "", "", "", "", content);
        assert!(out.text.contains("FAILED: something broke badly"));
    }

    #[test]
    fn error_lines_never_truncated_even_when_mega() {
        let long_err = format!("assertion failed: {}", "x".repeat(2000));
        let out = prune_tool_result(PrunerFamily::TestBash, "bash", "", "", "", "", &long_err);
        assert!(out.text.contains(&"x".repeat(2000)));
    }

    #[test]
    fn unknown_tool_classification_bypasses() {
        assert_eq!(classify_tool("write"), None);
        assert_eq!(classify_tool("todo"), None);
        assert_eq!(classify_tool("memory"), None);
    }

    #[test]
    fn blank_lines_dropped_with_marker() {
        let out = prune_tool_result(PrunerFamily::TestBash, "bash", "", "", "", "", "a\n\n\nb");
        assert!(out.text.contains("(filtered 2 lines)"));
    }

    #[test]
    fn single_gap_healed() {
        let out = prune_tool_result(PrunerFamily::TestBash, "bash", "", "", "", "", "aaa\n\nbbb");
        assert!(!out.text.contains("filtered"));
    }

    #[test]
    fn nothing_dropped_returns_original() {
        let content = "short\nlines";
        let out = prune_tool_result(PrunerFamily::TestBash, "bash", "", "", "", "", content);
        assert_eq!(out.text, content);
        assert_eq!(out.filtered_lines, 0);
    }

    #[test]
    fn mega_line_truncated_with_marker() {
        let content = format!("{}\nshort tail", "y".repeat(700));
        let out = prune_tool_result(PrunerFamily::TestBash, "bash", "", "", "", "", &content);
        assert!(out.text.contains("(truncated 500 chars)"));
    }
}
