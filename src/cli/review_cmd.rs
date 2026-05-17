//! Review Command — git diff based code review
//!
//! Extracted from commands.rs for better modularity.


// Review Command — git diff based code review
// ════════════════════════════════════════════════════════════════════

/// Run code review against git changes
pub async fn run_review_command(
    staged: bool,
    diff: Option<&str>,
    security: bool,
    json: bool,
) -> Result<()> {
    // Get git diff
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
        eprintln!("\n📋 Code Review\n");
        eprintln!("  No changes to review (working tree clean).");
        return Ok(());
    }

    // Parse diff into file-level changes
    let files = parse_diff_files(&diff_text);

    if json {
        let report = serde_json::json!({
            "files_changed": files.len(),
            "files": files,
            "security_mode": security,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let review_type = if security { "Security Review" } else { "Code Review" };
    eprintln!("\n📋 {} — {} file(s) changed\n", review_type, files.len());

    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;

    for file_info in &files {
        let (additions, deletions) = count_diff_stats(&file_info.diff);
        total_additions += additions;
        total_deletions += deletions;

        eprintln!("  📄 {} (+{}/-{})", file_info.path, additions, deletions);

        if security {
            // Security-focused review highlights
            let sec_issues = find_security_issues(&file_info.diff);
            if !sec_issues.is_empty() {
                eprintln!("    ⚠️  Potential security issues:");
                for issue in &sec_issues {
                    eprintln!("      - {}:{} — {}", file_info.path, issue.line, issue.description);
                }
            }
        }

        // Show the diff summary
        let lines: Vec<&str> = file_info.diff.lines().collect();
        let max_show = 30.min(lines.len());
        if max_show > 0 {
            for line in &lines[..max_show] {
                if line.starts_with('+') && !line.starts_with("+++") {
                    eprintln!("    {}", line);
                } else if line.starts_with('-') && !line.starts_with("---") {
                    eprintln!("    {}", line);
                }
            }
            if lines.len() > max_show {
                eprintln!("    ... ({} more lines)", lines.len() - max_show);
            }
        }
        eprintln!();
    }

    eprintln!("  --------------------------------------");
    eprintln!("  Total: +{} / -{} lines across {} file(s)",
        total_additions, total_deletions, files.len());
    eprintln!();

    if security && files.is_empty() {
        eprintln!("  ✅ No security issues detected.");
    } else if security {
        eprintln!("  ⚠️  Review the flagged items above for security best practices.");
    }

    eprintln!("  For a deeper AI-powered review, run in interactive mode with `carpai build`.");
    Ok(())
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
            // Extract file path from "diff --git a/path b/path"
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

struct SecurityIssue {
    line: usize,
    description: String,
}

fn find_security_issues(diff: &str) -> Vec<SecurityIssue> {
    let mut issues = Vec::new();
    let patterns = [
        ("password", "Hardcoded password detected"),
        ("secret", "Possible secret/key exposure"),
        ("token", "Possible token exposure"),
        ("api_key", "Possible API key exposure"),
        ("apikey", "Possible API key exposure"),
        ("ssh-rsa", "SSH key embedded in code"),
        ("-----BEGIN", "Private key block detected"),
        ("eval(", "Use of eval() — code injection risk"),
        ("exec(", "Use of exec() — command injection risk"),
        ("unsafe", "Unsafe Rust block — manual memory safety verification needed"),
    ];

    for (i, line) in diff.lines().enumerate() {
        let lower = line.to_lowercase();
        for (pattern, desc) in &patterns {
            if lower.contains(pattern) && line.starts_with('+') {
                issues.push(SecurityIssue {
                    line: i + 1,
                    description: desc.to_string(),
                });
            }
        }
    }

    issues
}

// ════════════════════════════════════════════════════════════════════
// ════════════════════════════════════════════════════════════════════
// Debug Commands — DAP (Debug Adapter Protocol) integration
// ════════════════════════════════════════════════════════════════════

mod dap;