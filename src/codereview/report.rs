use super::review::ReviewResult;

/// Detailed review report formatted for output
pub struct ReviewReport;

impl ReviewReport {
    pub fn format(result: &ReviewResult) -> String {
        result.format()
    }

    pub fn summary(result: &ReviewResult) -> String {
        format!("Score: {}/100 | {} critical, {} high, {} medium | {} total findings",
            result.score, result.critical_count, result.high_count,
            result.medium_count, result.findings.len())
    }

    pub fn markdown(result: &ReviewResult) -> String {
        let mut md = String::new();

        md.push_str(&format!("# Code Review Report\n\n"));
        md.push_str(&format!("**Score:** {}/100\n\n", result.score));

        md.push_str("## Summary\n\n");
        md.push_str(&format!("| Severity | Count |\n|---|---|\n"));
        md.push_str(&format!("| Critical | {} |\n", result.critical_count));
        md.push_str(&format!("| High | {} |\n", result.high_count));
        md.push_str(&format!("| Medium | {} |\n", result.medium_count));
        md.push_str(&format!("| Low | {} |\n", result.low_count));
        md.push('\n');

        if !result.findings.is_empty() {
            md.push_str("## Findings\n\n");
            for (i, finding) in result.findings.iter().enumerate() {
                md.push_str(&format!("### {}. [{}] {} ({})\n\n",
                    i + 1,
                    finding.severity.label(),
                    finding.title,
                    finding.file));
                md.push_str(&format!("- **Severity:** {}\n", finding.severity.label()));
                md.push_str(&format!("- **Category:** {}\n", finding.category.label()));
                md.push_str(&format!("- **Line:** {}\n", finding.line));
                md.push_str(&format!("- **Description:** {}\n", finding.description));
                if let Some(suggestion) = &finding.suggestion {
                    md.push_str(&format!("- **Suggestion:** {}\n", suggestion));
                }
                md.push('\n');
            }
        }

        md
    }
}