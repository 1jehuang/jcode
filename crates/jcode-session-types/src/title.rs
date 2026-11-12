//! Fallback session titles derived from the first real user prompt.
//!
//! Sessions without a rename, bookmark label, or todo goal would otherwise
//! show up as an anonymous "New session" everywhere. The first prompt is the
//! most recognizable summary available without spending a model call.

const MAX_TITLE_CHARS: usize = 64;

/// Turn a user prompt into a compact one-line title.
///
/// Internal wrappers (`<system-reminder>` blocks, voice `<transcription>`
/// tags) are removed, whitespace is collapsed, and slash commands produce no
/// title because they describe an action rather than the conversation.
pub fn prompt_title(prompt: &str) -> Option<String> {
    let without_reminders = strip_blocks(prompt, "<system-reminder>", "</system-reminder>", false);
    let visible = strip_blocks(
        &without_reminders,
        "<transcription>",
        "</transcription>",
        true,
    );
    let normalized = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.starts_with("[Scheduled task]")
    {
        return None;
    }
    let mut chars = normalized.chars();
    let title: String = chars.by_ref().take(MAX_TITLE_CHARS).collect();
    Some(if chars.next().is_some() {
        format!("{}…", title.trim_end())
    } else {
        title
    })
}

/// Remove complete `open`..`close` segments. With `keep_inner`, only the tags
/// are dropped. Unbalanced text is kept verbatim.
fn strip_blocks(text: &str, open: &str, close: &str, keep_inner: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(close) else {
            break;
        };
        out.push_str(&rest[..start]);
        if keep_inner {
            out.push(' ');
            out.push_str(&after_open[..end]);
            out.push(' ');
        }
        rest = &after_open[end + close.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::prompt_title;

    #[test]
    fn prompt_becomes_a_compact_single_line_title() {
        assert_eq!(
            prompt_title("  help me   improve the session sidebar\nplease ").as_deref(),
            Some("help me improve the session sidebar please")
        );
        let long = prompt_title(&"word ".repeat(40)).unwrap();
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= 65);
    }

    #[test]
    fn wrappers_and_commands_are_not_titles() {
        assert_eq!(
            prompt_title("<transcription>\nRename the sidebar rows.\n</transcription>").as_deref(),
            Some("Rename the sidebar rows.")
        );
        assert_eq!(
            prompt_title("<system-reminder>\n# Session Context\n</system-reminder>"),
            None
        );
        assert_eq!(
            prompt_title("<system-reminder>ctx</system-reminder>\nFix the build").as_deref(),
            Some("Fix the build")
        );
        assert_eq!(prompt_title(" /model gpt-5 "), None);
        assert_eq!(prompt_title("[Scheduled task]\nrun it"), None);
        assert_eq!(prompt_title("   "), None);
    }
}
