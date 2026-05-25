use super::*;
use serde::{Deserialize, Serialize};

fn percentile_ms(samples_ms: &[f64], percentile: f64) -> f64 {
    if samples_ms.is_empty() {
        return 0.0;
    }
    let percentile = percentile.clamp(0.0, 1.0);
    let rank = ((samples_ms.len() - 1) as f64 * percentile).round() as usize;
    samples_ms[rank.min(samples_ms.len() - 1)]
}

fn summarize_mermaid_ui_bench(
    samples: &[MermaidUiBenchSample],
    protocol_supported: bool,
    protocol: Option<String>,
) -> MermaidUiBenchSummary {
    let mut elapsed_ms = 0.0;
    let mut first_worker_render_frame = None;
    let mut first_protocol_render_frame = None;
    let mut first_deferred_idle_frame = None;
    let mut pending_frames = 0usize;
    let mut protocol_render_frames = 0usize;
    let mut protocol_rebuild_frames = 0usize;
    let mut saw_pending = false;
    let mut time_to_first_worker_render_ms = None;
    let mut time_to_first_protocol_render_ms = None;
    let mut time_to_deferred_idle_ms = None;

    for sample in samples {
        elapsed_ms += sample.frame_ms;
        if sample.deferred_pending_after > 0 {
            saw_pending = true;
            pending_frames += 1;
        }
        if first_worker_render_frame.is_none() && sample.deferred_worker_renders > 0 {
            first_worker_render_frame = Some(sample.frame);
            time_to_first_worker_render_ms = Some(elapsed_ms);
        }
        let protocol_rendered = sample.image_state_hits > 0
            || sample.image_state_misses > 0
            || sample.fit_state_reuse_hits > 0
            || sample.fit_protocol_rebuilds > 0
            || sample.viewport_state_reuse_hits > 0
            || sample.viewport_protocol_rebuilds > 0;
        if protocol_rendered {
            protocol_render_frames += 1;
            if first_protocol_render_frame.is_none() {
                first_protocol_render_frame = Some(sample.frame);
                time_to_first_protocol_render_ms = Some(elapsed_ms);
            }
        }
        if sample.fit_protocol_rebuilds > 0 || sample.viewport_protocol_rebuilds > 0 {
            protocol_rebuild_frames += 1;
        }
        if saw_pending && first_deferred_idle_frame.is_none() && sample.deferred_pending_after == 0
        {
            first_deferred_idle_frame = Some(sample.frame);
            time_to_deferred_idle_ms = Some(elapsed_ms);
        }
    }

    MermaidUiBenchSummary {
        protocol_supported,
        protocol,
        pending_frames,
        protocol_render_frames,
        protocol_rebuild_frames,
        first_worker_render_frame,
        first_protocol_render_frame,
        first_deferred_idle_frame,
        time_to_first_worker_render_ms,
        time_to_first_protocol_render_ms,
        time_to_deferred_idle_ms,
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugSnapshot {
    state: serde_json::Value,
    frame: Option<crate::tui::visual_debug::FrameCapture>,
    recent_messages: Vec<DebugMessage>,
    queued_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugMessage {
    role: String,
    content: String,
    tool_calls: Vec<String>,
    duration_secs: Option<f32>,
    title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DebugAssertion {
    field: String,
    op: String,
    value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugAssertResult {
    ok: bool,
    field: String,
    op: String,
    expected: serde_json::Value,
    actual: serde_json::Value,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugStepResult {
    step: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DebugScript {
    steps: Vec<String>,
    assertions: Vec<DebugAssertion>,
    wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugRunReport {
    ok: bool,
    steps: Vec<DebugStepResult>,
    assertions: Vec<DebugAssertResult>,
}

fn estimate_display_message_bytes(message: &DisplayMessage) -> usize {
    message.role.len()
        + message.content.len()
        + message
            .tool_calls
            .iter()
            .map(|call| call.len())
            .sum::<usize>()
        + message.title.as_ref().map(|title| title.len()).unwrap_or(0)
        + message
            .tool_data
            .as_ref()
            .map(crate::process_memory::estimate_json_bytes)
            .unwrap_or(0)
}

fn estimate_string_vec_bytes(values: &[String]) -> usize {
    values.iter().map(|value| value.capacity()).sum()
}

fn estimate_pair_vec_bytes(values: &[(String, usize)]) -> usize {
    values.iter().map(|(name, _)| name.capacity()).sum()
}

fn estimate_pending_images_bytes(values: &[(String, String)]) -> usize {
    values
        .iter()
        .map(|(media_type, data)| media_type.capacity() + data.capacity())
        .sum()
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ScrollTestConfig {
    width: Option<u16>,
    height: Option<u16>,
    step: Option<usize>,
    max_steps: Option<usize>,
    padding: Option<usize>,
    diagrams: Option<usize>,
    include_frames: Option<bool>,
    include_paused: Option<bool>,
    diagram: Option<String>,
    diagram_mode: Option<crate::config::DiagramDisplayMode>,
    expect_inline: Option<bool>,
    expect_pane: Option<bool>,
    expect_widget: Option<bool>,
    require_no_anomalies: Option<bool>,
}

#[derive(Debug, Clone)]
pub(super) struct ScrollTestExpectations {
    expect_inline: bool,
    expect_pane: bool,
    expect_widget: bool,
    require_no_anomalies: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ScrollSuiteConfig {
    widths: Option<Vec<u16>>,
    heights: Option<Vec<u16>>,
    diagram_modes: Option<Vec<crate::config::DiagramDisplayMode>>,
    diagrams: Option<usize>,
    step: Option<usize>,
    max_steps: Option<usize>,
    padding: Option<usize>,
    include_frames: Option<bool>,
    include_paused: Option<bool>,
    diagram: Option<String>,
    require_no_anomalies: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SidePanelLatencyConfig {
    width: Option<u16>,
    height: Option<u16>,
    iterations: Option<usize>,
    warmup_iterations: Option<usize>,
    padding: Option<usize>,
    diagrams: Option<usize>,
    include_samples: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct MermaidUiBenchConfig {
    width: Option<u16>,
    height: Option<u16>,
    frames: Option<usize>,
    warmup_frames: Option<usize>,
    padding: Option<usize>,
    diagrams: Option<usize>,
    include_samples: Option<bool>,
    keep_mermaid_cache: Option<bool>,
    sleep_between_frames_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SidePanelLatencySample {
    iteration: usize,
    direction: &'static str,
    scroll_only: bool,
    latency_ms: f64,
    render_ms: Option<f32>,
    scroll_before: usize,
    scroll_after: usize,
    frame_id_before: Option<u64>,
    frame_id_after: Option<u64>,
    scroll_changed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct MermaidUiBenchSample {
    frame: usize,
    frame_ms: f64,
    render_ms: Option<f32>,
    image_regions: usize,
    deferred_pending_after: usize,
    deferred_enqueued: u64,
    deferred_deduped: u64,
    deferred_worker_renders: u64,
    image_state_hits: u64,
    image_state_misses: u64,
    fit_state_reuse_hits: u64,
    fit_protocol_rebuilds: u64,
    viewport_state_reuse_hits: u64,
    viewport_protocol_rebuilds: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct MermaidUiBenchSummary {
    protocol_supported: bool,
    protocol: Option<String>,
    pending_frames: usize,
    protocol_render_frames: usize,
    protocol_rebuild_frames: usize,
    first_worker_render_frame: Option<usize>,
    first_protocol_render_frame: Option<usize>,
    first_deferred_idle_frame: Option<usize>,
    time_to_first_worker_render_ms: Option<f64>,
    time_to_first_protocol_render_ms: Option<f64>,
    time_to_deferred_idle_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugEvent {
    at_ms: u64,
    kind: String,
    detail: String,
}

pub(super) struct DebugTrace {
    pub(super) enabled: bool,
    pub(super) started_at: Instant,
    pub(super) events: Vec<DebugEvent>,
}

impl DebugTrace {
    pub(super) fn new() -> Self {
        Self {
            enabled: false,
            started_at: Instant::now(),
            events: Vec::new(),
        }
    }

    pub(super) fn record(&mut self, kind: &str, detail: String) {
        if !self.enabled {
            return;
        }
        let at_ms = self.started_at.elapsed().as_millis() as u64;
        self.events.push(DebugEvent {
            at_ms,
            kind: kind.to_string(),
            detail,
        });
    }
}

const LARGE_DISPLAY_BLOB_THRESHOLD_BYTES: usize = 16 * 1024;

#[derive(Default)]
struct ProviderMessageMemoryStats {
    content_blocks: usize,
    text_bytes: usize,
    reasoning_bytes: usize,
    tool_use_input_json_bytes: usize,
    tool_result_bytes: usize,
    image_data_bytes: usize,
    openai_compaction_bytes: usize,
    large_blob_count: usize,
    large_blob_bytes: usize,
    large_tool_result_count: usize,
    large_tool_result_bytes: usize,
    max_block_bytes: usize,
}

impl ProviderMessageMemoryStats {
    fn record_bytes(&mut self, bytes: usize) {
        self.max_block_bytes = self.max_block_bytes.max(bytes);
        if bytes >= LARGE_DISPLAY_BLOB_THRESHOLD_BYTES {
            self.large_blob_count += 1;
            self.large_blob_bytes += bytes;
        }
    }

    fn record_message(&mut self, message: &crate::message::Message) {
        for block in &message.content {
            self.content_blocks += 1;
            match block {
                crate::message::ContentBlock::Text { text, .. } => {
                    self.text_bytes += text.len();
                    self.record_bytes(text.len());
                }
                crate::message::ContentBlock::Reasoning { text } => {
                    self.reasoning_bytes += text.len();
                    self.record_bytes(text.len());
                }
                crate::message::ContentBlock::ToolUse { input, .. } => {
                    let bytes = crate::process_memory::estimate_json_bytes(input);
                    self.tool_use_input_json_bytes += bytes;
                    self.record_bytes(bytes);
                }
                crate::message::ContentBlock::ToolResult { content, .. } => {
                    self.tool_result_bytes += content.len();
                    if content.len() >= LARGE_DISPLAY_BLOB_THRESHOLD_BYTES {
                        self.large_tool_result_count += 1;
                        self.large_tool_result_bytes += content.len();
                    }
                    self.record_bytes(content.len());
                }
                crate::message::ContentBlock::Image { data, .. } => {
                    self.image_data_bytes += data.len();
                    self.record_bytes(data.len());
                }
                crate::message::ContentBlock::OpenAICompaction { encrypted_content } => {
                    self.openai_compaction_bytes += encrypted_content.len();
                    self.record_bytes(encrypted_content.len());
                }
            }
        }
    }

    fn payload_text_bytes(&self) -> usize {
        self.text_bytes
            + self.reasoning_bytes
            + self.tool_result_bytes
            + self.image_data_bytes
            + self.openai_compaction_bytes
    }
}

#[derive(Default)]
struct DisplayMessageMemoryStats {
    role_bytes: usize,
    content_bytes: usize,
    tool_call_text_bytes: usize,
    title_bytes: usize,
    tool_data_json_bytes: usize,
    large_content_count: usize,
    large_content_bytes: usize,
    max_content_bytes: usize,
}

impl DisplayMessageMemoryStats {
    fn record_message(&mut self, message: &DisplayMessage) {
        self.role_bytes += message.role.len();
        self.content_bytes += message.content.len();
        self.tool_call_text_bytes += message
            .tool_calls
            .iter()
            .map(|call| call.len())
            .sum::<usize>();
        self.title_bytes += message.title.as_ref().map(|title| title.len()).unwrap_or(0);
        self.tool_data_json_bytes += message
            .tool_data
            .as_ref()
            .map(crate::process_memory::estimate_json_bytes)
            .unwrap_or(0);
        self.max_content_bytes = self.max_content_bytes.max(message.content.len());
        if message.content.len() >= LARGE_DISPLAY_BLOB_THRESHOLD_BYTES {
            self.large_content_count += 1;
            self.large_content_bytes += message.content.len();
        }
    }
}

#[derive(Clone)]
pub(super) struct ScrollTestState {
    display_messages: Vec<DisplayMessage>,
    display_messages_version: u64,
    side_panel: crate::side_panel::SidePanelSnapshot,
    scroll_offset: usize,
    auto_scroll_paused: bool,
    diff_mode: crate::config::DiffDisplayMode,
    diff_pane_scroll: usize,
    diff_pane_scroll_x: i32,
    side_panel_image_zoom_percent: u8,
    diff_pane_focus: bool,
    diff_pane_auto_scroll: bool,
    is_processing: bool,
    streaming_text: String,
    queued_messages: Vec<String>,
    interleave_message: Option<String>,
    pending_soft_interrupts: Vec<String>,
    input: String,
    cursor_pos: usize,
    status: ProcessingStatus,
    processing_started: Option<Instant>,
    status_notice: Option<(String, Instant)>,
    diagram_mode: crate::config::DiagramDisplayMode,
    diagram_focus: bool,
    diagram_index: usize,
    diagram_scroll_x: i32,
    diagram_scroll_y: i32,
    diagram_pane_ratio: u8,
    diagram_pane_ratio_from: u8,
    diagram_pane_ratio_target: u8,
    diagram_pane_anim_start: Option<Instant>,
    diagram_pane_enabled: bool,
    diagram_pane_position: crate::config::DiagramPanePosition,
    diagram_zoom: u8,
}

impl ScrollTestState {
    fn capture(app: &App) -> Self {
        Self {
            display_messages: app.display_messages.clone(),
            display_messages_version: app.display_messages_version,
            side_panel: app.side_panel.clone(),
            scroll_offset: app.scroll_offset,
            auto_scroll_paused: app.auto_scroll_paused,
            diff_mode: app.diff_mode,
            diff_pane_scroll: app.diff_pane_scroll,
            diff_pane_scroll_x: app.diff_pane_scroll_x,
            side_panel_image_zoom_percent: app.side_panel_image_zoom_percent,
            diff_pane_focus: app.diff_pane_focus,
            diff_pane_auto_scroll: app.diff_pane_auto_scroll,
            is_processing: app.is_processing,
            streaming_text: app.streaming_text.clone(),
            queued_messages: app.queued_messages.clone(),
            interleave_message: app.interleave_message.clone(),
            pending_soft_interrupts: app.pending_soft_interrupts.clone(),
            input: app.input.clone(),
            cursor_pos: app.cursor_pos,
            status: app.status.clone(),
            processing_started: app.processing_started,
            status_notice: app.status_notice.clone(),
            diagram_mode: app.diagram_mode,
            diagram_focus: app.diagram_focus,
            diagram_index: app.diagram_index,
            diagram_scroll_x: app.diagram_scroll_x,
            diagram_scroll_y: app.diagram_scroll_y,
            diagram_pane_ratio: app.diagram_pane_ratio,
            diagram_pane_ratio_from: app.diagram_pane_ratio_from,
            diagram_pane_ratio_target: app.diagram_pane_ratio_target,
            diagram_pane_anim_start: app.diagram_pane_anim_start,
            diagram_pane_enabled: app.diagram_pane_enabled,
            diagram_pane_position: app.diagram_pane_position,
            diagram_zoom: app.diagram_zoom,
        }
    }

    fn restore(self, app: &mut App) {
        app.display_messages = self.display_messages;
        app.display_messages_version = self.display_messages_version;
        app.apply_side_panel_snapshot(self.side_panel);
        app.scroll_offset = self.scroll_offset;
        app.auto_scroll_paused = self.auto_scroll_paused;
        app.diff_mode = self.diff_mode;
        app.diff_pane_scroll = self.diff_pane_scroll;
        app.diff_pane_scroll_x = self.diff_pane_scroll_x;
        app.side_panel_image_zoom_percent = self.side_panel_image_zoom_percent;
        app.diff_pane_focus = self.diff_pane_focus;
        app.diff_pane_auto_scroll = self.diff_pane_auto_scroll;
        app.is_processing = self.is_processing;
        app.replace_streaming_text(self.streaming_text);
        app.queued_messages = self.queued_messages;
        app.interleave_message = self.interleave_message;
        app.pending_soft_interrupts = self.pending_soft_interrupts;
        app.input = self.input;
        app.cursor_pos = self.cursor_pos;
        app.status = self.status;
        app.processing_started = self.processing_started;
        app.status_notice = self.status_notice;
        app.diagram_mode = self.diagram_mode;
        app.diagram_focus = self.diagram_focus;
        app.diagram_index = self.diagram_index;
        app.diagram_scroll_x = self.diagram_scroll_x;
        app.diagram_scroll_y = self.diagram_scroll_y;
        app.diagram_pane_ratio = self.diagram_pane_ratio;
        app.diagram_pane_ratio_from = self.diagram_pane_ratio_from;
        app.diagram_pane_ratio_target = self.diagram_pane_ratio_target;
        app.diagram_pane_anim_start = self.diagram_pane_anim_start;
        app.diagram_pane_enabled = self.diagram_pane_enabled;
        app.diagram_pane_position = self.diagram_pane_position;
        app.diagram_zoom = self.diagram_zoom;
    }
}

#[path = "debug_bench.rs"]
mod debug_bench;
#[path = "debug_cmds.rs"]
mod debug_cmds;
#[path = "debug_profile.rs"]
mod debug_profile;
#[path = "debug_script.rs"]
mod debug_script;

pub(super) fn handle_debug_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/debug-visual" || trimmed == "/debug-visual on" {
        use crate::tui::visual_debug;
        visual_debug::enable();
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Visual debugging enabled. Frames are being captured.\n\
                     Use `/debug-visual dump` to write captured frames to file.\n\
                     Use `/debug-visual off` to disable."
                .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        app.set_status_notice("Visual debug: ON");
        return true;
    }

    if trimmed == "/debug-visual off" {
        use crate::tui::visual_debug;
        visual_debug::disable();
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Visual debugging disabled.".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        app.set_status_notice("Visual debug: OFF");
        return true;
    }

    if trimmed == "/debug-visual dump" {
        use crate::tui::visual_debug;
        match visual_debug::dump_to_file() {
            Ok(path) => {
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "Visual debug dump written to:\n`{}`\n\n\
                         This file contains frame captures with:\n\
                         - Layout computations\n\
                         - State snapshots\n\
                         - Rendered text content\n\
                         - Any detected anomalies",
                        path.display()
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            Err(e) => {
                app.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Failed to write visual debug dump: {}", e),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
        return true;
    }

    if trimmed.starts_with("/debug-visual ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/debug-visual` (on), `/debug-visual off`, `/debug-visual dump`".to_string(),
        ));
        return true;
    }

    if trimmed == "/screenshot-mode" || trimmed == "/screenshot-mode on" {
        use crate::tui::screenshot;
        screenshot::enable();
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Screenshot mode enabled.\n\n\
                     Run the watcher in another terminal:\n\
                     ```bash\n\
                     ./scripts/screenshot_watcher.sh\n\
                     ```\n\n\
                     Use `/screenshot <state>` to trigger a capture.\n\
                     Use `/screenshot-mode off` to disable."
                .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/screenshot-mode off" {
        use crate::tui::screenshot;
        screenshot::disable();
        screenshot::clear_all_signals();
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "Screenshot mode disabled.".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed.starts_with("/screenshot ") {
        use crate::tui::screenshot;
        let state_name = trimmed.strip_prefix("/screenshot ").unwrap_or("").trim();
        if !state_name.is_empty() {
            screenshot::signal_ready(
                state_name,
                serde_json::json!({
                    "manual_trigger": true,
                }),
            );
            app.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!("Screenshot signal sent: {}", state_name),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }
        return true;
    }

    if trimmed == "/record" || trimmed == "/record start" {
        use crate::tui::test_harness;
        test_harness::start_recording();
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "🎬 Recording started.\n\n\
                     All your keystrokes are now being recorded.\n\
                     Use `/record stop` to stop and save.\n\
                     Use `/record cancel` to discard."
                .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/record stop" {
        use crate::tui::test_harness;
        test_harness::stop_recording();
        let json = test_harness::get_recorded_events_json();
        let event_count = json.matches("\"type\"").count();

        let recording_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("jcode")
            .join("recordings");
        let _ = std::fs::create_dir_all(&recording_dir);

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = format!("recording_{}.json", timestamp);
        let filepath = recording_dir.join(&filename);

        if let Ok(mut file) = std::fs::File::create(&filepath) {
            use std::io::Write;
            let _ = file.write_all(json.as_bytes());
        }

        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: format!(
                "🎬 Recording stopped.\n\n\
                 **Events recorded:** {}\n\
                 **Saved to:** `{}`\n\n\
                 To replay as video, run:\n\
                 ```bash\n\
                 ./scripts/replay_recording.sh {}\n\
                 ```",
                event_count,
                filepath.display(),
                filepath.display()
            ),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/record cancel" {
        use crate::tui::test_harness;
        test_harness::stop_recording();
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: "🎬 Recording cancelled.".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed.starts_with("/record ") {
        app.push_display_message(DisplayMessage::error(
            "Usage: `/record` (start), `/record stop`, `/record cancel`".to_string(),
        ));
        return true;
    }

    false
}
