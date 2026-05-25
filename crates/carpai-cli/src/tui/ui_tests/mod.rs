use super::*;
use crate::tui::session_picker;
use crate::tui::ui::tools_ui;
use std::sync::{Mutex, OnceLock};

fn viewport_snapshot_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn parse_changelog_from_supports_timestamped_entries() {
    let changelog = concat!(
        "abc123\x1ev1.2.2\x1e1711234500\x1eCut release\x1f",
        "def456\x1e\x1e1711234600\x1eFix follow-up"
    );

    let entries = parse_changelog_from(changelog);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].hash, "abc123");
    assert_eq!(entries[0].tag, "v1.2.2");
    assert_eq!(entries[0].timestamp, Some(1711234500));
    assert_eq!(entries[0].subject, "Cut release");
    assert_eq!(entries[1].timestamp, Some(1711234600));
}

#[test]
fn group_changelog_entries_includes_release_times() {
    let entries = vec![
        ChangelogEntry {
            hash: "aaa111",
            tag: "",
            timestamp: Some(1711235600),
            subject: "Latest unreleased fix",
        },
        ChangelogEntry {
            hash: "bbb222",
            tag: "v1.2.2",
            timestamp: Some(1711234500),
            subject: "Cut release",
        },
        ChangelogEntry {
            hash: "ccc333",
            tag: "",
            timestamp: Some(1711234400),
            subject: "Earlier release commit",
        },
    ];

    let groups = group_changelog_entries(&entries, "v1.2.3 (deadbee)", "2024-03-23 16:46:40 +0000");

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].version, "v1.2.3 (unreleased)");
    assert_eq!(
        groups[0].released_at.as_deref(),
        Some("2024-03-23 16:46 UTC")
    );
    assert_eq!(groups[0].entries, vec!["Latest unreleased fix"]);

    assert_eq!(groups[1].version, "v1.2.2");
    assert_eq!(
        groups[1].released_at.as_deref(),
        Some("2024-03-23 22:55 UTC")
    );
    assert_eq!(
        groups[1].entries,
        vec!["Cut release", "Earlier release commit"]
    );
}

#[test]
fn parse_changelog_from_supports_legacy_entries_without_timestamps() {
    let entries = parse_changelog_from("abc123:v1.2.2:Legacy entry");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hash, "abc123");
    assert_eq!(entries[0].tag, "v1.2.2");
    assert_eq!(entries[0].timestamp, None);
    assert_eq!(entries[0].subject, "Legacy entry");
}

#[test]
fn split_native_scrollbar_area_reserves_one_column_when_enabled() {
    let (content, scrollbar) = split_native_scrollbar_area(Rect::new(3, 4, 20, 8), true);
    assert_eq!(content, Rect::new(3, 4, 19, 8));
    assert_eq!(scrollbar, Some(Rect::new(22, 4, 1, 8)));
}

#[test]
fn split_native_scrollbar_area_skips_tiny_regions() {
    let (content, scrollbar) = split_native_scrollbar_area(Rect::new(1, 2, 1, 5), true);
    assert_eq!(content, Rect::new(1, 2, 1, 5));
    assert!(scrollbar.is_none());
}

#[test]
fn left_aligned_content_inset_only_applies_when_not_centered() {
    assert_eq!(left_aligned_content_inset(40, true), 0);
    assert_eq!(left_aligned_content_inset(40, false), 1);
    assert_eq!(left_aligned_content_inset(1, false), 0);
}

#[test]
fn native_scrollbar_visibility_requires_overflow() {
    assert!(!native_scrollbar_visible(false, 20, 5));
    assert!(!native_scrollbar_visible(true, 0, 5));
    assert!(!native_scrollbar_visible(true, 5, 5));
    assert!(!native_scrollbar_visible(true, 4, 5));
    assert!(native_scrollbar_visible(true, 6, 5));
}

#[derive(Clone, Default)]
struct TestState {
    input: String,
    cursor_pos: usize,
    display_messages: Vec<DisplayMessage>,
    messages_version: u64,
    streaming_text: String,
    batch_progress: Option<crate::bus::BatchProgress>,
    queued_messages: Vec<String>,
    pending_soft_interrupts: Vec<String>,
    interleave_message: Option<String>,
    status: ProcessingStatus,
    queue_mode: bool,
    active_skill: Option<String>,
    centered_mode: bool,
    anim_elapsed: f32,
    time_since_activity: Option<Duration>,
    remote_startup_phase_active: bool,
    inline_view_state: Option<crate::tui::InlineViewState>,
    inline_interactive_state: Option<crate::tui::InlineInteractiveState>,
    changelog_scroll: Option<usize>,
    help_scroll: Option<usize>,
    chat_native_scrollbar: bool,
}

impl crate::tui::TuiState for TestState {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }
    fn display_user_message_count(&self) -> usize {
        self.display_messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
    }
    fn has_display_edit_tool_messages(&self) -> bool {
        self.display_messages.iter().any(|message| {
            message
                .tool_data
                .as_ref()
                .map(|tool| tools_ui::is_edit_tool_name(&tool.name))
                .unwrap_or(false)
        })
    }
    fn side_pane_images(&self) -> Vec<crate::session::RenderedImage> {
        Vec::new()
    }
    fn display_messages_version(&self) -> u64 {
        self.messages_version
    }
    fn streaming_text(&self) -> &str {
        &self.streaming_text
    }
    fn input(&self) -> &str {
        &self.input
    }
    fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }
    fn is_processing(&self) -> bool {
        !matches!(self.status, ProcessingStatus::Idle)
    }
    fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }
    fn interleave_message(&self) -> Option<&str> {
        self.interleave_message.as_deref()
    }
    fn pending_soft_interrupts(&self) -> &[String] {
        &self.pending_soft_interrupts
    }
    fn scroll_offset(&self) -> usize {
        0
    }
    fn auto_scroll_paused(&self) -> bool {
        false
    }
    fn provider_name(&self) -> String {
        "mock".to_string()
    }
    fn provider_model(&self) -> String {
        "mock-model".to_string()
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
        (0, 0)
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
        None
    }
    fn status(&self) -> ProcessingStatus {
        self.status.clone()
    }
    fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        Vec::new()
    }
    fn active_skill(&self) -> Option<String> {
        self.active_skill.clone()
    }
    fn subagent_status(&self) -> Option<String> {
        None
    }
    fn batch_progress(&self) -> Option<crate::bus::BatchProgress> {
        self.batch_progress.clone()
    }
    fn time_since_activity(&self) -> Option<Duration> {
        self.time_since_activity
    }
    fn total_session_tokens(&self) -> Option<(u64, u64)> {
        None
    }
    fn is_remote_mode(&self) -> bool {
        false
    }
    fn is_canary(&self) -> bool {
        false
    }
    fn is_replay(&self) -> bool {
        false
    }
    fn diff_mode(&self) -> crate::config::DiffDisplayMode {
        crate::config::DiffDisplayMode::Inline
    }
    fn current_session_id(&self) -> Option<String> {
        None
    }
    fn session_display_name(&self) -> Option<String> {
        None
    }
    fn server_display_name(&self) -> Option<String> {
        None
    }
    fn server_display_icon(&self) -> Option<String> {
        None
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
        self.remote_startup_phase_active
    }
    fn dictation_key_label(&self) -> Option<String> {
        None
    }
    fn animation_elapsed(&self) -> f32 {
        self.anim_elapsed
    }
    fn rate_limit_remaining(&self) -> Option<Duration> {
        None
    }
    fn queue_mode(&self) -> bool {
        self.queue_mode
    }
    fn next_prompt_new_session_armed(&self) -> bool {
        false
    }
    fn has_stashed_input(&self) -> bool {
        false
    }
    fn context_info(&self) -> crate::prompt::ContextInfo {
        Default::default()
    }
    fn context_limit(&self) -> Option<usize> {
        None
    }
    fn client_update_available(&self) -> bool {
        false
    }
    fn server_update_available(&self) -> Option<bool> {
        None
    }
    fn info_widget_data(&self) -> info_widget::InfoWidgetData {
        Default::default()
    }
    fn render_streaming_markdown(&self, _width: usize) -> Vec<Line<'static>> {
        markdown::render_markdown_with_width(&self.streaming_text, Some(_width))
    }
    fn centered_mode(&self) -> bool {
        self.centered_mode
    }
    fn auth_status(&self) -> crate::auth::AuthStatus {
        Default::default()
    }
    fn update_cost(&mut self) {}
    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode {
        Default::default()
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
    fn diff_line_wrap(&self) -> bool {
        true
    }
    fn inline_interactive_state(&self) -> Option<&crate::tui::InlineInteractiveState> {
        self.inline_interactive_state.as_ref()
    }
    fn inline_view_state(&self) -> Option<&crate::tui::InlineViewState> {
        self.inline_view_state.as_ref()
    }
    fn changelog_scroll(&self) -> Option<usize> {
        self.changelog_scroll
    }
    fn help_scroll(&self) -> Option<usize> {
        self.help_scroll
    }
    fn session_picker_overlay(&self) -> Option<&std::cell::RefCell<session_picker::SessionPicker>> {
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
        None
    }
    fn now_millis(&self) -> u64 {
        0
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
    fn chat_native_scrollbar(&self) -> bool {
        self.chat_native_scrollbar
    }
    fn side_panel_native_scrollbar(&self) -> bool {
        false
    }
}

fn reset_prompt_viewport_state_for_test() {
    TEST_PROMPT_VIEWPORT_STATE.with(|state| {
        *state.borrow_mut() = PromptViewportState::default();
    });
}

#[path = "basic.rs"]
mod basic;
#[path = "diagrams.rs"]
mod diagrams;
#[path = "prepare.rs"]
mod prepared_messages_tests;
#[path = "rendering.rs"]
mod rendering;
#[path = "tools.rs"]
mod tools;
