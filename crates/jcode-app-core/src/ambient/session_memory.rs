//! Integration layer for `jcode-session-memory`.
//!
//! Provides budget enforcement for session notes and template-based prompt
//! building. When saving ambient state or building prompts, call into this
//! module to validate that session notes fit within the 12K token budget and
//! to render custom templates with variable substitution.

use jcode_session_memory::{
    BudgetReport, TemplateLoader, VariableSubstitutor,
    analyze_sections, check_budget, check_budget_from_content, generate_budget_warnings,
    truncate_section,
};
use std::collections::HashMap;

/// Maximum tokens allowed per section.
pub const MAX_SECTION_TOKENS: usize = jcode_session_memory::MAX_SECTION_LENGTH;
/// Maximum total tokens across all sections.
pub const MAX_TOTAL_TOKENS: usize = jcode_session_memory::MAX_TOTAL_SESSION_MEMORY_TOKENS;

/// Validate session notes against the budget and return a report.
///
/// Use this when saving ambient state to detect oversized notes before
/// persisting them.
pub fn validate_session_notes(content: &str) -> BudgetReport {
    check_budget_from_content(content)
}

/// Check budget from a pre-built [`SectionAnalysis`].
pub fn validate_from_analysis(
    analysis: &jcode_session_memory::SectionAnalysis,
) -> BudgetReport {
    check_budget(analysis)
}

/// Generate human-readable budget warnings for a report.
pub fn budget_warnings(report: &BudgetReport) -> Vec<String> {
    generate_budget_warnings(report)
}

/// Truncate a section to fit within the per-section token budget.
pub fn fit_section(content: &str) -> String {
    truncate_section(content, MAX_SECTION_TOKENS)
}

/// Build a session notes prompt from a custom template.
///
/// Loads `template.md` from `~/.jcode/session-memory/config/`, substitutes
/// `{{variable}}` placeholders, and returns the rendered text. Falls back
/// to the raw content if no template is found.
pub fn render_session_template(content: &str, variables: &HashMap<String, String>) -> String {
    let substitutor = VariableSubstitutor::new(variables.clone());
    let loader = TemplateLoader::new();

    match loader.load_template() {
        Some(template) => {
            // Merge user content into the template sections, then substitute
            let mut rendered = template.raw.clone();
            for section in &template.sections {
                if let Some(value) = variables.get(&section.name) {
                    let section_header = format!("## {}", section.name);
                    rendered = rendered.replace(&format!("{section_header}\n{}", section.body), value);
                }
            }
            substitutor.substitute(&rendered)
        }
        None => substitutor.substitute(content),
    }
}

/// Analyze sections of a markdown document for size diagnostics.
pub fn diagnose_sections(content: &str) -> jcode_session_memory::SectionAnalysis {
    analyze_sections(content)
}

/// Find sections that exceed the per-section token limit.
pub fn oversized_sections(
    content: &str,
) -> Vec<(String, usize)> {
    let analysis = analyze_sections(content);
    jcode_session_memory::find_oversized_sections(&analysis, MAX_SECTION_TOKENS)
}

/// Convenience: check if content fits within the total token budget.
pub fn fits_budget(content: &str) -> bool {
    !check_budget_from_content(content).is_over_budget
}
