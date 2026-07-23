use jcode_plan::PlanItem;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const MAX_SWARM_COMPLETION_REPORT_CHARS: usize = 4000;
pub const SWARM_COMPLETION_REPORT_MARKER: &str = "SWARM COMPLETION REPORT REQUIRED";

/// Message/report bodies longer than this require a sender-provided `tldr`
/// so receiving UIs can render them collapsed to one line with an expand
/// control instead of dumping the full body into the transcript.
pub const SWARM_TLDR_REQUIRED_OVER_CHARS: usize = 240;

/// Upper bound for a sender-provided `tldr`. Anything longer defeats the
/// purpose of a one-line collapsed summary.
pub const MAX_SWARM_TLDR_CHARS: usize = 200;

/// Validate a sender-provided `tldr` against the message body it summarizes.
///
/// Returns the normalized (trimmed, whitespace-collapsed) tldr when present,
/// `Ok(None)` when the body is short enough to not need one, and a
/// human/model-actionable error when a long body is missing a tldr or the
/// tldr itself is malformed (too long or multi-line).
pub fn validate_swarm_tldr(
    tldr: Option<&str>,
    body: &str,
    context: &str,
) -> Result<Option<String>, String> {
    let normalized = tldr
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty());

    if let Some(ref tldr) = normalized {
        let chars = tldr.chars().count();
        if chars > MAX_SWARM_TLDR_CHARS {
            return Err(format!(
                "'tldr' for {context} is too long ({chars} chars, max {MAX_SWARM_TLDR_CHARS}). \
                 Provide a single short line summarizing the message."
            ));
        }
        return Ok(normalized);
    }

    let body_chars = body.chars().count();
    if body_chars > SWARM_TLDR_REQUIRED_OVER_CHARS {
        return Err(format!(
            "'tldr' is required for {context} because the body is {body_chars} chars \
             (over {SWARM_TLDR_REQUIRED_OVER_CHARS}). Add a one-line 'tldr' (under \
             {MAX_SWARM_TLDR_CHARS} chars) summarizing it; recipients see the tldr \
             collapsed with an expand control."
        ));
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

pub fn append_swarm_completion_report_instructions(message: &str) -> String {
    if message.contains(SWARM_COMPLETION_REPORT_MARKER) {
        return message.to_string();
    }

    let mut out = message.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("<system-reminder>\n");
    out.push_str(SWARM_COMPLETION_REPORT_MARKER);
    out.push_str(
        "\nBefore finishing, call the swarm tool with action=\"report\" to submit your completion report. \
Include a concise message, validation/tests performed, and blockers or follow-ups. \
After the report tool succeeds, also write a brief final assistant response. \
Do not finish with only tool output, a lifecycle status change, or no final response. \
Do not send a separate DM for the final report unless you need interactive coordination before finishing.\n",
    );
    out.push_str("</system-reminder>");
    out
}

/// Idempotency marker for [`append_deep_node_instructions`].
pub const SWARM_DEEP_NODE_MARKER: &str = "DEEP TASK GRAPH NODE";

/// Append the deep-mode execution contract to a task-graph node assignment.
///
/// Deep mode's comprehensiveness is structural: it only materializes when every
/// worker knows it can decompose its node into parallel children and must close
/// its node with a typed artifact. A freshly spawned worker has none of that
/// context (the seeding session's `swarm-deep` directive is not inherited), so
/// without this the budget goes unused: workers grind through nodes serially
/// and auto-complete without artifacts, silently downgrading deep mode to
/// light. This directive travels with the assignment itself, so it reaches
/// every worker at any spawn depth. Idempotent via [`SWARM_DEEP_NODE_MARKER`].
pub fn append_deep_node_instructions(message: &str, node_id: &str) -> String {
    if message.contains(SWARM_DEEP_NODE_MARKER) {
        return message.to_string();
    }

    let explicitly_non_expandable = message
        .to_ascii_lowercase()
        .contains("do not expand this node");

    let mut out = message.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("<system-reminder>\n");
    out.push_str(SWARM_DEEP_NODE_MARKER);
    if explicitly_non_expandable {
        out.push_str(&format!(
            "\nYou are executing node '{node_id}' of a deep task graph. The planner explicitly marked this node as bounded and non-expandable. Do NOT call expand_node, even if the node contains multiple concerns. Execute the assigned scope atomically, then call the swarm tool with action=\"complete_node\", node_id=\"{node_id}\", and a typed artifact containing findings, evidence (file:line refs), validation, open_questions, a REQUIRED confidence (low, medium, or high), and an honest what_i_did_not_check list. A turn that ends without complete_node gets re-queued and may fail.\n"
        ));
        out.push_str("</system-reminder>");
        return out;
    }
    out.push_str(&format!(
        "\nYou are executing node '{node_id}' of a deep task graph with a configurable parallel agent \
budget (32 live workers by default, with a hard maximum of {MAX_SWARM_MEMBERS}; use available \
parallelism deliberately, but do not spawn redundant agents). \
Choose one of exactly two finishes for this node:\n\
1. Decompose for parallelism: if this node contains more than one independently checkable \
concern, do NOT work through it serially. Call the swarm tool with action=\"expand_node\", \
node_id=\"{node_id}\", and MANY independent children (add depends_on edges only for real data \
dependencies, so the ready set stays wide). Then finish your turn; the children fan out to \
parallel agents and you will be re-woken to synthesize their results.\n\
2. Execute atomically: do the work, then call the swarm tool with action=\"complete_node\", \
node_id=\"{node_id}\", and a typed artifact: findings, evidence (file:line refs), validation, \
open_questions, a REQUIRED confidence (low, medium, or high; report low honestly, it routes \
follow-up work to shore up your scope instead of counting against you), and an honest \
what_i_did_not_check (the critique gate turns those into new nodes, so listing them is how \
coverage grows).\n\
These are the ONLY two ways this node can close: a turn that ends without expand_node or \
complete_node gets the node re-queued to a fresh agent, and a repeat fails it.\n"
    ));
    out.push_str("</system-reminder>");
    out
}

/// Append the deep-mode gate contract to a critique/verify gate assignment.
///
/// Gates are the adversarial half of deep mode: they exist to spend budget on
/// gaps. A gate that just rubber-stamps its parent wastes the swarm's capacity,
/// so the directive names the two legal finishes (`inject_gap` with new nodes,
/// or `complete_node` when genuinely clean) and reminds the gate to mine the
/// children's `what_i_did_not_check` lists. `audited_ids` is the gate's audit
/// scope: the server rejects a pass whose artifact does not account for each of
/// these ids by name (enumerated accounting is what separates an audit from a
/// rubber stamp), so the directive lists them up front. `low_confidence_siblings`
/// are completed scope nodes whose artifacts self-reported low confidence: the
/// strictest debts, named as priority probe targets. Shares the idempotency
/// marker with [`append_deep_node_instructions`] since a single assignment gets
/// exactly one deep directive.
pub fn append_deep_gate_instructions(
    message: &str,
    gate_id: &str,
    audited_ids: &[String],
    low_confidence_siblings: &[String],
) -> String {
    if message.contains(SWARM_DEEP_NODE_MARKER) {
        return message.to_string();
    }

    let mut out = message.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("<system-reminder>\n");
    out.push_str(SWARM_DEEP_NODE_MARKER);
    out.push_str(&format!(
        "\nYou are executing critique/verify gate '{gate_id}' of a deep task graph. Your job is \
to find gaps, not to pass work through. Read every audited artifact, especially each \
what_i_did_not_check list, and probe them. Finish in one of exactly two ways:\n\
1. Gaps or failures found: call the swarm tool with action=\"inject_gap\", \
gate_id=\"{gate_id}\", and one new node per gap (they run in parallel and you re-run \
afterwards). The parent cannot close until they drain, so be thorough now. Injecting nodes \
is SUCCESS for a gate, not failure: a growing graph is the system working.\n\
2. Genuinely clean: call the swarm tool with action=\"complete_node\", node_id=\"{gate_id}\", \
and an artifact whose findings account for EVERY node you audited BY ID with what you \
checked and why no gaps remain. The server rejects a pass whose findings/open_questions \
do not name each audited node id.\n"
    ));
    if !audited_ids.is_empty() {
        out.push_str(&format!(
            "AUDIT SCOPE: you are auditing node(s) [{}]. A passing artifact must address each \
of these ids explicitly.\n",
            audited_ids.join(", ")
        ));
    }
    if !low_confidence_siblings.is_empty() {
        out.push_str(&format!(
            "PRIORITY: sibling node(s) [{}] completed with LOW confidence. The server will \
REJECT your pass unless you either inject follow-up nodes that shore up that work, or name \
each of those ids in your artifact findings with why the low confidence is acceptable. \
Injecting follow-ups adds breadth but does not erase the record: when you re-run after they \
drain, your passing artifact must STILL name each low-confidence id (e.g. 'X was shored up \
by Y').\n",
            low_confidence_siblings.join(", ")
        ));
    }
    out.push_str("Do not pass the gate without doing one of these.\n");
    out.push_str("</system-reminder>");
    out
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
        Some(report) => format!("{intro}\n\nReport:\n{report}\n\n{followup}"),
        None => format!("{intro}\n\nNo final textual report was produced. {followup}"),
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
    fn validate_swarm_tldr_requires_tldr_for_long_body() {
        let body = "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 1);
        let err = validate_swarm_tldr(None, &body, "this DM").unwrap_err();
        assert!(err.contains("'tldr' is required"), "{err}");
        assert!(err.contains("this DM"), "{err}");
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
    fn validate_swarm_tldr_rejects_overlong_tldr() {
        let tldr = "y".repeat(MAX_SWARM_TLDR_CHARS + 1);
        let err = validate_swarm_tldr(Some(&tldr), "body", "this message").unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn validate_swarm_tldr_blank_tldr_counts_as_missing() {
        let body = "x".repeat(SWARM_TLDR_REQUIRED_OVER_CHARS + 1);
        assert!(validate_swarm_tldr(Some("   "), &body, "this DM").is_err());
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
    fn append_swarm_completion_report_instructions_is_idempotent() {
        let prompt = "Do work";
        let with_instructions = append_swarm_completion_report_instructions(prompt);
        assert!(with_instructions.contains(SWARM_COMPLETION_REPORT_MARKER));
        assert_eq!(
            append_swarm_completion_report_instructions(&with_instructions),
            with_instructions
        );
    }

    #[test]
    fn deep_node_instructions_carry_expand_and_artifact_contract() {
        let out = append_deep_node_instructions("Investigate the parser", "explore.parser");
        assert!(out.starts_with("Investigate the parser"));
        assert!(out.contains(SWARM_DEEP_NODE_MARKER));
        // The two legal finishes must both name the node id explicitly.
        assert!(out.contains("action=\"expand_node\", node_id=\"explore.parser\""));
        assert!(out.contains("action=\"complete_node\", node_id=\"explore.parser\""));
        // The budget is advertised so workers know fan-out is expected.
        assert!(out.contains(&MAX_SWARM_MEMBERS.to_string()));
        assert!(out.contains("what_i_did_not_check"));
        // Idempotent: re-appending (even with a different id) is a no-op.
        assert_eq!(append_deep_node_instructions(&out, "other"), out);
    }

    #[test]
    fn deep_node_instructions_honor_explicit_non_expansion_marker() {
        let out = append_deep_node_instructions(
            "Audit the bounded surface. Do not expand this node.",
            "audit.bounded",
        );

        assert!(out.contains("bounded and non-expandable"));
        assert!(out.contains("Do NOT call expand_node"));
        assert!(!out.contains("action=\"expand_node\""));
        assert!(out.contains("action=\"complete_node\", node_id=\"audit.bounded\""));
        assert!(out.contains("what_i_did_not_check"));
    }

    #[test]
    fn deep_gate_instructions_carry_inject_gap_contract() {
        let out = append_deep_gate_instructions("Critique the work", "root::gate", &[], &[]);
        assert!(out.contains(SWARM_DEEP_NODE_MARKER));
        assert!(out.contains("action=\"inject_gap\", gate_id=\"root::gate\""));
        assert!(out.contains("action=\"complete_node\", node_id=\"root::gate\""));
        assert!(out.contains("what_i_did_not_check"));
        // No audit scope / low-confidence siblings: no callouts.
        assert!(!out.contains("AUDIT SCOPE"));
        assert!(!out.contains("PRIORITY"));
        // Shares the marker with the node directive: one deep directive per assignment.
        assert_eq!(
            append_deep_gate_instructions(&out, "root::gate", &[], &[]),
            out
        );
        assert_eq!(append_deep_node_instructions(&out, "root::gate"), out);
    }

    #[test]
    fn deep_gate_instructions_enumerate_audit_scope() {
        let scope = vec!["root.a".to_string(), "root.b".to_string()];
        let out = append_deep_gate_instructions("Critique the work", "root::gate", &scope, &[]);
        assert!(out.contains("AUDIT SCOPE"));
        assert!(out.contains("root.a, root.b"));
        // The coverage contract is stated: each id must be addressed.
        assert!(out.contains("address each"));
    }

    #[test]
    fn deep_gate_instructions_name_low_confidence_probe_targets() {
        let shaky = vec!["root.shaky".to_string(), "root.wobble".to_string()];
        let out = append_deep_gate_instructions("Critique the work", "root::gate", &shaky, &shaky);
        assert!(out.contains("PRIORITY"));
        assert!(out.contains("root.shaky, root.wobble"));
        assert!(out.contains("LOW confidence"));
        // The enforcement is explained: pass is rejected unless addressed.
        assert!(out.contains("REJECT"));
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
