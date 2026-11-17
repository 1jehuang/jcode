use crate::protocol::NotificationType;
#[path = "remote_notifications_presentation.rs"]
mod presentation;
pub(super) use presentation::file_activity_stack_message;
use presentation::present_swarm_notification_inner;
pub(super) use presentation::swarm_delivery_display_message;

use crate::tui::ui::capitalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SwarmNotificationPresentation {
    pub title: String,
    pub message: String,
    pub status_notice: String,
}

fn compact_swarm_session_label(session: &str) -> String {
    crate::id::extract_session_name(session)
        .unwrap_or(session)
        .to_string()
}

fn compact_swarm_summary(summary: &str) -> String {
    summary.replace(", ", " · ")
}

fn strip_message_prefix<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    message.strip_prefix(prefix).map(str::trim)
}

fn compact_direct_message_body(message: &str) -> String {
    if let Some((_, body)) = message
        .strip_prefix("DM from ")
        .and_then(|rest| rest.split_once(": "))
    {
        return body.trim().to_string();
    }
    message.to_string()
}

fn compact_channel_message_body(message: &str) -> String {
    if let Some((_, body)) = message
        .strip_prefix('#')
        .and_then(|rest| rest.split_once(": "))
    {
        return body.trim().to_string();
    }
    message.to_string()
}

fn compact_broadcast_message_body(message: &str) -> String {
    if let Some((_, body)) = message
        .strip_prefix("broadcast from ")
        .and_then(|rest| rest.split_once(": "))
    {
        return body.trim().to_string();
    }
    message.to_string()
}

fn compact_plan_message_body(message: &str) -> String {
    if message.starts_with("Plan updated by ")
        && message.ends_with(')')
        && let Some(summary) = message.rsplit_once(" (").map(|(_, summary)| summary)
    {
        return compact_swarm_summary(summary.trim_end_matches(')'));
    }

    if let Some(rest) = strip_message_prefix(message, "Plan updated: task '")
        && let Some((task_id, assignee)) = rest.split_once("' assigned to ")
    {
        return format!(
            "Assigned {} → {}",
            task_id.trim(),
            compact_swarm_session_label(assignee.trim_end_matches('.').trim())
        );
    }

    if let Some(rest) = strip_message_prefix(message, "Plan approved by coordinator: ")
        && let Some((count, proposer)) = rest.split_once(" items added from ")
    {
        return format!(
            "Approved {} items from {}",
            count.trim(),
            compact_swarm_session_label(proposer.trim_end_matches('.').trim())
        );
    }

    if let Some(summary) = message
        .strip_prefix("Plan attached to this session (")
        .and_then(|rest| rest.strip_suffix(")."))
    {
        return format!("Attached · {}", compact_swarm_summary(summary));
    }

    message.to_string()
}

fn compact_swarm_path(path: &str) -> String {
    let trimmed = path.trim();
    let parts: Vec<&str> = trimmed
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();

    if parts.len() <= 4 {
        trimmed.to_string()
    } else {
        format!("…/{}", parts[parts.len() - 4..].join("/"))
    }
}

fn sanitize_code_fence_content(text: &str) -> String {
    text.replace("```", "``\u{200b}`")
}

fn file_activity_summary_line(operation: &str, summary: Option<&str>) -> String {
    summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(capitalize)
        .unwrap_or_else(|| capitalize(operation))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Single-line file-activity preview in every notifications mode. Intent and
/// diff details live behind the explicit expand control, never inline by default.
fn format_file_activity_message_compact(
    path: &str,
    operation: &str,
    summary: Option<&str>,
) -> String {
    format!(
        "`{}` · {}",
        compact_swarm_path(path),
        file_activity_summary_line(operation, summary)
    )
}

/// One peer edit shown in a file-activity row. Consecutive edits stack into a
/// single transcript row instead of adding a row each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileActivityEdit {
    pub sender: String,
    pub path: String,
    pub operation: String,
    pub intent: Option<String>,
    pub summary: Option<String>,
    pub detail: Option<String>,
    /// `(added, removed)` lines, when the server reported them.
    pub delta: Option<(u64, u64)>,
    pub edits: u32,
}

fn parse_file_activity_count<T: std::str::FromStr>(text: &str) -> Option<T>
where
    T::Err: std::fmt::Display,
{
    match text.parse() {
        Ok(count) => Some(count),
        Err(error) => {
            crate::logging::debug(&format!("Invalid file activity count: {error}"));
            None
        }
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

impl FileActivityEdit {
    pub(super) fn from_notification(
        sender: &str,
        notification_type: &NotificationType,
    ) -> Option<Self> {
        let NotificationType::FileConflict {
            path,
            operation,
            intent,
            summary,
            detail,
            added,
            removed,
        } = notification_type
        else {
            return None;
        };
        Some(Self {
            sender: sender.to_string(),
            path: path.clone(),
            operation: operation.clone(),
            intent: non_empty(intent.as_deref()),
            summary: non_empty(summary.as_deref()),
            detail: non_empty(detail.as_deref()),
            delta: added.zip(*removed),
            edits: 1,
        })
    }

    fn is_conflict(&self) -> bool {
        self.operation.to_ascii_lowercase().contains("conflict")
            || self.summary.as_deref().is_some_and(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("conflict") || value.contains("concurrent")
            })
    }

    /// `+3 -1`, or the tool's own summary when the server sent no counts.
    fn change_label(&self) -> String {
        match self.delta {
            Some((added, removed)) => format!("+{added} -{removed}"),
            None => file_activity_summary_line(&self.operation, self.summary.as_deref()),
        }
    }
}

/// Add `edit` to a stack, folding repeated edits by one agent to one file.
pub(super) fn stack_file_activity_edit(stack: &mut Vec<FileActivityEdit>, edit: FileActivityEdit) {
    let Some(existing) = stack
        .iter_mut()
        .find(|existing| existing.sender == edit.sender && existing.path == edit.path)
    else {
        stack.push(edit);
        return;
    };
    existing.delta = match (existing.delta, edit.delta) {
        (Some((a, r)), Some((added, removed))) => Some((a + added, r + removed)),
        _ => None,
    };
    existing.edits += edit.edits;
    existing.operation = edit.operation;
    existing.summary = edit.summary;
    if edit.intent.is_some() {
        existing.intent = edit.intent;
    }
    if edit.detail.is_some() {
        existing.detail = edit.detail;
    }
}

impl super::App {
    /// Show a peer edit. While the newest transcript row is already a
    /// file-activity row, the edit is folded into it. Returns the row when a
    /// new one was added.
    pub(super) fn push_file_activity(
        &mut self,
        edit: FileActivityEdit,
    ) -> Option<crate::tui::DisplayMessage> {
        let last = self.display_messages.len().checked_sub(1);
        if let Some((idx, mut stack)) = self.file_activity_stack.take()
            && Some(idx) == last
            && let Some(row) = self.display_messages.get(idx)
            && row.role == "swarm"
            && row
                .title
                .as_deref()
                .is_some_and(|title| title.starts_with("File "))
        {
            let expanded = jcode_tui_messages::parse_collapsible_swarm_content(&row.content)
                .is_some_and(|parsed| parsed.expanded);
            stack_file_activity_edit(&mut stack, edit);
            let message = file_activity_stack_message(&stack, expanded);
            self.replace_display_message_title_and_content(idx, message.title, message.content);
            self.file_activity_stack = Some((idx, stack));
            return None;
        }
        let stack = vec![edit];
        let message = file_activity_stack_message(&stack, false);
        self.push_display_message(message.clone());
        self.file_activity_stack = self
            .display_messages
            .len()
            .checked_sub(1)
            .map(|idx| (idx, stack));
        Some(message)
    }
}

/// Parse one entry of the model-facing notice:
/// `agent edited path +3-1 [x2] [L4-9] [overlap] [earlier][: intent]`.
fn parse_file_activity_notice_entry(line: &str) -> Option<FileActivityEdit> {
    let (sender, rest) = line.trim().split_once(' ')?;
    let (operation, rest) = rest.split_once(' ')?;
    if sender.is_empty() || !matches!(operation, "edited" | "wrote") {
        return None;
    }
    // The counts are the first ` +A-R` token, so paths may contain spaces.
    let (path, added, removed, after) = rest.match_indices(" +").find_map(|(at, _)| {
        let counts = &rest[at + 2..];
        let end = counts.find([' ', ':']).unwrap_or(counts.len());
        let (added, removed) = counts[..end].split_once('-')?;
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        (digits(added) && digits(removed) && at > 0)
            .then(|| (&rest[..at], added, removed, &counts[end..]))
    })?;
    let (facts, tail) = match after.find(": ") {
        Some(at) => (&after[..at], &after[at..]),
        None => (after, ""),
    };
    let edits = facts
        .split_whitespace()
        .find_map(|fact| fact.strip_prefix('x'))
        .and_then(parse_file_activity_count)
        .unwrap_or(1);
    Some(FileActivityEdit {
        sender: compact_swarm_session_label(sender),
        path: path.to_string(),
        operation: operation.to_string(),
        intent: non_empty(tail.strip_prefix(": ")),
        summary: None,
        detail: None,
        delta: Some((
            parse_file_activity_count(added)?,
            parse_file_activity_count(removed)?,
        )),
        edits,
    })
}

/// Parse a delivered model-facing file-activity notice (one or many entries).
/// The notice has no prefix. Live deliveries are identified by their
/// `file_activity` display role in both live delivery and restored history.
pub(super) fn parse_file_activity_notice(content: &str) -> Option<Vec<FileActivityEdit>> {
    let content = content.trim();
    // Notices delivered before the prefix was dropped.
    let legacy = content.strip_prefix("⚠ File activity: ");
    let rest = legacy.unwrap_or(content);
    let parse_entry = |line: &str| {
        parse_file_activity_notice_entry(line).or_else(|| {
            // The earlier dense format omitted the verb, but carried a
            // prefix. Only try that shape for explicitly prefixed history.
            legacy?;
            let (sender, rest) = line.split_once(' ')?;
            parse_file_activity_notice_entry(&format!("{sender} edited {rest}"))
        })
    };
    let mut lines = rest.lines();
    let first = lines.next()?;
    let mut stack = vec![parse_entry(first)?];
    for line in lines {
        stack_file_activity_edit(&mut stack, parse_entry(line)?);
    }
    Some(stack)
}

/// A worker completion handoff generated by the server
/// (`Agent <name> <status>\n\n<details>`).
struct CompletionNotice<'a> {
    sender: &'a str,
    /// `None` for a normal finish, otherwise the abnormal outcome.
    outcome: Option<&'static str>,
    details: &'a str,
}

/// Match the generated envelope, not arbitrary user prose about agents.
fn parse_completion_notice(trimmed: &str) -> Option<CompletionNotice<'_>> {
    let rest = trimmed.strip_prefix("Agent ")?;
    let (intro, details) = rest.split_once("\n\n")?;
    let (sender, status) = intro.split_once(' ')?;
    let outcome = match status {
        "finished their work and is ready for more." | "completed their work." => None,
        "finished with status failed." => Some("failed"),
        "stopped." => Some("stopped"),
        "crashed while working." => Some("crashed"),
        _ => return None,
    };
    if sender.is_empty()
        || !(details.starts_with("Recent output (automatically captured):\n")
            || details.starts_with("No recent assistant output was captured. ")
            || details.starts_with("Lane plan:\n")
            || details.starts_with("Final response:\n"))
    {
        return None;
    }
    Some(CompletionNotice {
        sender,
        outcome,
        details,
    })
}

/// `lane 1/2` for the worker's lane, `lane 1/2 +1` when it holds several.
fn completion_lane_label(details: &str) -> Option<String> {
    let plan = details.strip_prefix("Lane plan:\n")?;
    let plan = plan.split("\n\nFinal response:\n").next().unwrap_or(plan);
    let mut lanes = plan.lines().filter_map(|line| {
        let line = line.trim();
        if line.starts_with("- ") {
            return None;
        }
        let (name, progress) = line.strip_suffix(" completed")?.rsplit_once(": ")?;
        let name = match name
            .strip_prefix("Phase ")
            .and_then(|rest| rest.split_once(" · "))
        {
            Some((number, name)) if number.bytes().all(|b| b.is_ascii_digit()) => name,
            _ => name,
        };
        Some(format!("{} {}", name.trim(), progress.trim()))
    });
    let first = lanes.next()?;
    Some(match lanes.count() {
        0 => first,
        more => format!("{first} +{more}"),
    })
}

/// First line of what the worker actually said, if anything was captured.
fn completion_response_line(details: &str) -> Option<String> {
    let response = details
        .split_once("Final response:\n")
        .or_else(|| details.split_once("Recent output (automatically captured):\n"))?
        .1;
    let line = jcode_tui_messages::swarm_preview_line(response, usize::MAX);
    (!line.is_empty()
        && line != "No final assistant response was captured."
        && line != "No report captured.")
        .then_some(line)
}

/// Collapsed transcript content for a completion handoff. The row already
/// leads with the agent's emoji, so the preview names the lane and its
/// progress, then what the worker reported, instead of repeating the same
/// generic sentence for every finished turn. The full handoff stays behind the
/// expand control.
fn completion_notice_content(trimmed: &str, tldr: Option<&str>) -> Option<String> {
    let notice = parse_completion_notice(trimmed)?;
    let summary = tldr
        .map(|tldr| jcode_tui_messages::swarm_preview_line(tldr, usize::MAX))
        .filter(|tldr| !tldr.is_empty())
        .or_else(|| completion_response_line(notice.details));
    let parts: Vec<String> = [
        completion_lane_label(notice.details),
        notice.outcome.map(str::to_string),
        summary,
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return None;
    }
    let preview = jcode_tui_messages::swarm_preview_line(
        &parts.join(" · "),
        jcode_tui_messages::SWARM_INLINE_BODY_MAX_CHARS,
    );
    Some(jcode_tui_messages::encode_collapsible_swarm_content(
        &preview, trimmed,
    ))
}

pub(super) fn present_swarm_notification(
    sender: &str,
    notification_type: &NotificationType,
    message: &str,
    compact: bool,
) -> SwarmNotificationPresentation {
    let mut presentation =
        present_swarm_notification_inner(sender, notification_type, message, compact);
    if let NotificationType::Message { scope, tldr, .. } = notification_type
        && scope.as_deref() == Some("swarm")
        && let Some(content) = completion_notice_content(message.trim(), tldr.as_deref())
    {
        if let Some(parsed) = jcode_tui_messages::parse_collapsible_swarm_content(&content) {
            presentation.status_notice =
                format!("{} · {}", presentation.status_notice, parsed.tldr);
        }
        presentation.message = content;
        return presentation;
    }
    // Sender-provided tldr: store the full body but render it collapsed to the
    // tldr line with an expand control. The status line shows the tldr too.
    if let NotificationType::Message {
        tldr: Some(tldr), ..
    } = notification_type
    {
        let tldr = jcode_tui_messages::swarm_preview_line(
            tldr,
            jcode_tui_messages::SWARM_INLINE_BODY_MAX_CHARS,
        );
        if !tldr.is_empty() && !presentation.message.trim().is_empty() {
            presentation.status_notice = format!("{} · {}", presentation.status_notice, tldr);
            presentation.message =
                jcode_tui_messages::encode_collapsible_swarm_content(&tldr, &presentation.message);
        }
    }
    // Never render long agent prose inline: anything beyond one short row is
    // collapsed behind a derived one-row preview, expandable on demand.
    if collapses_to_one_row(notification_type) {
        presentation.message =
            jcode_tui_messages::collapse_long_swarm_content(&presentation.message);
    }
    presentation
}

/// Notification kinds whose transcript body is agent-authored prose that
/// should never render as more than one row by default. Plan, await and
/// background-task notices have their own compact renderers.
fn collapses_to_one_row(notification_type: &NotificationType) -> bool {
    match notification_type {
        NotificationType::Message { scope, .. } => !matches!(
            scope.as_deref(),
            Some("plan" | "background_task" | "swarm_await")
        ),
        NotificationType::SharedContext { .. } => true,
        NotificationType::FileConflict { .. } => false,
    }
}

/// Full underlying body of swarm display content, collapsed or not.
#[cfg(test)]
pub(super) fn swarm_display_full_body(content: &str) -> &str {
    match jcode_tui_messages::parse_collapsible_swarm_content(content) {
        Some(parsed) if !parsed.body.trim().is_empty() => parsed.body.trim(),
        Some(parsed) => parsed.tldr.trim(),
        None => content.trim(),
    }
}

pub(super) fn typed_file_activity_display_message(
    content: &str,
) -> Option<crate::tui::DisplayMessage> {
    let stack = parse_file_activity_notice(content)?;
    Some(file_activity_stack_message(&stack, false))
}

/// Restore the same compact presentation for model-facing file notices that
/// older transcripts saved without the structured notification payload.
pub(super) fn file_activity_delivery_display_message(
    content: &str,
) -> Option<crate::tui::DisplayMessage> {
    let trimmed = content.trim();
    if trimmed.starts_with("⚠ File activity: ")
        && let Some(stack) = parse_file_activity_notice(trimmed)
    {
        return Some(file_activity_stack_message(&stack, false));
    }
    let rest = trimmed
        .strip_prefix("⚠️ File activity: ")
        .or_else(|| trimmed.strip_prefix("⚠ File activity: "))?;
    let (path, activity) = if let Some((path, rest)) = rest.split_once(" \u{2014} ") {
        // Current wire format includes a scope (overlapping/nearby) segment.
        let (_, activity) = rest.split_once(" \u{2014} ")?;
        (path, activity)
    } else {
        // Older messages used a hyphen and had no scope segment.
        rest.split_once(" - ")?
    };
    let (sender, activity) = activity.split_once(' ')?;
    let activity = activity
        .strip_prefix("previously ")
        .or_else(|| activity.strip_prefix("just "))?;
    if sender.is_empty() || sender.contains(char::is_whitespace) {
        return None;
    }
    let (operation, tail) = activity.split_once(" this file")?;
    let tail = tail
        .strip_prefix(" you previously worked with")
        .unwrap_or(tail);
    let summary = tail
        .strip_prefix(": ")
        .map(|value| value.split(" \u{2014} intent: ").next().unwrap_or(value));
    let preview = format_file_activity_message_compact(path, operation, summary).replace('`', "");
    Some(crate::tui::DisplayMessage::swarm(
        format!("File activity · {}", compact_swarm_session_label(sender)),
        jcode_tui_messages::encode_collapsible_swarm_content(&preview, trimmed),
    ))
}

/// Rewrite user/system transcript rows that are really delivered swarm
/// messages into one-row swarm rows. Used for restored history, where the
/// original notification card is not available.
pub(super) fn normalize_swarm_delivery_messages(messages: &mut [crate::tui::DisplayMessage]) {
    for message in messages.iter_mut() {
        if message.role == "file_activity" {
            if let Some(stack) = parse_file_activity_notice(&message.content) {
                *message = file_activity_stack_message(&stack, false);
            } else {
                message.role = "system".to_string();
            }
            continue;
        }
        if !matches!(message.role.as_str(), "user" | "system") {
            continue;
        }
        if message.role == "system"
            && let Some(activity) = file_activity_delivery_display_message(&message.content)
        {
            *message = activity;
            continue;
        }
        if let Some(swarm) = swarm_delivery_display_message(&message.content) {
            *message = swarm;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_activity_counts_reject_overflow_and_invalid_wire_values() {
        assert_eq!(
            super::parse_file_activity_count::<u64>("18446744073709551615"),
            Some(u64::MAX)
        );
        assert_eq!(
            super::parse_file_activity_count::<u64>("18446744073709551616"),
            None
        );
        assert_eq!(
            super::parse_file_activity_count::<u32>("4294967295"),
            Some(u32::MAX)
        );
        assert_eq!(super::parse_file_activity_count::<u32>("4294967296"), None);
        assert_eq!(super::parse_file_activity_count::<u64>("-1"), None);
        assert_eq!(super::parse_file_activity_count::<u64>("unknown"), None);
    }

    use super::{compact_plan_message_body, present_swarm_notification};
    use crate::protocol::NotificationType;

    #[test]
    fn restored_file_activity_is_compact_and_retains_full_body() {
        let notices = [
            "⚠ File activity: /home/jeremy/jcode/src/parser.rs \u{2014} overlapping lines \u{2014} moss just edited this file you previously worked with: edited lines 2-9 \u{2014} intent: fix parser",
            "⚠️ File activity: /home/jeremy/jcode/src/parser.rs - moss previously read this file: read lines 2-9",
            "⚠ File activity: /home/jeremy/jcode/src/parser.rs \u{2014} nearby lines \u{2014} moss previously read this file",
        ];
        for content in notices {
            let mut messages = vec![crate::tui::DisplayMessage::system(content)];
            super::normalize_swarm_delivery_messages(&mut messages);
            let message = &messages[0];
            assert_eq!(message.role, "swarm");
            assert_eq!(message.title.as_deref(), Some("File activity · moss"));
            let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&message.content)
                .expect("restored activity retains expandable details");
            assert!(!parsed.expanded);
            assert!(parsed.tldr.starts_with("…/jeremy/jcode/src/parser.rs · "));
            assert!(!parsed.tldr.contains("previously"));
            assert!(!parsed.tldr.contains("intent:"));
            assert_eq!(parsed.body, content);
            let normalized = message.content.clone();
            super::normalize_swarm_delivery_messages(&mut messages);
            assert_eq!(
                messages[0].content, normalized,
                "normalization is idempotent"
            );
        }
    }

    #[test]
    fn file_activity_normalizer_preserves_user_prose_and_unrecognized_notices() {
        for content in [
            "⚠ File activity: file.rs - not a delivered file notice",
            "⚠ File activity: file.rs \u{2014} missing peer information",
            "A normal multiline\nstatus notice",
        ] {
            let mut messages = vec![crate::tui::DisplayMessage::system(content)];
            super::normalize_swarm_delivery_messages(&mut messages);
            assert_eq!(messages[0].role, "system");
            assert_eq!(messages[0].content, content);
        }
        let content = "⚠ File activity: file.rs - moss previously read this file";
        let mut message = crate::tui::DisplayMessage::system(content);
        message.role = "user".to_string();
        let mut messages = vec![message];
        super::normalize_swarm_delivery_messages(&mut messages);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, content);
    }

    #[test]
    fn file_activity_is_single_row_with_details_hidden_for_both_settings() {
        let notification = NotificationType::FileConflict {
            path: "/home/jeremy/jcode/src/parser.rs".to_string(),
            operation: "edited".to_string(),
            intent: Some("fix parser".to_string()),
            summary: Some("edited lines 2-9\n  and adjusted tests".to_string()),
            detail: Some("- old line\n+ new line".to_string()),
            added: Some(1),
            removed: Some(1),
        };
        let render = |compact| {
            let presentation =
                present_swarm_notification("moss", &notification, "raw notice", compact);
            let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
                .expect("details remain collapsible");
            assert!(!parsed.tldr.contains('\n'));
            assert!(!parsed.expanded);
            assert!(parsed.body.contains("- old line"));
            let message =
                crate::tui::DisplayMessage::swarm(presentation.title, presentation.message);
            let lines = crate::tui::ui::render_swarm_message(
                &message,
                60,
                crate::config::DiffDisplayMode::Off,
            );
            assert_eq!(lines.len(), 1);
            assert!(lines[0].width() <= 60);
            let text = lines[0]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(text.contains("+1 -1 · fix parser"), "{text}");
            assert!(!text.contains("old line"));
            assert!(!text.contains("parser.rs"));
            text
        };
        assert_eq!(render(false), render(true));
    }

    #[test]
    fn completion_rows_show_lane_progress_and_report_not_generic_intro() {
        let tldr_of = |content: &str| {
            jcode_tui_messages::parse_collapsible_swarm_content(content)
                .expect("completion is collapsible")
                .tldr
                .to_string()
        };
        let compact = "Agent tulip finished their work and is ready for more.\n\nFinal response:\nNo report captured.";
        let compact_row =
            super::swarm_delivery_display_message(compact).expect("compact completion");
        assert_eq!(compact_row.title.as_deref(), Some("Swarm · tulip"));
        assert!(!tldr_of(&compact_row.content).contains("No report captured."));
        let handoff = "Agent panda finished their work and is ready for more.\n\nLane plan:\nPhase 2 · resident-query-archive: 1/2 completed\n- [completed] Archive\n- [pending] Upload cap\n\nFinal response:\n\nReadback verified.\nMore detail.";
        let restored = super::swarm_delivery_display_message(handoff).expect("swarm row");
        assert_eq!(restored.title.as_deref(), Some("Swarm · panda"));
        assert_eq!(
            tldr_of(&restored.content),
            "resident-query-archive 1/2 · Readback verified."
        );
        assert_eq!(super::swarm_display_full_body(&restored.content), handoff);

        // Live notifications agree, and a sender tldr replaces the derived line.
        let live = |tldr: Option<&str>| {
            present_swarm_notification(
                "panda",
                &NotificationType::Message {
                    scope: Some("swarm".to_string()),
                    channel: None,
                    tldr: tldr.map(str::to_string),
                },
                handoff,
                false,
            )
        };
        assert_eq!(live(None).message, restored.content);
        assert_eq!(
            tldr_of(&live(Some("Cap raised")).message),
            "resident-query-archive 1/2 · Cap raised"
        );

        let lines = crate::tui::ui::render_swarm_message(
            &restored,
            60,
            crate::config::DiffDisplayMode::Off,
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].width() <= 60);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.starts_with(&format!(
                "{} resident-query-archive 1/2",
                crate::id::session_icon("panda")
            )),
            "unexpected completion preview: {text:?}"
        );
        assert!(!text.contains("finished their work"));

        // Abnormal outcomes stay visible, extra lanes are counted, and a
        // handoff with nothing to summarize keeps its original wording.
        let failed = "Agent owl crashed while working.\n\nLane plan:\nparser: 0/1 completed\n- [in_progress] Parse\nPhase 3 · docs: 0/2 completed\n\nFinal response:\nNo final assistant response was captured.";
        let failed = super::swarm_delivery_display_message(failed).expect("swarm row");
        assert_eq!(tldr_of(&failed.content), "parser 0/1 +1 · crashed");
        let bare = "Agent owl finished their work and is ready for more.\n\nLane plan:\nNo lane plan available.\n\nFinal response:\nNo final assistant response was captured.";
        let bare = super::swarm_delivery_display_message(bare).expect("swarm row");
        assert_eq!(
            tldr_of(&bare.content),
            "Agent owl finished their work and is ready for more."
        );
        assert!(super::swarm_delivery_display_message("Agent owl is nice.\n\nReally.").is_none());
    }

    #[test]
    fn compact_plan_message_body_drops_redundant_plan_prefix() {
        assert_eq!(
            compact_plan_message_body("Plan updated by sheep (4 items, v1)"),
            "4 items · v1"
        );
        assert_eq!(
            compact_plan_message_body(
                "Plan updated: task 'issue41-memory-headed' assigned to session_mouse_1774660180567.",
            ),
            "Assigned issue41-memory-headed → mouse"
        );
    }

    #[test]
    fn present_swarm_notification_with_tldr_encodes_collapsed_content() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: Some("fixed the flaky test".to_string()),
            },
            "DM from sheep: The flaky test was caused by a race in the setup helper. I rewrote it to use a barrier and verified 200 consecutive runs pass.",
            false,
        );

        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
            .expect("tldr message should encode collapsible content");
        assert!(!parsed.expanded);
        assert_eq!(parsed.tldr, "fixed the flaky test");
        assert!(parsed.body.contains("race in the setup helper"));
        assert!(
            presentation.status_notice.contains("fixed the flaky test"),
            "{}",
            presentation.status_notice
        );
    }

    #[test]
    fn present_swarm_notification_without_tldr_collapses_long_body() {
        let body = "Investigated the parser. ".repeat(20);
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            &format!("DM from sheep: {body}\n\nMore details here."),
            false,
        );
        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
            .expect("long body must collapse");
        assert!(!parsed.expanded);
        assert!(!parsed.tldr.contains('\n'));
        assert!(parsed.tldr.chars().count() <= jcode_tui_messages::SWARM_INLINE_BODY_MAX_CHARS);
        assert!(parsed.body.contains("More details here."));
    }

    #[test]
    fn swarm_delivery_display_message_collapses_injected_dm() {
        let long = "x".repeat(400);
        let msg = super::swarm_delivery_display_message(&format!("DM from fox: {long}"))
            .expect("dm delivery");
        assert_eq!(msg.role, "swarm");
        assert_eq!(msg.title.as_deref(), Some("DM from fox"));
        let parsed =
            jcode_tui_messages::parse_collapsible_swarm_content(&msg.content).expect("collapsed");
        assert_eq!(parsed.body, long);

        let chan = super::swarm_delivery_display_message("#parser from owl: hi").expect("channel");
        assert_eq!(chan.title.as_deref(), Some("#parser · owl"));
        assert_eq!(chan.content, "hi");

        assert!(super::swarm_delivery_display_message("DM from my boss: please review").is_none());
        assert!(super::swarm_delivery_display_message("hello there").is_none());
    }

    #[test]
    fn present_swarm_notification_without_tldr_keeps_plain_content() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            "DM from sheep: short note",
            false,
        );
        assert!(
            jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message).is_none()
        );
        assert_eq!(presentation.message, "short note");
    }

    #[test]
    fn present_swarm_notification_formats_task_assignments_as_tasks() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            "Task assigned to you by coordinator: Implement compaction asymptotic fixes - You own the compaction task.",
            false,
        );

        assert_eq!(presentation.title, "Task · sheep");
        assert_eq!(
            presentation.message,
            "Implement compaction asymptotic fixes - You own the compaction task."
        );
        assert_eq!(presentation.status_notice, "Task assigned by sheep");
    }

    #[test]
    fn present_swarm_notification_compacts_swarm_await_without_report_prose() {
        let presentation = present_swarm_notification(
            "swarm await",
            &NotificationType::Message {
                scope: Some("swarm_await".to_string()),
                channel: None,
                tldr: None,
            },
            "🐝 **Swarm await finished**\n\nAll members done. All 2 members are done: fox, wolf\n\nMember statuses:\n  ✓ fox (completed)\n  ✓ wolf (completed)\n\nRecent outputs:\n\n--- fox (completed) ---\nParser tests pass.",
            false,
        );

        assert_eq!(presentation.title, "🐝 Swarm await");
        assert_eq!(presentation.message, "2/2 matched · 2 completed");
        assert!(!presentation.message.contains("fox"));
        assert!(!presentation.message.contains("Parser tests"));
        assert_eq!(presentation.status_notice, "🐝 Swarm await finished");
    }

    #[test]
    fn present_swarm_notification_formats_background_task_scope_cleanly() {
        let presentation = present_swarm_notification(
            "background task",
            &NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
                tldr: None,
            },
            "Background task failed · selfdev-build · exit 101",
            false,
        );

        assert_eq!(presentation.title, "Background task");
        assert_eq!(
            presentation.message,
            "Background task failed · selfdev-build · exit 101"
        );
        assert_eq!(presentation.status_notice, "Background task update");
    }

    #[test]
    fn present_swarm_notification_formats_background_task_progress_notice() {
        let presentation = present_swarm_notification(
            "background task",
            &NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
                tldr: None,
            },
            "**Background task progress** `bg123` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
            false,
        );

        assert_eq!(presentation.title, "Background task progress");
        assert_eq!(
            presentation.status_notice,
            "Background task · bash · 42% · Running tests"
        );
    }

    #[test]
    fn present_swarm_notification_strips_redundant_dm_prefix() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            "DM from sheep: I can see your worktree diff.",
            false,
        );

        assert_eq!(presentation.title, "DM from sheep");
        assert_eq!(presentation.message, "I can see your worktree diff.");
        assert_eq!(presentation.status_notice, "DM from sheep");
    }

    #[test]
    fn present_swarm_notification_compacts_plan_titles_and_bodies() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("plan".to_string()),
                channel: None,
                tldr: None,
            },
            "Plan updated by sheep (4 items, v1)",
            false,
        );

        assert_eq!(presentation.title, "Plan · sheep");
        assert_eq!(presentation.message, "4 items · v1");
        assert_eq!(presentation.status_notice, "Swarm plan updated");
    }

    fn edit(sender: &str, path: &str, delta: (u64, u64), intent: &str) -> super::FileActivityEdit {
        super::FileActivityEdit::from_notification(
            sender,
            &NotificationType::FileConflict {
                path: path.to_string(),
                operation: "edited".to_string(),
                intent: Some(intent.to_string()),
                summary: Some("edited lines 323-348 (1 occurrence)".to_string()),
                detail: Some("323- old line\n323+ new line".to_string()),
                added: Some(delta.0),
                removed: Some(delta.1),
            },
        )
        .expect("file conflict")
    }

    fn collapsed(message: &crate::tui::DisplayMessage) -> (String, String) {
        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&message.content)
            .expect("file activity is collapsible");
        assert!(!parsed.expanded);
        (parsed.tldr.to_string(), parsed.body.to_string())
    }

    #[test]
    fn single_file_activity_shows_line_counts_and_intent_with_diff_behind_expand() {
        for compact in [false, true] {
            let presentation = present_swarm_notification(
                "moss",
                &NotificationType::FileConflict {
                    path: "/home/jeremy/jcode/src/tool/communicate.rs".to_string(),
                    operation: "edited".to_string(),
                    intent: Some("wire swarm intent display".to_string()),
                    summary: Some("edited lines 323-348 (1 occurrence)".to_string()),
                    detail: Some("323- old line\n323+ new line".to_string()),
                    added: Some(3),
                    removed: Some(1),
                },
                "raw notice",
                compact,
            );
            assert_eq!(presentation.title, "File activity · moss");
            let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
                .expect("file activity should be collapsible");
            assert_eq!(parsed.tldr, "+3 -1 · wire swarm intent display");
            assert!(parsed.body.contains("…/jcode/src/tool/communicate.rs"));
            assert!(parsed.body.contains("Intent: wire swarm intent display"));
            assert!(
                parsed
                    .body
                    .contains("```text\n323- old line\n323+ new line\n```")
            );
            assert_eq!(
                presentation.status_notice,
                "File activity · …/jcode/src/tool/communicate.rs"
            );
        }
    }

    #[test]
    fn file_activity_without_counts_or_intent_falls_back_to_summary_and_path() {
        let edit = super::FileActivityEdit::from_notification(
            "moss",
            &NotificationType::FileConflict {
                path: "src/a.rs".to_string(),
                operation: "edited".to_string(),
                intent: None,
                summary: Some("edited lines 2-9".to_string()),
                detail: None,
                added: None,
                removed: None,
            },
        )
        .unwrap();
        let (tldr, _) = collapsed(&super::file_activity_stack_message(&[edit], false));
        assert_eq!(tldr, "Edited lines 2-9 · src/a.rs");
    }

    #[test]
    fn stacked_file_activity_shows_only_agent_emojis_and_summed_line_counts() {
        let mut stack = Vec::new();
        super::stack_file_activity_edit(&mut stack, edit("moss", "src/a.rs", (3, 1), "first"));
        super::stack_file_activity_edit(&mut stack, edit("moss", "src/a.rs", (2, 0), "second"));
        // Same agent and file: still a single-agent row, counts summed.
        let message = super::file_activity_stack_message(&stack, false);
        assert_eq!(message.title.as_deref(), Some("File activity · moss"));
        assert_eq!(collapsed(&message).0, "+5 -1 · 2 edits · second");

        super::stack_file_activity_edit(&mut stack, edit("moss", "src/b.rs", (10, 4), "third"));
        super::stack_file_activity_edit(&mut stack, edit("fox", "src/a.rs", (1, 1), "fourth"));
        let message = super::file_activity_stack_message(&stack, false);
        assert_eq!(message.title.as_deref(), Some("File activity"));
        let (tldr, body) = collapsed(&message);
        assert_eq!(
            tldr,
            format!(
                "{} +15 -5 · 3 edits  {} +1 -1",
                crate::id::session_icon("moss"),
                crate::id::session_icon("fox")
            )
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
        );
        for hidden in ["third", "fourth", "src/a.rs", "src/b.rs", "moss", "fox"] {
            assert!(!tldr.contains(hidden), "{hidden} leaked into {tldr}");
            assert!(body.contains(hidden), "{hidden} missing from details");
        }
        let lines =
            crate::tui::ui::render_swarm_message(&message, 60, crate::config::DiffDisplayMode::Off);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].width() <= 60);
    }

    #[test]
    fn consecutive_peer_edits_stack_into_one_transcript_row() {
        let mut app = crate::tui::app::tests::create_test_app();
        let before = app.display_messages.len();
        assert!(
            app.push_file_activity(edit("moss", "src/a.rs", (3, 1), "one"))
                .is_some()
        );
        assert!(
            app.push_file_activity(edit("fox", "src/b.rs", (2, 2), "two"))
                .is_none()
        );
        assert!(
            app.push_file_activity(edit("moss", "src/a.rs", (1, 0), "three"))
                .is_none()
        );
        assert_eq!(app.display_messages.len(), before + 1);
        let row = app.display_messages.last().unwrap().clone();
        assert_eq!(row.title.as_deref(), Some("File activity"));
        assert!(collapsed(&row).0.contains("+4 -1 · 2 edits"));

        // Anything else in the transcript ends the stack.
        app.push_display_message(crate::tui::DisplayMessage::system("unrelated"));
        assert!(
            app.push_file_activity(edit("moss", "src/a.rs", (9, 9), "four"))
                .is_some()
        );
        assert_eq!(app.display_messages.len(), before + 3);
        assert_eq!(
            collapsed(app.display_messages.last().unwrap()).0,
            "+9 -9 · four"
        );
    }

    #[test]
    fn delivered_model_notice_restores_as_the_same_compact_row() {
        let single = "moss edited src/parser.rs +3-1 L2-9 overlap: fix parser";
        let mut message = crate::tui::DisplayMessage::system(single);
        message.role = "file_activity".to_string();
        let mut messages = vec![message];
        super::normalize_swarm_delivery_messages(&mut messages);
        assert_eq!(messages[0].role, "swarm");
        assert_eq!(messages[0].title.as_deref(), Some("File activity · moss"));
        assert_eq!(collapsed(&messages[0]).0, "+3 -1 · fix parser");

        let batch =
            "moss edited src/my parser.rs +3-1 x2: fix parser\nfox wrote src/lib.rs +10-0 earlier";
        for prose in [
            "moss edited the parser and it works",
            "Plan proposal received in swarm s1\nFrom: moss",
            "moss edited a.rs +3-1\nand then some prose",
        ] {
            assert!(
                super::parse_file_activity_notice(prose).is_none(),
                "{prose}"
            );
        }
        let stack = super::parse_file_activity_notice(batch).expect("batch notice parses");
        assert_eq!(stack.len(), 2);
        assert_eq!(stack[0].edits, 2);
        assert_eq!(stack[1].delta, Some((10, 0)));
        assert_eq!(stack[1].intent, None);
        let mut message = crate::tui::DisplayMessage::system(batch);
        message.role = "file_activity".to_string();
        let mut messages = vec![message];
        super::normalize_swarm_delivery_messages(&mut messages);
        assert_eq!(messages[0].title.as_deref(), Some("File activity"));
    }
}
