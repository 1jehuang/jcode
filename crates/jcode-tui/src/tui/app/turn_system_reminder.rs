use super::*;

/// Body of a model-facing `<system-reminder>` block, without its tags.
/// The tags are wire syntax for the model and must never reach the transcript.
pub(in crate::tui::app) fn system_reminder_body(content: &str) -> Option<&str> {
    let rest = content.trim().strip_prefix("<system-reminder>")?;
    Some(
        rest.strip_suffix("</system-reminder>")
            .unwrap_or(rest)
            .trim(),
    )
}

/// Header of the late MCP tool announcement built by the agent
/// (`announce_late_mcp_tools`). Must match its first line.
const LATE_MCP_ANNOUNCEMENT_MARKER: &str = "New MCP tools are available.";

/// One-line display summary for a late MCP tool announcement. The model still
/// receives the full list with schemas; the transcript only needs a count and
/// the servers involved.
pub(in crate::tui::app) fn late_mcp_announcement_summary(body: &str) -> Option<String> {
    if !body.starts_with(LATE_MCP_ANNOUNCEMENT_MARKER) {
        return None;
    }
    let mut count = 0usize;
    let mut servers: Vec<&str> = Vec::new();
    for line in body.lines() {
        if let Some(entry) = line.strip_prefix("- ") {
            count += 1;
            let server = entry
                .split_once("server: ")
                .and_then(|(_, rest)| rest.split_whitespace().next());
            if let Some(server) = server
                && !servers.contains(&server)
            {
                servers.push(server);
            }
        } else if let Some(rest) = line.strip_prefix("... and ") {
            count += rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
        }
    }
    let noun = if count == 1 { "tool" } else { "tools" };
    let tokens = crate::util::format_approx_token_count(crate::util::estimate_tokens(body));
    let mut summary = format!("{count} new MCP {noun} available via mcp");
    if !servers.is_empty() {
        summary.push_str(&format!(" ({})", servers.join(", ")));
    }
    summary.push_str(&format!(" · ~{tokens}"));
    Some(summary)
}

#[cfg(test)]
mod late_mcp_summary_tests {
    use super::late_mcp_announcement_summary;

    #[test]
    fn summarizes_announcement_to_one_line() {
        let body = "New MCP tools are available. They are not in your tool list; call them with mcp action=call.\n\
            - mcp__gh__issues: server: gh  tool: issues  (List issues)\n  input_schema: {}\n\
            - mcp__gh__prs: server: gh  tool: prs  (List PRs)\n  input_schema: {}\n\
            - mcp__fs__read: server: fs  tool: read  (Read)\n  input_schema: {}\n\
            ... and 4 more; use mcp action=search to find them.";
        let expected = format!(
            "7 new MCP tools available via mcp (gh, fs) · ~{}",
            crate::util::format_approx_token_count(crate::util::estimate_tokens(body))
        );
        assert_eq!(
            late_mcp_announcement_summary(body).as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(late_mcp_announcement_summary("Something else"), None);
    }
}

impl App {
    /// Append a hidden `<system-reminder>` message after the turn's prompt.
    /// Per-turn reminders must never go in the system prompt: it precedes the
    /// whole conversation, so changing it invalidates every cached message.
    pub(in crate::tui::app) fn append_turn_system_reminder(&mut self, reminder: Option<String>) {
        let Some(reminder) = reminder
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        let text = format!("<system-reminder>\n{reminder}\n</system-reminder>");
        self.add_provider_message(Message::user(&text));
        self.session.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
            Some(crate::session::StoredDisplayRole::System),
        );
    }
}
