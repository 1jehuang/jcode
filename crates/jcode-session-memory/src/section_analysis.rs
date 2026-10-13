//! Token counting and markdown section size analysis.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
const CHARS_PER_TOKEN: f64 = 4.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCount {
    pub chars: usize,
    pub tokens: usize,
}
impl TokenCount {
    pub fn from_chars(chars: usize) -> Self {
        Self {
            chars,
            tokens: (chars as f64 / CHARS_PER_TOKEN).ceil() as usize,
        }
    }
}

/// Per-section analysis used by the budget enforcer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    pub name: String,
    pub index: usize,
    pub token_count: TokenCount,
    pub over_budget: bool,
}
impl SectionInfo {
    pub fn analyze(index: usize, name: &str, text: &str) -> Self {
        Self {
            name: name.to_string(),
            index,
            token_count: TokenCount::from_chars(text.len()),
            over_budget: false,
        }
    }
}

/// Aggregate analysis of all `# `-delimited sections in a markdown document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionAnalysis {
    pub sections: HashMap<String, usize>,
    pub total_tokens: usize,
}

fn count_body(lines: &[&str]) -> usize {
    TokenCount::from_chars(lines.join("\n").trim().len()).tokens
}

/// Parse markdown by `# ` headers and estimate per-section token counts.
pub fn analyze_sections(content: &str) -> SectionAnalysis {
    let mut sections = HashMap::new();
    let mut heading = String::new();
    let mut lines: Vec<&str> = Vec::new();
    for line in content.lines() {
        if line.starts_with("# ") {
            if !heading.is_empty() {
                sections.insert(heading.clone(), count_body(&lines));
            }
            heading = line.to_string();
            lines.clear();
        } else if !heading.is_empty() {
            lines.push(line);
        }
    }
    if !heading.is_empty() {
        sections.insert(heading, count_body(&lines));
    }
    let total_tokens = sections.values().sum();
    SectionAnalysis {
        sections,
        total_tokens,
    }
}

/// Return sections exceeding `max_per_section` tokens, sorted largest first.
pub fn find_oversized_sections(
    analysis: &SectionAnalysis,
    max_per_section: usize,
) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = analysis
        .sections
        .iter()
        .filter(|(_, t)| **t > max_per_section)
        .map(|(n, &t)| (n.clone(), t))
        .collect();
    v.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_count_rounds_up() {
        assert_eq!(TokenCount::from_chars(100).tokens, 25);
        assert_eq!(TokenCount::from_chars(15).tokens, 4);
    }
    #[test]
    fn section_info_fields() {
        let i = SectionInfo::analyze(2, "Key Decisions", "Chose X over Y.");
        assert_eq!(i.index, 2);
        assert_eq!(i.name, "Key Decisions");
    }
    #[test]
    fn analyze_splits_by_header() {
        let a = analyze_sections("# A\nalpha\n\n# B\nbeta");
        assert_eq!(a.sections.len(), 2);
        assert!(a.sections.contains_key("# A") && a.total_tokens > 0);
    }
    #[test]
    fn analyze_no_headers() {
        let a = analyze_sections("just plain text");
        assert!(a.sections.is_empty() && a.total_tokens == 0);
    }
    #[test]
    fn oversized_sorted_descending() {
        let mut s = HashMap::new();
        s.insert("# Small".into(), 10);
        s.insert("# Big".into(), 500);
        let a = SectionAnalysis {
            sections: s,
            total_tokens: 510,
        };
        assert_eq!(
            find_oversized_sections(&a, 100),
            vec![("# Big".to_string(), 500)]
        );
    }
}
