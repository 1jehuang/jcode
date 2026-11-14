//! Budget enforcement for session memory.
//!
//! Enforces a total budget of 12K tokens and 2K per section,
//! mirroring Claude Code's session memory constraints.

use crate::section_analysis::{SectionAnalysis, analyze_sections, find_oversized_sections};

/// Maximum total tokens for all session memory sections combined.
pub const MAX_TOTAL_SESSION_MEMORY_TOKENS: usize = 12_000;
/// Maximum tokens allowed for a single section.
pub const MAX_SECTION_LENGTH: usize = 2_000;

/// Result of a budget check against session memory.
#[derive(Debug, Clone)]
pub struct BudgetReport {
    /// Total estimated tokens across all sections.
    pub total_tokens: usize,
    /// Sections exceeding `MAX_SECTION_LENGTH`, sorted largest-first.
    pub oversized_sections: Vec<(String, usize)>,
    /// Whether total tokens exceed `MAX_TOTAL_SESSION_MEMORY_TOKENS`.
    pub is_over_budget: bool,
}

/// Check a section analysis against budget constraints.
pub fn check_budget(analysis: &SectionAnalysis) -> BudgetReport {
    let oversized = find_oversized_sections(analysis, MAX_SECTION_LENGTH);
    BudgetReport {
        total_tokens: analysis.total_tokens,
        oversized_sections: oversized,
        is_over_budget: analysis.total_tokens > MAX_TOTAL_SESSION_MEMORY_TOKENS,
    }
}

/// Generate human-readable warnings for a budget report.
pub fn generate_budget_warnings(report: &BudgetReport) -> Vec<String> {
    let mut warnings = Vec::new();
    if report.is_over_budget {
        warnings.push(format!(
            "CRITICAL: Session memory is ~{} tokens, exceeding the \
             maximum of {} tokens. Condense aggressively.",
            report.total_tokens, MAX_TOTAL_SESSION_MEMORY_TOKENS,
        ));
    }
    if !report.oversized_sections.is_empty() {
        let label = if report.is_over_budget {
            "Oversized sections to condense"
        } else {
            "Sections exceeding per-section limit"
        };
        let mut lines = vec![format!("{}:", label)];
        for (name, tokens) in &report.oversized_sections {
            lines.push(format!(
                "- \"{}\" is ~{} tokens (limit: {})",
                name, tokens, MAX_SECTION_LENGTH,
            ));
        }
        warnings.push(lines.join("\n"));
    }
    warnings
}

/// Truncate text to fit within a token budget (4 chars/token estimate, word-boundary cut).
pub fn truncate_section(content: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(4);
    if content.len() <= max_chars {
        return content.to_string();
    }
    let end = content.floor_char_boundary(max_chars.min(content.len()));
    let truncated = &content[..end];
    match truncated.rfind(' ') {
        Some(pos) => format!("{} ...", &truncated[..pos]),
        None => format!("{} ...", truncated),
    }
}

/// Convenience: check budget directly from a markdown string.
pub fn check_budget_from_content(content: &str) -> BudgetReport {
    check_budget(&analyze_sections(content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::section_analysis::SectionAnalysis;
    fn mk(map: Vec<(&str, usize)>, total: usize) -> SectionAnalysis {
        SectionAnalysis {
            sections: map.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            total_tokens: total,
        }
    }

    #[test]
    fn constants_match_spec() {
        assert_eq!(MAX_TOTAL_SESSION_MEMORY_TOKENS, 12_000);
        assert_eq!(MAX_SECTION_LENGTH, 2_000);
    }

    #[test]
    fn check_budget_within_limits() {
        let r = check_budget(&mk(vec![("# S", 100), ("# N", 200)], 300));
        assert!(!r.is_over_budget);
        assert!(r.oversized_sections.is_empty());
    }

    #[test]
    fn check_budget_over_total() {
        let r = check_budget(&mk(vec![("# A", 6000), ("# B", 7000)], 13_000));
        assert!(r.is_over_budget);
    }

    #[test]
    fn check_budget_oversized_section() {
        let r = check_budget(&mk(vec![("# Short", 100), ("# Huge", 2500)], 2600));
        assert!(!r.is_over_budget);
        assert_eq!(r.oversized_sections.len(), 1);
        assert_eq!(r.oversized_sections[0].0, "# Huge");
    }

    #[test]
    fn warnings_empty_when_ok() {
        let r = check_budget(&mk(vec![("# S", 50)], 50));
        assert!(generate_budget_warnings(&r).is_empty());
    }

    #[test]
    fn warnings_include_critical() {
        let r = check_budget(&mk(vec![("# S", 13_000)], 13_000));
        let w = generate_budget_warnings(&r);
        assert!(w[0].contains("CRITICAL"));
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate_section("Hello world", 2000), "Hello world");
    }

    #[test]
    fn truncate_long_at_word_boundary() {
        let text = "word ".repeat(2000); // 10000 chars > 8000 max_chars
        let result = truncate_section(&text, 2000);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 8005);
    }

    #[test]
    fn check_budget_from_content_works() {
        let r = check_budget_from_content("# Summary\nHi\n\n# Details\nThere");
        assert!(!r.is_over_budget);
    }
}
