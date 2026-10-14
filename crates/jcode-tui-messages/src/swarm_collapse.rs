/// Collapsible swarm notification content.
///
/// When a swarm message/report arrives with a sender-provided `tldr`, the TUI
/// stores the full body in the transcript `DisplayMessage` but renders only
/// the tldr line plus an expand hint. The collapsed/expanded state is encoded
/// in the message content itself (behind zero-width markers), so toggling
/// rewrites the content and every content-hash-keyed render cache invalidates
/// naturally.
///
/// Encoded layout: `{MARKER}{state}{MARKER}{tldr}\n{body}` where `state` is
/// `collapsed` or `expanded`. The markers are zero-width so even if the raw
/// content leaks somewhere (copy, logs), it degrades to readable text.
const MARKER: &str = "\u{200b}\u{200b}";
const STATE_COLLAPSED: &str = "swarm-tldr:collapsed";
const STATE_EXPANDED: &str = "swarm-tldr:expanded";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollapsibleSwarmContent<'a> {
    pub tldr: &'a str,
    pub body: &'a str,
    pub expanded: bool,
}

/// Encode a tldr + full body as collapsed swarm content.
pub fn encode_collapsible_swarm_content(tldr: &str, body: &str) -> String {
    format!("{MARKER}{STATE_COLLAPSED}{MARKER}{tldr}\n{body}")
}

/// Parse collapsible swarm content. Returns `None` for plain content.
pub fn parse_collapsible_swarm_content(content: &str) -> Option<CollapsibleSwarmContent<'_>> {
    let rest = content.strip_prefix(MARKER)?;
    let (state, rest) = rest.split_once(MARKER)?;
    let expanded = match state {
        STATE_COLLAPSED => false,
        STATE_EXPANDED => true,
        _ => return None,
    };
    let (tldr, body) = rest.split_once('\n').unwrap_or((rest, ""));
    Some(CollapsibleSwarmContent {
        tldr,
        body,
        expanded,
    })
}

/// Toggle the collapsed/expanded state of encoded content. Returns `None`
/// when the content is not collapsible.
pub fn toggle_collapsible_swarm_content(content: &str) -> Option<String> {
    let parsed = parse_collapsible_swarm_content(content)?;
    let state = if parsed.expanded {
        STATE_COLLAPSED
    } else {
        STATE_EXPANDED
    };
    Some(format!(
        "{MARKER}{state}{MARKER}{}\n{}",
        parsed.tldr, parsed.body
    ))
}

/// Longest body (in chars) a swarm notification may show inline without being
/// collapsed behind a one-row preview.
pub const SWARM_INLINE_BODY_MAX_CHARS: usize = 120;

/// One-row preview of `text`: first non-empty line, whitespace collapsed,
/// truncated to `max_chars` with an ellipsis.
pub fn swarm_preview_line(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let max_chars = max_chars.max(1);
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut out: String = collapsed.chars().take(max_chars - 1).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// Ensure swarm content renders as at most one row by default. Content that is
/// already collapsible, or a short single line, is returned unchanged. Anything
/// longer or multi-line is collapsed behind a derived one-row preview so the
/// full body is only shown on explicit expand.
pub fn collapse_long_swarm_content(content: &str) -> String {
    if parse_collapsible_swarm_content(content).is_some() {
        return content.to_string();
    }
    let trimmed = content.trim();
    let single_line = !trimmed.contains('\n');
    if single_line && trimmed.chars().count() <= SWARM_INLINE_BODY_MAX_CHARS {
        return trimmed.to_string();
    }
    let preview = swarm_preview_line(trimmed, SWARM_INLINE_BODY_MAX_CHARS);
    encode_collapsible_swarm_content(&preview, trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_single_line_stays_plain() {
        assert_eq!(collapse_long_swarm_content("  quick note "), "quick note");
    }

    #[test]
    fn long_or_multiline_content_collapses_to_one_row_preview() {
        let long = "word ".repeat(80);
        let collapsed = collapse_long_swarm_content(&long);
        let parsed = parse_collapsible_swarm_content(&collapsed).expect("collapsible");
        assert!(!parsed.expanded);
        assert!(parsed.tldr.chars().count() <= SWARM_INLINE_BODY_MAX_CHARS);
        assert!(parsed.tldr.ends_with('…'));
        assert!(!parsed.tldr.contains('\n'));

        let multi = collapse_long_swarm_content("first line\n\nsecond line");
        let parsed = parse_collapsible_swarm_content(&multi).expect("collapsible");
        assert_eq!(parsed.tldr, "first line");
        assert_eq!(parsed.body, "first line\n\nsecond line");
    }

    #[test]
    fn already_collapsible_content_is_untouched() {
        let encoded = encode_collapsible_swarm_content("tldr", "body\nmore");
        assert_eq!(collapse_long_swarm_content(&encoded), encoded);
    }

    #[test]
    fn roundtrip_collapsed_then_toggle() {
        let encoded = encode_collapsible_swarm_content("did the thing", "long\nbody\nhere");
        let parsed = parse_collapsible_swarm_content(&encoded).expect("collapsible");
        assert_eq!(parsed.tldr, "did the thing");
        assert_eq!(parsed.body, "long\nbody\nhere");
        assert!(!parsed.expanded);

        let expanded = toggle_collapsible_swarm_content(&encoded).expect("toggle");
        let parsed = parse_collapsible_swarm_content(&expanded).expect("collapsible");
        assert!(parsed.expanded);
        assert_eq!(parsed.body, "long\nbody\nhere");

        let collapsed_again = toggle_collapsible_swarm_content(&expanded).expect("toggle");
        assert_eq!(collapsed_again, encoded);
    }

    #[test]
    fn plain_content_is_not_collapsible() {
        assert!(parse_collapsible_swarm_content("just a message").is_none());
        assert!(toggle_collapsible_swarm_content("just a message").is_none());
    }

    #[test]
    fn empty_body_parses() {
        let encoded = encode_collapsible_swarm_content("tldr only", "");
        let parsed = parse_collapsible_swarm_content(&encoded).expect("collapsible");
        assert_eq!(parsed.tldr, "tldr only");
        assert_eq!(parsed.body, "");
    }
}
