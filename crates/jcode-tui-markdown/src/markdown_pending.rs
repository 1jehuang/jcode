use super::*;

/// True when `line` is a deferred-render pending placeholder (mermaid or
/// math). Tolerates leading/trailing padding spans added by centered display
/// modes and the truncated tail produced when a narrow width wraps it.
///
/// Matches on the line's joined text rather than a single span: the wrapping
/// pass splits a line into one span per word, so a per-span test silently
/// stops recognizing placeholders once content reaches the wrapper. That made
/// prepared bodies miss their staleness stamp, leaving "rendering..." on
/// screen permanently because no cache layer knew to rebuild.
pub fn line_is_mermaid_pending_placeholder(line: &Line<'_>) -> bool {
    let joined: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let joined = joined.trim();
    let narrow = joined.trim_end_matches('…');
    joined.starts_with(MERMAID_PENDING_MATCH_PREFIX)
        || (narrow.starts_with('↻') && MERMAID_PENDING_MATCH_PREFIX.starts_with(narrow))
}
