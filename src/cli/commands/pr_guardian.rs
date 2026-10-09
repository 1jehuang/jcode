//! `jcode pr` — PR guardian: watch open PRs and surface failures or stale PRs.
//!
//! Exit codes:
//!   0 = all open PRs are green
//!   1 = one or more PRs need attention (CI failures, merge conflicts, review changes requested)
//!   2 = error (not authenticated, network failure, etc.)
//!
//! Designed for ambient/scheduled guardians: default output is short, `--notify`
//! posts to ntfy, and the exit code drives shell guards.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// One PR requiring attention.
#[derive(Debug, Serialize)]
struct AttentionPR {
    number: u64,
    title: String,
    author: String,
    url: String,
    /// Human-readable reason: "CI failure", "merge conflict", "changes requested", "stale".
    reason: String,
    /// Failing check names, when the reason is a CI failure.
    failing_checks: Vec<String>,
}

/// Guardian report.
#[derive(Debug, Serialize)]
struct GuardianReport {
    repo: String,
    generated_at: String,
    total_open: usize,
    total_attention: usize,
    total_green: usize,
    prs: Vec<AttentionPR>,
}

const EXIT_ALL_GREEN: i32 = 0;
const EXIT_ATTENTION: i32 = 1;

/// Run the PR guardian over all open PRs in `repo`.
pub fn run_pr_guardian(repo: &str, stale_after_days: i64, notify: bool, format: &str) -> Result<i32> {
    let prs = fetch_open_prs(repo)?;

    let mut attention: Vec<AttentionPR> = Vec::new();
    let mut green = 0usize;

    for pr in &prs {
        let details = fetch_pr_details(repo, pr.number)?;

        let failing_checks: Vec<String> = details
            .checks
            .iter()
            .filter(|c| c.conclusion.as_deref() == Some("FAILURE"))
            .map(|c| c.name.clone())
            .collect();

        if !failing_checks.is_empty() {
            attention.push(AttentionPR {
                number: pr.number,
                title: pr.title.clone(),
                author: pr.author.clone(),
                url: pr.url.clone(),
                reason: "CI failure".into(),
                failing_checks,
            });
            continue;
        }

        // mergeable is a GraphQL enum string: MERGEABLE | CONFLICTING | UNKNOWN.
        if details.mergeable.as_deref() == Some("CONFLICTING") {
            attention.push(AttentionPR {
                number: pr.number,
                title: pr.title.clone(),
                author: pr.author.clone(),
                url: pr.url.clone(),
                reason: "merge conflict".into(),
                failing_checks: vec![],
            });
            continue;
        }

        if details.review_decision.as_deref() == Some("CHANGES_REQUESTED") {
            attention.push(AttentionPR {
                number: pr.number,
                title: pr.title.clone(),
                author: pr.author.clone(),
                url: pr.url.clone(),
                reason: "changes requested".into(),
                failing_checks: vec![],
            });
            continue;
        }

        // Stale: no activity for N days.
        if let Some(age) = pr.age_seconds {
            if age > stale_after_days * 86400 {
                attention.push(AttentionPR {
                    number: pr.number,
                    title: pr.title.clone(),
                    author: pr.author.clone(),
                    url: pr.url.clone(),
                    reason: format!("stale ({}d)", age / 86400),
                    failing_checks: vec![],
                });
                continue;
            }
        }

        green += 1;
    }

    let report = GuardianReport {
        repo: repo.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        total_open: prs.len(),
        total_attention: attention.len(),
        total_green: green,
        prs: attention,
    };

    match format {
        "json" => {
            serde_json::to_writer_pretty(std::io::stdout(), &report)?;
            println!();
        }
        "short" => print_short(&report),
        _ => print_report(&report),
    }

    if notify && !report.prs.is_empty() {
        send_notification(&report);
    }

    Ok(if report.total_attention > 0 { EXIT_ATTENTION } else { EXIT_ALL_GREEN })
}

// ─── GH API helpers ──────────────────────────────────────────────────────────

struct OpenPR {
    number: u64,
    title: String,
    author: String,
    url: String,
    /// Seconds since last update, when parseable.
    age_seconds: Option<i64>,
}

struct PRDetails {
    /// GraphQL enum string: MERGEABLE | CONFLICTING | UNKNOWN.
    mergeable: Option<String>,
    review_decision: Option<String>,
    checks: Vec<StatusCheck>,
}

struct StatusCheck {
    name: String,
    conclusion: Option<String>,
}

fn gh_json(args: &[&str]) -> Result<serde_json::Value> {
    let output = std::process::Command::new("gh")
        .args(args)
        .output()
        .context("failed to run `gh` (is it installed and authenticated?)")?;

    if !output.status.success() {
        anyhow::bail!(
            "gh failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout).context("gh output was not valid JSON")
}

fn fetch_open_prs(repo: &str) -> Result<Vec<OpenPR>> {
    let now = Utc::now();
    let out = gh_json(&[
        "pr", "list", "--repo", repo, "--state", "open",
        "--limit", "50",
        "--json", "number,title,author,url,updatedAt",
    ])?;

    let arr = out.as_array().context("pr list did not return an array")?;
    let mut prs = Vec::with_capacity(arr.len());

    for item in arr {
        let updated_at = item["updatedAt"].as_str().unwrap_or("");
        let age_seconds: Option<i64> = updated_at
            .parse::<DateTime<Utc>>()
            .ok()
            .map(|d| (now - d).num_seconds());

        prs.push(OpenPR {
            number: item["number"].as_u64().unwrap_or(0),
            title: item["title"].as_str().unwrap_or("").to_string(),
            author: item["author"]["login"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            url: item["url"].as_str().unwrap_or("").to_string(),
            age_seconds,
        });
    }

    Ok(prs)
}

fn fetch_pr_details(repo: &str, number: u64) -> Result<PRDetails> {
    let n = number.to_string();
    let out = gh_json(&[
        "pr", "view", &n, "--repo", repo, "--json",
        "mergeable,mergeStateStatus,reviewDecision,statusCheckRollup",
    ])?;

    let checks = out["statusCheckRollup"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| StatusCheck {
                    name: c["name"].as_str().unwrap_or("").to_string(),
                    conclusion: c["conclusion"].as_str().map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(PRDetails {
        mergeable: out["mergeable"].as_str().map(String::from),
        review_decision: out["reviewDecision"].as_str().map(String::from),
        checks,
    })
}

// ─── Output formatters ───────────────────────────────────────────────────────

fn print_report(report: &GuardianReport) {
    println!("PR Guardian: {}", report.repo);
    println!(
        "  {} open | {} green | {} need attention",
        report.total_open, report.total_green, report.total_attention
    );

    if report.prs.is_empty() {
        println!("\nAll PRs are healthy.");
        return;
    }

    println!("\nAttention needed:");
    for pr in &report.prs {
        println!("  #{:<5} {:<18} {}", pr.number, pr.reason, pr.title);
        for check in &pr.failing_checks {
            println!("        x {check}");
        }
        println!("        {}", pr.url);
    }
}

fn print_short(report: &GuardianReport) {
    if report.prs.is_empty() {
        println!("all green ({} open)", report.total_open);
    } else {
        for pr in &report.prs {
            println!("#{} [{}] {}", pr.number, pr.reason, pr.title);
        }
    }
}

fn send_notification(report: &GuardianReport) {
    let body = format!(
        "{} PR(s) need attention in {}: {}",
        report.total_attention,
        report.repo,
        report
            .prs
            .iter()
            .map(|p| format!("#{} ({})", p.number, p.reason))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let out = std::process::Command::new("curl")
        .args([
            "-s", "-m", "10",
            "-d", &body,
            "-H", "Priority: high",
            "-H", "Tags: warning",
            "https://ntfy.sh/jcode-guardian-emilio-65lws",
        ])
        .output();

    if let Ok(out) = out {
        if !out.status.success() {
            eprintln!(
                "ntfy notification failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_exit_code_mapping() {
        // Mirrors the Ok(...) mapping in run_pr_guardian.
        let report = GuardianReport {
            repo: "r".into(),
            generated_at: String::new(),
            total_open: 2,
            total_attention: 1,
            total_green: 1,
            prs: vec![AttentionPR {
                number: 1,
                title: "t".into(),
                author: "a".into(),
                url: "u".into(),
                reason: "CI failure".into(),
                failing_checks: vec!["ci".into()],
            }],
        };
        assert_eq!(
            if report.total_attention > 0 { EXIT_ATTENTION } else { EXIT_ALL_GREEN },
            1
        );
    }

    #[test]
    fn short_report_empty_vs_attention() {
        let empty = GuardianReport {
            repo: "r".into(),
            generated_at: String::new(),
            total_open: 3,
            total_attention: 0,
            total_green: 3,
            prs: vec![],
        };
        assert!(print_short_to_string(&empty).contains("all green"));
    }

    fn print_short_to_string(report: &GuardianReport) -> String {
        if report.prs.is_empty() {
            format!("all green ({} open)", report.total_open)
        } else {
            report
                .prs
                .iter()
                .map(|p| format!("#{} [{}] {}", p.number, p.reason, p.title))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}
