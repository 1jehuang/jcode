//! Deterministic evidence-span extraction for highlight-then-summarize (Phase 1).
//!
//! Converged-design compaction runner, grounding stage only:
//!
//! 1. **Ground FIRST.** [`extract_spans`] scans one transcript message and
//!    emits [`EvidenceSpan`]s for the load-bearing substrings: file paths,
//!    error strings, identifiers, and decision markers.
//! 2. **Compress around them.** A later stage (not this module) compresses
//!    the transcript while keeping span text verbatim, so paths and error
//!    strings survive compaction exactly — the coding-agent failure mode
//!    where a paraphrased path no longer resolves.
//!
//! Grounding: task brief (PROGRAM.md R6, evidence H2S 2609.31382) —
//! highlight-then-summarize for compaction: ground evidence spans first,
//! then summarize around them.
//!
//! Deliberately out of scope: the fluent summary itself (NEEDS-BRAIN —
//! judging relevance and writing prose needs a model and belongs in Phase
//! 2), ranking spans by importance, and any wiring into compaction paths.
//! The extractor outputs structured spans, nothing more. This module never
//! calls a model and never touches `memory.rs` scoring, `prompt.rs`,
//! `agent.rs`, or any compaction path.
//!
//! Precision bias: bare lowercase words (`error`, `failed`, bare `foo`) are
//! everywhere in prose, so they only count with a code-shaped companion
//! (punctuation, path separator, uppercase shape, backticks). Shaped
//! diagnostics (`error[E0428]`, `TypeError`, `Traceback`) match on their own
//! — prose almost never contains them.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// Clip span text past this length. Spans are verbatim substrings, so
/// clipping only touches pathological inputs (a pasted megabyte on one
/// line); offsets still locate the full original span.
pub const MAX_SPAN_TEXT_LEN: usize = 500;

/// Cap on spans emitted per message. Bounded so one pasted log dump cannot
/// bloat the output; the earliest spans win (a summary walks the transcript
/// in order, so early anchors matter most).
pub const MAX_SPANS_PER_MESSAGE: usize = 64;

/// What kind of load-bearing content a span carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// A file path, kept verbatim (location suffixes like `:10:5` included).
    FilePath,
    /// A full diagnostic line (compiler error, panic, traceback, ...).
    ErrorString,
    /// A code-shaped token: symbol, type, command, or config key.
    Identifier,
    /// A full line stating a decision taken.
    Decision,
}

impl SpanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpanKind::FilePath => "file_path",
            SpanKind::ErrorString => "error_string",
            SpanKind::Identifier => "identifier",
            SpanKind::Decision => "decision",
        }
    }

    /// Deterministic tiebreak when spans share a start offset: line-level
    /// spans (broader context) sort before token-level ones.
    fn rank(self) -> u8 {
        match self {
            SpanKind::ErrorString => 0,
            SpanKind::Decision => 1,
            SpanKind::FilePath => 2,
            SpanKind::Identifier => 3,
        }
    }
}

/// One grounded span: verbatim text plus where it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpan {
    /// Stable id from (kind, message, offsets, text). Re-extracting the same
    /// message yields the same ids, so a later stage can cite and dedupe.
    pub span_id: String,
    pub kind: SpanKind,
    /// Verbatim substring of the message (clipped past [`MAX_SPAN_TEXT_LEN`]).
    pub text: String,
    /// Which message of the transcript this came from.
    pub message_index: usize,
    /// Byte offset of the span start in the original message text.
    pub start: usize,
    /// Byte offset of the span end in the original message text.
    pub end: usize,
}

/// Deterministic span id from (kind, message index, offsets, text). Inline
/// FNV-1a (not `DefaultHasher`, whose SipHash keys are random per process)
/// so ids are stable across runs and across machines.
pub fn span_id_for(
    kind: SpanKind,
    message_index: usize,
    start: usize,
    end: usize,
    text: &str,
) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in format!("{}|{message_index}|{start}|{end}|{text}", kind.as_str()).as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("es-{hash:016x}")
}

fn clip_span(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() <= MAX_SPAN_TEXT_LEN {
        return trimmed.to_string();
    }
    let mut end = MAX_SPAN_TEXT_LEN;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

static PATH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        [A-Za-z]:[\\/][^\s'""`\]\)\],;!?]+
        | \b[A-Za-z0-9_.\-+]+(?:/[A-Za-z0-9_.\-+]+)*/[A-Za-z0-9_.\-+]+\.[A-Za-z0-9]+(?::\d+(?::\d+)?)?
        | \b[A-Za-z0-9_.\-+]+(?:/[A-Za-z0-9_.\-+]+){2,}(?::\d+(?::\d+)?)?
        | (~|\.{1,2})?/[A-Za-z0-9_.\-+~][A-Za-z0-9_.\-+~/]*(?::\d+(?::\d+)?)?
        | \b(?:Makefile|Dockerfile|LICENSE|README|CHANGELOG|CONTRIBUTING|AGENTS|CLAUDE)(?:\.[A-Za-z]+)?\b
        | \b[A-Za-z0-9_+\-]+\.(?:rs|toml|md|json|jsonl|yaml|yml|py|js|ts|tsx|jsx|go|java|c|cpp|h|hpp|sh|fish|conf|lock|txt|log|html|css|svg|png)\b
        "#,
    )
    .expect("path regex")
});

/// Shaped diagnostics: match on their own, no context needed. The
/// case-insensitive flag is scoped to the `\w+error` alternative only — a
/// bare lowercase `failed`/`fatal`/`traceback` still needs code context
/// (see [`ERROR_WEAK_RE`]), or prose like "the test failed yesterday"
/// would ground a whole line as a diagnostic.
static ERROR_STRONG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"error\[E\d+\]|\bE\d{3,5}\b|\bENOENT\b|\bEACCES\b|\bEPERM\b|(?i:\b\w+(?:error|exception)\b)|\bFAILED\b|\bFATAL\b|\bTraceback\b")
        .expect("error strong regex")
});

/// Bare lowercase words: only count with code-shaped punctuation on the line.
static ERROR_WEAK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(error|failed|failure|panicked?|exception|traceback|fatal|aborted|segfault)\b",
    )
    .expect("error weak regex")
});

/// Code-shaped punctuation for the weak-error gate. Excludes `/`: a bare
/// slash appears in plain prose (`and/or`, `either/or`), so it must not
/// count as code context on its own. A real path on the line still counts
/// (checked separately via [`has_path_shape`]), so `build failed in
/// crates/foo/bar` still grounds.
static CODEISH_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"['""`:\\=()\[\]{}<>|*]"#).expect("codeish regex"));

/// Past or commissive decision markers. Deliberately excludes future-tense
/// intent (`will use`, `we'll use`): "I will use the store tomorrow" is a
/// plan, not a recorded decision, and it is common in plain prose.
static DECISION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(decided|decision|going with|let's go with|let us go with|agreed|conclusion|chose|chosen|selected|opted for|settled on|final answer|sticking with)\b")
        .expect("decision regex")
});

static IDENT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"`([^`\n]{1,200})`|\b[A-Za-z_][A-Za-z0-9_]*\b").expect("identifier regex")
});

/// Trailing characters that are sentence punctuation, not path text.
fn strip_trailing_path_punct(raw: &str) -> &str {
    raw.trim_end_matches([
        '.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"', '‘', '’', '“', '”', '`',
    ])
}

/// Whether a bare word looks like code: snake_case, CamelCase, lowerCamel,
/// or ALL_CAPS. Plain lowercase prose words (`call`, `with`, `retry`) and
/// single-capitals prose words (`Call`, `User`, `Assistant`) return false —
/// the latter matters because transcript role prefixes (`User:`, `Tool:`)
/// sit next to colons constantly.
fn is_code_shaped(token: &str) -> bool {
    if token.len() < 2 {
        return false;
    }
    // snake_case / SCREAMING_SNAKE / anything joined by underscores.
    if token.contains('_') {
        return true;
    }
    let mut chars = token.chars();
    let first = chars.next().unwrap_or('\0');
    let mut upper_past_first = false;
    let mut has_lower = first.is_lowercase();
    let mut has_alpha = first.is_alphabetic();
    for ch in chars {
        if ch.is_uppercase() {
            upper_past_first = true;
        }
        if ch.is_lowercase() {
            has_lower = true;
        }
        if ch.is_alphabetic() {
            has_alpha = true;
        }
    }
    // CamelCase / lowerCamel: an uppercase letter past the first character
    // plus at least one lowercase letter.
    if upper_past_first && has_lower {
        return true;
    }
    // ALL_CAPS (optionally with digits): no lowercase, at least one letter.
    if !has_lower && has_alpha {
        return true;
    }
    false
}

/// Whether a word sits in a code-shaped position: a call (`foo(`), a method
/// (`.foo`), or a path separator (`::foo`). A trailing colon is NOT enough:
/// transcript role prefixes (`User:`, `Assistant:`, `Tool:`) would all match.
fn is_code_adjacent(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    if end < bytes.len() && bytes[end] == b'(' {
        return true;
    }
    if start > 0 && (bytes[start - 1] == b'.' || bytes[start - 1] == b':') {
        return true;
    }
    false
}

struct RawSpan {
    kind: SpanKind,
    text: String,
    start: usize,
    end: usize,
}

fn push_span(out: &mut Vec<RawSpan>, kind: SpanKind, text: &str, start: usize, end: usize) {
    if text.trim().is_empty() {
        return;
    }
    out.push(RawSpan {
        kind,
        text: clip_span(text),
        start,
        end,
    });
}

/// Whether `line` contains at least one real path span (word-boundary
/// guarded via [`find_path_spans`]). Used by the weak-error gate so a path
/// on the line counts as code context without letting `and/or` through.
fn has_path_shape(line: &str) -> bool {
    !find_path_spans(line).is_empty()
}

/// Path spans in `text`, with ranges. A match glued to a word character on
/// the left (`and/or`, `either/or`) or right (`src/main.rsx` is one token,
/// not a `.rs` file) is prose punctuation, not a path. Suffix-bearing
/// matches (`src/main.rs:10`) still verify their left edge against the
/// unstripped hit start, so `foo/src/main.rs:10` keeps its context.
fn find_path_spans(text: &str) -> Vec<RawSpan> {
    let mut out = Vec::new();
    for hit in PATH_RE.find_iter(text) {
        let matched = strip_trailing_path_punct(hit.as_str());
        if matched.is_empty() || matched == "." || matched == ".." || matched == "~" {
            continue;
        }
        let start = hit.start();
        let end = start + matched.len();
        // Left edge must not be glued to a word char: rejects `and/or`.
        if start > 0 {
            let prev = text.as_bytes()[start - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                continue;
            }
        }
        // Right edge: the unstripped hit may extend into a longer word
        // (`lib.rsx` is consumed whole by the greedy extension match, so a
        // follower here means trailing punctuation like a sentence period,
        // which `strip_trailing_path_punct` already removed from the span).
        if hit.end() < text.len() {
            let next = text.as_bytes()[hit.end()];
            if next.is_ascii_alphanumeric() || next == b'_' {
                continue;
            }
        }
        out.push(RawSpan {
            kind: SpanKind::FilePath,
            text: clip_span(matched),
            start,
            end,
        });
    }
    out
}

/// Extract evidence spans from one transcript message. `message_index` tags
/// provenance; extraction itself is per-message (no cross-message state),
/// so callers can index messages however they like.
pub fn extract_spans(text: &str, message_index: usize) -> Vec<EvidenceSpan> {
    let mut raw: Vec<RawSpan> = Vec::new();

    // File paths: whole-text scan; location suffixes (`:10:5`) stay verbatim.
    let path_spans = find_path_spans(text);
    let path_ranges: Vec<(usize, usize)> = path_spans
        .iter()
        .map(|span| (span.start, span.end))
        .collect();
    raw.extend(path_spans);

    // Identifiers: whole-text scan, skipping anything inside a path span
    // (paths ground once, as paths) and anything that is plain prose (code
    // shape or code adjacency required).
    for hit in IDENT_RE.find_iter(text) {
        let (token, start, end) = match IDENT_RE
            .captures(hit.as_str())
            .and_then(|captures| captures.get(1))
        {
            // Backtick-quoted: high precision (markdown code span), keep the
            // inner text without the backticks.
            Some(inner) => {
                let inner_start = hit.start() + inner.start();
                (inner.as_str(), inner_start, inner_start + inner.len())
            }
            None => (hit.as_str(), hit.start(), hit.end()),
        };
        if token.len() > MAX_SPAN_TEXT_LEN {
            continue;
        }
        if path_ranges
            .iter()
            .any(|(range_start, range_end)| start < *range_end && end > *range_start)
        {
            continue;
        }
        // Backticked content is trusted as code; bare words must look or sit
        // like code, or prose like "call the store" would ground.
        let backticked = hit.as_str().starts_with('`');
        if !(backticked || is_code_shaped(token) || is_code_adjacent(text, start, end)) {
            continue;
        }
        push_span(&mut raw, SpanKind::Identifier, token, start, end);
    }

    // Line-level spans: error strings and decisions keep the whole line.
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let content_start = line_start + (line.len() - line.trim_start().len());
        let content_end = line_start + line.len() - (line.len() - line.trim_end().len());
        let is_error = ERROR_STRONG_RE.is_match(trimmed)
            || (ERROR_WEAK_RE.is_match(trimmed)
                && (CODEISH_RE.is_match(trimmed) || has_path_shape(trimmed)));
        if is_error {
            push_span(
                &mut raw,
                SpanKind::ErrorString,
                trimmed,
                content_start,
                content_end,
            );
        }
        if DECISION_RE.is_match(trimmed) {
            push_span(
                &mut raw,
                SpanKind::Decision,
                trimmed,
                content_start,
                content_end,
            );
        }
    }

    // Deterministic order: offset asc, then kind rank. Dedup exact repeats
    // (same kind + text) within the message, keeping the first offset.
    raw.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(a.kind.rank().cmp(&b.kind.rank()))
    });
    let mut seen: std::collections::HashSet<(SpanKind, &str)> = std::collections::HashSet::new();
    let mut spans: Vec<EvidenceSpan> = Vec::new();
    for span in &raw {
        if spans.len() >= MAX_SPANS_PER_MESSAGE {
            break;
        }
        if !seen.insert((span.kind, span.text.as_str())) {
            continue;
        }
        spans.push(EvidenceSpan {
            span_id: span_id_for(span.kind, message_index, span.start, span.end, &span.text),
            kind: span.kind,
            text: span.text.clone(),
            message_index,
            start: span.start,
            end: span.end,
        });
    }
    spans
}

/// Extract spans over a whole transcript: one [`extract_spans`] pass per
/// message, concatenated in message order (each message's spans already
/// offset-ordered, so the whole output walks the transcript front to back).
pub fn extract_transcript(messages: &[&str]) -> Vec<EvidenceSpan> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(index, text)| extract_spans(text, index))
        .collect()
}

/// Kind histogram for introspection: span kind to count, over one
/// extraction pass. Lets callers report what a transcript is grounded in
/// without reimplementing the kind taxonomy.
pub fn span_counts(spans: &[EvidenceSpan]) -> std::collections::HashMap<SpanKind, usize> {
    let mut counts = std::collections::HashMap::new();
    for span in spans {
        *counts.entry(span.kind).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(spans: &[EvidenceSpan]) -> Vec<&str> {
        spans.iter().map(|span| span.text.as_str()).collect()
    }

    #[test]
    fn slashed_paths_kept_with_location_suffixes() {
        let spans = extract_spans(
            "The fix is in crates/jcode-base/src/lib.rs:107 near src/main.rs.",
            0,
        );
        let paths: Vec<&EvidenceSpan> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::FilePath)
            .collect();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].text, "crates/jcode-base/src/lib.rs:107");
        assert_eq!(paths[1].text, "src/main.rs");
    }

    #[test]
    fn bare_filenames_with_known_extensions_caught() {
        let spans = extract_spans("Update Cargo.toml and README.md before running.", 0);
        let paths: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::FilePath)
            .map(|span| span.text.as_str())
            .collect();
        assert!(paths.contains(&"Cargo.toml"), "paths: {paths:?}");
        assert!(paths.contains(&"README.md"), "paths: {paths:?}");
    }

    #[test]
    fn sentence_period_is_not_path_text() {
        let spans = extract_spans("See crates/jcode-base/src/lib.rs.", 0);
        let paths: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::FilePath)
            .map(|span| span.text.as_str())
            .collect();
        assert_eq!(paths, vec!["crates/jcode-base/src/lib.rs"]);
    }

    #[test]
    fn and_slash_or_prose_is_not_a_path() {
        let spans = extract_spans("Use trial and/or error handling for this case.", 0);
        assert!(
            !spans.iter().any(|span| span.kind == SpanKind::FilePath),
            "spans: {spans:?}"
        );
    }

    #[test]
    fn rustc_error_line_kept_whole_and_verbatim() {
        let text =
            "error[E0428]: the name `build` is defined multiple times\n  --> src/main.rs:10:5";
        let spans = extract_spans(text, 0);
        let errors: Vec<&EvidenceSpan> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::ErrorString)
            .collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].text,
            "error[E0428]: the name `build` is defined multiple times"
        );
        // The location line is a path span, kept verbatim too.
        assert!(texts(&spans).contains(&"src/main.rs:10:5"));
    }

    #[test]
    fn panic_and_traceback_lines_kept() {
        let text = "thread 'main' panicked at src/main.rs:10:5:\ncalled `Option::unwrap()` on a `None` value\nTraceback (most recent call last):\nValueError: invalid literal for int()";
        let spans = extract_spans(text, 0);
        let errors: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::ErrorString)
            .map(|span| span.text.as_str())
            .collect();
        assert_eq!(errors.len(), 3, "errors: {errors:?}");
        assert!(errors.contains(&"thread 'main' panicked at src/main.rs:10:5:"));
        assert!(errors.contains(&"Traceback (most recent call last):"));
        assert!(errors.contains(&"ValueError: invalid literal for int()"));
    }

    #[test]
    fn bare_error_word_needs_code_context() {
        // Prose "trial and error" carries no diagnostic: skipped.
        let spans = extract_spans("We used trial and error to find the design.", 0);
        assert!(
            !spans.iter().any(|span| span.kind == SpanKind::ErrorString),
            "spans: {spans:?}"
        );
        // Lowercase "failed" in plain prose is not a diagnostic either.
        let spans = extract_spans("The test failed yesterday after lunch.", 0);
        assert!(
            !spans.iter().any(|span| span.kind == SpanKind::ErrorString),
            "spans: {spans:?}"
        );
        // Same word with a code-shaped line is a real diagnostic.
        let spans = extract_spans("test failed: assertion left == right", 0);
        assert!(
            spans.iter().any(|span| span.kind == SpanKind::ErrorString),
            "spans: {spans:?}"
        );
    }

    #[test]
    fn identifiers_require_code_shape() {
        let spans = extract_spans(
            "Call SystemTweaks::load with my_var, then retry MAX_RETRIES.",
            0,
        );
        let idents: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::Identifier)
            .map(|span| span.text.as_str())
            .collect();
        assert!(idents.contains(&"SystemTweaks"), "idents: {idents:?}");
        assert!(idents.contains(&"load"), "idents: {idents:?}");
        assert!(idents.contains(&"my_var"), "idents: {idents:?}");
        assert!(idents.contains(&"MAX_RETRIES"), "idents: {idents:?}");
        // Plain prose words on the same line are not identifiers.
        assert!(!idents.contains(&"Call"));
        assert!(!idents.contains(&"with"));
        assert!(!idents.contains(&"then"));
        assert!(!idents.contains(&"retry"));
    }

    #[test]
    fn role_prefixes_are_not_identifiers() {
        let spans = extract_spans("User: look at src/main.rs\nAssistant: sure\nTool: done", 0);
        let idents: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::Identifier)
            .map(|span| span.text.as_str())
            .collect();
        assert!(!idents.contains(&"User"), "idents: {idents:?}");
        assert!(!idents.contains(&"Assistant"), "idents: {idents:?}");
        assert!(!idents.contains(&"Tool"), "idents: {idents:?}");
    }

    #[test]
    fn backtick_spans_kept_without_backticks() {
        let spans = extract_spans("Run `cargo fmt` before pushing.", 0);
        let idents: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::Identifier)
            .map(|span| span.text.as_str())
            .collect();
        assert!(idents.contains(&"cargo fmt"), "idents: {idents:?}");
    }

    #[test]
    fn identifiers_inside_paths_are_not_double_counted() {
        let spans = extract_spans("Open crates/jcode-base/src/lib.rs now.", 0);
        let idents: Vec<&str> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::Identifier)
            .map(|span| span.text.as_str())
            .collect();
        assert!(!idents.contains(&"crates"), "idents: {idents:?}");
        assert!(!idents.contains(&"lib"), "idents: {idents:?}");
    }

    #[test]
    fn decision_lines_kept_whole() {
        let spans = extract_spans("We decided to use libsqlite3-sys with bundled features.", 0);
        let decisions: Vec<&EvidenceSpan> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::Decision)
            .collect();
        assert_eq!(decisions.len(), 1);
        assert_eq!(
            decisions[0].text,
            "We decided to use libsqlite3-sys with bundled features."
        );
    }

    #[test]
    fn future_intent_is_not_a_decision() {
        // "I will use ..." is a plan, not a recorded decision.
        let spans = extract_spans("I will use the store tomorrow for this.", 0);
        assert!(
            !spans.iter().any(|span| span.kind == SpanKind::Decision),
            "spans: {spans:?}"
        );
        // "undecided" must not trip the "decided" marker (word boundary).
        let spans = extract_spans("We are still undecided about the approach.", 0);
        assert!(
            !spans.iter().any(|span| span.kind == SpanKind::Decision),
            "spans: {spans:?}"
        );
    }

    #[test]
    fn plain_prose_yields_no_spans() {
        let text = "I went to the store yesterday and thought about the design for a while.";
        let spans = extract_spans(text, 0);
        assert!(spans.is_empty(), "spans: {spans:?}");
    }

    #[test]
    fn realistic_transcript_catches_all_four_kinds() {
        let messages = [
            "User: the build broke after my change, can you look at crates/jcode-base/src/compaction.rs today",
            "Assistant: sure, running the tests now to see what happened",
            "Tool: error[E0428]: the name `build` is defined multiple times\n  --> src/main.rs:10:5\nthread 'main' panicked at src/main.rs:10:5:",
            "Assistant: We decided to rename the second build helper to build_once and keep SystemTweaks as is. Open Cargo.toml to check features.",
        ];
        let spans = extract_transcript(&messages);
        let counts = span_counts(&spans);
        assert!(
            counts
                .get(&SpanKind::FilePath)
                .is_some_and(|count| *count >= 3),
            "counts: {counts:?}"
        );
        assert!(
            counts
                .get(&SpanKind::ErrorString)
                .is_some_and(|count| *count >= 2),
            "counts: {counts:?}"
        );
        assert!(
            counts
                .get(&SpanKind::Identifier)
                .is_some_and(|count| *count >= 2),
            "counts: {counts:?}"
        );
        assert_eq!(counts.get(&SpanKind::Decision), Some(&1));
        // Provenance walks the transcript front to back.
        let indices: Vec<usize> = spans.iter().map(|span| span.message_index).collect();
        let mut sorted = indices.clone();
        sorted.sort();
        assert_eq!(indices, sorted);
    }

    #[test]
    fn extraction_is_deterministic_with_stable_ids() {
        let text = "error[E0428]: dup symbol in src/main.rs. We decided to rename build_once.";
        let first = extract_spans(text, 2);
        let second = extract_spans(text, 2);
        assert_eq!(first, second);
        assert!(!first.is_empty());
        // Same text at a different message index tags provenance: same
        // shape, but ids differ by construction, deterministically.
        let other = extract_spans(text, 3);
        assert_eq!(first.len(), other.len());
        assert_ne!(first[0].span_id, other[0].span_id);
        assert_eq!(other[0].message_index, 3);
        // Offsets locate token-level spans in the original message.
        for span in &first {
            if span.kind == SpanKind::Identifier || span.kind == SpanKind::FilePath {
                assert!(text[span.start..span.end].contains(&span.text[..span.text.len().min(20)]));
            }
        }
    }

    #[test]
    fn repeats_dedupe_and_output_is_bounded() {
        let mut text = String::new();
        for _ in 0..200 {
            text.push_str("see src/main.rs for details. ");
        }
        let spans = extract_spans(&text, 0);
        // Same path repeated 200 times emits once (dedup within a message).
        let paths: Vec<&EvidenceSpan> = spans
            .iter()
            .filter(|span| span.kind == SpanKind::FilePath)
            .collect();
        assert_eq!(paths.len(), 1);
        assert!(spans.len() <= MAX_SPANS_PER_MESSAGE);
    }

    #[test]
    fn span_counts_histogram_shape() {
        let spans = extract_spans(
            "error[E0428]: bad in src/main.rs. Going with build_once.",
            0,
        );
        let counts = span_counts(&spans);
        assert_eq!(counts.get(&SpanKind::ErrorString), Some(&1));
        assert_eq!(counts.get(&SpanKind::FilePath), Some(&1));
        assert_eq!(counts.get(&SpanKind::Decision), Some(&1));
        assert!(span_counts(&[]).is_empty());
    }
}
