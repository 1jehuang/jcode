use jcode_plan::PlanItem;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const MAX_SWARM_COMPLETION_REPORT_CHARS: usize = 4000;
/// Approximate recent-output budget, using the shared four-bytes-per-token heuristic.
pub const SWARM_OUTPUT_TAIL_TOKENS: usize = 1000;

/// Message/report bodies longer than this get a derived preview when the sender
/// omits `tldr`, so receiving UIs can still collapse them with an expand control.
pub const SWARM_TLDR_REQUIRED_OVER_CHARS: usize = 240;

/// Recommended upper bound for a sender-provided `tldr`, not a hard limit.
/// Models should keep collapsed summaries short, but length must not block delivery.
pub const MAX_SWARM_TLDR_CHARS: usize = 200;

/// Validate a sender-provided `tldr` against the message body it summarizes.
///
/// Returns the normalized (trimmed, whitespace-collapsed) tldr when present,
/// `Ok(None)` when the body is short enough to not need one, and a derived
/// preview when a long body is missing a tldr. Missing or overlong summaries
/// must never block delivery. The result signature is retained for compatibility.
pub fn validate_swarm_tldr(
    tldr: Option<&str>,
    body: &str,
    _context: &str,
) -> Result<Option<String>, String> {
    let normalized = tldr
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty());

    if normalized.is_some() {
        return Ok(normalized);
    }

    let body_chars = body.chars().count();
    if body_chars > SWARM_TLDR_REQUIRED_OVER_CHARS {
        let preview = truncate_detail(body, MAX_SWARM_TLDR_CHARS);
        return Ok((!preview.is_empty()).then_some(preview));
    }

    Ok(None)
}

/// Absolute maximum number of live members in a single swarm. Servers also apply
/// the lower configurable live-worker RAM budget before reaching this hard stop.
pub const MAX_SWARM_MEMBERS: usize = 1000;

/// Upper bound for a member's derived task label, sized for one-line UI chips.
pub const MAX_SWARM_TASK_LABEL_CHARS: usize = 48;

/// Derive a short, stable task label from a spawn prompt or task assignment.
///
/// Takes the first non-empty line, strips common markdown/list prefixes,
/// collapses whitespace, and truncates on a char boundary with an ellipsis.
/// Returns `None` when the text has no usable content.
pub fn derive_swarm_task_label(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line
        .trim_start_matches(['#', '-', '*', '>', ' '])
        .trim_end_matches(':')
        .trim();
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() <= MAX_SWARM_TASK_LABEL_CHARS {
        return Some(collapsed);
    }
    let truncated: String = collapsed
        .chars()
        .take(MAX_SWARM_TASK_LABEL_CHARS.saturating_sub(1))
        .collect();
    Some(format!("{}…", truncated.trim_end()))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwarmRole {
    Agent,
    Coordinator,
    Other(String),
}

impl SwarmRole {
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Agent => Cow::Borrowed("agent"),
            Self::Coordinator => Cow::Borrowed("coordinator"),
            Self::Other(value) => Cow::Borrowed(value.as_str()),
        }
    }
}

impl From<String> for SwarmRole {
    fn from(value: String) -> Self {
        match value.as_str() {
            "agent" => Self::Agent,
            "coordinator" => Self::Coordinator,
            _ => Self::Other(value),
        }
    }
}

impl Serialize for SwarmRole {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str().as_ref())
    }
}

impl<'de> Deserialize<'de> for SwarmRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from(String::deserialize(deserializer)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SwarmLifecycleStatus {
    Spawned,
    Ready,
    Running,
    RunningStale,
    Completed,
    Done,
    Failed,
    Stopped,
    Crashed,
    Queued,
    Blocked,
    Pending,
    Todo,
    Other(String),
}

impl SwarmLifecycleStatus {
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Spawned => Cow::Borrowed("spawned"),
            Self::Ready => Cow::Borrowed("ready"),
            Self::Running => Cow::Borrowed("running"),
            Self::RunningStale => Cow::Borrowed("running_stale"),
            Self::Completed => Cow::Borrowed("completed"),
            Self::Done => Cow::Borrowed("done"),
            Self::Failed => Cow::Borrowed("failed"),
            Self::Stopped => Cow::Borrowed("stopped"),
            Self::Crashed => Cow::Borrowed("crashed"),
            Self::Queued => Cow::Borrowed("queued"),
            Self::Blocked => Cow::Borrowed("blocked"),
            Self::Pending => Cow::Borrowed("pending"),
            Self::Todo => Cow::Borrowed("todo"),
            Self::Other(value) => Cow::Borrowed(value.as_str()),
        }
    }
}

impl From<String> for SwarmLifecycleStatus {
    fn from(value: String) -> Self {
        match value.as_str() {
            "spawned" => Self::Spawned,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "running_stale" => Self::RunningStale,
            "completed" => Self::Completed,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "stopped" => Self::Stopped,
            "crashed" => Self::Crashed,
            "queued" => Self::Queued,
            "blocked" => Self::Blocked,
            "pending" => Self::Pending,
            "todo" => Self::Todo,
            _ => Self::Other(value),
        }
    }
}

impl Serialize for SwarmLifecycleStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str().as_ref())
    }
}

impl<'de> Deserialize<'de> for SwarmLifecycleStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from(String::deserialize(deserializer)?))
    }
}

/// Durable, persistable portion of a swarm member.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmMemberRecord {
    pub session_id: String,
    pub working_dir: Option<PathBuf>,
    pub swarm_id: Option<String>,
    pub swarm_enabled: bool,
    pub status: SwarmLifecycleStatus,
    pub detail: Option<String>,
    /// Stable label of the task/role this member was spawned or assigned for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_label: Option<String>,
    pub friendly_name: Option<String>,
    pub report_back_to_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_completion_report: Option<String>,
    pub role: SwarmRole,
    pub is_headless: bool,
}

/// Bidirectional index for swarm channel subscriptions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelIndex {
    pub by_swarm_channel: HashMap<String, HashMap<String, HashSet<String>>>,
    pub by_session: HashMap<String, HashMap<String, HashSet<String>>>,
}

impl ChannelIndex {
    pub fn subscribe(&mut self, session_id: &str, swarm_id: &str, channel: &str) {
        self.by_swarm_channel
            .entry(swarm_id.to_string())
            .or_default()
            .entry(channel.to_string())
            .or_default()
            .insert(session_id.to_string());
        self.by_session
            .entry(session_id.to_string())
            .or_default()
            .entry(swarm_id.to_string())
            .or_default()
            .insert(channel.to_string());
    }

    pub fn unsubscribe(&mut self, session_id: &str, swarm_id: &str, channel: &str) {
        let mut remove_swarm = false;
        if let Some(swarm_subs) = self.by_swarm_channel.get_mut(swarm_id) {
            if let Some(members) = swarm_subs.get_mut(channel) {
                members.remove(session_id);
                if members.is_empty() {
                    swarm_subs.remove(channel);
                }
            }
            remove_swarm = swarm_subs.is_empty();
        }
        if remove_swarm {
            self.by_swarm_channel.remove(swarm_id);
        }

        let mut remove_session_entry = false;
        if let Some(session_subs) = self.by_session.get_mut(session_id) {
            let mut remove_swarm_entry = false;
            if let Some(channels) = session_subs.get_mut(swarm_id) {
                channels.remove(channel);
                remove_swarm_entry = channels.is_empty();
            }
            if remove_swarm_entry {
                session_subs.remove(swarm_id);
            }
            remove_session_entry = session_subs.is_empty();
        }
        if remove_session_entry {
            self.by_session.remove(session_id);
        }
    }

    pub fn remove_session(&mut self, session_id: &str) {
        if let Some(session_subscriptions) = self.by_session.remove(session_id) {
            for (swarm_id, channels) in session_subscriptions {
                let mut remove_swarm = false;
                if let Some(swarm_subs) = self.by_swarm_channel.get_mut(&swarm_id) {
                    for channel_name in channels {
                        if let Some(members) = swarm_subs.get_mut(&channel_name) {
                            members.remove(session_id);
                            if members.is_empty() {
                                swarm_subs.remove(&channel_name);
                            }
                        }
                    }
                    remove_swarm = swarm_subs.is_empty();
                }
                if remove_swarm {
                    self.by_swarm_channel.remove(&swarm_id);
                }
            }
            return;
        }

        let swarm_ids: Vec<String> = self.by_swarm_channel.keys().cloned().collect();
        for swarm_id in swarm_ids {
            let mut remove_swarm = false;
            if let Some(swarm_subs) = self.by_swarm_channel.get_mut(&swarm_id) {
                let channel_names: Vec<String> = swarm_subs.keys().cloned().collect();
                for channel_name in channel_names {
                    if let Some(members) = swarm_subs.get_mut(&channel_name) {
                        members.remove(session_id);
                        if members.is_empty() {
                            swarm_subs.remove(&channel_name);
                        }
                    }
                }
                remove_swarm = swarm_subs.is_empty();
            }
            if remove_swarm {
                self.by_swarm_channel.remove(&swarm_id);
            }
        }
    }

    pub fn members(&self, swarm_id: &str, channel: &str) -> Vec<String> {
        let mut members = self
            .by_swarm_channel
            .get(swarm_id)
            .and_then(|swarm_subs| swarm_subs.get(channel))
            .map(|members| members.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        members.sort();
        members
    }

    #[cfg(test)]
    pub fn channels_for_session(&self, session_id: &str, swarm_id: &str) -> Vec<String> {
        let mut channels = self
            .by_session
            .get(session_id)
            .and_then(|session_subs| session_subs.get(swarm_id))
            .map(|channels| channels.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        channels.sort();
        channels
    }
}

/// Keep the newest output, not the beginning, within an approximate token budget.
/// Chunks are assistant-visible output in chronological order, excluding reasoning
/// and tool results. UTF-8 boundaries are preserved even with a one-token budget.
pub fn recent_output_tail<'a>(
    chunks: impl DoubleEndedIterator<Item = &'a str>,
    max_tokens: usize,
) -> Option<String> {
    let mut remaining = max_tokens.saturating_mul(4);
    let mut parts = Vec::new();
    for chunk in chunks
        .rev()
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
    {
        if remaining == 0 {
            break;
        }
        if !parts.is_empty() {
            if remaining <= 2 {
                break;
            }
            remaining -= 2;
        }
        let mut start = chunk.len().saturating_sub(remaining);
        while !chunk.is_char_boundary(start) {
            start += 1;
        }
        let tail = &chunk[start..];
        if tail.is_empty() {
            break;
        }
        remaining = remaining.saturating_sub(tail.len());
        parts.push(tail);
        if start > 0 {
            break;
        }
    }
    parts.reverse();
    let output = parts.join("\n\n");
    let output = output.trim();
    (!output.is_empty()).then(|| output.to_string())
}

pub fn format_structured_completion_report(
    message: &str,
    validation: Option<&str>,
    follow_up: Option<&str>,
) -> String {
    let mut report = message.trim().to_string();
    if let Some(validation) = validation.map(str::trim).filter(|value| !value.is_empty()) {
        if !report.is_empty() {
            report.push_str("\n\n");
        }
        report.push_str("Validation:\n");
        report.push_str(validation);
    }
    if let Some(follow_up) = follow_up.map(str::trim).filter(|value| !value.is_empty()) {
        if !report.is_empty() {
            report.push_str("\n\n");
        }
        report.push_str("Follow-ups/blockers:\n");
        report.push_str(follow_up);
    }
    report
}

pub fn normalize_completion_report(report: Option<String>) -> Option<String> {
    let report = report?.trim().to_string();
    if report.is_empty() {
        return None;
    }

    let char_count = report.chars().count();
    if char_count <= MAX_SWARM_COMPLETION_REPORT_CHARS {
        return Some(report);
    }

    let suffix = "\n\n[Report truncated by jcode before delivery.]";
    let keep_chars = MAX_SWARM_COMPLETION_REPORT_CHARS.saturating_sub(suffix.chars().count());
    let mut truncated: String = report.chars().take(keep_chars).collect();
    truncated.push_str(suffix);
    Some(truncated)
}

fn completion_status_intro(name: &str, status: &str) -> String {
    match status {
        "ready" => format!("Agent {} finished their work and is ready for more.", name),
        "failed" => format!("Agent {} finished with status failed.", name),
        "stopped" => format!("Agent {} stopped.", name),
        "crashed" => format!("Agent {} crashed while working.", name),
        _ => format!("Agent {} completed their work.", name),
    }
}

fn completion_followup(status: &str, has_report: bool) -> &'static str {
    match (status, has_report) {
        ("ready", true) => {
            "Use assign_task to give them more work, stop to remove them, or summary/read_context for full context."
        }
        ("ready", false) => {
            "Use summary/read_context to inspect results, assign_task for more work, or stop to remove them."
        }
        ("failed", true) => {
            "Use summary/read_context for full context, retry with guidance, or stop to remove them."
        }
        ("failed", false) => {
            "Use summary/read_context to inspect results, assign_task to retry with guidance, or stop to remove them."
        }
        ("stopped", _) => "Use summary/read_context to inspect results or stop to remove them.",
        ("crashed", _) => {
            "Any swarm task assignments they held are requeued automatically where possible. \
             Check plan_status, and spawn a replacement or use retry/assign_task if work remains."
        }
        (_, true) => {
            "Use assign_task to give them new work, stop to remove them, or summary/read_context for full context."
        }
        (_, false) => "Use assign_task to give them new work, or stop to remove them.",
    }
}

pub fn completion_notification_message(name: &str, status: &str, report: Option<&str>) -> String {
    let intro = completion_status_intro(name, status);
    let followup = completion_followup(status, report.is_some());
    match report {
        Some(report) => {
            format!("{intro}\n\nRecent output (automatically captured):\n{report}\n\n{followup}")
        }
        None => format!("{intro}\n\nNo recent assistant output was captured. {followup}"),
    }
}

pub fn truncate_detail(text: &str, max_len: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let max_len = max_len.max(1);
    if trimmed.chars().count() <= max_len {
        return trimmed.to_string();
    }
    if max_len <= 3 {
        return trimmed.chars().take(max_len).collect();
    }
    let mut out: String = trimmed.chars().take(max_len - 3).collect();
    out.push_str("...");
    out
}

pub fn summarize_plan_items(items: &[PlanItem], max_items: usize) -> String {
    if items.is_empty() {
        return "no items".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for item in items.iter().take(max_items.max(1)) {
        parts.push(item.content.clone());
    }
    let mut summary = parts.join("; ");
    if items.len() > max_items.max(1) {
        summary.push_str(&format!(" (+{} more)", items.len() - max_items.max(1)));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_item(id: &str, content: &str) -> PlanItem {
        PlanItem {
            id: id.to_string(),
            content: content.to_string(),
            status: "queued".to_string(),
            priority: "normal".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    #[test]
    fn truncate_detail_collapses_whitespace_and_ellipsizes() {
        assert_eq!(truncate_detail("hello   there\nworld", 11), "hello th...");
    }

    #[test]
    fn validate_swarm_tldr_allows_short_body_without_tldr() {
        assert_eq!(validate_swarm_tldr(None, "quick note", "this DM"), Ok(None));
    }

    #[test]
    fn validate_swarm_tldr_derives_preview_for_long_body() {
        for length in [241, 244, 4000] {
            let body = "界".repeat(length);
            for context in [
                "this DM",
                "this message",
                "this broadcast",
                "this channel message",
                "this report",
            ] {
                let preview = validate_swarm_tldr(None, &body, context).unwrap().unwrap();
                assert_eq!(
                    preview,
                    format!("{}...", "界".repeat(MAX_SWARM_TLDR_CHARS - 3))
                );
                assert_eq!(body.chars().count(), length);
            }
        }
    }

    #[test]
    fn validate_swarm_tldr_fallback_collapses_whitespace() {
        let body = format!("  hello\n\tworld{}", " ".repeat(240));
        assert_eq!(
            validate_swarm_tldr(None, &body, "this DM"),
            Ok(Some("hello world".into()))
        );
        assert_eq!(
            validate_swarm_tldr(None, &" ".repeat(241), "this DM"),
            Ok(None)
        );
        assert_eq!(
            validate_swarm_tldr(None, &"界".repeat(240), "this DM"),
            Ok(None)
        );
    }

    #[test]
    fn validate_swarm_tldr_normalizes_whitespace() {
        let body = "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 1);
        assert_eq!(
            validate_swarm_tldr(Some("  did\nthe   thing  "), &body, "this report"),
            Ok(Some("did the thing".to_string()))
        );
    }

    #[test]
    fn validate_swarm_tldr_preserves_overlong_tldr() {
        for summary_chars in [MAX_SWARM_TLDR_CHARS, MAX_SWARM_TLDR_CHARS + 1, 1000] {
            let tldr = "界".repeat(summary_chars);
            for body_chars in [4, SWARM_TLDR_REQUIRED_OVER_CHARS + 1] {
                let body = "x".repeat(body_chars);
                assert_eq!(
                    validate_swarm_tldr(Some(&tldr), &body, "this message"),
                    Ok(Some(tldr.clone()))
                );
            }
        }
    }

    #[test]
    fn validate_swarm_tldr_normalizes_overlong_tldr() {
        let tldr = "summary".repeat(MAX_SWARM_TLDR_CHARS);
        assert_eq!(
            validate_swarm_tldr(Some(&format!("  {tldr}\n  done  ")), "body", "this report"),
            Ok(Some(format!("{tldr} done")))
        );
    }

    #[test]
    fn validate_swarm_tldr_blank_tldr_counts_as_missing() {
        let body = "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 1);
        assert_eq!(
            validate_swarm_tldr(Some(" \n\t "), &body, "this DM"),
            validate_swarm_tldr(None, &body, "this DM")
        );
        assert_eq!(
            validate_swarm_tldr(Some("   "), "short", "this DM"),
            Ok(None)
        );
    }

    #[test]
    fn summarize_plan_items_limits_output() {
        let items = vec![
            plan_item("a", "first"),
            plan_item("b", "second"),
            plan_item("c", "third"),
        ];
        assert_eq!(summarize_plan_items(&items, 2), "first; second (+1 more)");
    }

    #[test]
    fn output_tail_keeps_newest_chunks_within_token_budget() {
        assert_eq!(
            recent_output_tail(["old", "new", "last"].into_iter(), 3),
            Some("d\n\nnew\n\nlast".into())
        );
        assert_eq!(
            recent_output_tail(["prefix12345678"].into_iter(), 2),
            Some("12345678".into())
        );
        assert_eq!(recent_output_tail(["  ", ""].into_iter(), 100), None);
        assert_eq!(recent_output_tail(["text"].into_iter(), 0), None);
        for budget in 0..20 {
            let tail = recent_output_tail(["世界🦊éfin"].into_iter(), budget);
            if let Some(tail) = tail {
                assert!(tail.len() <= budget * 4);
                assert!("世界🦊éfin".ends_with(&tail));
            }
        }
    }

    #[test]
    fn completion_report_normalization_trims_and_truncates() {
        assert_eq!(
            normalize_completion_report(Some("  done  ".to_string())),
            Some("done".to_string())
        );
        assert_eq!(normalize_completion_report(Some("   ".to_string())), None);
        let long = "x".repeat(MAX_SWARM_COMPLETION_REPORT_CHARS + 100);
        let normalized = normalize_completion_report(Some(long)).unwrap();
        assert_eq!(
            normalized.chars().count(),
            MAX_SWARM_COMPLETION_REPORT_CHARS
        );
        assert!(normalized.ends_with("[Report truncated by jcode before delivery.]"));
    }

    #[test]
    fn channel_index_keeps_bidirectional_maps_in_sync() {
        let mut index = ChannelIndex::default();
        index.subscribe("worker-1", "swarm-a", "build");
        index.subscribe("worker-1", "swarm-a", "tests");
        index.subscribe("worker-2", "swarm-a", "build");

        assert_eq!(
            index.members("swarm-a", "build"),
            vec!["worker-1", "worker-2"]
        );
        assert_eq!(
            index.channels_for_session("worker-1", "swarm-a"),
            vec!["build", "tests"]
        );

        index.unsubscribe("worker-1", "swarm-a", "build");
        assert_eq!(index.members("swarm-a", "build"), vec!["worker-2"]);

        index.remove_session("worker-1");
        assert!(index.channels_for_session("worker-1", "swarm-a").is_empty());
        assert_eq!(index.members("swarm-a", "tests"), Vec::<String>::new());
    }

    #[test]
    fn task_label_takes_first_line_strips_prefixes_and_collapses_whitespace() {
        assert_eq!(
            derive_swarm_task_label("Fix the   parser\n\nMore detail here"),
            Some("Fix the parser".to_string())
        );
        assert_eq!(
            derive_swarm_task_label("\n\n  ## Investigate flaky test:  \nbody"),
            Some("Investigate flaky test".to_string())
        );
        assert_eq!(
            derive_swarm_task_label("- review PR #42"),
            Some("review PR #42".to_string())
        );
    }

    #[test]
    fn task_label_truncates_long_prompts_with_ellipsis() {
        let long = "implement the entire authentication subsystem including oauth flows";
        let label = derive_swarm_task_label(long).unwrap();
        assert!(label.chars().count() <= MAX_SWARM_TASK_LABEL_CHARS);
        assert!(label.ends_with('…'), "got: {label}");
    }

    #[test]
    fn task_label_rejects_empty_or_marker_only_text() {
        assert_eq!(derive_swarm_task_label(""), None);
        assert_eq!(derive_swarm_task_label("   \n\t\n"), None);
        assert_eq!(derive_swarm_task_label("###"), None);
    }
}
