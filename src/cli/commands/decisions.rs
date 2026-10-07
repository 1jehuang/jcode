//! `jcode decisions` - extract the decision trail from sessions.
//!
//! Scans persisted sessions, extracts tool calls that carry an `intent` label,
//! and prints them as a TSV decision log grouped by day. Each intent is the
//! model's own stated reason for the action, captured at call time.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use jcode_message_types::{ContentBlock, Role};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

/// One decision: a tool call with its stated intent.
#[derive(Debug, Clone, Serialize)]
struct Decision {
    session_id: String,
    session_title: String,
    tool: String,
    /// The intent the model attached to this call.
    intent: String,
    /// Optional: a file or target the tool operated on.
    target: Option<String>,
}

/// One day's decisions.
#[derive(Debug, Serialize, Default)]
struct DecisionDay {
    #[serde(default)]
    date: String,
    #[serde(default)]
    decisions: Vec<Decision>,
}

/// Decision report.
#[derive(Debug, Serialize)]
struct DecisionReport {
    since: String,
    generated_at: String,
    days: Vec<DecisionDay>,
    total_decisions: usize,
}

/// Parse a human duration like `2h`, `1d`, `3d`, `1w`.
fn parse_since(input: &str) -> Result<chrono::Duration> {
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
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        "w" => chrono::Duration::weeks(n),
        other => anyhow::bail!(
            "unknown duration unit {other:?} in {input:?} (supported: m, h, d, w)"
        ),
    };
    Ok(dur)
}

/// Extract intent from a tool_use content block.
fn extract_intent(block: &ContentBlock) -> Option<(String, Option<String>)> {
    let ContentBlock::ToolUse { name: _, input, .. } = block else {
        return None;
    };
    let intent = input.get("intent")?.as_str()?.trim();
    if intent.is_empty() {
        return None;
    }
    // Capture a likely target: file path, URL, or named resource.
    let target = input
        .get("file_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| {
            input
                .get("target")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        });
    Some((intent.to_string(), target))
}

/// Scan sessions updated after `cutoff` and extract intent-labeled tool calls.
/// Returns decisions paired with the originating message timestamp (for
/// day grouping; the session's updated_at is only a prefilter).
fn collect_decisions(
    cutoff: DateTime<Utc>,
    working_dir_root: Option<&str>,
) -> Result<Vec<(Decision, Option<DateTime<Utc>>)>> {
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
        // Full load: the startup stub skips the transcript, and decisions
        // live in the transcript.
        let Ok(session) = crate::session::Session::load(stem) else {
            continue;
        };
        if session.updated_at < cutoff {
            continue;
        }
        if let Some(root) = working_dir_root {
            if !session
                .working_dir
                .as_ref()
                .is_some_and(|d| Path::new(d).starts_with(root))
            {
                continue;
            }
        }
        let session_id = session.id.clone();
        let session_title = session
            .custom_title
            .clone()
            .or(session.title.clone())
            .unwrap_or_else(|| session_id.clone());
    let mut decisions_with_ts: Vec<(Decision, Option<DateTime<Utc>>)> = Vec::new();
    for msg in &session.messages {
        if !matches!(msg.role, Role::Assistant) {
            continue;
        }
        for block in &msg.content {
            if let Some((intent, target)) = extract_intent(block) {
                decisions_with_ts.push((
                    Decision {
                        session_id: session_id.clone(),
                        session_title: session_title.clone(),
                        tool: match block {
                            ContentBlock::ToolUse { name, .. } => name.clone(),
                            _ => String::new(),
                        },
                        intent,
                        target,
                    },
                    msg.timestamp,
                ));
            }
        }
    }
    decisions_with_ts.sort_by(|a, b| {
        let ta = a.1.map(|t| t.timestamp()).unwrap_or(0);
        let tb = b.1.map(|t| t.timestamp()).unwrap_or(0);
        tb.cmp(&ta)
    });
    out.extend(decisions_with_ts.into_iter());
    }
    Ok(out)
}

/// Render as human-readable text grouped by day.
fn render_text(report: &DecisionReport) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Decisions: {} ({} decisions)\n",
        report.since, report.total_decisions
    ));
    if report.days.is_empty() {
        s.push_str("\nNo decisions found in this window.\n");
        return s;
    }
    for day in &report.days {
        s.push_str(&format!("\n== {} ==\n", day.date));
        for d in &day.decisions {
            let target = d.target.as_deref().unwrap_or("-");
            s.push_str(&format!(
                "  [{}] {} | {} | {} | {}\n",
                d.tool,
                d.session_title
                    .chars()
                    .take(30)
                    .collect::<String>()
                    .replace('\n', " "),
                d.intent.chars().take(80).collect::<String>(),
                target,
                &d.session_id[..d.session_id.len().min(16)]
            ));
        }
    }
    s
}

/// Render as TSV (the primary format for this command).
fn render_tsv(report: &DecisionReport) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "date\tsession_title\ttool\tintent\ttarget\tsession_id"
    )
    .ok();
    for day in &report.days {
        for d in &day.decisions {
            writeln!(
                s,
                "{}\t{}\t{}\t{}\t{}\t{}",
                day.date,
                d.session_title.replace('\t', " ").replace('\n', " "),
                d.tool,
                d.intent.replace('\t', " ").replace('\n', " "),
                d.target.as_deref().unwrap_or("-"),
                d.session_id,
            )
            .ok();
        }
    }
    s
}

/// Entry point for `jcode decisions`.
pub fn run_decisions_command(
    since: &str,
    working_dir: Option<&str>,
    format: &str,
) -> Result<()> {
    let dur = parse_since(since)?;
    let now = Utc::now();
    let cutoff = now - dur;

    let decisions = collect_decisions(cutoff, working_dir)?;

    // Group by the message timestamp date, falling back to today when the
    // message carries no timestamp.
    let mut by_day: BTreeMap<String, DecisionDay> = BTreeMap::new();
    for (dec, ts) in &decisions {
        let date = ts
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| now.format("%Y-%m-%d").to_string());
        by_day.entry(date).or_default().decisions.push(dec.clone());
    }
    // Sort each day's decisions newest-first by session_id.
    for day in by_day.values_mut() {
        day.decisions.sort_by(|a, b| b.session_id.cmp(&a.session_id));
    }
    let days: Vec<DecisionDay> = by_day
        .into_iter()
        .map(|(date, mut d)| {
            d.date = date;
            d
        })
        .collect();
    let total_decisions: usize = days.iter().map(|d| d.decisions.len()).sum();

    // Re-sort days by date descending.
    let mut sorted_days: Vec<_> = days;
    sorted_days.sort_by(|a, b| b.date.cmp(&a.date));

    let report = DecisionReport {
        since: since.to_string(),
        generated_at: now.to_rfc3339(),
        days: sorted_days,
        total_decisions,
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match format {
        "tsv" => write!(out, "{}", render_tsv(&report))?,
        "text" => write!(out, "{}", render_text(&report))?,
        "json" => {
            serde_json::to_writer_pretty(&mut out, &report)?;
            writeln!(out)?;
        }
        other => anyhow::bail!("unknown format {other:?} (supported: tsv, text, json)"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_since_units() {
        assert_eq!(parse_since("30m").unwrap(), chrono::Duration::minutes(30));
        assert_eq!(parse_since("2h").unwrap(), chrono::Duration::hours(2));
        assert_eq!(parse_since("1d").unwrap(), chrono::Duration::days(1));
        assert_eq!(parse_since("3w").unwrap(), chrono::Duration::weeks(3));
        assert!(parse_since("d").is_err());
        assert!(parse_since("5x").is_err());
        assert!(parse_since("").is_err());
    }

    #[test]
    fn extract_intent_present_and_absent() {
        let block = ContentBlock::ToolUse {
            id: "t1".into(),
            name: "bash".into(),
            input: json!({"command": "ls", "intent": "list files"}),
            thought_signature: None,
        };
        let (intent, target) = extract_intent(&block).unwrap();
        assert_eq!(intent, "list files");
        assert_eq!(target, None);

        // No intent key -> None.
        let no_intent = ContentBlock::ToolUse {
            id: "t2".into(),
            name: "bash".into(),
            input: json!({"command": "ls"}),
            thought_signature: None,
        };
        assert!(extract_intent(&no_intent).is_none());

        // Blank intent -> None.
        let blank = ContentBlock::ToolUse {
            id: "t3".into(),
            name: "bash".into(),
            input: json!({"intent": "   "}),
            thought_signature: None,
        };
        assert!(extract_intent(&blank).is_none());

        // Non-tool block -> None.
        let text = ContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        };
        assert!(extract_intent(&text).is_none());
    }

    #[test]
    fn extract_intent_target_precedence() {
        let with_file = ContentBlock::ToolUse {
            id: "t4".into(),
            name: "edit".into(),
            input: json!({"file_path": "/tmp/a.rs", "intent": "fix brace"}),
            thought_signature: None,
        };
        let (intent, target) = extract_intent(&with_file).unwrap();
        assert_eq!(intent, "fix brace");
        assert_eq!(target.as_deref(), Some("/tmp/a.rs"));

        // Empty file_path falls through to `target`.
        let with_target = ContentBlock::ToolUse {
            id: "t5".into(),
            name: "browser".into(),
            input: json!({"file_path": "", "target": "example.com", "intent": "check page"}),
            thought_signature: None,
        };
        let (_, target) = extract_intent(&with_target).unwrap();
        assert_eq!(target.as_deref(), Some("example.com"));
    }

    #[test]
    fn render_tsv_escapes_tabs_and_newlines() {
        let report = DecisionReport {
            since: "1d".into(),
            generated_at: "2026-10-07T00:00:00Z".into(),
            total_decisions: 1,
            days: vec![DecisionDay {
                date: "2026-10-07".into(),
                decisions: vec![Decision {
                    session_id: "s1".into(),
                    session_title: "bad\ttitle\nhere".into(),
                    tool: "bash".into(),
                    intent: "multi\nline\tintent".into(),
                    target: None,
                }],
            }],
        };
        let tsv = render_tsv(&report);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(lines.len(), 2); // header + 1 row
        let fields: Vec<&str> = lines[1].split('\t').collect();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[1], "bad title here");
        assert_eq!(fields[3], "multi line intent");
        assert_eq!(fields[4], "-"); // None target renders as dash
    }

    #[test]
    fn render_text_includes_day_and_total() {
        let report = DecisionReport {
            since: "2h".into(),
            generated_at: "2026-10-07T00:00:00Z".into(),
            total_decisions: 1,
            days: vec![DecisionDay {
                date: "2026-10-07".into(),
                decisions: vec![Decision {
                    session_id: "s1".into(),
                    session_title: "sess".into(),
                    tool: "edit".into(),
                    intent: "fix bug".into(),
                    target: Some("/x.rs".into()),
                }],
            }],
        };
        let text = render_text(&report);
        assert!(text.contains("Decisions: 2h"));
        assert!(text.contains("== 2026-10-07 =="));
        assert!(text.contains("fix bug"));
        assert!(text.contains("/x.rs"));

        let empty = DecisionReport {
            since: "2h".into(),
            generated_at: String::new(),
            total_decisions: 0,
            days: vec![],
        };
        assert!(render_text(&empty).contains("No decisions found"));
    }
}
