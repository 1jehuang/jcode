use std::path::PathBuf;

use super::review::{ReviewResult, ReviewFinding, ReviewSeverity, ReviewCategory};

/// Security-focused code review engine
pub struct SecurityReview {
    pub project_root: PathBuf,
}

impl SecurityReview {
    pub fn new(project_root: PathBuf) -> Self {
        SecurityReview { project_root }
    }

    pub fn review_file(&self, file_path: &PathBuf) -> ReviewResult {
        let mut result = ReviewResult::new();
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => return result,
        };

        let file_name = file_path.to_string_lossy().to_string();

        self.check_hardcoded_secrets(&content, &file_name, &mut result);
        self.check_sql_injection(&content, &file_name, &mut result);
        self.check_command_injection(&content, &file_name, &mut result);
        self.check_path_traversal(&content, &file_name, &mut result);
        self.check_unsafe_code(&content, &file_name, &mut result);
        self.check_insecure_crypto(&content, &file_name, &mut result);

        result.summary = format!("Security review of {} - {} findings", file_name, result.findings.len());
        result
    }

    pub fn review_directory(&self, dir: &PathBuf, patterns: &[&str]) -> ReviewResult {
        let mut result = ReviewResult::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !path.is_symlink() {
                    let sub = self.review_directory(&path, patterns);
                    for finding in sub.findings {
                        result.add_finding(finding);
                    }
                } else if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if patterns.is_empty() || patterns.iter().any(|p| ext == *p) {
                        let file_result = self.review_file(&path);
                        for finding in file_result.findings {
                            result.add_finding(finding);
                        }
                    }
                }
            }
        }

        result
    }

    fn check_hardcoded_secrets(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        let patterns = ["password", "secret", "api_key", "api-key", "apikey", "token", "auth_key", "auth-key",
                        "aws_secret", "private_key", "access_key"];
        let _line_lower = content.to_lowercase();

        for (i, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            if patterns.iter().any(|p| lower.contains(p)) {
                if lower.contains('=') || lower.contains(':') {
                    result.add_finding(ReviewFinding {
                        file: file_name.to_string(),
                        line: (i + 1) as u32,
                        column: None,
                        severity: if line.contains('=') || line.contains(':') { ReviewSeverity::High } else { ReviewSeverity::Medium },
                        category: ReviewCategory::Security,
                        title: "Potential hardcoded secret".to_string(),
                        description: format!("Line may contain hardcoded credentials: {}", line.trim().chars().take(60).collect::<String>()),
                        suggestion: Some("Use environment variables or a secrets manager instead".to_string()),
                        code_snippet: Some(line.trim().chars().take(40).collect()),
                    });
                }
            }
        }
    }

    fn check_sql_injection(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            if (lower.contains("format!(") || lower.contains(&format!("query")) || lower.contains("execute("))
                && (lower.contains("select") || lower.contains("insert") || lower.contains("delete"))
                && !lower.contains("?") && !lower.contains("$1") && !lower.contains(":param")
            {
                result.add_finding(ReviewFinding {
                    file: file_name.to_string(),
                    line: (i + 1) as u32,
                    column: None,
                    severity: ReviewSeverity::Critical,
                    category: ReviewCategory::Security,
                    title: "Possible SQL injection".to_string(),
                    description: "Raw string formatting used for SQL query, possible injection risk".to_string(),
                    suggestion: Some("Use parameterized queries or an ORM".to_string()),
                    code_snippet: Some(line.trim().to_string()),
                });
            }
        }
    }

    fn check_command_injection(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            if lower.contains("std::process::command") || lower.contains("tokio::process::command") || lower.contains("std::process::Command") {
                if !lower.contains("--") || lower.contains("format!") {
                    result.add_finding(ReviewFinding {
                        file: file_name.to_string(),
                        line: (i + 1) as u32,
                        column: None,
                        severity: ReviewSeverity::Medium,
                        category: ReviewCategory::Security,
                        title: "Potential command injection".to_string(),
                        description: "Constructed shell command may be vulnerable to injection".to_string(),
                        suggestion: Some("Use structured arguments rather than shell strings".to_string()),
                        code_snippet: Some(line.trim().to_string()),
                    });
                }
            }
        }
    }

    fn check_path_traversal(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            if lower.contains("read_to_string") || lower.contains("open(") || lower.contains("read_dir") {
                if lower.contains("../") || lower.contains("..\\") || lower.contains("../../") {
                    result.add_finding(ReviewFinding {
                        file: file_name.to_string(),
                        line: (i + 1) as u32,
                        column: None,
                        severity: ReviewSeverity::Critical,
                        category: ReviewCategory::Security,
                        title: "Path traversal risk".to_string(),
                        description: "Using relative path with directory traversal".to_string(),
                        suggestion: Some("Canonicalize paths and restrict to allowed directories".to_string()),
                        code_snippet: Some(line.trim().to_string()),
                    });
                }
            }
        }
    }

    fn check_unsafe_code(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("unsafe ") || trimmed == "unsafe" || trimmed.starts_with("unsafe{") || trimmed.starts_with("unsafe {") {
                result.add_finding(ReviewFinding {
                    file: file_name.to_string(),
                    line: (i + 1) as u32,
                    column: None,
                    severity: ReviewSeverity::High,
                    category: ReviewCategory::Security,
                    title: "Unsafe code block".to_string(),
                    description: "Unsafe code bypasses Rust's safety guarantees".to_string(),
                    suggestion: Some("Minimize unsafe blocks and document safety invariants".to_string()),
                    code_snippet: Some(trimmed.to_string()),
                });
            }
        }
    }

    fn check_insecure_crypto(&self, content: &str, file_name: &str, result: &mut ReviewResult) {
        let weak_algos = ["md5", "sha1", "des", "rc4", "blowfish", "sha-1"];
        for (i, line) in content.lines().enumerate() {
            let lower = line.to_lowercase();
            for algo in &weak_algos {
                if lower.contains(algo) {
                    result.add_finding(ReviewFinding {
                        file: file_name.to_string(),
                        line: (i + 1) as u32,
                        column: None,
                        severity: ReviewSeverity::High,
                        category: ReviewCategory::Security,
                        title: format!("Weak cryptographic algorithm: {}", algo),
                        description: format!("The '{}' algorithm is considered cryptographically weak", algo),
                        suggestion: Some("Use SHA-256, SHA-3, or Argon2 instead".to_string()),
                        code_snippet: Some(line.trim().to_string()),
                    });
                }
            }
        }
    }
}