//! Code review command - AI-driven code quality analysis
//!
//!对标: Claude Code `review` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewIssue {
    pub severity: String, // critical/high/medium/low
    pub category: String, // security/performance/best_practice/code_smell
    pub file: String,
    pub line: Option<u32>,
    pub title: String,
    pub description: String,
    pub suggestion: Option<String>,
    pub auto_fixable: bool,
}

pub struct ReviewCommand;

impl Command for ReviewCommand {
    fn name(&self) -> &str {
        "review"
    }

    fn description(&self) -> &str {
        "AI-driven code review for git diff or staged changes"
    }

    fn aliases(&self) -> &[&str] {
        &["code-review"]
    }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        let mut staged = false;
        let mut diff_ref: Option<String> = None;
        let mut security_mode = false;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--staged" => staged = true,
                "--diff" => {
                    if i + 1 < args.len() {
                        diff_ref = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                "--security" => security_mode = true,
                _ => {}
            }
            i += 1;
        }

        println!("🔍 Starting code review...");

        // Get diff
        let diff = if staged {
            get_staged_diff().await?
        } else if let Some(ref ref_spec) = diff_ref {
            get_diff_against(ref_spec).await?
        } else {
            get_unstaged_diff().await?
        };

        if diff.is_empty() {
            return Ok(CommandResult::success("No changes to review"));
        }

        // Analyze with AI
        let issues = analyze_with_ai(&diff, security_mode).await?;

        // Render results
        render_review_report(&issues)?;

        Ok(CommandResult::success(format!(
            "Review complete: {} issues found",
            issues.len()
        )))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

async fn get_staged_diff() -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(&["diff", "--cached"])
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn get_unstaged_diff() -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(&["diff"])
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn get_diff_against(ref_spec: &str) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(&["diff", ref_spec])
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn analyze_with_ai(diff: &str, security_mode: bool) -> Result<Vec<ReviewIssue>> {
    // TODO: Integrate with LLM provider for AI analysis
    // For now, return placeholder issues

    let mut issues = Vec::new();

    // Simple pattern-based analysis as placeholder
    if diff.contains("println!") {
        issues.push(ReviewIssue {
            severity: "low".to_string(),
            category: "best_practice".to_string(),
            file: "unknown".to_string(),
            line: None,
            title: "Debug print statement found".to_string(),
            description: "Consider using tracing/log instead of println!".to_string(),
            suggestion: Some("Use tracing::info! or log::info!".to_string()),
            auto_fixable: false,
        });
    }

    if diff.contains("unwrap()") {
        issues.push(ReviewIssue {
            severity: "medium".to_string(),
            category: "best_practice".to_string(),
            file: "unknown".to_string(),
            line: None,
            title: "Unwrap usage detected".to_string(),
            description: "Using unwrap() can cause panics. Consider using ? operator or match.".to_string(),
            suggestion: Some("Replace with ? operator or proper error handling".to_string()),
            auto_fixable: false,
        });
    }

    if security_mode && diff.contains("password") {
        issues.push(ReviewIssue {
            severity: "high".to_string(),
            category: "security".to_string(),
            file: "unknown".to_string(),
            line: None,
            title: "Potential hardcoded credential".to_string(),
            description: "Found 'password' in diff. Ensure credentials are not hardcoded.".to_string(),
            suggestion: Some("Use environment variables or secret management".to_string()),
            auto_fixable: false,
        });
    }

    Ok(issues)
}

fn render_review_report(issues: &[ReviewIssue]) -> Result<()> {
    if issues.is_empty() {
        println!("\n✅ No issues found. Code looks good!");
        return Ok(());
    }

    println!("\n📋 Code Review Report");
    println!("{}", "=".repeat(60));

    let critical_count = issues.iter().filter(|i| i.severity == "critical").count();
    let high_count = issues.iter().filter(|i| i.severity == "high").count();
    let medium_count = issues.iter().filter(|i| i.severity == "medium").count();
    let low_count = issues.iter().filter(|i| i.severity == "low").count();

    println!("\nSummary:");
    println!("  🔴 Critical: {}", critical_count);
    println!("  🟠 High:     {}", high_count);
    println!("  🟡 Medium:   {}", medium_count);
    println!("  🔵 Low:      {}", low_count);
    println!("\n{}", "-".repeat(60));

    for (idx, issue) in issues.iter().enumerate() {
        let icon = match issue.severity.as_str() {
            "critical" => "🔴",
            "high" => "🟠",
            "medium" => "🟡",
            _ => "🔵",
        };

        println!("\n{} {}. [{}] {}", icon, idx + 1, issue.severity.to_uppercase(), issue.title);
        println!("   File: {}", issue.file);
        if let Some(line) = issue.line {
            println!("   Line: {}", line);
        }
        println!("   Category: {}", issue.category);
        println!("   {}", issue.description);

        if let Some(ref suggestion) = issue.suggestion {
            println!("   💡 Suggestion: {}", suggestion);
        }

        if issue.auto_fixable {
            println!("   ✅ Auto-fixable");
        }
    }

    println!("\n{}", "=".repeat(60));
    Ok(())
}
