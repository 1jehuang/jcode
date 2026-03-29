use super::App;
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
    let mut markdown = format!(
        "# Observe\n\nLatest tool result added to context.\n\n- Tool: `{}`\n- Status: {}\n",
        tool_call.name,
        if is_error { "error" } else { "completed" }
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
    matches!(name, "side_panel" | "goal" | "todoread" | "todowrite")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0)
}
