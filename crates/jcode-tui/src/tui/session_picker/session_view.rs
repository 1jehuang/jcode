//! A read-only [`TuiState`] for the selected resume candidate.
//!
//! The preview draws through the real chat renderer (`ui::draw`) instead of a
//! copied layout, so header, transcript, pinned todos, composer, status line,
//! and info widgets match the opened session by construction. Every fact comes
//! from the selected session's persisted snapshot (or its live feed), never
//! from the App behind the picker.
use crate::message::ToolCall;
use crate::tui::info_widget::InfoWidgetData;
use crate::tui::{DisplayMessage, ProcessingStatus, TuiState, markdown};
use ratatui::text::Line;
use std::time::Duration;

/// Live in-flight turn state for a running session, when a feed is attached.
#[derive(Clone, Debug, Default)]
pub(crate) struct LiveTurn {
    pub streaming_text: String,
    pub status: ProcessingStatus,
    pub elapsed: Option<Duration>,
    pub tokens: (u64, u64),
}

pub(crate) struct SessionView<'a> {
    pub messages: &'a [DisplayMessage],
    /// Unique per transcript content so the shared prepare caches never
    /// return the live session's frame (or another preview's).
    pub version: u64,
    pub hidden_prompts: usize,
    pub info: InfoWidgetData,
    pub todos_payload: Option<&'a str>,
    pub session_name: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub server_name: Option<String>,
    pub server_icon: Option<String>,
    pub is_canary: bool,
    pub working_dir: Option<String>,
    /// Cloud VM or SSH host the session belongs to.
    pub machine: Option<String>,
    /// Local credentials only describe sessions that resume on this machine.
    pub local_auth: bool,
    /// Top-anchored scroll when paused; `None` follows the tail.
    pub scroll: Option<usize>,
    pub live: Option<LiveTurn>,
    pub now_ms: u64,
}

/// Distinguish preview versions from the live App's small monotonic counter.
pub(crate) fn preview_messages_version(content_hash: u64) -> u64 {
    content_hash | (1 << 63)
}

pub(crate) fn display_messages_from_preview(
    preview: &[super::PreviewMessage],
) -> Vec<DisplayMessage> {
    preview
        .iter()
        .map(|msg| DisplayMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
            tool_calls: msg.tool_calls.clone(),
            duration_secs: msg.duration_secs,
            title: msg.title.clone(),
            tool_data: msg.tool_data.clone(),
        })
        .collect()
}

impl SessionView<'_> {
    fn live_status(&self) -> ProcessingStatus {
        self.live
            .as_ref()
            .map(|live| live.status.clone())
            .unwrap_or_default()
    }
}

impl TuiState for SessionView<'_> {
    fn display_messages(&self) -> &[DisplayMessage] {
        self.messages
    }
    fn display_user_message_count(&self) -> usize {
        self.messages.iter().filter(|m| m.role == "user").count()
    }
    fn compacted_hidden_user_prompts(&self) -> usize {
        self.hidden_prompts
    }
    fn has_display_edit_tool_messages(&self) -> bool {
        self.messages.iter().any(|message| {
            message
                .tool_data
                .as_ref()
                .is_some_and(|tool| crate::tui::ui::tools_ui::is_edit_tool_name(&tool.name))
        })
    }
    fn pinned_todos_payload(&self) -> Option<&str> {
        self.todos_payload
    }
    fn side_pane_images(&self) -> Vec<crate::session::RenderedImage> {
        Vec::new()
    }
    fn display_messages_version(&self) -> u64 {
        self.version
    }
    fn streaming_text(&self) -> &str {
        self.live
            .as_ref()
            .map_or("", |live| live.streaming_text.as_str())
    }
    fn input(&self) -> &str {
        ""
    }
    fn cursor_pos(&self) -> usize {
        0
    }
    fn is_processing(&self) -> bool {
        !matches!(self.live_status(), ProcessingStatus::Idle)
    }
    fn queued_messages(&self) -> &[String] {
        &[]
    }
    fn interleave_message(&self) -> Option<&str> {
        None
    }
    fn pending_soft_interrupts(&self) -> &[String] {
        &[]
    }
    fn scroll_offset(&self) -> usize {
        self.scroll.unwrap_or(0)
    }
    fn auto_scroll_paused(&self) -> bool {
        self.scroll.is_some()
    }
    fn provider_name(&self) -> String {
        self.provider.clone().unwrap_or_default()
    }
    fn provider_model(&self) -> String {
        self.model.clone().unwrap_or_default()
    }
    fn upstream_provider(&self) -> Option<String> {
        None
    }
    fn connection_type(&self) -> Option<String> {
        None
    }
    fn status_detail(&self) -> Option<String> {
        None
    }
    fn mcp_servers(&self) -> Vec<(String, usize)> {
        Vec::new()
    }
    fn available_skills(&self) -> Vec<String> {
        Vec::new()
    }
    fn streaming_tokens(&self) -> (u64, u64) {
        self.live.as_ref().map_or((0, 0), |live| live.tokens)
    }
    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>) {
        (None, None)
    }
    fn output_tps(&self) -> Option<f32> {
        None
    }
    fn streaming_tool_calls(&self) -> Vec<ToolCall> {
        Vec::new()
    }
    fn elapsed(&self) -> Option<Duration> {
        self.live.as_ref().and_then(|live| live.elapsed)
    }
    fn status(&self) -> ProcessingStatus {
        self.live_status()
    }
    fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        Vec::new()
    }
    fn active_skill(&self) -> Option<String> {
        None
    }
    fn subagent_status(&self) -> Option<String> {
        None
    }
    fn batch_progress(&self) -> Option<crate::bus::BatchProgress> {
        None
    }
    fn time_since_activity(&self) -> Option<Duration> {
        None
    }
    fn total_session_tokens(&self) -> Option<(u64, u64)> {
        None
    }
    fn is_remote_mode(&self) -> bool {
        false
    }
    fn is_canary(&self) -> bool {
        self.is_canary
    }
    fn is_replay(&self) -> bool {
        false
    }
    fn diff_mode(&self) -> crate::config::DiffDisplayMode {
        crate::config::config().display.diff_mode
    }
    fn current_session_id(&self) -> Option<String> {
        None
    }
    fn session_display_name(&self) -> Option<String> {
        self.session_name.clone()
    }
    fn server_display_name(&self) -> Option<String> {
        self.server_name.clone()
    }
    fn server_display_icon(&self) -> Option<String> {
        self.server_icon.clone()
    }
    fn server_sessions(&self) -> Vec<String> {
        Vec::new()
    }
    fn connected_clients(&self) -> Option<usize> {
        None
    }
    fn status_notice(&self) -> Option<String> {
        None
    }
    fn remote_startup_phase_active(&self) -> bool {
        false
    }
    fn dictation_key_label(&self) -> Option<String> {
        None
    }
    fn animation_elapsed(&self) -> f32 {
        self.now_ms as f32 / 1000.0
    }
    fn rate_limit_remaining(&self) -> Option<Duration> {
        None
    }
    fn queue_mode(&self) -> bool {
        false
    }
    fn has_stashed_input(&self) -> bool {
        false
    }
    fn context_info(&self) -> crate::prompt::ContextInfo {
        Default::default()
    }
    fn context_limit(&self) -> Option<usize> {
        self.info.context_limit
    }
    fn client_update_available(&self) -> bool {
        false
    }
    fn server_update_available(&self) -> Option<bool> {
        None
    }
    fn info_widget_data(&self) -> InfoWidgetData {
        self.info.clone()
    }
    fn render_streaming_markdown(&self, width: usize) -> Vec<Line<'static>> {
        markdown::render_markdown_with_width(self.streaming_text(), Some(width))
    }
    fn centered_mode(&self) -> bool {
        // The opened session uses the user's alignment preference.
        crate::config::config().display.centered
    }
    fn auth_status(&self) -> crate::auth::AuthStatus {
        if self.local_auth && !crate::tui::is_ssh_remote() {
            crate::auth::AuthStatus::check_fast_nonblocking()
        } else {
            Default::default()
        }
    }
    fn update_cost(&mut self) {}
    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode {
        crate::config::DiagramDisplayMode::None
    }
    fn diagram_focus(&self) -> bool {
        false
    }
    fn diagram_index(&self) -> usize {
        0
    }
    fn diagram_scroll(&self) -> (i32, i32) {
        (0, 0)
    }
    fn diagram_pane_ratio(&self) -> u8 {
        50
    }
    fn diagram_pane_ratio_user_adjusted(&self) -> bool {
        false
    }
    fn diagram_pane_animating(&self) -> bool {
        false
    }
    fn diagram_pane_enabled(&self) -> bool {
        false
    }
    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition {
        Default::default()
    }
    fn diagram_zoom(&self) -> u8 {
        100
    }
    fn diff_pane_scroll(&self) -> usize {
        0
    }
    fn diff_pane_scroll_x(&self) -> i32 {
        0
    }
    fn side_panel_image_zoom_percent(&self) -> u8 {
        100
    }
    fn diff_pane_focus(&self) -> bool {
        false
    }
    fn side_panel(&self) -> &crate::side_panel::SidePanelSnapshot {
        static EMPTY: std::sync::LazyLock<crate::side_panel::SidePanelSnapshot> =
            std::sync::LazyLock::new(crate::side_panel::SidePanelSnapshot::default);
        &EMPTY
    }
    fn pin_images(&self) -> bool {
        false
    }
    fn chat_native_scrollbar(&self) -> bool {
        crate::config::config().display.native_scrollbars.chat
    }
    fn side_panel_native_scrollbar(&self) -> bool {
        false
    }
    fn inline_interactive_state(&self) -> Option<&crate::tui::InlineInteractiveState> {
        None
    }
    fn changelog_scroll(&self) -> Option<usize> {
        None
    }
    fn help_scroll(&self) -> Option<usize> {
        None
    }
    fn session_picker_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<crate::tui::session_picker::SessionPicker>> {
        None
    }
    fn login_picker_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<crate::tui::login_picker::LoginPicker>> {
        None
    }
    fn account_picker_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<crate::tui::account_picker::AccountPicker>> {
        None
    }
    fn usage_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<crate::tui::usage_overlay::UsageOverlay>> {
        None
    }
    fn working_dir(&self) -> Option<String> {
        self.working_dir.clone()
    }
    fn remote_machine(&self) -> Option<String> {
        self.machine.clone()
    }
    fn now_millis(&self) -> u64 {
        self.now_ms
    }
    fn copy_badge_ui(&self) -> crate::tui::CopyBadgeUiState {
        Default::default()
    }
    fn copy_selection_mode(&self) -> bool {
        false
    }
    fn copy_selection_range(&self) -> Option<crate::tui::CopySelectionRange> {
        None
    }
    fn copy_selection_status(&self) -> Option<crate::tui::CopySelectionStatus> {
        None
    }
    fn suggestion_prompts(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn cache_ttl_status(&self) -> Option<crate::tui::CacheTtlInfo> {
        None
    }
}

/// Render `view` with the real chat renderer into `area` of `frame`, isolated
/// from the live session's published state. Returns the resolved geometry and
/// the prepared transcript it drew.
pub(crate) fn draw_session_view(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    view: &SessionView<'_>,
) -> (
    crate::tui::ui::SecondaryGeometry,
    Option<std::sync::Arc<jcode_tui_messages::PreparedChatFrame>>,
) {
    use ratatui::{Terminal, backend::TestBackend};
    if area.width == 0 || area.height == 0 {
        return (Default::default(), None);
    }
    let center = crate::tui::markdown::center_code_blocks();
    let (rendered, geometry) = crate::tui::markdown::with_center_code_blocks(center, || {
        crate::tui::ui::with_secondary_render(|| {
            jcode_tui_mermaid::with_image_frame_offset(area.x, area.y, || {
                let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).ok()?;
                let completed = terminal
                    .draw(|inner| crate::tui::ui::draw(inner, view))
                    .ok()?;
                Some(completed.buffer.clone())
            })
        })
    });
    let prepared = crate::tui::ui::take_secondary_chat_frame();
    if let Some(buf) = rendered {
        let dst = frame.buffer_mut();
        for y in 0..area.height.min(buf.area.height) {
            for x in 0..area.width.min(buf.area.width) {
                if let (Some(src), Some(cell)) =
                    (buf.cell((x, y)), dst.cell_mut((area.x + x, area.y + y)))
                {
                    *cell = src.clone();
                }
            }
        }
    }
    (geometry, prepared)
}

/// Restyle visible search matches in the rendered preview. Operating on the
/// drawn buffer keeps highlighting independent of how the renderer wrapped.
pub(crate) fn highlight_buffer_matches(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    tokens: &[String],
) {
    use ratatui::style::{Color, Modifier};
    for y in area.top()..area.bottom() {
        let mut cells: Vec<(u16, char)> = Vec::new();
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell((x, y)) {
                let mut chars = cell.symbol().chars();
                if let (Some(ch), None) = (chars.next(), chars.next()) {
                    cells.push((x, ch.to_lowercase().next().unwrap_or(ch)));
                } else {
                    cells.push((x, '\0'));
                }
            }
        }
        for token in tokens {
            let needle: Vec<char> = token.chars().collect();
            if needle.is_empty() || needle.len() > cells.len() {
                continue;
            }
            let mut i = 0;
            while i + needle.len() <= cells.len() {
                if cells[i..i + needle.len()]
                    .iter()
                    .zip(&needle)
                    .all(|((_, c), n)| c == n)
                {
                    for (x, _) in &cells[i..i + needle.len()] {
                        if let Some(cell) = buf.cell_mut((*x, y)) {
                            cell.set_fg(Color::Rgb(255, 214, 90));
                            cell.modifier.insert(Modifier::BOLD);
                        }
                    }
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
    }
}
