use super::{App, DisplayMessage};
use crate::message::ToolCall;
use crate::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};

pub(super) const OBSERVE_PAGE_ID: &str = "observe";
const OBSERVE_PAGE_TITLE: &str = "Observe";

impl App {
    pub(super) fn observe_mode_enabled(&self) -> bool {
        self.observe_mode_enabled
    }

    fn should_observe_tool(&self, tool_call: &ToolCall) -> bool {
        self.observe_mode_enabled && !is_noise_tool(&tool_call.name)
    }

    pub(super) fn set_observe_mode_enabled(&mut self, enabled: bool, focus: bool) {
        self.observe_mode_enabled = enabled;
        let mut snapshot = self.snapshot_without_observe();
        if enabled {
            if self.observe_page_markdown.trim().is_empty() {
                self.observe_page_markdown = observe_placeholder_markdown();
                self.observe_page_updated_at_ms = now_ms();
            }
            snapshot = self.decorate_side_panel_with_observe(snapshot, focus);
        } else if snapshot.focused_page_id.is_none() {
            snapshot.focused_page_id = self
                .last_side_panel_focus_id
                .clone()
                .filter(|id| snapshot.pages.iter().any(|page| page.id == *id))
                .or_else(|| snapshot.pages.first().map(|page| page.id.clone()));
        }
        self.apply_side_panel_snapshot(snapshot);
    }

    pub(super) fn observe_tool_call(&mut self, tool_call: &ToolCall) {
        if !self.should_observe_tool(tool_call) {
            return;
        }
        self.observe_page_markdown = build_observe_tool_call_markdown(tool_call);
        self.observe_page_updated_at_ms = now_ms();
        self.refresh_observe_page();
    }

    pub(super) fn observe_tool_result(
        &mut self,
        tool_call: &ToolCall,
        output: &str,
        is_error: bool,
        title: Option<&str>,
    ) {
        if !self.should_observe_tool(tool_call) {
            return;
        }
        self.observe_page_markdown =
            build_observe_tool_result_markdown(tool_call, output, is_error, title);
        self.observe_page_updated_at_ms = now_ms();
        self.refresh_observe_page();
    }

    /// React to a completed tool by forcing the info-widget caches it may have
    /// dirtied to refetch on the next frame, instead of waiting out their TTLs.
    ///
    /// The info widget's git-status and todos panels are stale-while-revalidate
    /// caches (5s / 1s TTL). The agent's own tools are exactly what mutate those
    /// underlying sources, so without an explicit nudge the widget would show the
    /// repo/todo state from *before* the tool ran for a full TTL. This keeps the
    /// SWR perf benefit (no synchronous git/disk work on the render path) while
    /// making self-inflicted staleness self-correct immediately: the next read
    /// returns the last value and kicks a background refresh.
    ///
    /// Called unconditionally on every tool completion (local and remote paths),
    /// independent of observe mode. Tool names are matched leniently because the
    /// same logical tool surfaces under several aliases (e.g. `bash`/`shell`,
    /// `write`/`write_file`).
    pub(super) fn note_tool_completed(&mut self, tool_call: &ToolCall, is_error: bool) {
        if is_error {
            // A failed tool did not change the working tree or todos.
            return;
        }

        let name = tool_call.name.to_ascii_lowercase();

        // Any filesystem- or repo-mutating tool can change git status (branch,
        // staged/modified/untracked counts). `batch` can wrap any of these, so
        // treat it as potentially mutating too.
        let mutates_repo = matches!(
            name.as_str(),
            "bash"
                | "shell"
                | "shell_exec"
                | "write"
                | "write_file"
                | "edit"
                | "edit_file"
                | "multiedit"
                | "patch"
                | "apply_patch"
                | "batch"
                | "run_shell"
        );
        if mutates_repo {
            super::helpers::invalidate_git_info_cache();
        }

        // The todo tool rewrites the per-session todo list.
        if (name == "todo" || name == "todowrite" || name == "todo_write")
            && let Some(session_id) = self.active_client_session_id().map(str::to_string)
        {
            super::helpers::invalidate_todos_cache(&session_id);
            // Local sessions also receive TodoUpdated, while remote sessions only
            // observe the completed tool call. Refresh here as well so both paths
            // adopt the same todo-derived title that /resume displays.
            self.update_terminal_title();
            // Long-task subscribe nudge: arms while incomplete todos exist,
            // fires once when a 1h+ batch completes with quality gates passed.
            self.note_todo_update_for_subscribe_nudge(&session_id);
        }

        // The schedule tool queues/cancels ambient tasks, which the ambient panel
        // surfaces (queue count, next wake).
        if name == "schedule" {
            super::helpers::invalidate_ambient_info_cache();
        }
    }

    /// Surface private todo quality-gate decisions to the user without exposing
    /// their numeric thresholds to the model or the transcript.
    pub(super) fn note_todo_gate_result(
        &mut self,
        tool_call: &ToolCall,
        output: &str,
        is_error: bool,
    ) {
        if let Some(notice) = todo_gate_notice(&tool_call.name, output, is_error) {
            self.push_display_message(DisplayMessage::system(notice));
        }
    }

    pub(super) fn decorate_side_panel_with_observe(
        &self,
        mut snapshot: SidePanelSnapshot,
        focus_observe: bool,
    ) -> SidePanelSnapshot {
        snapshot.pages.retain(|page| page.id != OBSERVE_PAGE_ID);
        snapshot.pages.push(self.observe_page());
        snapshot.pages.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        if focus_observe || snapshot.focused_page_id.is_none() {
            snapshot.focused_page_id = Some(OBSERVE_PAGE_ID.to_string());
        }
        snapshot
    }

    pub(super) fn snapshot_without_observe(&self) -> SidePanelSnapshot {
        let mut snapshot = self.side_panel.clone();
        snapshot.pages.retain(|page| page.id != OBSERVE_PAGE_ID);
        if snapshot.focused_page_id.as_deref() == Some(OBSERVE_PAGE_ID) {
            snapshot.focused_page_id = None;
        }
        snapshot
    }

    fn refresh_observe_page(&mut self) {
        if !self.observe_mode_enabled {
            return;
        }

        let focus_observe = self.side_panel.focused_page_id.as_deref() == Some(OBSERVE_PAGE_ID);
        let snapshot =
            self.decorate_side_panel_with_observe(self.snapshot_without_observe(), focus_observe);
        self.apply_side_panel_snapshot(snapshot);
    }

    fn observe_page(&self) -> SidePanelPage {
        SidePanelPage {
            id: OBSERVE_PAGE_ID.to_string(),
            title: OBSERVE_PAGE_TITLE.to_string(),
            file_path: "observe://latest-context".to_string(),
            format: SidePanelPageFormat::Markdown,
            source: SidePanelPageSource::Ephemeral,
            content: if self.observe_page_markdown.trim().is_empty() {
                observe_placeholder_markdown()
            } else {
                self.observe_page_markdown.clone()
            },
            updated_at_ms: self.observe_page_updated_at_ms.max(1),
        }
    }
}

fn todo_gate_notice(name: &str, output: &str, is_error: bool) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    if !matches!(name.as_str(), "todo" | "todowrite" | "todo_write") {
        return None;
    }

    if output.contains(crate::todo::TODO_OWNERSHIP_CONTINUATION_MESSAGE) {
        Some("🛑 Todo completion gate: end-to-end ownership needs full-outcome follow-through.")
    } else if !is_error && output.contains(crate::todo::TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE)
    {
        Some("👉 Todo quality gate: hill-climbability needs a stronger feedback loop.")
    } else {
        None
    }
}

fn observe_placeholder_markdown() -> String {
    "# Observe\n\nWaiting for the next tool call or tool result.\n\nThis page is transient and only shows the **latest** useful context-bearing tool activity. UI/bookkeeping tools like `side_panel`, `goal`, and todo reads/writes are skipped. It is not persisted to disk.\n".to_string()
}

fn build_observe_tool_call_markdown(tool_call: &ToolCall) -> String {
    format!(
        "# Observe\n\nLatest tool call emitted by the model.\n\n- Tool: `{}`\n- Status: running\n\n## Tool input\n{}\n",
        tool_call.name,
        fenced_block("json", &pretty_json(&tool_call.input))
    )
}

fn build_observe_tool_result_markdown(
    tool_call: &ToolCall,
    output: &str,
    is_error: bool,
    title: Option<&str>,
) -> String {
    let token_count = crate::util::estimate_tokens(output);
    let token_label = crate::util::format_approx_token_count(token_count);
    let output_chars = crate::util::format_number(output.len());
    // Keep these severity badges ASCII-only. Emoji/variation-selector glyphs
    // like ⚠️ and 🔴 are prone to width mismatches in terminal emulators and can
    // leave stale cells behind when the observe pane repaints.
    let size_note = match crate::util::approx_tool_output_token_severity(token_count) {
        crate::util::ApproxTokenSeverity::Normal => None,
        crate::util::ApproxTokenSeverity::Warning => Some(" [large]"),
        crate::util::ApproxTokenSeverity::Danger => Some(" [very large]"),
    };
    let mut markdown = format!(
        "# Observe\n\nLatest tool result added to context.\n\n- Tool: `{}`\n- Status: {}\n- Returned to context: `{}` · `{} chars`{}\n",
        tool_call.name,
        if is_error { "error" } else { "completed" },
        token_label,
        output_chars,
        size_note.unwrap_or("")
    );
    if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
        markdown.push_str(&format!("- Title: `{}`\n", title.trim()));
    }
    markdown.push_str(&format!(
        "\n## Tool input\n{}\n\n## Tool output\n{}\n",
        fenced_block("json", &pretty_json(&tool_call.input)),
        fenced_block("text", if output.is_empty() { "(empty)" } else { output })
    ));
    markdown
}

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn fenced_block(language: &str, text: &str) -> String {
    let max_run = text
        .split('\n')
        .flat_map(|line| line.split(|ch| ch != '`'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(max_run.max(3) + 1);
    if language.trim().is_empty() {
        format!("{fence}\n{text}\n{fence}")
    } else {
        format!("{fence}{language}\n{text}\n{fence}")
    }
}

fn is_noise_tool(name: &str) -> bool {
    matches!(
        name,
        "side_panel" | "goal" | "todo" | "todoread" | "todowrite"
    )
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_gate_notices_are_user_visible_without_private_thresholds() {
        let ownership = todo_gate_notice(
            "todo",
            crate::todo::TODO_OWNERSHIP_CONTINUATION_MESSAGE,
            false,
        )
        .expect("ownership gate should produce a notice");
        let hill = todo_gate_notice(
            "todo",
            crate::todo::TODO_HILL_CLIMBABILITY_CONTINUATION_MESSAGE,
            false,
        )
        .expect("hill-climbability gate should produce a notice");

        assert!(ownership.contains("completion gate"));
        assert!(hill.contains("quality gate"));
        assert!(!ownership.contains(&crate::todo::QUALITY_GATE_THRESHOLD.to_string()));
        assert!(!hill.contains(&crate::todo::QUALITY_GATE_THRESHOLD.to_string()));
        assert!(todo_gate_notice("bash", ownership, true).is_none());
    }
}
