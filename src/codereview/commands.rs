use std::path::PathBuf;
use super::review::{CodeReview, ReviewResult};

/// Ultra-review: deep, comprehensive code analysis
pub struct UltraReview {
    project_root: PathBuf,
}

impl UltraReview {
    pub fn new(project_root: PathBuf) -> Self {
        UltraReview { project_root }
    }

    pub fn execute(&self, target: &str, depth: &str) -> ReviewResult {
        let code_review = CodeReview::new(self.project_root.clone());

        match depth {
            "quick" => self.quick_review(target),
            "deep" => self.deep_review(target),
            _ => self.standard_review(target, &code_review),
        }
    }

    fn quick_review(&self, target: &str) -> ReviewResult {
        let mut result = ReviewResult::new();
        let path = self.project_root.join(target);

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                result.summary = format!("Could not read: {}", target);
                return result;
            }
        };

        for (i, line) in content.lines().enumerate() {
            if line.len() > 120 {
                let review = super::review::ReviewFinding {
                    file: target.to_string(),
                    line: (i + 1) as u32,
                    column: None,
                    severity: super::review::ReviewSeverity::Low,
                    category: super::review::ReviewCategory::Style,
                    title: "Line too long".to_string(),
                    description: format!("{} characters exceeds 120 limit", line.len()),
                    suggestion: None,
                    code_snippet: None,
                };
                result.add_finding(review);
            }
        }

        result.summary = format!("Quick review of {}: {} issues", target, result.findings.len());
        result
    }

    fn standard_review(&self, target: &str, code_review: &CodeReview) -> ReviewResult {
        let path = self.project_root.join(target);
        if path.is_file() {
            code_review.review_file(&path)
        } else if path.is_dir() {
            code_review.review_directory(&path, &["rs", "toml", "md"])
        } else {
            let mut result = ReviewResult::new();
            result.summary = format!("Target not found: {}", target);
            result
        }
    }

    fn deep_review(&self, target: &str) -> ReviewResult {
        let mut result = self.standard_review(target, &CodeReview::new(self.project_root.clone()));

        let security = super::security::SecurityReview::new(self.project_root.clone());
        let path = self.project_root.join(target);

        if path.is_file() {
            let sec_result = security.review_file(&path);
            for finding in sec_result.findings {
                result.add_finding(finding);
            }
        } else if path.is_dir() {
            let sec_result = security.review_directory(&path, &["rs", "toml", "js", "py"]);
            for finding in sec_result.findings {
                result.add_finding(finding);
            }
        }

        result.summary = format!("Deep review of {}: {} issues ({} critical/high)",
            target, result.findings.len(), result.critical_count + result.high_count);
        result
    }
}

/// Review commands that integrate with CLI
pub struct ReviewCommand;

impl ReviewCommand {
    pub fn execute(args: &[String], repo_path: &PathBuf) -> String {
        let target = args.first().map(|s| s.as_str()).unwrap_or(".");
        let depth = if args.contains(&"--deep".to_string()) || args.contains(&"-d".to_string()) {
            "deep"
        } else if args.contains(&"--quick".to_string()) || args.contains(&"-q".to_string()) {
            "quick"
        } else {
            "standard"
        };

        let review = UltraReview::new(repo_path.clone());
        let result = review.execute(target, depth);
        result.format()
    }
}

/// Security review command
pub struct SecurityReviewCommand;

impl SecurityReviewCommand {
    pub fn execute(args: &[String], repo_path: &PathBuf) -> String {
        let target = args.first().map(|s| s.as_str()).unwrap_or(".");
        let security = super::security::SecurityReview::new(repo_path.clone());

        let path = repo_path.join(target);
        let result = if path.is_file() {
            security.review_file(&path)
        } else {
            security.review_directory(&path, &["rs", "toml", "js", "py", "ts"])
        };

        result.format()
    }
}

/// Ultra-review command
pub struct UltraReviewCommand;

impl UltraReviewCommand {
    pub fn execute(args: &[String], repo_path: &PathBuf) -> String {
        ReviewCommand::execute(args, repo_path)
    }
}