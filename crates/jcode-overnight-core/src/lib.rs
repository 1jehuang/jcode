use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const OVERNIGHT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OvernightDuration {
    pub minutes: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OvernightCommand {
    Start {
        duration: OvernightDuration,
        mission: Option<String>,
    },
    Status,
    Log,
    Review,
    Cancel,
    Help,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OvernightRunStatus {
    Running,
    CancelRequested,
    Completed,
    Failed,
}

impl OvernightRunStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::CancelRequested => "cancel requested",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightManifest {
    pub version: u32,
    pub run_id: String,
    pub parent_session_id: String,
    pub coordinator_session_id: String,
    pub coordinator_session_name: String,
    pub started_at: DateTime<Utc>,
    pub target_wake_at: DateTime<Utc>,
    pub handoff_ready_at: DateTime<Utc>,
    pub post_wake_grace_until: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub morning_report_posted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub status: OvernightRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub max_agents_guidance: u8,
    pub process_id: u32,
    pub run_dir: PathBuf,
    pub events_path: PathBuf,
    pub human_log_path: PathBuf,
    pub review_path: PathBuf,
    pub review_notes_path: PathBuf,
    pub preflight_path: PathBuf,
    pub task_cards_dir: PathBuf,
    pub issue_drafts_dir: PathBuf,
    pub validation_dir: PathBuf,
    pub last_activity_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightEvent {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub details: Value,
    #[serde(default)]
    pub meaningful: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceSnapshot {
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_available_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_total_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_free_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_one: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_available_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageProviderSnapshot {
    pub provider_name: String,
    pub hard_limit_reached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub limits: Vec<UsageLimitSnapshot>,
    pub extra_info: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimitSnapshot {
    pub name: String,
    pub usage_percent: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageProjection {
    pub captured_at: DateTime<Utc>,
    pub risk: String,
    pub confidence: String,
    pub projected_delta_min_percent: Option<f32>,
    pub projected_delta_max_percent: Option<f32>,
    pub projected_end_min_percent: Option<f32>,
    pub projected_end_max_percent: Option<f32>,
    pub providers: Vec<UsageProviderSnapshot>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_count: Option<usize>,
    #[serde(default)]
    pub dirty_summary: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightPreflight {
    pub captured_at: DateTime<Utc>,
    pub usage: UsageProjection,
    pub resources: ResourceSnapshot,
    pub git: GitSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OvernightTaskCardBefore {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OvernightTaskCardAfter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    #[serde(default)]
    pub files_changed: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OvernightTaskCardValidation {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OvernightTaskCard {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifiability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default)]
    pub before: OvernightTaskCardBefore,
    #[serde(default)]
    pub after: OvernightTaskCardAfter,
    #[serde(default)]
    pub validation: OvernightTaskCardValidation,
    #[serde(default)]
    pub followups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OvernightTaskStatusCounts {
    pub completed: usize,
    pub active: usize,
    pub blocked: usize,
    pub deferred: usize,
    pub failed: usize,
    pub skipped: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OvernightTaskCardSummary {
    pub total: usize,
    pub counts: OvernightTaskStatusCounts,
    pub validated: usize,
    pub high_risk: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightProgressCard {
    pub run_id: String,
    pub status: String,
    pub phase: String,
    pub coordinator_session_id: String,
    pub coordinator_session_name: String,
    pub elapsed_label: String,
    pub target_duration_label: String,
    pub progress_percent: f32,
    pub target_wake_at: String,
    pub time_relation: String,
    pub last_activity_label: String,
    pub next_prompt_label: String,
    pub usage_risk: String,
    pub usage_confidence: String,
    pub usage_projection: String,
    pub resources_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_event_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_event_summary: Option<String>,
    pub task_summary: OvernightTaskCardSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_task_title: Option<String>,
    pub review_path: String,
    pub log_path: String,
    pub run_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

pub fn parse_overnight_command(trimmed: &str) -> Option<Result<OvernightCommand, String>> {
    let rest = trimmed.strip_prefix("/overnight")?.trim();
    if rest.is_empty() || rest == "help" || rest == "--help" || rest == "-h" {
        return Some(Ok(OvernightCommand::Help));
    }

    match rest {
        "status" => return Some(Ok(OvernightCommand::Status)),
        "log" => return Some(Ok(OvernightCommand::Log)),
        "review" | "open" => return Some(Ok(OvernightCommand::Review)),
        "cancel" | "stop" => return Some(Ok(OvernightCommand::Cancel)),
        _ => {}
    }

    if rest.starts_with("status ")
        || rest.starts_with("log ")
        || rest.starts_with("review ")
        || rest.starts_with("cancel ")
    {
        return Some(Err(overnight_usage().to_string()));
    }

    let mut parts = rest.splitn(2, char::is_whitespace);
    let duration_raw = parts.next().unwrap_or_default();
    let mission = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let duration = match parse_duration(duration_raw) {
        Ok(duration) => duration,
        Err(error) => return Some(Err(error)),
    };

    Some(Ok(OvernightCommand::Start { duration, mission }))
}

pub fn overnight_usage() -> &'static str {
    "Usage: `/overnight <hours>[h|m] [mission]`, `/overnight status`, `/overnight log`, `/overnight review`, or `/overnight cancel`"
}

pub fn parse_duration(input: &str) -> std::result::Result<OvernightDuration, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(overnight_usage().to_string());
    }

    let (number, multiplier) = if let Some(hours) = raw.strip_suffix('h') {
        (hours, 60.0)
    } else if let Some(minutes) = raw.strip_suffix('m') {
        (minutes, 1.0)
    } else {
        (raw, 60.0)
    };

    let value: f64 = number.parse().map_err(|_| {
        format!(
            "Invalid overnight duration `{}`. {}",
            raw,
            overnight_usage()
        )
    })?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "Invalid overnight duration `{}`. Duration must be greater than zero.",
            raw
        ));
    }
    let minutes = (value * multiplier).round() as u32;
    if minutes == 0 || minutes > 72 * 60 {
        return Err("Overnight duration must be between 1 minute and 72 hours.".to_string());
    }
    Ok(OvernightDuration { minutes })
}

pub fn summarize_task_cards_slice(cards: &[OvernightTaskCard]) -> OvernightTaskCardSummary {
    let mut summary = OvernightTaskCardSummary {
        total: cards.len(),
        ..Default::default()
    };
    for card in cards {
        match task_status_bucket(&card.status) {
            "completed" => summary.counts.completed += 1,
            "active" => summary.counts.active += 1,
            "blocked" => summary.counts.blocked += 1,
            "deferred" => summary.counts.deferred += 1,
            "failed" => summary.counts.failed += 1,
            "skipped" => summary.counts.skipped += 1,
            _ => summary.counts.unknown += 1,
        }
        if task_card_validated(card) {
            summary.validated += 1;
        }
        if card
            .risk
            .as_deref()
            .map(|risk| risk.to_ascii_lowercase().contains("high"))
            .unwrap_or(false)
        {
            summary.high_risk += 1;
        }
    }
    if let Some(latest) = cards.last() {
        summary.latest_title = Some(task_card_title(latest));
        summary.latest_status = Some(if latest.status.trim().is_empty() {
            "unknown".to_string()
        } else {
            latest.status.clone()
        });
    }
    summary
}

pub fn task_card_title(card: &OvernightTaskCard) -> String {
    if !card.title.trim().is_empty() {
        card.title.clone()
    } else if !card.id.trim().is_empty() {
        card.id.clone()
    } else {
        "untitled task".to_string()
    }
}

pub fn task_status_bucket(status: &str) -> &'static str {
    let normalized = status.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "done" | "complete" | "completed" | "fixed" | "validated" | "merged" => "completed",
        "active" | "running" | "in_progress" | "working" | "verifying" | "planned" => "active",
        "blocked" | "needs_user" | "waiting" => "blocked",
        "deferred" | "queued" | "backlog" | "todo" => "deferred",
        "failed" | "error" | "abandoned" => "failed",
        "skipped" | "rejected" | "not_started" => "skipped",
        _ => "unknown",
    }
}

pub fn task_card_validated(card: &OvernightTaskCard) -> bool {
    let result = card
        .validation
        .result
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    result.contains("pass")
        || result.contains("success")
        || result.contains("validated")
        || result == "ok"
}

pub fn event_class(kind: &str) -> &'static str {
    if kind.contains("failed") || kind.contains("cancel") {
        "bad"
    } else if kind.contains("warning") || kind.contains("requested") || kind.contains("handoff") {
        "warn"
    } else if kind.contains("completed") || kind.contains("started") {
        "ok"
    } else {
        "info"
    }
}

pub fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn render_task_cards_html(cards: &[OvernightTaskCard]) -> String {
    if cards.is_empty() {
        return "<p class=\"meta\">No structured task cards have been written yet. The coordinator should create `task-cards/*.json` as meaningful tasks are selected.</p>".to_string();
    }

    let mut out = String::from("<div class=\"task-grid\">\n");
    for card in cards.iter().rev() {
        out.push_str("<article class=\"task-card\">\n");
        out.push_str(&format!(
            "<h3>{}</h3>\n<div class=\"meta\"><span class=\"status-pill\">{}</span>{}</div>\n",
            html_escape(&task_card_title(card)),
            html_escape(if card.status.trim().is_empty() {
                "unknown"
            } else {
                card.status.trim()
            }),
            html_escape(&task_card_meta(card))
        ));
        push_optional_task_paragraph(&mut out, "Why selected", card.why_selected.as_deref());
        push_optional_task_paragraph(&mut out, "Verifiability", card.verifiability.as_deref());
        push_optional_task_paragraph(&mut out, "Before", card.before.problem.as_deref());
        push_list(&mut out, "Before evidence", &card.before.evidence);
        push_optional_task_paragraph(&mut out, "After", card.after.change.as_deref());
        push_list(&mut out, "Files changed", &card.after.files_changed);
        push_list(&mut out, "After evidence", &card.after.evidence);
        push_list(&mut out, "Validation commands", &card.validation.commands);
        push_optional_task_paragraph(
            &mut out,
            "Validation result",
            card.validation.result.as_deref(),
        );
        push_list(&mut out, "Validation evidence", &card.validation.evidence);
        push_optional_task_paragraph(&mut out, "Outcome", card.outcome.as_deref());
        push_list(&mut out, "Followups", &card.followups);
        out.push_str("</article>\n");
    }
    out.push_str("</div>");
    out
}

pub fn task_card_meta(card: &OvernightTaskCard) -> String {
    let mut parts = Vec::new();
    if let Some(priority) = card
        .priority
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("priority: {}", priority.trim()));
    }
    if let Some(source) = card
        .source
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("source: {}", source.trim()));
    }
    if let Some(risk) = card
        .risk
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("risk: {}", risk.trim()));
    }
    if let Some(updated_at) = card
        .updated_at
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("updated: {}", updated_at.trim()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" · "))
    }
}

pub fn push_optional_task_paragraph(out: &mut String, label: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    out.push_str(&format!(
        "<p><strong>{}</strong>: {}</p>\n",
        html_escape(label),
        html_escape(value)
    ));
}

pub fn push_list(out: &mut String, label: &str, values: &[String]) {
    let values: Vec<&str> = values
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    if values.is_empty() {
        return;
    }
    out.push_str(&format!(
        "<p><strong>{}</strong></p>\n<ul>\n",
        html_escape(label)
    ));
    for value in values {
        out.push_str(&format!("<li>{}</li>\n", html_escape(value)));
    }
    out.push_str("</ul>\n");
}

pub fn build_review_html(
    manifest: &OvernightManifest,
    events: &[OvernightEvent],
    notes: &str,
    preflight: &str,
    task_cards: &[OvernightTaskCard],
) -> String {
    let task_summary = summarize_task_cards_slice(task_cards);
    let task_cards_html = render_task_cards_html(task_cards);
    let timeline = render_timeline_html(events, 200);

    let status = manifest.status.label();
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>Overnight run {run_id}</title>
<style>
body {{ font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 0; background: #0f1117; color: #e8eaf0; }}
a {{ color: #8ab4ff; }}
header {{ padding: 28px 36px; background: linear-gradient(135deg, #1d2340, #12141c); border-bottom: 1px solid #30364a; }}
main {{ padding: 24px 36px 48px; max-width: 1200px; margin: 0 auto; }}
.cards {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 14px; margin-top: 18px; }}
	.card {{ background: #171b26; border: 1px solid #2c3347; border-radius: 14px; padding: 16px; }}
	.card .label {{ color: #9aa4bc; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }}
	.card .value {{ font-size: 18px; margin-top: 6px; }}
	.task-grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 14px; }}
	.task-card {{ background: #111620; border: 1px solid #2b3348; border-radius: 14px; padding: 16px; }}
	.task-card h3 {{ margin: 0 0 8px; }}
	.task-card p {{ margin: 8px 0; }}
	.task-card ul {{ margin: 8px 0 0 18px; padding: 0; }}
	.meta {{ color: #9aa4bc; font-size: 13px; }}
	.status-pill {{ display: inline-block; margin-right: 6px; padding: 3px 8px; border-radius: 999px; background: #24314f; color: #cfe0ff; font-size: 12px; }}
	section {{ margin-top: 28px; background: #151923; border: 1px solid #2a3041; border-radius: 16px; padding: 20px; }}
	h1, h2 {{ margin: 0 0 12px; }}
ul.timeline {{ list-style: none; padding: 0; margin: 0; }}
.timeline li {{ display: grid; grid-template-columns: 86px 240px 1fr; gap: 12px; padding: 9px 0; border-bottom: 1px solid #252b3a; }}
.timeline li:last-child {{ border-bottom: none; }}
.timeline time {{ color: #9aa4bc; }}
.timeline strong {{ color: #d9def0; }}
.timeline .ok strong {{ color: #8ee99a; }}
.timeline .warn strong {{ color: #ffd166; }}
.timeline .bad strong {{ color: #ff7b7b; }}
pre {{ white-space: pre-wrap; word-break: break-word; background: #0b0d12; color: #e8eaf0; padding: 14px; border-radius: 12px; border: 1px solid #272d3c; overflow-x: auto; }}
.badge {{ display: inline-block; padding: 4px 9px; border-radius: 999px; background: #24314f; color: #cfe0ff; font-size: 12px; }}
.path {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: #b7c4e8; }}
</style>
</head>
<body>
<header>
  <h1>🌙 Overnight run <code>{run_id}</code></h1>
  <div class="badge">{status}</div>
  <div class="cards">
    <div class="card"><div class="label">Coordinator</div><div class="value"><code>{coordinator}</code><br>{coordinator_name}</div></div>
	    <div class="card"><div class="label">Started</div><div class="value">{started}</div></div>
	    <div class="card"><div class="label">Target wake</div><div class="value">{target}</div></div>
	    <div class="card"><div class="label">Last activity</div><div class="value">{last_activity}</div></div>
	    <div class="card"><div class="label">Task cards</div><div class="value">{task_completed}/{task_total} complete<br><span class="meta">{task_active} active · {task_blocked} blocked · {task_deferred} deferred</span></div></div>
	  </div>
	</header>
	<main>
<section>
  <h2>Executive summary</h2>
  <p>Mission: {mission}</p>
	  <p>Working directory: <span class="path">{working_dir}</span></p>
	  <p>Provider/model: <code>{provider}</code> / <code>{model}</code></p>
	</section>
	<section>
	  <h2>Structured task cards</h2>
	  {task_cards_html}
	</section>
	<section>
	  <h2>Coordinator review notes</h2>
  <pre>{notes}</pre>
</section>
<section>
  <h2>Timeline</h2>
  <ul class="timeline">
  {timeline}
  </ul>
</section>
<section>
  <h2>Preflight, usage, and resources</h2>
  <pre>{preflight}</pre>
</section>
<section>
  <h2>Artifacts</h2>
  <ul>
    <li>Human log: <span class="path">{human_log}</span></li>
    <li>Events JSONL: <span class="path">{events_path}</span></li>
    <li>Task cards: <span class="path">{task_cards}</span></li>
    <li>Issue drafts: <span class="path">{issue_drafts}</span></li>
    <li>Validation outputs: <span class="path">{validation}</span></li>
  </ul>
</section>
</main>
</body>
</html>"#,
        run_id = html_escape(&manifest.run_id),
        status = html_escape(status),
        coordinator = html_escape(&manifest.coordinator_session_id),
        coordinator_name = html_escape(&manifest.coordinator_session_name),
        started = html_escape(&manifest.started_at.to_rfc3339()),
        target = html_escape(&manifest.target_wake_at.to_rfc3339()),
        last_activity = html_escape(&manifest.last_activity_at.to_rfc3339()),
        task_total = task_summary.total,
        task_completed = task_summary.counts.completed,
        task_active = task_summary.counts.active,
        task_blocked = task_summary.counts.blocked,
        task_deferred = task_summary.counts.deferred,
        mission = html_escape(
            manifest
                .mission
                .as_deref()
                .unwrap_or("Continue the current session's highest-value verified work.")
        ),
        working_dir = html_escape(manifest.working_dir.as_deref().unwrap_or("unknown")),
        provider = html_escape(&manifest.provider_name),
        model = html_escape(&manifest.model),
        task_cards_html = task_cards_html,
        notes = html_escape(notes),
        timeline = timeline,
        preflight = html_escape(preflight),
        human_log = html_escape(&manifest.human_log_path.display().to_string()),
        events_path = html_escape(&manifest.events_path.display().to_string()),
        task_cards = html_escape(&manifest.task_cards_dir.display().to_string()),
        issue_drafts = html_escape(&manifest.issue_drafts_dir.display().to_string()),
        validation = html_escape(&manifest.validation_dir.display().to_string()),
    )
}

pub fn render_timeline_html(events: &[OvernightEvent], limit: usize) -> String {
    let mut timeline = String::new();
    for event in events
        .iter()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let class = event_class(&event.kind);
        timeline.push_str(&format!(
            "<li class=\"{}\"><time>{}</time><strong>{}</strong><span>{}</span></li>\n",
            class,
            html_escape(&event.timestamp.format("%H:%M:%S").to_string()),
            html_escape(&event.kind),
            html_escape(&event.summary)
        ));
    }
    timeline
}

pub fn resource_summary(snapshot: &ResourceSnapshot) -> String {
    let memory = snapshot
        .memory_used_percent
        .map(|pct| format!("RAM {:.0}%", pct))
        .unwrap_or_else(|| "RAM unknown".to_string());
    let load = snapshot
        .load_one
        .zip(snapshot.cpu_count)
        .map(|(load, cpus)| format!("load {:.1}/{}", load, cpus))
        .unwrap_or_else(|| "load unknown".to_string());
    let battery = snapshot
        .battery_percent
        .map(|pct| {
            format!(
                "battery {}%{}",
                pct,
                snapshot
                    .battery_status
                    .as_ref()
                    .map(|status| format!(" {}", status))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "battery unknown".to_string());
    format!("{}, {}, {}", memory, load, battery)
}

pub fn git_summary(snapshot: &GitSnapshot) -> String {
    if let Some(error) = snapshot.error.as_ref() {
        return format!("git unavailable ({})", error);
    }
    let dirty = snapshot.dirty_count.unwrap_or(0);
    let branch = snapshot.branch.as_deref().unwrap_or("unknown branch");
    if dirty == 0 {
        format!("{} clean", branch)
    } else {
        format!(
            "{} with {} dirty file{}",
            branch,
            dirty,
            if dirty == 1 { "" } else { "s" }
        )
    }
}

pub fn format_minutes(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    let hours = minutes / 60;
    let mins = minutes % 60;
    if mins == 0 {
        format!("{}h", hours)
    } else {
        format!("{}h {}m", hours, mins)
    }
}

pub fn build_progress_card_from_parts(
    manifest: &OvernightManifest,
    events: &[OvernightEvent],
    preflight: Option<&OvernightPreflight>,
    task_cards: &[OvernightTaskCard],
    now: DateTime<Utc>,
) -> OvernightProgressCard {
    let target_minutes = manifest
        .target_wake_at
        .signed_duration_since(manifest.started_at)
        .num_minutes()
        .max(1) as u32;
    let elapsed_minutes = now
        .signed_duration_since(manifest.started_at)
        .num_minutes()
        .max(0) as u32;
    let progress_percent = ((elapsed_minutes as f32 / target_minutes as f32) * 100.0).min(100.0);
    let latest_event = events
        .iter()
        .rev()
        .find(|event| event.meaningful)
        .or_else(|| events.last());
    let latest_resource = events
        .iter()
        .rev()
        .find(|event| event.kind == "resource_sample")
        .and_then(|event| serde_json::from_value::<ResourceSnapshot>(event.details.clone()).ok())
        .or_else(|| preflight.map(|preflight| preflight.resources.clone()));
    let resources_summary = latest_resource
        .as_ref()
        .map(resource_summary)
        .unwrap_or_else(|| "resources pending".to_string());
    let usage = preflight.map(|preflight| &preflight.usage);
    let usage_projection = usage
        .and_then(|usage| {
            usage
                .projected_end_min_percent
                .zip(usage.projected_end_max_percent)
        })
        .map(|(min, max)| format!("projected {:.0}% to {:.0}%", min, max))
        .unwrap_or_else(|| "projection pending".to_string());
    let task_summary = summarize_task_cards_slice(task_cards);
    let active_task_title = task_cards
        .iter()
        .rev()
        .find(|card| matches!(task_status_bucket(&card.status), "active" | "blocked"))
        .map(task_card_title)
        .or_else(|| task_summary.latest_title.clone());

    OvernightProgressCard {
        run_id: manifest.run_id.clone(),
        status: manifest.status.label().to_string(),
        phase: overnight_phase(manifest, now).to_string(),
        coordinator_session_id: manifest.coordinator_session_id.clone(),
        coordinator_session_name: manifest.coordinator_session_name.clone(),
        elapsed_label: format_minutes(elapsed_minutes),
        target_duration_label: format_minutes(target_minutes),
        progress_percent,
        target_wake_at: manifest.target_wake_at.to_rfc3339(),
        time_relation: time_relation_to_target(manifest, now),
        last_activity_label: relative_time(manifest.last_activity_at, now),
        next_prompt_label: next_prompt_label(manifest, now),
        usage_risk: usage
            .map(|usage| usage.risk.clone())
            .unwrap_or_else(|| "pending".to_string()),
        usage_confidence: usage
            .map(|usage| usage.confidence.clone())
            .unwrap_or_else(|| "pending".to_string()),
        usage_projection,
        resources_summary,
        latest_event_kind: latest_event.map(|event| event.kind.clone()),
        latest_event_summary: latest_event.map(|event| event.summary.clone()),
        task_summary,
        active_task_title,
        review_path: manifest.review_path.display().to_string(),
        log_path: manifest.human_log_path.display().to_string(),
        run_dir: manifest.run_dir.display().to_string(),
        completed_at: manifest.completed_at.map(|at| at.to_rfc3339()),
    }
}

pub fn format_status_markdown_from_summary(
    manifest: &OvernightManifest,
    task_summary: &OvernightTaskCardSummary,
    now: DateTime<Utc>,
) -> String {
    let remaining = manifest
        .target_wake_at
        .signed_duration_since(now)
        .num_minutes();
    let remaining_line = if remaining >= 0 {
        format!("Target wake time in {}.", format_minutes(remaining as u32))
    } else {
        format!(
            "Target wake time passed {} ago.",
            format_minutes((-remaining) as u32)
        )
    };
    format!(
        "🌙 **Overnight run `{}`**\n\nStatus: **{}**\nCoordinator: `{}` ({})\n{}\nTask cards: **{} complete**, **{} active**, **{} blocked**, **{} deferred** ({} total, {} validated)\nPost-wake soft grace until: `{}`\nLast meaningful activity: {}\nReview: `{}`\nLog: `{}`",
        manifest.run_id,
        manifest.status.label(),
        manifest.coordinator_session_id,
        manifest.coordinator_session_name,
        remaining_line,
        task_summary.counts.completed,
        task_summary.counts.active,
        task_summary.counts.blocked,
        task_summary.counts.deferred,
        task_summary.total,
        task_summary.validated,
        manifest.post_wake_grace_until.to_rfc3339(),
        manifest.last_activity_at.to_rfc3339(),
        manifest.review_path.display(),
        manifest.human_log_path.display()
    )
}

pub fn format_log_markdown_from_events(
    manifest: &OvernightManifest,
    events: &[OvernightEvent],
    max_lines: usize,
) -> String {
    let start = events.len().saturating_sub(max_lines);
    let mut out = format!("🌙 **Overnight log `{}`**\n\n", manifest.run_id);
    for event in &events[start..] {
        out.push_str(&format!(
            "- `{}` **{}**: {}\n",
            event.timestamp.format("%H:%M:%S"),
            event.kind,
            event.summary
        ));
    }
    if events.is_empty() {
        out.push_str("No events recorded yet.\n");
    }
    out.push_str(&format!(
        "\nFull log: `{}`",
        manifest.human_log_path.display()
    ));
    out
}

mod prompts;

pub use prompts::{
    build_continuation_prompt, build_coordinator_prompt, build_final_wrapup_prompt,
    build_handoff_ready_prompt, build_morning_report_prompt, build_post_wake_continuation_prompt,
    build_visible_current_session_prompt, prompt_event_summary,
};
use prompts::{next_prompt_label, overnight_phase, relative_time, time_relation_to_target};

pub fn preflight_summary(preflight: &OvernightPreflight) -> String {
    format!(
        "Usage risk: {} (confidence: {}). Projected end: {}. Resources: {}. Git: {}.",
        preflight.usage.risk,
        preflight.usage.confidence,
        match (
            preflight.usage.projected_end_min_percent,
            preflight.usage.projected_end_max_percent,
        ) {
            (Some(min), Some(max)) => format!("{:.0}% to {:.0}%", min, max),
            _ => "unknown".to_string(),
        },
        resource_summary(&preflight.resources),
        git_summary(&preflight.git),
    )
}

#[cfg(test)]
mod helper_tests;
