//! Compactable tool identification for micro-compaction.
//!
//! Defines the set of tools whose results are safe to compact (replace with
//! short placeholders) during micro-compaction. These are tools that produce
//! large outputs — file reads, shell output, search results — where the
//! assistant rarely needs the full raw result after extracting info.

use std::collections::HashSet;
use std::sync::LazyLock;

/// Tool names whose results are candidates for micro-compaction.
///
/// Mirrors the `COMPACTABLE_TOOLS` set from Claude Code's `microCompact.ts`.
/// Edit/Glob/WebSearch/WebFetch are included because their outputs are
/// typically consumed once and not referenced again in full.
pub const COMPACTABLE_TOOLS: &[&str] = &[
    "Read",
    "Bash",
    "Grep",
    "Glob",
    "WebSearch",
    "WebFetch",
    "Edit",
    "Write",
];

/// Lazily-initialized `HashSet` for O(1) lookup.
static COMPACTABLE_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| COMPACTABLE_TOOLS.iter().copied().collect());

/// Returns `true` if `tool_name` is in the compactable set.
///
/// # Examples
///
/// ```
/// use jcode_micro_compact::compactable_tools::is_compactable;
///
/// assert!(is_compactable("Read"));
/// assert!(is_compactable("Bash"));
/// assert!(!is_compactable("Agent"));
/// ```
pub fn is_compactable(tool_name: &str) -> bool {
    COMPACTABLE_SET.contains(tool_name)
}

/// Returns the static slice of compactable tool names.
pub fn get_compactable_tools() -> &'static [&'static str] {
    COMPACTABLE_TOOLS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_expected_tools_are_compactable() {
        for name in &[
            "Read",
            "Bash",
            "Grep",
            "Glob",
            "WebSearch",
            "WebFetch",
            "Edit",
            "Write",
        ] {
            assert!(is_compactable(name), "{name} should be compactable");
        }
    }

    #[test]
    fn non_compactable_tools_are_rejected() {
        for name in &["Agent", "TodoWrite", "Task", "PowerShell", "Config"] {
            assert!(!is_compactable(name), "{name} should not be compactable");
        }
    }

    #[test]
    fn get_compactable_tools_matches_constant() {
        assert_eq!(get_compactable_tools(), COMPACTABLE_TOOLS);
    }

    #[test]
    fn constant_has_expected_length() {
        assert_eq!(COMPACTABLE_TOOLS.len(), 8);
    }

    #[test]
    fn case_sensitive_lookup() {
        assert!(!is_compactable("read"));
        assert!(!is_compactable("READ"));
    }
}
