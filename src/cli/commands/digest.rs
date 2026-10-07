//! `jcode digest` - summarize authored work over a time window.
//!
//! Reads persisted sessions (titles, message counts, user prompts) and git
//! commits across known project roots, grouping by day. Pure read-only CLI:
//! no server round-trip, no session mutation. Designed to answer "what did I
//! get done" without spawning a model call.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use jcode_message_types::{ContentBlock, Role};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One session's contribution to the digest.
#[derive(Debug, Serialize)]
struct DigestSession {
    id: String,
    title: String,
    working_dir: Option<String>,
    updated_at: String,
    user_messages: usize,
    assistant_messages: usize,
    first_user_prompt: Option<String>,
}

/// One git commit's contribution.
#[derive(Debug, Serialize)]
struct DigestCommit {
    hash: String,
    subject: String,
    author: String,
    timestamp: String,
    repo: String,
}

/// One day's aggregate.
#[derive(Debug, Serialize, Default)]
struct DigestDay {
    #[serde(default)]
    date: String,
    #[serde(default)]
    sessions: Vec<DigestSession>,
    #[serde(default)]
    commits: Vec<DigestCommit>,
}

/// Full digest result.
#[derive(Debug, Serialize)]
struct DigestReport {
    since: String,
    generated_at: String,
    days: Vec<DigestDay>,
    total_sessions: usize,
    total_commits: usize,
}

/// Parse a human duration like `2h`, `1d`, `3d`, `1w`, `30m`.
fn parse_since(input: &str) -> Result<Duration> {
    let input = input.trim();
    let (digits, unit) = input.split_at(
        input
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(input.len()),
    );
    let n: i64 = digits
        .parse()
        .with_context(|| format!("invalid duration number in {input:?}"))?;
    if n <= 0 {
        anyhow::bail!("duration must be positive, got {input:?}");
    }
    let dur = match unit {
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        "w" => Duration::weeks(n),
        other => anyhow::bail!(
            "unknown duration unit {other:?} in {input:?} (supported: m, h, d, w)"
        ),
    };
    Ok(dur)
}

/// Extract plain text from a message's content blocks (first Text block wins).
fn first_text(blocks: &[ContentBlock]) -> Option<String> {
    blocks.iter().find_map(|b| match b {
        ContentBlock::Text { text, .. } => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    })
}

/// True when the session's working_dir is under `root` (or no filter given).
fn working_dir_matches(session_dir: &Option<String>, root: Option<&str>) -> bool {
    let Some(root) = root else {
        return true;
    };
    let Some(dir) = session_dir else {
        return false;
    };
    Path::new(dir).starts_with(root)
}

/// Scan the sessions directory for sessions updated after `cutoff`.
fn collect_sessions(
    cutoff: DateTime<Utc>,
    working_dir_root: Option<&str>,
) -> Result<Vec<DigestSession>> {
    let sessions_dir = crate::storage::jcode_dir()?.join("sessions");
    let mut out = Vec::new();
    if !sessions_dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }
        // Cheap pre-filter on the file's updated_at before full parse: load
        // only if mtime is in range. Sessions updated long ago are skipped
        // without deserializing multi-MB transcripts.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        let mtime: DateTime<Utc> = mtime.into();
        if mtime < cutoff {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(session) = crate::session::Session::load_startup_stub(stem) else {
            continue;
        };
        if session.updated_at < cutoff {
            continue;
        }
        if !working_dir_matches(&session.working_dir, working_dir_root) {
            continue;
        }
        // Stub messages may be truncated; count what we have.
        let user_messages = session
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .count();
        let assistant_messages = session
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::Assistant))
            .count();
        let first_user_prompt = session
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::User))
            .and_then(|m| first_text(&m.content))
            .map(|t| t.chars().take(120).collect::<String>());
        out.push(DigestSession {
            id: session.id.clone(),
            title: session
                .custom_title
                .clone()
                .or(session.title.clone())
                .unwrap_or_else(|| session.id.clone()),
            working_dir: session.working_dir.clone(),
            updated_at: session.updated_at.to_rfc3339(),
            user_messages,
            assistant_messages,
            first_user_prompt,
        });
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

/// Discover git repos to scan: the user's common project roots.
/// Scans ~/src/* and ~/app/projects/* (one level deep) plus $PWD.
fn discover_repos() -> Vec<PathBuf> {
    let mut repos = Vec::new();
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return repos,
    };
    let mut push_if_repo = |p: PathBuf| {
        if p.join(".git").exists() && !repos.contains(&p) {
            repos.push(p);
        }
    };
    // Direct cwd if it is a repo.
    if let Ok(cwd) = std::env::current_dir() {
        push_if_repo(cwd);
    }
    for base in [home.join("src"), home.join("app/projects")] {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                push_if_repo(p.clone());
                // One nested level: ~/src/org/repo
                if let Ok(nested) = std::fs::read_dir(&p) {
                    for n in nested.flatten() {
                        let np = n.path();
                        if np.is_dir() {
                            push_if_repo(np);
                        }
                    }
                }
            }
        }
    }
    repos
}

/// Scan one repo for commits by the current user since `cutoff`.
fn scan_repo(repo: &Path, cutoff: DateTime<Utc>) -> Result<Vec<DigestCommit>> {
    let since = cutoff.format("%Y-%m-%dT%H:%M:%S").to_string();
    let output = std::process::Command::new("git")
        .args([
            "-C",
            repo.to_string_lossy().as_ref(),
            "log",
            "--since",
            &since,
            "--pretty=%H%x1f%an%x1f%cI%x1f%s",
        ])
        .output()
        .context("git log failed")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let repo_name = repo
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() != 4 {
            continue;
        }
        out.push(DigestCommit {
            hash: parts[0].chars().take(10).collect(),
            subject: parts[3].to_string(),
            author: parts[1].to_string(),
            timestamp: parts[2].to_string(),
            repo: repo_name.clone(),
        });
    }
    Ok(out)
}

/// Render the report as human-readable text.
fn render_text(report: &DigestReport) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Digest: {} ({} sessions, {} commits)\n",
        report.since, report.total_sessions, report.total_commits
    ));
    if report.days.is_empty() {
        s.push_str("\nNo activity found in this window.\n");
        return s;
    }
    for day in &report.days {
        s.push_str(&format!("\n== {} ==\n", day.date));
        if !day.sessions.is_empty() {
            s.push_str("  Sessions:\n");
            for ses in &day.sessions {
                let dir = ses
                    .working_dir
                    .as_deref()
                    .unwrap_or("(no working dir)");
                s.push_str(&format!(
                    "    - {} ({}u/{}a msgs) [{}]\n",
                    ses.title, ses.user_messages, ses.assistant_messages, dir
                ));
            }
        }
        if !day.commits.is_empty() {
            s.push_str("  Commits:\n");
            for c in &day.commits {
                s.push_str(&format!("    - [{}] {} {}\n", c.repo, c.hash, c.subject));
            }
        }
    }
    s
}

/// Entry point for `jcode digest`.
pub fn run_digest_command(since: &str, working_dir: Option<&str>, json: bool, no_git: bool) -> Result<()> {
    let dur = parse_since(since)?;
    let now = Utc::now();
    let cutoff = now - dur;

    let sessions = collect_sessions(cutoff, working_dir)?;
    let commits = if no_git {
        Vec::new()
    } else {
        let mut all = Vec::new();
        for repo in discover_repos() {
            match scan_repo(&repo, cutoff) {
                Ok(mut c) => all.append(&mut c),
                Err(_) => continue,
            }
        }
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all
    };

    // Group by local date.
    let mut by_day: BTreeMap<String, DigestDay> = BTreeMap::new();
    for ses in sessions {
        let date = ses
            .updated_at
            .parse::<DateTime<Utc>>()
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        by_day.entry(date).or_default().sessions.push(ses);
    }
    for c in commits {
        let date = c
            .timestamp
            .parse::<DateTime<Utc>>()
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        by_day.entry(date).or_default().commits.push(c);
    }
    let days: Vec<DigestDay> = by_day
        .into_iter()
        .map(|(date, mut d)| {
            d.date = date;
            d
        })
        .collect();
    let total_sessions: usize = days.iter().map(|d| d.sessions.len()).sum();
    let total_commits: usize = days.iter().map(|d| d.commits.len()).sum();

    let report = DigestReport {
        since: since.to_string(),
        generated_at: now.to_rfc3339(),
        days,
        total_sessions,
        total_commits,
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &report)?;
        writeln!(out)?;
    } else {
        write!(out, "{}", render_text(&report))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_accepts_supported_units() {
        assert_eq!(parse_since("30m").unwrap(), Duration::minutes(30));
        assert_eq!(parse_since("2h").unwrap(), Duration::hours(2));
        assert_eq!(parse_since("1d").unwrap(), Duration::days(1));
        assert_eq!(parse_since("3w").unwrap(), Duration::weeks(3));
        // Bare number with no unit is rejected, not silently treated as
        // minutes or seconds.
        assert!(parse_since("5").is_err());
        assert!(parse_since("0d").is_err());
        assert!(parse_since("-1d").is_err());
        assert!(parse_since("abc").is_err());
        assert!(parse_since("1x").is_err());
    }

    #[test]
    fn working_dir_filter_matches_subtree_only() {
        // No filter: everything matches.
        assert!(working_dir_matches(&Some("/a/b".into()), None));
        assert!(working_dir_matches(&None, None));
        // With a filter, sessions without a working_dir are excluded.
        assert!(!working_dir_matches(&None, Some("/a")));
        // Subtree matches, sibling does not.
        assert!(working_dir_matches(&Some("/a/b/c".into()), Some("/a/b")));
        assert!(!working_dir_matches(&Some("/a/bx".into()), Some("/a/b")));
        assert!(!working_dir_matches(&Some("/other".into()), Some("/a/b")));
    }

    #[test]
    fn first_text_returns_first_nonempty_text_block() {
        let blocks = vec![
            ContentBlock::Reasoning { text: "hidden".into() },
            ContentBlock::Text { text: "  \n".into(), cache_control: None },
            ContentBlock::Text { text: "  hello  ".into(), cache_control: None },
        ];
        assert_eq!(first_text(&blocks).as_deref(), Some("hello"));
        assert_eq!(first_text(&[]), None);
    }

    #[test]
    fn render_text_reports_empty_window() {
        let report = DigestReport {
            since: "1d".into(),
            generated_at: "2026-10-07T00:00:00Z".into(),
            days: vec![],
            total_sessions: 0,
            total_commits: 0,
        };
        let text = render_text(&report);
        assert!(text.contains("No activity"));
        assert!(text.contains("1d"));
    }

    #[test]
    fn render_text_lists_sessions_and_commits_under_day_headers() {
        let report = DigestReport {
            since: "1d".into(),
            generated_at: "2026-10-07T00:00:00Z".into(),
            days: vec![DigestDay {
                date: "2026-10-07".into(),
                sessions: vec![DigestSession {
                    id: "s1".into(),
                    title: "Fix the thing".into(),
                    working_dir: Some("/repo".into()),
                    updated_at: "2026-10-07T01:00:00Z".into(),
                    user_messages: 3,
                    assistant_messages: 5,
                    first_user_prompt: None,
                }],
                commits: vec![DigestCommit {
                    hash: "abc1234567".into(),
                    subject: "fix: the thing".into(),
                    author: "tester".into(),
                    timestamp: "2026-10-07T02:00:00Z".into(),
                    repo: "demo".into(),
                }],
            }],
            total_sessions: 1,
            total_commits: 1,
        };
        let text = render_text(&report);
        assert!(text.contains("== 2026-10-07 =="));
        assert!(text.contains("Fix the thing"));
        assert!(text.contains("(3u/5a msgs)"));
        assert!(text.contains("[demo] abc1234567 fix: the thing"));
    }
}
