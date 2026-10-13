//! # jcode-session-memory
//!
//! Structured session notes with 10-section layout, token counting,
//! budget enforcement, custom template loading, and variable substitution.
//!
//! ## Sections
//!
//! Each session note is divided into up to 10 named sections:
//!
//! 1. **Session Summary** - One-line goal description
//! 2. **Key Decisions** - Architectural and design choices
//! 3. **Files Changed** - List of modified files with rationale
//! 4. **Current State** - What works, what's broken, what's next
//! 5. **User Preferences** - Explicit requirements or constraints
//! 6. **Pending Tasks** - Remaining work items
//! 7. **Technical Notes** - Implementation details worth preserving
//! 8. **Errors & Fixes** - Bugs encountered and their solutions
//! 9. **Context Links** - References to issues, PRs, docs
//! 10. **Next Steps** - Immediate next actions
//!
//! ## Budget
//!
//! The total session note is capped at **12K tokens** (2K per section).
//! The [`budget`] module enforces per-section and total limits.
//!
//! ## Templates
//!
//! Custom templates can be loaded from `~/.jcode/session-memory/config/template.md`
//! and must use `{{variable}}` syntax for substitution.

pub mod budget;
pub mod section_analysis;
pub mod substitute;
pub mod template;

pub use budget::{
    BudgetReport, MAX_SECTION_LENGTH, MAX_TOTAL_SESSION_MEMORY_TOKENS, check_budget,
    check_budget_from_content, generate_budget_warnings, truncate_section,
};
pub use section_analysis::{
    SectionAnalysis, SectionInfo, TokenCount, analyze_sections, find_oversized_sections,
};
pub use substitute::VariableSubstitutor;
pub use template::{Template, TemplateLoader};
