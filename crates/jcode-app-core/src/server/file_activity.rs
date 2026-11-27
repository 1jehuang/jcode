use super::FileAccess;

pub(crate) fn parse_file_activity_line_range(summary: Option<&str>) -> Option<(u64, u64)> {
    let summary = summary?;
    let marker_start = summary
        .find("lines ")
        .map(|idx| idx + "lines ".len())
        .or_else(|| summary.find("line ").map(|idx| idx + "line ".len()))?;
    let rest = &summary[marker_start..];
    let mut digits = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    let start = match digits.parse::<u64>() {
        Ok(start) => start,
        Err(error) => {
            crate::logging::info(&format!(
                "Invalid file activity line start {digits:?}: {error}"
            ));
            return None;
        }
    };
    if chars.peek() == Some(&'-') {
        chars.next();
        let mut end_digits = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_digit() {
                end_digits.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        let end = match end_digits.parse::<u64>() {
            Ok(end) => end,
            Err(error) => {
                crate::logging::info(&format!(
                    "Invalid file activity line end {end_digits:?}: {error}"
                ));
                return None;
            }
        };
        Some((start.min(end), start.max(end)))
    } else {
        Some((start, start))
    }
}

#[cfg(test)]
mod line_range_tests {
    use super::parse_file_activity_line_range;

    #[test]
    fn rejects_missing_or_overflowing_range_endpoints() {
        for summary in [
            "lines 5-",
            "line -2",
            "lines 5-18446744073709551616",
            "line 18446744073709551616",
        ] {
            assert_eq!(
                parse_file_activity_line_range(Some(summary)),
                None,
                "{summary}"
            );
        }
        assert_eq!(
            parse_file_activity_line_range(Some("lines 9-3")),
            Some((3, 9))
        );
        assert_eq!(parse_file_activity_line_range(Some("line 5")), Some((5, 5)));
    }
}

pub(crate) fn file_activity_scope_label(
    previous: &FileAccess,
    current: &crate::bus::FileTouch,
) -> &'static str {
    match (
        parse_file_activity_line_range(previous.summary.as_deref()),
        parse_file_activity_line_range(current.summary.as_deref()),
    ) {
        (Some((prev_start, prev_end)), Some((current_start, current_end)))
            if prev_start <= current_end && current_start <= prev_end =>
        {
            "overlapping lines"
        }
        (Some(_), Some(_)) => "same file, non-overlapping lines",
        _ => "same file",
    }
}

/// Most entries one model-facing notice may carry before a fresh one starts.
pub(crate) const FILE_ACTIVITY_NOTICE_MAX_ENTRIES: usize = 12;
const FILE_ACTIVITY_INTENT_MAX_CHARS: usize = 60;

/// One peer's change to one file, as told to another agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileActivityEntry {
    pub agent: String,
    /// "edited" or "wrote".
    pub operation: String,
    /// True when the change predates the recipient's own first edit.
    pub previously: bool,
    /// Path relative to the recipient's working directory when possible.
    pub path: String,
    pub added: u64,
    pub removed: u64,
    pub edits: u32,
    pub lines: Option<(u64, u64)>,
    /// The change overlaps lines the recipient edited.
    pub overlap: bool,
    pub intent: Option<String>,
}

/// Fold `entry` into `entries`, summing repeated edits by the same agent to
/// the same file so a burst of edits stays one line.
pub(crate) fn merge_file_activity_entry(
    entries: &mut Vec<FileActivityEntry>,
    entry: FileActivityEntry,
) {
    let Some(existing) = entries.iter_mut().find(|existing| {
        existing.agent == entry.agent
            && existing.path == entry.path
            && existing.previously == entry.previously
    }) else {
        entries.push(entry);
        return;
    };
    existing.added += entry.added;
    existing.removed += entry.removed;
    existing.edits += entry.edits;
    existing.overlap |= entry.overlap;
    existing.operation = entry.operation;
    existing.lines = match (existing.lines, entry.lines) {
        (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
        _ => None,
    };
    if entry.intent.is_some() {
        existing.intent = entry.intent;
    }
}

fn format_file_activity_entry(entry: &FileActivityEntry) -> String {
    let mut facts = vec![format!("+{}-{}", entry.added, entry.removed)];
    if entry.edits > 1 {
        facts.push(format!("x{}", entry.edits));
    }
    match entry.lines {
        Some((start, end)) if start == end => facts.push(format!("L{start}")),
        Some((start, end)) => facts.push(format!("L{start}-{end}")),
        None => {}
    }
    if entry.overlap {
        facts.push("overlap".to_string());
    }
    if entry.previously {
        facts.push("earlier".to_string());
    }
    let intent = entry
        .intent
        .as_deref()
        .map(|intent| intent.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|intent| !intent.is_empty())
        .map(|intent| {
            if intent.chars().count() > FILE_ACTIVITY_INTENT_MAX_CHARS {
                let cut: String = intent
                    .chars()
                    .take(FILE_ACTIVITY_INTENT_MAX_CHARS - 1)
                    .collect();
                format!(": {}…", cut.trim_end())
            } else {
                format!(": {intent}")
            }
        })
        .unwrap_or_default();
    format!(
        "{} {} {} {}{}",
        entry.agent,
        entry.operation,
        entry.path,
        facts.join(" "),
        intent
    )
}

/// Model-facing notice, one line per peer and file:
/// `agent edited path +A-R [xN] [Lstart-end] [overlap] [earlier][: intent]`.
/// `xN` is the number of edits merged, `overlap` means the change touches
/// lines the recipient edited, and `earlier` means it predates the
/// recipient's own first edit. The diff itself is never included, and there
/// is no prefix: the notice is identified by its interrupt source.
pub(crate) fn format_file_activity_notice(entries: &[FileActivityEntry]) -> String {
    let lines: Vec<String> = entries.iter().map(format_file_activity_entry).collect();
    lines.join("\n")
}

/// `path` relative to the recipient's working directory when it is inside it.
pub(crate) fn file_activity_display_path(
    path: &std::path::Path,
    working_dir: Option<&std::path::Path>,
) -> String {
    working_dir
        .and_then(|dir| path.strip_prefix(dir).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A notice queued for one recipient that later peer edits may still fold into.
pub(crate) struct PendingFileActivityNotice {
    entries: Vec<FileActivityEntry>,
    content: String,
}

/// Queue `entry` for `recipient`. While an earlier notice is still waiting to
/// be injected, the entry is folded into it, so a burst of peer edits reaches
/// the model as one short message instead of one message per edit.
pub(crate) async fn deliver_file_activity_notice(
    recipient: &str,
    entry: FileActivityEntry,
    pending: &mut std::collections::HashMap<String, PendingFileActivityNotice>,
    queues: &super::state::SessionInterruptQueues,
    sessions: &super::SessionAgents,
) {
    // Keep only notices that are still waiting in a live session's queue, so
    // the map never outgrows the sessions with an undelivered notice.
    if !pending.is_empty() {
        let queues = queues.read().await;
        pending.retain(|session_id, notice| {
            queues.get(session_id).is_some_and(|queue| {
                queue.lock().is_ok_and(|queued| {
                    queued.iter().any(|message| {
                        message.source == jcode_agent_runtime::SoftInterruptSource::FileActivity
                            && message.content == notice.content
                    })
                })
            })
        });
    }
    if let Some(notice) = pending.get_mut(recipient) {
        let mut entries = notice.entries.clone();
        merge_file_activity_entry(&mut entries, entry.clone());
        if entries.len() <= FILE_ACTIVITY_NOTICE_MAX_ENTRIES {
            let content = format_file_activity_notice(&entries);
            if super::state::replace_pending_soft_interrupt(
                recipient,
                &notice.content,
                content.clone(),
                queues,
            )
            .await
            {
                notice.entries = entries;
                notice.content = content;
                return;
            }
        }
    }
    let entries = vec![entry];
    let content = format_file_activity_notice(&entries);
    if super::state::queue_soft_interrupt_for_session(
        recipient,
        content.clone(),
        false,
        jcode_agent_runtime::SoftInterruptSource::FileActivity,
        queues,
        sessions,
    )
    .await
    {
        pending.insert(
            recipient.to_string(),
            PendingFileActivityNotice { entries, content },
        );
    } else {
        pending.remove(recipient);
        crate::logging::warn(&format!(
            "Failed to queue file-activity soft interrupt for session {recipient}"
        ));
    }
}
