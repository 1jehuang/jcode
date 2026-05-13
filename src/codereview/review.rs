use std::path::{Path, PathBuf};

/// Severity of a review finding
#[derive(Debug, Clone, PartialEq)]
pub enum ReviewSeverity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl ReviewSeverity {
    pub fn label(&self) -> &str {
        match self {
            ReviewSeverity::Critical => "CRITICAL",
            ReviewSeverity::High => "HIGH",
            ReviewSeverity::Medium => "MEDIUM",
            ReviewSeverity::Low => "LOW",
            ReviewSeverity::Info => "INFO",
        }
    }
}

/// Category of code review finding
#[derive(Debug, Clone)]
pub enum ReviewCategory {
    Security,
    Performance,
    Maintainability,
    Style,
    Logic,
    ErrorHandling,
    Testing,
    Documentation,
    BestPractice,
    Complexity,
    Duplication,
    Custom(String),
}

impl ReviewCategory {
    pub fn label(&self) -> &str {
        match self {
            ReviewCategory::Security => "security",
            ReviewCategory::Performance => "performance",
            ReviewCategory::Maintainability => "maintainability",
            ReviewCategory::Style => "style",
            ReviewCategory::Logic => "logic",
            ReviewCategory::ErrorHandling => "error-handling",
            ReviewCategory::Testing => "testing",
            ReviewCategory::Documentation => "documentation",
            ReviewCategory::BestPractice => "best-practice",
            ReviewCategory::Complexity => "complexity",
            ReviewCategory::Duplication => "duplication",
            ReviewCategory::Custom(s) => s.as_str(),
        }
    }
}

/// A single review finding
#[derive(Debug, Clone)]
pub struct ReviewFinding {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    pub severity: ReviewSeverity,
    pub category: ReviewCategory,
    pub title: String,
    pub description: String,
    pub suggestion: Option<String>,
    pub code_snippet: Option<String>,
}

/// Full code review result
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub findings: Vec<ReviewFinding>,
    pub summary: String,
    pub score: u32,
    pub critical_count: u32,
    pub high_count: u32,
    pub medium_count: u32,
    pub low_count: u32,
    pub info_count: u32,
}

impl ReviewResult {
    pub fn new() -> Self {
        ReviewResult {
            findings: vec![],
            summary: String::new(),
            score: 100,
            critical_count: 0,
            high_count: 0,
            medium_count: 0,
            low_count: 0,
            info_count: 0,
        }
    }

    pub fn add_finding(&mut self, finding: ReviewFinding) {
        match finding.severity {
            ReviewSeverity::Critical => self.critical_count += 1,
            ReviewSeverity::High => self.high_count += 1,
            ReviewSeverity::Medium => self.medium_count += 1,
            ReviewSeverity::Low => self.low_count += 1,
            ReviewSeverity::Info => self.info_count += 1,
        }
        self.score = self.score.saturating_sub(match finding.severity {
            ReviewSeverity::Critical => 15,
            ReviewSeverity::High => 8,
            ReviewSeverity::Medium => 4,
            ReviewSeverity::Low => 2,
            ReviewSeverity::Info => 0,
        });
        self.findings.push(finding);
    }

    pub fn has_critical_issues(&self) -> bool {
        self.critical_count > 0 || self.high_count > 0
    }

    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("Review Score: {}/100\n", self.score));
        output.push_str(&format!("Findings: {} critical, {} high, {} medium, {} low\n\n",
            self.critical_count, self.high_count, self.medium_count, self.low_count));

        for finding in &self.findings {
            output.push_str(&format!("[{}][{}] {}:{} - {}\n",
                finding.severity.label(),
                finding.category.label(),
                finding.file, finding.line, finding.title));
            output.push_str(&format!("  {}\n", finding.description));
            if let Some(suggestion) = &finding.suggestion {
                output.push_str(&format!("  Suggestion: {}\n", suggestion));
            }
            output.push('\n');
        }

        if !self.summary.is_empty() {
            output.push_str(&format!("\nSummary: {}", self.summary));
        }

        output
    }
}

impl Default for ReviewResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Comprehensive code review engine
pub struct CodeReview {
    pub project_root: PathBuf,
}

impl CodeReview {
    pub fn new(project_root: PathBuf) -> Self {
        CodeReview { project_root }
    }

    pub fn review_file(&self, file_path: &Path) -> ReviewResult {
        let mut result = ReviewResult::new();
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => return result,
        };

        let file_name = file_path.to_string_lossy().to_string();

        // Static analysis checks
        self.check_line_length(&content, &file_name, &mut result);
        self.check_todo_comments(&content, &file_name, &mut result);
        self.check_debug_statements(&content, &file_name, &mut result);
        self.check_long_functions(&content, &file_name, &mut result);

        result.summary = format!("Reviewed {} - {} issues found", file_name, result.findings.len());
        result
    }

    pub fn review_directory(&self, dir: &Path, patterns: &[&str]) -> ReviewResult {
        let mut result = ReviewResult::new();
        let mut file_count = 0u32;

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !path.is_symlink() {
                    let sub_result = self.review_directory(&path, patterns);
                    for finding in sub_result.findings {
                        result.add_finding(finding);
                    }
                    file_count += sub_result.score.min(1);
                } else if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if patterns.is_empty() || patterns.iter().any(|p| ext == *p) {
                        let file_result = self.review_file(&path);
                        for finding in file_result.findings {
                            result.add_finding(finding);
                        }
                        file_count += 1;
                    }
                }
            }
        }

        result.summary = format!("Reviewed {} files - {} issues found", file_count, result.findings.len());
        result
    }

    fn check_line_length(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            if line.len() > 120 {
                result.add_finding(ReviewFinding {
                    file: file_name.to_string(),
                    line: (i + 1) as u32,
                    column: None,
                    severity: ReviewSeverity::Low,
                    category: ReviewCategory::Style,
                    title: "Line too long".to_string(),
                    description: format!("Line has {} characters (max 120)", line.len()),
                    suggestion: Some("Break the line into multiple lines or refactor".to_string()),
                    code_snippet: Some(line.chars().take(80).collect()),
                });
            }
        }
    }

    fn check_todo_comments(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            if lower.contains("todo") || lower.contains("fixme") || lower.contains("hack") {
                result.add_finding(ReviewFinding {
                    file: file_name.to_string(),
                    line: (i + 1) as u32,
                    column: None,
                    severity: ReviewSeverity::Info,
                    category: ReviewCategory::Maintainability,
                    title: "Unresolved TODO/FIXME/HACK".to_string(),
                    description: format!("Found TODO/FIXME/HACK comment: {}", line.trim()),
                    suggestion: Some("Address or track this item in your task tracker".to_string()),
                    code_snippet: Some(line.trim().to_string()),
                });
            }
        }
    }

    fn check_debug_statements(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("dbg!(") || trimmed.starts_with("println!") || trimmed.starts_with("eprintln!") {
                // This is a relaxed check - only flag if it's clearly debug-only
                if trimmed.contains("debug") || trimmed.contains("DEBUG") || trimmed.contains("temporary") {
                    result.add_finding(ReviewFinding {
                        file: file_name.to_string(),
                        line: (i + 1) as u32,
                        column: None,
                        severity: ReviewSeverity::Low,
                        category: ReviewCategory::ErrorHandling,
                        title: "Debug statement found".to_string(),
                        description: "A debug print statement was found in production code".to_string(),
                        suggestion: Some("Remove debug statements before committing".to_string()),
                        code_snippet: Some(trimmed.to_string()),
                    });
                }
            }
        }
    }

    fn check_long_functions(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        let mut brace_depth = 0i32;
        let mut fn_start = 0usize;
        let mut fn_name = String::new();
        let mut in_fn = false;

        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if !in_fn && (trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ")) {
                in_fn = true;
                fn_start = i;
                fn_name = trimmed.split_whitespace()
                    .skip_while(|w| *w == "pub" || *w == "fn")
                    .next()
                    .unwrap_or("unknown")
                    .trim_end_matches('(')
                    .to_string();
            }

            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }

            if in_fn && brace_depth == 0 && i > fn_start {
                let line_count = i - fn_start + 1;
                if line_count > 100 {
                    result.add_finding(ReviewFinding {
                        file: file_name.to_string(),
                        line: (fn_start + 1) as u32,
                        column: None,
                        severity: ReviewSeverity::Medium,
                        category: ReviewCategory::Complexity,
                        title: "Function too long".to_string(),
                        description: format!("Function '{}' has {} lines (max recommended: 100)", fn_name, line_count),
                        suggestion: Some("Consider breaking this function into smaller functions".to_string()),
                        code_snippet: None,
                    });
                }
                in_fn = false;
            }
        }
    }
}