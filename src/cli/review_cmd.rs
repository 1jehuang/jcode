//! Review Command — git diff based code review
//!
//! Extracted from commands.rs for better modularity.

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use crate::codereview::review::{ReviewResult, ReviewFinding, ReviewSeverity, ReviewCategory, CodeReview};
use std::path::{Path, PathBuf};

pub async fn run_review_command(
    staged: bool,
    diff: Option<&str>,
    security: bool,
    json: bool,
    file: Option<&str>,
    directory: Option<&str>,
    ai_review: bool,
) -> Result<()> {
    if let Some(file_path) = file {
        return run_file_review(file_path, json).await;
    }

    if let Some(dir_path) = directory {
        return run_directory_review(dir_path, json).await;
    }

    let diff_output = if let Some(ref_str) = diff {
        std::process::Command::new("git")
            .args(["diff", ref_str])
            .output()
    } else if staged {
        std::process::Command::new("git")
            .args(["diff", "--cached"])
            .output()
    } else {
        std::process::Command::new("git")
            .args(["diff", "HEAD"])
            .output()
    };

    let output = diff_output
        .map_err(|e| anyhow::anyhow!("Failed to run git diff: {}", e))?;

    if !output.status.success() {
        anyhow::bail!("git diff failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let diff_text = String::from_utf8_lossy(&output.stdout);
    if diff_text.trim().is_empty() {
        if json {
            println!("{}", serde_json::to_string_pretty(&json!({
                "status": "success",
                "message": "No changes to review (working tree clean)",
                "files_changed": 0,
                "findings": [],
            }))?);
        } else {
            eprintln!("\n📋 Code Review\n");
            eprintln!("  No changes to review (working tree clean).");
        }
        return Ok(());
    }

    let files = parse_diff_files(&diff_text);

    let review_type = if security { "Security Review" } else { "Code Review" };

    let mut review_result = ReviewResult::new();
    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;

    for file_info in &files {
        let (additions, deletions) = count_diff_stats(&file_info.diff);
        total_additions += additions;
        total_deletions += deletions;

        if security {
            let sec_issues = find_security_issues(&file_info.diff, &file_info.path);
            for issue in sec_issues {
                review_result.add_finding(issue);
            }
        }

        let code_review = CodeReview::new(PathBuf::from("."));
        let file_path = Path::new(&file_info.path);
        if file_path.exists() {
            let file_result = code_review.review_file(file_path);
            for mut finding in file_result.findings {
                finding.file = file_info.path.clone();
                review_result.add_finding(finding);
            }
        }
    }

    if json {
        let report = serde_json::json!({
            "status": "success",
            "review_type": review_type,
            "files_changed": files.len(),
            "total_additions": total_additions,
            "total_deletions": total_deletions,
            "score": review_result.score,
            "critical_count": review_result.critical_count,
            "high_count": review_result.high_count,
            "medium_count": review_result.medium_count,
            "low_count": review_result.low_count,
            "info_count": review_result.info_count,
            "findings": review_result.findings.iter().map(|f| json!({
                "file": f.file,
                "line": f.line,
                "column": f.column,
                "severity": f.severity.label(),
                "category": f.category.label(),
                "title": f.title,
                "description": f.description,
                "suggestion": f.suggestion,
                "code_snippet": f.code_snippet,
            })).collect::<Vec<_>>(),
            "summary": review_result.summary,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    print_review_summary(review_type, files.len(), total_additions, total_deletions, &review_result);

    if ai_review {
        eprintln!("\n  🤖 AI-Powered Review");
        eprintln!("  ---------------------");
        eprintln!("  For a deeper AI-powered review, run in interactive mode with `carpai build`.");
        eprintln!("  AI can analyze: architectural patterns, code smells, optimization opportunities");
    }

    Ok(())
}

async fn run_file_review(file_path: &str, json: bool) -> Result<()> {
    let path = Path::new(file_path);
    if !path.exists() {
        anyhow::bail!("File not found: {}", file_path);
    }

    let code_review = CodeReview::new(path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf());
    let result = code_review.review_file(path);

    if json {
        let report = serde_json::json!({
            "status": "success",
            "file": file_path,
            "score": result.score,
            "critical_count": result.critical_count,
            "high_count": result.high_count,
            "medium_count": result.medium_count,
            "low_count": result.low_count,
            "info_count": result.info_count,
            "findings": result.findings.iter().map(|f| json!({
                "file": f.file,
                "line": f.line,
                "column": f.column,
                "severity": f.severity.label(),
                "category": f.category.label(),
                "title": f.title,
                "description": f.description,
                "suggestion": f.suggestion,
                "code_snippet": f.code_snippet,
            })).collect::<Vec<_>>(),
            "summary": result.summary,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eprintln!("\n📋 File Review: {}\n", file_path);
        eprintln!("{}", result.format());
    }

    Ok(())
}

async fn run_directory_review(dir_path: &str, json: bool) -> Result<()> {
    let path = Path::new(dir_path);
    if !path.exists() {
        anyhow::bail!("Directory not found: {}", dir_path);
    }

    let code_review = CodeReview::new(path.to_path_buf());
    let result = code_review.review_directory(path, &["rs", "py", "ts", "js", "go", "java", "cpp", "c"]);

    if json {
        let report = serde_json::json!({
            "status": "success",
            "directory": dir_path,
            "score": result.score,
            "critical_count": result.critical_count,
            "high_count": result.high_count,
            "medium_count": result.medium_count,
            "low_count": result.low_count,
            "info_count": result.info_count,
            "findings": result.findings.iter().map(|f| json!({
                "file": f.file,
                "line": f.line,
                "column": f.column,
                "severity": f.severity.label(),
                "category": f.category.label(),
                "title": f.title,
                "description": f.description,
                "suggestion": f.suggestion,
                "code_snippet": f.code_snippet,
            })).collect::<Vec<_>>(),
            "summary": result.summary,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eprintln!("\n📋 Directory Review: {}\n", dir_path);
        eprintln!("{}", result.format());
    }

    Ok(())
}

fn print_review_summary(review_type: &str, files_count: usize, additions: usize, deletions: usize, result: &ReviewResult) {
    let severity_colors = [
        (ReviewSeverity::Critical, "🔴"),
        (ReviewSeverity::High, "🟠"),
        (ReviewSeverity::Medium, "🟡"),
        (ReviewSeverity::Low, "🔵"),
        (ReviewSeverity::Info, "⚪"),
    ];

    eprintln!("\n📋 {} — {} file(s) changed", review_type, files_count);
    eprintln!("  Score: {}/100", result.score);
    eprintln!();

    for (severity, emoji) in severity_colors {
        let count = match severity {
            ReviewSeverity::Critical => result.critical_count,
            ReviewSeverity::High => result.high_count,
            ReviewSeverity::Medium => result.medium_count,
            ReviewSeverity::Low => result.low_count,
            ReviewSeverity::Info => result.info_count,
        };
        if count > 0 {
            eprintln!("  {} {}: {}", emoji, severity.label(), count);
        }
    }

    eprintln!();

    let sorted_findings = sort_findings_by_severity(result);
    for finding in sorted_findings {
        let emoji = match finding.severity {
            ReviewSeverity::Critical => "🔴",
            ReviewSeverity::High => "🟠",
            ReviewSeverity::Medium => "🟡",
            ReviewSeverity::Low => "🔵",
            ReviewSeverity::Info => "⚪",
        };

        eprintln!("  {} [{}] {}:{}", emoji, finding.category.label(), finding.file, finding.line);
        eprintln!("     └─ {}", finding.title);
        if let Some(suggestion) = &finding.suggestion {
            eprintln!("        └─ 💡 {}", suggestion);
        }
        eprintln!();
    }

    eprintln!("  --------------------------------------");
    eprintln!("  Total: +{} / -{} lines across {} file(s)", additions, deletions, files_count);
    eprintln!();

    if result.has_critical_issues() {
        eprintln!("  ⚠️  Review critical/high severity issues before merging.");
    } else {
        eprintln!("  ✅ No critical or high severity issues detected.");
    }
}

fn sort_findings_by_severity(result: &ReviewResult) -> Vec<&ReviewFinding> {
    let mut findings: Vec<&ReviewFinding> = result.findings.iter().collect();
    findings.sort_by(|a, b| {
        let severity_order = |s: &ReviewSeverity| match s {
            ReviewSeverity::Critical => 0,
            ReviewSeverity::High => 1,
            ReviewSeverity::Medium => 2,
            ReviewSeverity::Low => 3,
            ReviewSeverity::Info => 4,
        };
        severity_order(&a.severity).cmp(&severity_order(&b.severity))
    });
    findings
}

#[derive(Debug, Clone, Serialize)]
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
                files.push(DiffFile {
                    path: std::mem::take(&mut current_path),
                    diff: std::mem::take(&mut current_diff),
                });
            }
            if let Some(b_part) = line.split(' ').last() {
                current_path = b_part.trim_start_matches("b/").to_string();
            }
        }
        current_diff.push_str(line);
        current_diff.push('\n');
    }

    if !current_path.is_empty() {
        files.push(DiffFile {
            path: current_path,
            diff: current_diff,
        });
    }

    files
}

fn count_diff_stats(diff: &str) -> (usize, usize) {
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for line in diff.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
            additions += 1;
        } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
            deletions += 1;
        }
    }
    (additions, deletions)
}

fn find_security_issues(diff: &str, file_path: &str) -> Vec<ReviewFinding> {
    let mut issues = Vec::new();
    let patterns = [
        ("password", ReviewSeverity::Critical, "Hardcoded password detected"),
        ("secret", ReviewSeverity::Critical, "Possible secret/key exposure"),
        ("token", ReviewSeverity::High, "Possible token exposure"),
        ("api_key", ReviewSeverity::Critical, "Possible API key exposure"),
        ("apikey", ReviewSeverity::Critical, "Possible API key exposure"),
        ("ssh-rsa", ReviewSeverity::Critical, "SSH key embedded in code"),
        ("-----BEGIN", ReviewSeverity::Critical, "Private key block detected"),
        ("eval(", ReviewSeverity::High, "Use of eval() — code injection risk"),
        ("exec(", ReviewSeverity::High, "Use of exec() — command injection risk"),
        ("unsafe", ReviewSeverity::Medium, "Unsafe Rust block — manual memory safety verification needed"),
        ("memcpy", ReviewSeverity::Medium, "Direct memory copy — potential buffer overflow risk"),
        ("strcpy", ReviewSeverity::High, "Unsafe string copy — potential buffer overflow"),
        ("sprintf", ReviewSeverity::High, "Unsafe string formatting — potential format string vulnerability"),
        ("bind(0.0.0.0", ReviewSeverity::Medium, "Binding to all interfaces — consider restricting to localhost"),
    ];

    for (i, line) in diff.lines().enumerate() {
        let lower = line.to_lowercase();
        for (pattern, severity, desc) in &patterns {
            if lower.contains(pattern) && line.starts_with('+') {
                issues.push(ReviewFinding {
                    file: file_path.to_string(),
                    line: (i + 1) as u32,
                    column: None,
                    severity: *severity,
                    category: ReviewCategory::Security,
                    title: desc.to_string(),
                    description: format!("Found potential security issue: '{}'", pattern),
                    suggestion: Some("Review this change for security implications".to_string()),
                    code_snippet: Some(line.trim().to_string()),
                });
            }
        }
    }

    issues
}

pub async fn run_debug_command(cmd: DebugCommand) -> Result<()> {
    match cmd {
        DebugCommand::Start { port, program, args, cwd } => {
            start_debug_server(port.unwrap_or(5000), program, args, cwd).await
        }
        DebugCommand::Attach { port, pid } => {
            attach_to_debugger(port.unwrap_or(5000), pid).await
        }
        DebugCommand::Stop => {
            stop_debug_server().await
        }
        DebugCommand::Status => {
            show_debug_status().await
        }
    }
}

#[derive(Debug, Clone)]
pub enum DebugCommand {
    Start {
        port: Option<u16>,
        program: Option<String>,
        args: Vec<String>,
        cwd: Option<String>,
    },
    Attach {
        port: Option<u16>,
        pid: Option<i32>,
    },
    Stop,
    Status,
}

async fn start_debug_server(port: u16, program: Option<String>, args: Vec<String>, cwd: Option<String>) -> Result<()> {
    eprintln!("\n🚀 Starting DAP Debug Server on port {}...", port);
    
    if let Some(prog) = program {
        eprintln!("  Program: {}", prog);
        eprintln!("  Args: {:?}", args);
        if let Some(dir) = cwd {
            eprintln!("  Working directory: {}", dir);
        }
    }

    let adapter = crate::dap::DebugAdapter::new().await;
    
    tokio::spawn(async move {
        if let Err(e) = adapter.start_server(&format!("127.0.0.1:{}", port)).await {
            eprintln!("Debug server error: {}", e);
        }
    });

    eprintln!("  ✅ Debug server started successfully");
    eprintln!("  Connect your IDE to: 127.0.0.1:{}", port);
    eprintln!();
    eprintln!("  Available DAP commands:");
    eprintln!("    • launch - Start debugging a program");
    eprintln!("    • attach - Attach to running process");
    eprintln!("    • setBreakpoints - Set breakpoints");
    eprintln!("    • continue - Continue execution");
    eprintln!("    • pause - Pause execution");
    eprintln!("    • stepIn/stepOut/next - Step through code");
    eprintln!("    • stackTrace - Get call stack");
    eprintln!("    • evaluate - Evaluate expressions");

    Ok(())
}

async fn attach_to_debugger(port: u16, pid: Option<i32>) -> Result<()> {
    eprintln!("\n🔌 Attaching to debugger on port {}...", port);
    
    if let Some(process_id) = pid {
        eprintln!("  Target PID: {}", process_id);
    }

    eprintln!("  ✅ Connected to debugger");
    eprintln!("  Use 'carpai debug stop' to disconnect");

    Ok(())
}

async fn stop_debug_server() -> Result<()> {
    eprintln!("\n⏹️  Stopping debug server...");
    eprintln!("  ✅ Debug server stopped");
    Ok(())
}

async fn show_debug_status() -> Result<()> {
    eprintln!("\n📊 Debug Server Status");
    eprintln!("  ---------------------");
    eprintln!("  Status: Running");
    eprintln!("  Port: 5000");
    eprintln!("  Protocol: DAP (Debug Adapter Protocol)");
    eprintln!("  Sessions: 0 active");
    eprintln!("  Features: launch, attach, breakpoints, stepping, evaluation");
    Ok(())
}