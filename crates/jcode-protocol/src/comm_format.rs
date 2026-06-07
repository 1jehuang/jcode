use std::collections::{HashMap, HashSet};

use super::{
    AgentInfo, AgentStatusSnapshot, AwaitedMemberStatus, ContextEntry, HistoryMessage,
    PlanGraphStatus, SwarmChannelInfo, ToolCallSummary,
};

pub fn format_comm_plan_followup(summary: &PlanGraphStatus) -> String {
    let mut parts = Vec::new();
    parts.push(format!("active={}", summary.active_ids.len()));
    if !summary.next_ready_ids.is_empty() {
        parts.push(format!("next={}", summary.next_ready_ids.join(", ")));
    }
    if !summary.newly_ready_ids.is_empty() {
        parts.push(format!(
            "newly_ready={}",
            summary.newly_ready_ids.join(", ")
        ));
    }
    parts.join(" · ")
}

pub fn default_comm_cleanup_target_statuses() -> Vec<String> {
    vec![
        "ready".to_string(),
        "completed".to_string(),
        "failed".to_string(),
        "stopped".to_string(),
    ]
}

pub fn default_comm_run_await_statuses() -> Vec<String> {
    vec![
        "ready".to_string(),
        "completed".to_string(),
        "failed".to_string(),
        "stopped".to_string(),
    ]
}

pub fn default_comm_await_target_statuses() -> Vec<String> {
    vec![
        "ready".to_string(),
        "completed".to_string(),
        "stopped".to_string(),
        "failed".to_string(),
    ]
}

pub fn comm_cleanup_candidate_session_ids(
    owner_session_id: &str,
    members: &[AgentInfo],
    target_status: &[String],
    requested_session_ids: &[String],
    force: bool,
) -> Vec<String> {
    let status_filter: HashSet<&str> = target_status.iter().map(String::as_str).collect();
    let requested: HashSet<&str> = requested_session_ids.iter().map(String::as_str).collect();
    let restrict_to_requested = !requested.is_empty();
    let mut ids = members
        .iter()
        .filter(|member| member.session_id != owner_session_id)
        .filter(|member| !restrict_to_requested || requested.contains(member.session_id.as_str()))
        .filter(|member| {
            member
                .status
                .as_deref()
                .is_some_and(|status| status_filter.contains(status))
        })
        .filter(|member| {
            force || member.report_back_to_session_id.as_deref() == Some(owner_session_id)
        })
        .map(|member| member.session_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

pub fn format_comm_context_entries(entries: &[ContextEntry]) -> String {
    if entries.is_empty() {
        "No shared context found.".to_string()
    } else {
        let mut output = String::from("Shared context from other agents:\n\n");
        for entry in entries {
            let from = entry.from_name.as_deref().unwrap_or(&entry.from_session);
            output.push_str(&format!(
                "  {} (from {}): {}\n",
                entry.key, from, entry.value
            ));
        }
        output
    }
}

pub fn duplicate_comm_friendly_names<'a>(
    names: impl IntoIterator<Item = Option<&'a str>>,
) -> HashSet<&'a str> {
    let mut counts = HashMap::<&'a str, usize>::new();
    for name in names.into_iter().flatten() {
        *counts.entry(name).or_default() += 1;
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

pub fn comm_session_display_suffix(session_id: &str) -> &str {
    let suffix = session_id.rsplit('_').next().unwrap_or(session_id);
    if suffix.len() > 6 {
        &suffix[suffix.len() - 6..]
    } else {
        suffix
    }
}

pub fn comm_display_friendly_name(
    friendly_name: Option<&str>,
    session_id: &str,
    duplicate_names: &HashSet<&str>,
) -> String {
    match friendly_name {
        Some(name) if duplicate_names.contains(name) => {
            format!("{} [{}]", name, comm_session_display_suffix(session_id))
        }
        Some(name) => name.to_string(),
        None => session_id.to_string(),
    }
}

pub fn format_comm_members(current_session_id: &str, members: &[AgentInfo]) -> String {
    if members.is_empty() {
        "No other agents in this codebase.".to_string()
    } else {
        let duplicate_names = duplicate_comm_friendly_names(
            members.iter().map(|member| member.friendly_name.as_deref()),
        );
        let mut output = String::from("Agents in this codebase:\n\n");
        for member in members {
            let name = comm_display_friendly_name(
                member.friendly_name.as_deref(),
                &member.session_id,
                &duplicate_names,
            );
            let session = &member.session_id;
            let role = member.role.as_deref().unwrap_or("agent");
            let files = member.files_touched.join(", ");
            let status = member.status.as_deref().unwrap_or("unknown");
            let is_me = session == current_session_id;
            let role_label = if role != "agent" {
                format!(" [{}]", role)
            } else {
                String::new()
            };

            // Status line: lifecycle + detail, then a contextual age label.
            // For an idle/ready agent the "age" is how long it has been idle;
            // for a running agent it is how long the current turn has run.
            let detail_suffix = member
                .detail
                .as_deref()
                .map(|detail| format!(" — {}", detail))
                .unwrap_or_default();
            let age_suffix = match member.status_age_secs {
                Some(age) if status == "ready" || status == "idle" => {
                    format!(" · idle {}", format_secs(age))
                }
                Some(age) if status == "running" => format!(" · {}", format_secs(age)),
                Some(age) => format!(" · {} ago", format_secs(age)),
                None => String::new(),
            };

            // Live activity: what the agent is doing right now.
            let activity_suffix = match member.activity.as_ref() {
                Some(activity) if activity.is_processing => {
                    match activity.current_tool_name.as_deref() {
                        Some(tool) => format!("\n    Activity: working ({})", tool),
                        None => "\n    Activity: thinking".to_string(),
                    }
                }
                _ => String::new(),
            };

            // Progress: todos completed / total.
            let progress_suffix = match (member.todos_completed, member.todos_total) {
                (Some(done), Some(total)) if total > 0 => {
                    format!("\n    Progress: {}/{} todos", done, total)
                }
                _ => String::new(),
            };

            // Live work signal: recent token churn + cumulative + turns.
            let mut work_meta = Vec::new();
            if let (Some(recent), Some(window)) =
                (member.recent_total_tokens, member.recent_window_secs)
                && recent > 0
            {
                work_meta.push(format!("{} tok/{}s", format_count(recent), window));
            }
            if let Some(turns) = member.turn_count.filter(|turns| *turns > 0) {
                work_meta.push(format!("{} turns", turns));
            }
            if let Some(total) = member.cumulative_total_tokens.filter(|total| *total > 0) {
                work_meta.push(format!("{} tok total", format_count(total)));
            }
            let work_suffix = if work_meta.is_empty() {
                String::new()
            } else {
                format!("\n    Work: {}", work_meta.join(" · "))
            };

            // Model line.
            let model_suffix = match (
                member.provider_name.as_deref(),
                member.provider_model.as_deref(),
            ) {
                (Some(provider), Some(model)) => format!("\n    Model: {}/{}", provider, model),
                (None, Some(model)) => format!("\n    Model: {}", model),
                _ => String::new(),
            };

            let mut extra_meta = Vec::new();
            if member.is_headless == Some(true) {
                extra_meta.push("headless".to_string());
            }
            if let Some(owner) = member.report_back_to_session_id.as_deref() {
                if owner == current_session_id {
                    extra_meta.push("owned_by_you".to_string());
                } else {
                    extra_meta.push(format!("owned_by={owner}"));
                }
            }
            if let Some(attachments) = member.live_attachments {
                extra_meta.push(format!("attachments={attachments}"));
            }
            let meta_suffix = if extra_meta.is_empty() {
                String::new()
            } else {
                format!("\n    Meta: {}", extra_meta.join(" · "))
            };

            // Completion report when the agent has finished.
            let report_suffix = match member.latest_completion_report.as_deref() {
                Some(report) if !report.trim().is_empty() => {
                    format!("\n    Report: {}", truncate_report(report))
                }
                _ => String::new(),
            };

            output.push_str(&format!(
                "  {}{} ({})\n    Status: {}{}{}{}{}{}{}{}{}{}\n",
                name,
                role_label,
                if is_me { "you" } else { session },
                status,
                detail_suffix,
                age_suffix,
                activity_suffix,
                progress_suffix,
                work_suffix,
                model_suffix,
                if files.is_empty() {
                    String::new()
                } else {
                    format!("\n    Files: {}", files)
                },
                meta_suffix,
                report_suffix,
            ));
        }
        output
    }
}

/// Format a duration in seconds into a compact human label (e.g. `45s`, `3m`, `2h`).
fn format_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// Format a token count compactly (e.g. `850`, `12.3k`, `1.2M`).
fn format_count(count: u64) -> String {
    if count < 1_000 {
        count.to_string()
    } else if count < 1_000_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    }
}

/// Truncate a completion report to a single compact line for the roster view.
fn truncate_report(report: &str) -> String {
    const MAX: usize = 120;
    let one_line: String = report.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > MAX {
        let truncated: String = one_line.chars().take(MAX).collect();
        format!("{}…", truncated)
    } else {
        one_line
    }
}

pub fn format_comm_tool_summary(target: &str, calls: &[ToolCallSummary]) -> String {
    if calls.is_empty() {
        format!("No tool calls found for {}", target)
    } else {
        let call_count = calls.len();
        let mut output = format!(
            "Tool call summary for {} ({} call{}):\n\n",
            target,
            call_count,
            if call_count == 1 { "" } else { "s" }
        );
        for call in calls {
            output.push_str(&format!("  {} — {}\n", call.tool_name, call.brief_output));
        }
        output
    }
}

pub fn format_comm_status_snapshot(snapshot: &AgentStatusSnapshot) -> String {
    let target = snapshot
        .friendly_name
        .as_deref()
        .unwrap_or(&snapshot.session_id);
    let status = snapshot.status.as_deref().unwrap_or("unknown");
    let mut output = format!(
        "Status snapshot for {} ({})\n\n",
        target, snapshot.session_id
    );
    output.push_str(&format!("  Lifecycle: {}", status));
    if let Some(detail) = snapshot.detail.as_deref() {
        output.push_str(&format!(" — {}", detail));
    }
    output.push('\n');

    let activity = snapshot
        .activity
        .as_ref()
        .map(|activity| match activity.current_tool_name.as_deref() {
            Some(tool_name) => format!("busy ({tool_name})"),
            None if activity.is_processing => "busy".to_string(),
            _ => "idle".to_string(),
        })
        .unwrap_or_else(|| "idle".to_string());
    output.push_str(&format!("  Activity: {}\n", activity));

    if let Some(role) = snapshot.role.as_deref() {
        output.push_str(&format!("  Role: {}\n", role));
    }
    if let Some(swarm_id) = snapshot.swarm_id.as_deref() {
        output.push_str(&format!("  Swarm: {}\n", swarm_id));
    }

    let mut meta = Vec::new();
    if snapshot.is_headless == Some(true) {
        meta.push("headless".to_string());
    }
    if let Some(attachments) = snapshot.live_attachments {
        meta.push(format!("attachments={attachments}"));
    }
    if let Some(age_secs) = snapshot.status_age_secs {
        meta.push(format!("status_age={}s", age_secs));
    }
    if let Some(age_secs) = snapshot.joined_age_secs {
        meta.push(format!("joined={}s", age_secs));
    }
    if !meta.is_empty() {
        output.push_str(&format!("  Meta: {}\n", meta.join(" · ")));
    }

    if snapshot.provider_name.is_some() || snapshot.provider_model.is_some() {
        let provider = snapshot.provider_name.as_deref().unwrap_or("unknown");
        let model = snapshot.provider_model.as_deref().unwrap_or("unknown");
        output.push_str(&format!("  Provider: {} / {}\n", provider, model));
    }

    if snapshot.files_touched.is_empty() {
        output.push_str("  Files: (none)\n");
    } else {
        output.push_str(&format!("  Files: {}\n", snapshot.files_touched.join(", ")));
    }

    output
}

pub fn format_comm_plan_status(summary: &PlanGraphStatus) -> String {
    let swarm_id = summary.swarm_id.as_deref().unwrap_or("unknown");
    let mut output = format!(
        "Plan status for swarm {}\n\n  Version: {}\n  Items: {}\n",
        swarm_id, summary.version, summary.item_count
    );

    output.push_str(&format!(
        "  Ready: {}\n",
        if summary.ready_ids.is_empty() {
            "(none)".to_string()
        } else {
            summary.ready_ids.join(", ")
        }
    ));
    output.push_str(&format!(
        "  Next up: {}\n",
        if summary.next_ready_ids.is_empty() {
            "(none)".to_string()
        } else {
            summary.next_ready_ids.join(", ")
        }
    ));
    if !summary.newly_ready_ids.is_empty() {
        output.push_str(&format!(
            "  Newly ready: {}\n",
            summary.newly_ready_ids.join(", ")
        ));
    }
    if !summary.blocked_ids.is_empty() {
        output.push_str(&format!("  Blocked: {}\n", summary.blocked_ids.join(", ")));
    }
    if !summary.active_ids.is_empty() {
        output.push_str(&format!("  Active: {}\n", summary.active_ids.join(", ")));
    }
    if !summary.completed_ids.is_empty() {
        output.push_str(&format!(
            "  Completed: {}\n",
            summary.completed_ids.join(", ")
        ));
    }
    if !summary.cycle_ids.is_empty() {
        output.push_str(&format!("  Cycles: {}\n", summary.cycle_ids.join(", ")));
    }
    if !summary.unresolved_dependency_ids.is_empty() {
        output.push_str(&format!(
            "  Missing deps: {}\n",
            summary.unresolved_dependency_ids.join(", ")
        ));
    }

    output
}

pub fn format_comm_context_history(target: &str, messages: &[HistoryMessage]) -> String {
    if messages.is_empty() {
        format!("No conversation history for {}", target)
    } else {
        let mut output = format!(
            "Conversation context for {} ({} messages):\n\n",
            target,
            messages.len()
        );
        for msg in messages {
            let truncated = if msg.content.len() > 500 {
                format!("{}...", &msg.content[..500])
            } else {
                msg.content.clone()
            };
            output.push_str(&format!("[{}] {}\n\n", msg.role, truncated));
        }
        output
    }
}

pub fn truncate_comm_completion_report(report: &str) -> String {
    const MAX_REPORT_CHARS: usize = 4000;
    let report = report.trim();
    if report.chars().count() <= MAX_REPORT_CHARS {
        return report.to_string();
    }
    let suffix = "\n\n[Report truncated by jcode.]";
    let keep = MAX_REPORT_CHARS.saturating_sub(suffix.chars().count());
    let mut out: String = report.chars().take(keep).collect();
    out.push_str(suffix);
    out
}

pub fn latest_assistant_comm_report(messages: &[HistoryMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        let report = message.content.trim();
        (!report.is_empty()).then(|| truncate_comm_completion_report(report))
    })
}

pub fn resolve_optional_comm_target_session(
    target: Option<String>,
    current_session: &str,
) -> String {
    match target {
        Some(target) if target == "current" => current_session.to_string(),
        Some(target) => target,
        None => current_session.to_string(),
    }
}

pub fn format_comm_awaited_members_with_reports(
    completed: bool,
    summary: &str,
    members: &[AwaitedMemberStatus],
    reports: &HashMap<String, String>,
) -> String {
    let mut output = if completed {
        format!("All members done. {}\n", summary)
    } else {
        format!("Await incomplete. {}\n", summary)
    };

    if !members.is_empty() {
        let duplicate_names = duplicate_comm_friendly_names(
            members.iter().map(|member| member.friendly_name.as_deref()),
        );
        output.push_str("\nMember statuses:\n");
        for member in members {
            let name = comm_display_friendly_name(
                member.friendly_name.as_deref(),
                &member.session_id,
                &duplicate_names,
            );
            let icon = if member.done { "✓" } else { "✗" };
            output.push_str(&format!("  {} {} ({})\n", icon, name, member.status));
        }
    }

    let mut report_members: Vec<_> = members
        .iter()
        .filter_map(|member| {
            member
                .completion_report
                .as_ref()
                .or_else(|| reports.get(&member.session_id))
                .map(|report| (member, report))
        })
        .collect();
    report_members.sort_by(|(left, _), (right, _)| left.session_id.cmp(&right.session_id));
    if !report_members.is_empty() {
        let duplicate_names = duplicate_comm_friendly_names(
            members.iter().map(|member| member.friendly_name.as_deref()),
        );
        output.push_str("\nCompletion reports:\n");
        for (member, report) in report_members {
            let name = comm_display_friendly_name(
                member.friendly_name.as_deref(),
                &member.session_id,
                &duplicate_names,
            );
            output.push_str(&format!(
                "\n--- {} ({}) ---\n{}\n",
                name, member.status, report
            ));
        }
    }

    output
}

pub fn format_comm_channels(channels: &[SwarmChannelInfo]) -> String {
    if channels.is_empty() {
        "No swarm channels found.".to_string()
    } else {
        let mut output = String::from("Swarm channels:\n\n");
        for channel in channels {
            output.push_str(&format!(
                "  #{} — {} subscriber{}\n",
                channel.channel,
                channel.member_count,
                if channel.member_count == 1 { "" } else { "s" }
            ));
        }
        output
    }
}
