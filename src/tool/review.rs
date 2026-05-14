//! Review Tool — AI-driven code review from git diff
//!
//! Analyzes git changes and provides structured review output.
//! Supports staged, unstaged, and diff-ref modes. Optional security scan.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct ReviewTool;

impl ReviewTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ReviewInput {
    /// Review staged changes
    #[serde(default)]
    staged: bool,
    /// Git ref (commit/branch) to diff against
    diff: Option<String>,
    /// Run security-focused review
    #[serde(default)]
    security: bool,
    /// Output JSON format
    #[serde(default)]
    json: bool,
}

#[async_trait]
impl Tool for ReviewTool {
    fn name(&self) -> &str {
        "review"
    }

    fn description(&self) -> &str {
        "Run code review on git changes. Analyzes staged, unstaged, or diff-ref changes, optionally with security scanning."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "staged": {
                    "type": "boolean",
                    "description": "Review only staged (git add-ed) changes"
                },
                "diff": {
                    "type": "string",
                    "description": "Git ref to diff against, e.g. 'HEAD~1', 'main', 'abc123'"
                },
                "security": {
                    "type": "boolean",
                    "description": "Run security-focused review (detect secrets, tokens, unsafe code)"
                },
                "json": {
                    "type": "boolean",
                    "description": "Output review as JSON"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: ReviewInput = serde_json::from_value(input)?;

        // Build git diff command
        let diff_args = if let Some(ref diff_ref) = params.diff {
            vec!["diff", diff_ref]
        } else if params.staged {
            vec!["diff", "--cached"]
        } else {
            let has_unstaged = std::process::Command::new("git")
                .args(["diff", "--stat"])
                .output()
                .ok()
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false);
            if has_unstaged {
                vec!["diff", "HEAD"]
            } else {
                vec!["diff", "--cached"]
            }
        };

        let output = match std::process::Command::new("git").args(&diff_args).output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            Ok(o) => return Ok(ToolOutput::new(format!("git diff failed: {}",
                String::from_utf8_lossy(&o.stderr))).with_title("review: error")),
            Err(e) => return Ok(ToolOutput::new(format!("git error: {}", e))
                .with_title("review: error")),
        };

        if output.trim().is_empty() {
            return Ok(ToolOutput::new("No changes to review (working tree clean).")
                .with_title("review: clean"));
        }

        // Parse diff into file-level changes
        let files = parse_diff_files(&output);
        let mut review = String::new();

        let review_type = if params.security { "Security Review" } else { "Code Review" };
        review.push_str(&format!("# {} — {} file(s) changed\n\n", review_type, files.len()));

        let mut total_add = 0usize;
        let mut total_del = 0usize;
        let mut security_issues = Vec::new();

        for file_info in &files {
            let (add, del) = count_diff_stats(&file_info.diff);
            total_add += add;
            total_del += del;

            review.push_str(&format!("## `{}` (+{}/-{})\n", file_info.path, add, del));

            // Security scan
            if params.security {
                let issues = find_security_issues(&file_info.diff);
                for issue in &issues {
                    security_issues.push(format!("{}:{} — {}", file_info.path, issue.line, issue.desc));
                }
            }

            // Show diff hunks (max 20 lines per file)
            let diff_lines: Vec<&str> = file_info.diff.lines()
                .filter(|l| l.starts_with('+') || l.starts_with('-'))
                .collect();
            let max_show = 20.min(diff_lines.len());
            for line in &diff_lines[..max_show] {
                review.push_str(&format!("  {}\n", line));
            }
            if diff_lines.len() > max_show {
                review.push_str(&format!("  ... ({} more lines)\n", diff_lines.len() - max_show));
            }
            review.push('\n');
        }

        // Summary
        review.push_str(&format!("---\n**Total**: +{} / -{} across {} file(s)\n",
            total_add, total_del, files.len()));

        if params.security {
            if security_issues.is_empty() {
                review.push_str("\n✅ No security issues detected.\n");
            } else {
                review.push_str(&format!("\n⚠️  Security Issues ({})\n", security_issues.len()));
                for issue in &security_issues {
                    review.push_str(&format!("- {}\n", issue));
                }
            }
        }

        if params.json {
            let report = json!({
                "files_changed": files.len(),
                "total_additions": total_add,
                "total_deletions": total_del,
                "security_issues": security_issues,
                "files": files.iter().map(|f| {
                    let (a, d) = count_diff_stats(&f.diff);
                    json!({ "path": f.path, "additions": a, "deletions": d })
                }).collect::<Vec<_>>(),
            });
            return Ok(ToolOutput::new(serde_json::to_string_pretty(&report)?)
                .with_title("review: json report"));
        }

        Ok(ToolOutput::new(review).with_title(format!("review: {} files", files.len())))
    }
}

struct DiffFile {
    path: String,
    diff: String,
}

fn parse_diff_files(diff_text: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_path = String::new();
    let mut current_diff = String::new();

    for line in diff_text.lines() {
        if line.starts_with("diff --git") {
            if !current_path.is_empty() {
                files.push(DiffFile { path: std::mem::take(&mut current_path), diff: std::mem::take(&mut current_diff) });
            }
            if let Some(b_part) = line.split(' ').last() {
                current_path = b_part.trim_start_matches("b/").to_string();
            }
        }
        current_diff.push_str(line);
        current_diff.push('\n');
    }
    if !current_path.is_empty() {
        files.push(DiffFile { path: current_path, diff: current_diff });
    }
    files
}

fn count_diff_stats(diff: &str) -> (usize, usize) {
    let mut add = 0usize;
    let mut del = 0usize;
    for line in diff.lines() {
        let t = line.trim();
        if t.starts_with('+') && !t.starts_with("+++") { add += 1; }
        else if t.starts_with('-') && !t.starts_with("---") { del += 1; }
    }
    (add, del)
}

struct SecurityIssue { line: usize, desc: String }

fn find_security_issues(diff: &str) -> Vec<SecurityIssue> {
    let patterns = [
        ("password", "Hardcoded password"),
        ("secret", "Possible secret/key exposure"),
        ("token", "Possible token exposure"),
        ("api_key", "Possible API key exposure"),
        ("-----BEGIN", "Private key block"),
        ("eval(", "eval() — code injection risk"),
        ("unsafe", "Unsafe block — verify manually"),
    ];
    let mut issues = Vec::new();
    for (i, line) in diff.lines().enumerate() {
        if !line.starts_with('+') { continue; }
        let lower = line.to_lowercase();
        for (pat, desc) in &patterns {
            if lower.contains(pat) {
                issues.push(SecurityIssue { line: i + 1, desc: desc.to_string() });
            }
        }
    }
    issues
}
