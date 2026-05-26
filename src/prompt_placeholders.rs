//! Phase 4 prompt placeholder substitution helper.
//!
//! Provides a small `String -> String` transformation that replaces a fixed
//! set of `{{PLACEHOLDER}}` tokens with values supplied through a
//! [`PlaceholderContext`]. Designed to be a pure utility: no I/O, no errors,
//! no global state. Callers are responsible for assembling the context and
//! choosing where to apply substitution (system prompt, step prompt, etc.).
//!
//! Supported tokens (case-sensitive, exact match including the surrounding
//! double curly braces):
//!
//! - `{{FILE_TREE_SMALL}}`   — truncated project tree, max 2500 chars.
//! - `{{FILE_TREE}}`         — fuller project tree, max 10000 chars.
//! - `{{KNOWLEDGE_FILES}}`   — concatenated knowledge / context files (no limit).
//! - `{{GIT_CHANGES}}`       — `git diff` / status summary, max 30000 chars.
//! - `{{CURRENT_DATE}}`      — ISO `YYYY-MM-DD` date string.
//! - `{{REMAINING_STEPS}}`   — remaining-step counter (u32, decimal).
//! - `{{SYSTEM_INFO}}`       — OS / arch / shell summary.
//!
//! Empty `String` fields and `remaining_steps == 0` are replaced with an
//! empty string rather than the literal placeholder text. Tokens that are
//! not in the supported list are left untouched in the output, so this
//! function is safe to apply to text that may contain other Mustache-like
//! syntax.

/// Maximum char count retained for [`PlaceholderContext::file_tree_small`].
pub const FILE_TREE_SMALL_MAX_CHARS: usize = 2_500;

/// Maximum char count retained for [`PlaceholderContext::file_tree`].
pub const FILE_TREE_MAX_CHARS: usize = 10_000;

/// Maximum char count retained for [`PlaceholderContext::git_changes`].
pub const GIT_CHANGES_MAX_CHARS: usize = 30_000;

/// Container for values that can be substituted into prompt templates.
///
/// All `String` fields default to empty and `remaining_steps` defaults to 0.
/// Use [`PlaceholderContext::default`] and assign the fields you have data
/// for; missing fields will simply substitute as empty.
#[derive(Debug, Default, Clone)]
pub struct PlaceholderContext {
    /// Compact project file tree. Truncated to [`FILE_TREE_SMALL_MAX_CHARS`]
    /// chars during substitution.
    pub file_tree_small: String,
    /// Fuller project file tree. Truncated to [`FILE_TREE_MAX_CHARS`] chars
    /// during substitution.
    pub file_tree: String,
    /// Concatenated knowledge/context files. No length limit is applied.
    pub knowledge_files: String,
    /// Git diff / status summary. Truncated to [`GIT_CHANGES_MAX_CHARS`]
    /// chars during substitution.
    pub git_changes: String,
    /// Current date in ISO `YYYY-MM-DD` form.
    pub current_date: String,
    /// Remaining steps allowed for the current run/turn. Zero substitutes
    /// to an empty string.
    pub remaining_steps: u32,
    /// Free-form system info (OS / arch / shell).
    pub system_info: String,
}

/// Return at most `max_chars` characters from `s`, respecting char
/// boundaries. If `s` already fits within the limit it is returned
/// unchanged (cloned).
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

/// Replace every supported placeholder token in `prompt` with the matching
/// value from `ctx`. Unknown `{{TOKENS}}` are preserved verbatim. Empty
/// values (and `remaining_steps == 0`) replace the placeholder with an
/// empty string.
///
/// Length caps documented on [`PlaceholderContext`] are enforced here, so
/// callers may pass un-truncated input and trust the output to be bounded.
pub fn substitute_placeholders(prompt: &str, ctx: &PlaceholderContext) -> String {
    if prompt.is_empty() {
        return String::new();
    }

    let file_tree_small = truncate_chars(&ctx.file_tree_small, FILE_TREE_SMALL_MAX_CHARS);
    let file_tree = truncate_chars(&ctx.file_tree, FILE_TREE_MAX_CHARS);
    let git_changes = truncate_chars(&ctx.git_changes, GIT_CHANGES_MAX_CHARS);
    let remaining_steps = if ctx.remaining_steps == 0 {
        String::new()
    } else {
        ctx.remaining_steps.to_string()
    };

    // Each entry is (token, replacement). Order is irrelevant because
    // tokens never overlap, but we keep it stable for determinism.
    let replacements: [(&str, &str); 7] = [
        ("{{FILE_TREE_SMALL}}", file_tree_small.as_str()),
        ("{{FILE_TREE}}", file_tree.as_str()),
        ("{{KNOWLEDGE_FILES}}", ctx.knowledge_files.as_str()),
        ("{{GIT_CHANGES}}", git_changes.as_str()),
        ("{{CURRENT_DATE}}", ctx.current_date.as_str()),
        ("{{REMAINING_STEPS}}", remaining_steps.as_str()),
        ("{{SYSTEM_INFO}}", ctx.system_info.as_str()),
    ];

    let mut out = prompt.to_string();
    for (token, value) in replacements {
        if out.contains(token) {
            out = out.replace(token, value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_context_replaces_all_placeholders_with_empty() {
        let ctx = PlaceholderContext::default();
        let input = "tree=[{{FILE_TREE_SMALL}}] full=[{{FILE_TREE}}] \
                     k=[{{KNOWLEDGE_FILES}}] git=[{{GIT_CHANGES}}] \
                     date=[{{CURRENT_DATE}}] steps=[{{REMAINING_STEPS}}] \
                     sys=[{{SYSTEM_INFO}}]";
        let out = substitute_placeholders(input, &ctx);
        assert_eq!(
            out,
            "tree=[] full=[] k=[] git=[] date=[] steps=[] sys=[]"
        );
    }

    #[test]
    fn individual_placeholder_works() {
        let ctx = PlaceholderContext {
            current_date: "2026-05-25".to_string(),
            ..Default::default()
        };
        let out = substitute_placeholders("today is {{CURRENT_DATE}}.", &ctx);
        assert_eq!(out, "today is 2026-05-25.");

        // Unrelated placeholder stays empty in the same call.
        let out2 = substitute_placeholders(
            "date={{CURRENT_DATE}} steps={{REMAINING_STEPS}}",
            &ctx,
        );
        assert_eq!(out2, "date=2026-05-25 steps=");
    }

    #[test]
    fn multiple_placeholders_in_same_string_work() {
        let ctx = PlaceholderContext {
            file_tree_small: "src/\n  lib.rs".to_string(),
            knowledge_files: "AGENTS.md contents".to_string(),
            current_date: "2026-05-25".to_string(),
            remaining_steps: 7,
            system_info: "linux x86_64".to_string(),
            ..Default::default()
        };
        let input = "## Tree\n{{FILE_TREE_SMALL}}\n\n## Knowledge\n\
                     {{KNOWLEDGE_FILES}}\n\n## Meta\n\
                     date={{CURRENT_DATE}} steps={{REMAINING_STEPS}} \
                     sys={{SYSTEM_INFO}}";
        let out = substitute_placeholders(input, &ctx);
        let expected = "## Tree\nsrc/\n  lib.rs\n\n## Knowledge\n\
                        AGENTS.md contents\n\n## Meta\n\
                        date=2026-05-25 steps=7 sys=linux x86_64";
        assert_eq!(out, expected);
    }

    #[test]
    fn unknown_placeholder_text_remains_as_is() {
        let ctx = PlaceholderContext {
            current_date: "2026-05-25".to_string(),
            ..Default::default()
        };
        let input = "known={{CURRENT_DATE}} unknown={{NOT_A_REAL_TOKEN}} \
                     other={{ALSO_BOGUS}}";
        let out = substitute_placeholders(input, &ctx);
        assert_eq!(
            out,
            "known=2026-05-25 unknown={{NOT_A_REAL_TOKEN}} other={{ALSO_BOGUS}}"
        );
    }

    #[test]
    fn truncation_caps_long_inputs() {
        // Build a string longer than the file-tree-small cap.
        let big: String = "x".repeat(FILE_TREE_SMALL_MAX_CHARS + 1234);
        let ctx = PlaceholderContext {
            file_tree_small: big.clone(),
            ..Default::default()
        };
        let out = substitute_placeholders("[{{FILE_TREE_SMALL}}]", &ctx);
        // Two bracket characters plus the cap.
        assert_eq!(out.chars().count(), FILE_TREE_SMALL_MAX_CHARS + 2);
        assert!(out.starts_with('['));
        assert!(out.ends_with(']'));
    }
}
