use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use jcode::message::{ContentBlock, Role, ToolCall};
use jcode::perf::{SyntheticSystemProfile, TuiPerfPolicy, tui_policy_for};
use jcode::prompt::ContextInfo;
use jcode::session::{Session, StoredDisplayRole};
use jcode::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};
use jcode::tui::{DisplayMessage, ProcessingStatus, TuiState, info_widget::InfoWidgetData};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn is_edit_tool_name(name: &str) -> bool {
    matches!(
        name,
        "write"
            | "edit"
            | "multiedit"
            | "patch"
            | "apply_patch"
            | "Write"
            | "Edit"
            | "MultiEdit"
            | "Patch"
            | "ApplyPatch"
    )
}

fn percentile_ms(samples_ms: &[f64], percentile: f64) -> f64 {
    if samples_ms.is_empty() {
        return 0.0;
    }
    let percentile = percentile.clamp(0.0, 1.0);
    let rank = ((samples_ms.len() - 1) as f64 * percentile).round() as usize;
    samples_ms[rank.min(samples_ms.len() - 1)]
}

#[derive(Debug, Clone, Default, Serialize)]
struct TimingSummary {
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SidePanelFrameProfile {
    frame: usize,
    ms: f64,
    markdown_renders: u64,
    mermaid_requests: u64,
    mermaid_cache_hits: u64,
    mermaid_cache_misses: u64,
    mermaid_render_success: u64,
    side_panel_markdown_hits: u64,
    side_panel_markdown_misses: u64,
    side_panel_render_hits: u64,
    side_panel_render_misses: u64,
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

#[derive(Debug, Clone, Default, Serialize)]
struct MermaidUiBenchmarkSummary {
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
struct TuiPolicySummary {
    source: String,
    tier: String,
    redraw_fps: u32,
    animation_fps: u32,
    enable_decorative_animations: bool,
    enable_focus_change: bool,
    enable_mouse_capture: bool,
    enable_keyboard_enhancement: bool,
    simplified_model_picker: bool,
    linked_side_panel_refresh_ms: u64,
}

fn summarize_policy(source: &str, policy: TuiPerfPolicy) -> TuiPolicySummary {
    TuiPolicySummary {
        source: source.to_string(),
        tier: policy.tier.label().to_string(),
        redraw_fps: policy.redraw_fps,
        animation_fps: policy.animation_fps,
        enable_decorative_animations: policy.enable_decorative_animations,
        enable_focus_change: policy.enable_focus_change,
        enable_mouse_capture: policy.enable_mouse_capture,
        enable_keyboard_enhancement: policy.enable_keyboard_enhancement,
        simplified_model_picker: policy.simplified_model_picker,
        linked_side_panel_refresh_ms: policy.linked_side_panel_refresh_interval.as_millis() as u64,
    }
}

fn summarize_timing(samples_ms: &[f64]) -> TimingSummary {
    if samples_ms.is_empty() {
        return TimingSummary::default();
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    TimingSummary {
        avg_ms: samples_ms.iter().sum::<f64>() / samples_ms.len() as f64,
        p50_ms: percentile_ms(&sorted, 0.50),
        p95_ms: percentile_ms(&sorted, 0.95),
        p99_ms: percentile_ms(&sorted, 0.99),
        max_ms: sorted.last().copied().unwrap_or(0.0),
    }
}

fn summarize_mermaid_ui(
    profiles: &[SidePanelFrameProfile],
    protocol_supported: bool,
    protocol: Option<String>,
) -> MermaidUiBenchmarkSummary {
    let mut elapsed_ms = 0.0;
    let mut first_worker_render_frame = None;
    let mut first_protocol_render_frame = None;
    let mut first_deferred_idle_frame = None;
    let mut saw_pending = false;
    let mut pending_frames = 0usize;
    let mut protocol_render_frames = 0usize;
    let mut protocol_rebuild_frames = 0usize;
    let mut time_to_first_worker_render_ms = None;
    let mut time_to_first_protocol_render_ms = None;
    let mut time_to_deferred_idle_ms = None;

    for profile in profiles {
        elapsed_ms += profile.ms;
        if profile.deferred_pending_after > 0 {
            saw_pending = true;
            pending_frames += 1;
        }
        if first_worker_render_frame.is_none() && profile.deferred_worker_renders > 0 {
            first_worker_render_frame = Some(profile.frame);
            time_to_first_worker_render_ms = Some(elapsed_ms);
        }
        let protocol_rendered = profile.image_state_hits > 0
            || profile.image_state_misses > 0
            || profile.fit_state_reuse_hits > 0
            || profile.fit_protocol_rebuilds > 0
            || profile.viewport_state_reuse_hits > 0
            || profile.viewport_protocol_rebuilds > 0;
        if protocol_rendered {
            protocol_render_frames += 1;
            if first_protocol_render_frame.is_none() {
                first_protocol_render_frame = Some(profile.frame);
                time_to_first_protocol_render_ms = Some(elapsed_ms);
            }
        }
        if profile.fit_protocol_rebuilds > 0 || profile.viewport_protocol_rebuilds > 0 {
            protocol_rebuild_frames += 1;
        }
        if saw_pending && first_deferred_idle_frame.is_none() && profile.deferred_pending_after == 0
        {
            first_deferred_idle_frame = Some(profile.frame);
            time_to_deferred_idle_ms = Some(elapsed_ms);
        }
    }

    MermaidUiBenchmarkSummary {
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

#[derive(Parser, Debug)]
#[command(name = "tui_bench")]
#[command(about = "Autonomous TUI render benchmark")]
struct Args {
    /// Number of frames to render
    #[arg(long, default_value = "300")]
    frames: usize,

    /// Terminal width
    #[arg(long, default_value = "120")]
    width: u16,

    /// Terminal height
    #[arg(long, default_value = "40")]
    height: u16,

    /// Number of user/assistant turns to generate
    #[arg(long, default_value = "200")]
    turns: usize,

    /// User message length (chars)
    #[arg(long, default_value = "120")]
    user_len: usize,

    /// Assistant message length (chars)
    #[arg(long, default_value = "600")]
    assistant_len: usize,

    /// Streaming chunk size (chars)
    #[arg(long, default_value = "80")]
    stream_chunk: usize,

    /// Scroll cycle length (frames)
    #[arg(long, default_value = "80")]
    scroll_cycle: usize,

    /// Benchmark mode
    #[arg(long, value_enum, default_value = "idle")]
    mode: BenchMode,

    /// Side panel content source (used with --mode side-panel)
    #[arg(long, value_enum, default_value = "managed")]
    side_panel_source: SidePanelSource,

    /// Number of mermaid blocks to generate in side panel content
    #[arg(long, default_value = "4")]
    side_panel_mermaids: usize,

    /// Load realistic benchmark content from a saved session id or path
    #[arg(long)]
    session: Option<String>,

    /// Focus a specific side-panel page when loading from a session
    #[arg(long)]
    side_panel_page: Option<String>,

    /// Max historical session messages to import into the benchmark chat column
    #[arg(long, default_value = "120")]
    session_max_messages: usize,

    /// For synthetic linked-file side-panel benches, rewrite the file every N frames
    #[arg(long, default_value = "0")]
    linked_refresh_every: usize,

    /// Exclude the first N frames when reporting steady-state metrics
    #[arg(long, default_value = "1")]
    warmup_frames: usize,

    /// Emit machine-readable JSON benchmark output
    #[arg(long, default_value_t = false)]
    json: bool,

    /// Skip proactive side-panel prewarming before the benchmark loop
    #[arg(long, default_value_t = false)]
    no_side_panel_prewarm: bool,

    /// Report policy as if running under a synthetic environment profile
    #[arg(long, value_enum)]
    synthetic_profile: Option<BenchSyntheticProfile>,

    /// Keep any existing mermaid cache instead of forcing a cold-cache benchmark start
    #[arg(long, default_value_t = false)]
    keep_mermaid_cache: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BenchMode {
    Idle,
    Streaming,
    FileDiff,
    SidePanel,
    MermaidUi,
    MermaidFlicker,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum SidePanelSource {
    Managed,
    LinkedFile,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BenchSyntheticProfile {
    Native,
    Wsl,
    WslWindowsTerminal,
}

impl BenchSyntheticProfile {
    fn to_system_profile(self) -> SyntheticSystemProfile {
        match self {
            Self::Native => SyntheticSystemProfile::Native,
            Self::Wsl => SyntheticSystemProfile::Wsl,
            Self::WslWindowsTerminal => SyntheticSystemProfile::WslWindowsTerminal,
        }
    }
}

struct BenchState {
    messages: Vec<DisplayMessage>,
    messages_version: u64,
    streaming_text: String,
    input: String,
    cursor_pos: usize,
    queued_messages: Vec<String>,
    scroll_offset: usize,
    is_processing: bool,
    status: ProcessingStatus,
    diff_mode: jcode::config::DiffDisplayMode,
    queue_mode: bool,
    context_info: ContextInfo,
    info_widget: InfoWidgetData,
    provider_name: String,
    provider_model: String,
    started_at: Instant,
    diff_pane_scroll: usize,
    diff_pane_scroll_x: i32,
    diff_pane_focus: bool,
    side_panel: SidePanelSnapshot,
    bench_file_paths: Vec<PathBuf>,
    linked_refresh_path: Option<PathBuf>,
    linked_refresh_generation: usize,
    session_source: Option<String>,
}

impl BenchState {
    fn new(
        turns: usize,
        user_len: usize,
        assistant_len: usize,
        mode: BenchMode,
        side_panel_source: SidePanelSource,
        side_panel_mermaids: usize,
    ) -> Result<Self> {
        let mut messages = Vec::with_capacity(turns * 2);
        let mut bench_file_paths = Vec::new();
        let side_panel = if matches!(mode, BenchMode::SidePanel | BenchMode::MermaidUi) {
            make_bench_side_panel(
                assistant_len.max(240),
                side_panel_source,
                side_panel_mermaids,
                &mut bench_file_paths,
            )?
        } else {
            SidePanelSnapshot::default()
        };
        for idx in 0..turns {
            let user_text = make_text(user_len);
            messages.push(DisplayMessage::user(user_text));

            let mut assistant = String::new();
            assistant.push_str("### Update\n");
            assistant.push_str(&make_text(assistant_len));
            if idx % 4 == 0 {
                assistant.push_str("\n\n```rs\nfn bench() {\n    println!(\"hello\");\n}\n```\n");
            }
            if idx % 7 == 0 {
                assistant
                    .push_str("\n\n| col | val |\n| --- | --- |\n| a   | 1   |\n| b   | 2   |\n");
            }
            messages.push(DisplayMessage::assistant(assistant));

            if matches!(mode, BenchMode::FileDiff) {
                let file_path = make_bench_file(idx, assistant_len.max(240))?;
                let file_path_str = file_path.to_string_lossy().to_string();
                bench_file_paths.push(file_path.clone());
                let tool = ToolCall {
                    id: format!("bench_edit_{idx}"),
                    name: "edit".to_string(),
                    input: json!({
                        "file_path": file_path_str,
                        "old_string": format!("target line {}", idx),
                        "new_string": format!("target line {} updated", idx),
                    }),
                    intent: None,
                };
                let tool_output = format!(
                    "{line}- target line {idx}\n{line}+ target line {idx} updated",
                    line = idx + 1,
                );
                messages.push(DisplayMessage::tool(tool_output, tool));
            }
        }

        let is_processing = matches!(mode, BenchMode::Streaming);
        let status = if is_processing {
            ProcessingStatus::Streaming
        } else {
            ProcessingStatus::Idle
        };

        Ok(Self {
            messages,
            messages_version: 1,
            streaming_text: String::new(),
            input: String::new(),
            cursor_pos: 0,
            queued_messages: Vec::new(),
            scroll_offset: 0,
            is_processing,
            status,
            diff_mode: jcode::config::DiffDisplayMode::Off,
            queue_mode: true,
            context_info: ContextInfo::default(),
            info_widget: InfoWidgetData::default(),
            provider_name: "bench".to_string(),
            provider_model: "gpt-5.2-codex".to_string(),
            started_at: Instant::now(),
            diff_pane_scroll: usize::MAX,
            diff_pane_scroll_x: 0,
            diff_pane_focus: matches!(mode, BenchMode::FileDiff | BenchMode::SidePanel),
            side_panel,
            bench_file_paths,
            linked_refresh_path: matches!(mode, BenchMode::SidePanel)
                .then(|| match side_panel_source {
                    SidePanelSource::LinkedFile => Some(
                        std::env::temp_dir()
                            .join("jcode_tui_bench")
                            .join("side_panel_linked.md"),
                    ),
                    SidePanelSource::Managed => None,
                })
                .flatten(),
            linked_refresh_generation: 0,
            session_source: None,
        })
    }

    fn from_session(
        id_or_path: &str,
        mode: BenchMode,
        focused_page_id: Option<&str>,
        max_messages: usize,
    ) -> Result<Self> {
        let session = jcode::replay::load_session(id_or_path)
            .with_context(|| format!("failed to load session '{}'", id_or_path))?;
        let mut side_panel =
            jcode::side_panel::snapshot_for_session(&session.id).unwrap_or_default();
        if side_panel.pages.is_empty() {
            side_panel = reconstruct_side_panel_snapshot_from_session(&session);
        }
        if matches!(mode, BenchMode::SidePanel) && side_panel.pages.is_empty() {
            anyhow::bail!(
                "session '{}' has no side-panel content in storage or recoverable tool history",
                session.id
            );
        }

        if let Some(page_id) = focused_page_id {
            if side_panel.pages.iter().any(|page| page.id == page_id) {
                side_panel.focused_page_id = Some(page_id.to_string());
            } else {
                anyhow::bail!(
                    "side-panel page '{}' not found in session '{}'",
                    page_id,
                    session.id
                );
            }
        } else if side_panel.focused_page_id.is_none() {
            side_panel.focused_page_id = side_panel.pages.first().map(|page| page.id.clone());
        }

        Ok(Self {
            messages: session_to_display_messages(&session, max_messages),
            messages_version: 1,
            streaming_text: String::new(),
            input: String::new(),
            cursor_pos: 0,
            queued_messages: Vec::new(),
            scroll_offset: 0,
            is_processing: matches!(mode, BenchMode::Streaming),
            status: if matches!(mode, BenchMode::Streaming) {
                ProcessingStatus::Streaming
            } else {
                ProcessingStatus::Idle
            },
            diff_mode: if matches!(mode, BenchMode::FileDiff) {
                jcode::config::DiffDisplayMode::File
            } else {
                jcode::config::DiffDisplayMode::Off
            },
            queue_mode: true,
            context_info: ContextInfo::default(),
            info_widget: InfoWidgetData::default(),
            provider_name: session
                .provider_key
                .clone()
                .unwrap_or_else(|| "session".to_string()),
            provider_model: session
                .model
                .clone()
                .unwrap_or_else(|| "session-replay".to_string()),
            started_at: Instant::now(),
            diff_pane_scroll: usize::MAX,
            diff_pane_scroll_x: 0,
            diff_pane_focus: matches!(mode, BenchMode::FileDiff | BenchMode::SidePanel),
            side_panel,
            bench_file_paths: Vec::new(),
            linked_refresh_path: None,
            linked_refresh_generation: 0,
            session_source: Some(session.id),
        })
    }

    fn simulate_linked_refresh(&mut self) -> Result<()> {
        let Some(path) = self.linked_refresh_path.as_ref() else {
            return Ok(());
        };
        let Some(page) = self.side_panel.focused_page() else {
            return Ok(());
        };
        if page.source != SidePanelPageSource::LinkedFile {
            return Ok(());
        }

        self.linked_refresh_generation += 1;
        let content = make_side_panel_refresh_content(self.linked_refresh_generation);
        fs::write(path, &content).with_context(|| {
            format!(
                "failed to rewrite linked side-panel bench file {}",
                path.display()
            )
        })?;
        let _ = jcode::side_panel::refresh_linked_page_content(&mut self.side_panel, None);
        Ok(())
    }

    fn prewarm_side_panel(&self, width: u16, height: u16) -> bool {
        jcode::tui::prewarm_focused_side_panel(
            &self.side_panel,
            width,
            height,
            40,
            jcode::tui::mermaid::protocol_type().is_some(),
            false,
        )
    }
}

impl Drop for BenchState {
    fn drop(&mut self) {
        for path in &self.bench_file_paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn make_bench_file(idx: usize, approx_len: usize) -> Result<PathBuf> {
    let base_dir = std::env::temp_dir().join("jcode_tui_bench");
    fs::create_dir_all(&base_dir).with_context(|| {
        format!(
            "failed to create TUI bench directory {}",
            base_dir.display()
        )
    })?;
    let file_path = base_dir.join(format!("file_diff_{idx}.rs"));

    let mut content = String::from("fn bench_file() {\n");
    let repeated = make_text(approx_len);
    for line_idx in 0..120 {
        if line_idx == idx % 120 {
            content.push_str(&format!(
                "    let line_{line_idx} = \"target line {idx}\";\n"
            ));
        } else {
            content.push_str(&format!("    let line_{line_idx} = \"{}\";\n", repeated));
        }
    }
    content.push_str("}\n");

    fs::write(&file_path, content)
        .with_context(|| format!("failed to write bench file {}", file_path.display()))?;
    Ok(file_path)
}

fn make_bench_side_panel(
    approx_len: usize,
    source: SidePanelSource,
    mermaid_count: usize,
    bench_file_paths: &mut Vec<PathBuf>,
) -> Result<SidePanelSnapshot> {
    let content = make_side_panel_content(approx_len, mermaid_count.max(1));
    let source_kind = match source {
        SidePanelSource::Managed => SidePanelPageSource::Managed,
        SidePanelSource::LinkedFile => SidePanelPageSource::LinkedFile,
    };

    let file_path = match source {
        SidePanelSource::Managed => std::env::temp_dir()
            .join("jcode_tui_bench")
            .join("side_panel_managed.md"),
        SidePanelSource::LinkedFile => std::env::temp_dir()
            .join("jcode_tui_bench")
            .join("side_panel_linked.md"),
    };
    fs::create_dir_all(
        file_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    )
    .with_context(|| {
        format!(
            "failed to create side-panel bench directory for {}",
            file_path.display()
        )
    })?;
    fs::write(&file_path, &content).with_context(|| {
        format!(
            "failed to write side-panel bench file {}",
            file_path.display()
        )
    })?;
    bench_file_paths.push(file_path.clone());

    Ok(SidePanelSnapshot {
        focused_page_id: Some("bench_side_panel".to_string()),
        pages: vec![SidePanelPage {
            id: "bench_side_panel".to_string(),
            title: format!(
                "Bench Side Panel ({})",
                match source {
                    SidePanelSource::Managed => "managed",
                    SidePanelSource::LinkedFile => "linked-file",
                }
            ),
            file_path: file_path.display().to_string(),
            format: SidePanelPageFormat::Markdown,
            source: source_kind,
            content,
            updated_at_ms: 1,
        }],
    })
}

fn make_side_panel_content(approx_len: usize, mermaid_count: usize) -> String {
    let mut out = String::new();
    out.push_str("# Side Panel Benchmark\n\n");
    for idx in 0..mermaid_count {
        out.push_str(&format!("## Section {}\n\n", idx + 1));
        out.push_str(&make_text(approx_len));
        out.push_str("\n\n");
        out.push_str("```mermaid\nflowchart TD\n");
        out.push_str(&format!(
            "    A{idx}[Start {idx}] --> B{idx}[Load content]\n    B{idx} --> C{idx}{{Scroll?}}\n    C{idx} -- Yes --> D{idx}[Render viewport]\n    C{idx} -- No --> E{idx}[Reuse cache]\n    D{idx} --> F{idx}[Done]\n    E{idx} --> F{idx}[Done]\n"
        ));
        out.push_str("```\n\n");
        out.push_str("- scroll interaction\n- markdown wrapping\n- image viewport rendering\n\n");
    }
    out.push_str("## Final Notes\n\n");
    for idx in 0..24 {
        out.push_str(&format!("- Bench line {:02}: {}\n", idx + 1, make_text(64)));
    }
    out
}

fn make_side_panel_refresh_content(generation: usize) -> String {
    format!(
        "# Linked Refresh Benchmark\n\nGeneration: {generation}\n\n{}\n\n```mermaid\nflowchart TD\n    A[Refresh {generation}] --> B[Read file]\n    B --> C[Update snapshot]\n    C --> D[Reuse width cache]\n```\n",
        make_text(360)
    )
}

fn session_to_display_messages(session: &Session, max_messages: usize) -> Vec<DisplayMessage> {
    let start = session.messages.len().saturating_sub(max_messages);
    let mut out = Vec::new();

    for message in session.messages.iter().skip(start) {
        let text = stored_message_visible_text(message);
        if text.trim().is_empty() {
            continue;
        }
        match message.display_role {
            Some(StoredDisplayRole::System) => {
                out.push(DisplayMessage::system(text));
                continue;
            }
            Some(StoredDisplayRole::BackgroundTask) => {
                out.push(DisplayMessage::background_task(text));
                continue;
            }
            None => {}
        }
        match message.role {
            Role::User => out.push(DisplayMessage::user(text)),
            Role::Assistant => out.push(DisplayMessage::assistant(text)),
        }
    }

    if out.is_empty() {
        out.push(DisplayMessage::assistant(format!(
            "Loaded session {} for side-panel benchmarking.",
            session.id
        )));
    }

    out
}

fn stored_message_visible_text(message: &jcode::session::StoredMessage) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } | ContentBlock::Reasoning { text } => {
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
            }
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(format!("[tool:{} {}]", name, input));
            }
            ContentBlock::ToolResult { content, .. } => {
                if !content.trim().is_empty() {
                    parts.push(content.trim().to_string());
                }
            }
            ContentBlock::Image { media_type, .. } => {
                parts.push(format!("[image:{}]", media_type));
            }
            ContentBlock::OpenAICompaction { .. } => {}
        }
    }
    parts.join("\n\n")
}

fn reconstruct_side_panel_snapshot_from_session(session: &Session) -> SidePanelSnapshot {
    use std::collections::HashMap;

    let mut pages: HashMap<String, SidePanelPage> = HashMap::new();
    let mut focused_page_id: Option<String> = None;
    let mut revision = 1u64;

    for message in &session.messages {
        for block in &message.content {
            let ContentBlock::ToolUse { name, input, .. } = block else {
                continue;
            };
            if name != "side_panel" {
                continue;
            }

            let action = input
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            match action {
                "write" | "append" => {
                    let Some(page_id) = input.get("page_id").and_then(|value| value.as_str())
                    else {
                        continue;
                    };
                    let title = input
                        .get("title")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or(page_id)
                        .to_string();
                    let content = input
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    let page = pages
                        .entry(page_id.to_string())
                        .or_insert_with(|| SidePanelPage {
                            id: page_id.to_string(),
                            title: title.clone(),
                            file_path: format!("session://{}/{}.md", session.id, page_id),
                            format: SidePanelPageFormat::Markdown,
                            source: SidePanelPageSource::Managed,
                            content: String::new(),
                            updated_at_ms: revision,
                        });
                    page.title = title;
                    if action == "append"
                        && !page.content.is_empty()
                        && !page.content.ends_with('\n')
                    {
                        page.content.push('\n');
                    }
                    if action == "append" {
                        page.content.push_str(content);
                    } else {
                        page.content = content.to_string();
                    }
                    page.updated_at_ms = revision;
                    if input
                        .get("focus")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true)
                    {
                        focused_page_id = Some(page_id.to_string());
                    }
                    revision = revision.saturating_add(1);
                }
                "load" => {
                    let page_id = input
                        .get("page_id")
                        .and_then(|value| value.as_str())
                        .or_else(|| {
                            input
                                .get("file_path")
                                .and_then(|value| value.as_str())
                                .and_then(|path| {
                                    std::path::Path::new(path)
                                        .file_stem()
                                        .and_then(|stem| stem.to_str())
                                })
                        });
                    let Some(page_id) = page_id else {
                        continue;
                    };
                    let Some(file_path) = input.get("file_path").and_then(|value| value.as_str())
                    else {
                        continue;
                    };
                    let title = input
                        .get("title")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or(page_id)
                        .to_string();
                    let content = fs::read_to_string(file_path).unwrap_or_default();
                    pages.insert(
                        page_id.to_string(),
                        SidePanelPage {
                            id: page_id.to_string(),
                            title,
                            file_path: file_path.to_string(),
                            format: SidePanelPageFormat::Markdown,
                            source: SidePanelPageSource::LinkedFile,
                            content,
                            updated_at_ms: revision,
                        },
                    );
                    if input
                        .get("focus")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true)
                    {
                        focused_page_id = Some(page_id.to_string());
                    }
                    revision = revision.saturating_add(1);
                }
                "focus" => {
                    if let Some(page_id) = input.get("page_id").and_then(|value| value.as_str()) {
                        focused_page_id = Some(page_id.to_string());
                    }
                }
                "delete" => {
                    if let Some(page_id) = input.get("page_id").and_then(|value| value.as_str()) {
                        pages.remove(page_id);
                        if focused_page_id.as_deref() == Some(page_id) {
                            focused_page_id = None;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut pages: Vec<SidePanelPage> = pages.into_values().collect();
    pages.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    if focused_page_id.is_none() {
        focused_page_id = pages.first().map(|page| page.id.clone());
    }

    SidePanelSnapshot {
        focused_page_id,
        pages,
    }
}

impl TuiState for BenchState {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.messages
    }

    fn display_user_message_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
    }

    fn has_display_edit_tool_messages(&self) -> bool {
        self.messages.iter().any(|message| {
            message
                .tool_data
                .as_ref()
                .map(|tool| is_edit_tool_name(&tool.name))
                .unwrap_or(false)
        })
    }

    fn side_pane_images(&self) -> Vec<jcode::session::RenderedImage> {
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
        self.is_processing
    }

    fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    fn interleave_message(&self) -> Option<&str> {
        None
    }

    fn pending_soft_interrupts(&self) -> &[String] {
        &[]
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn auto_scroll_paused(&self) -> bool {
        false
    }

    fn provider_name(&self) -> String {
        self.provider_name.clone()
    }

    fn provider_model(&self) -> String {
        self.provider_model.clone()
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
        None
    }

    fn subagent_status(&self) -> Option<String> {
        None
    }

    fn batch_progress(&self) -> Option<jcode::bus::BatchProgress> {
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
        false
    }

    fn is_replay(&self) -> bool {
        false
    }

    fn diff_mode(&self) -> jcode::config::DiffDisplayMode {
        self.diff_mode
    }

    fn current_session_id(&self) -> Option<String> {
        Some("bench".to_string())
    }

    fn session_display_name(&self) -> Option<String> {
        Some("bench".to_string())
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
        false
    }

    fn dictation_key_label(&self) -> Option<String> {
        None
    }

    fn animation_elapsed(&self) -> f32 {
        let elapsed = self.started_at.elapsed().as_secs_f32();
        if elapsed > 2.0 { 2.0 } else { elapsed }
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

    fn context_info(&self) -> ContextInfo {
        self.context_info.clone()
    }

    fn context_limit(&self) -> Option<usize> {
        Some(jcode::provider::DEFAULT_CONTEXT_LIMIT)
    }

    fn client_update_available(&self) -> bool {
        false
    }

    fn server_update_available(&self) -> Option<bool> {
        None
    }

    fn info_widget_data(&self) -> InfoWidgetData {
        self.info_widget.clone()
    }

    fn update_cost(&mut self) {
        // Benchmark doesn't track cost
    }

    fn render_streaming_markdown(&self, width: usize) -> Vec<ratatui::text::Line<'static>> {
        // For benchmarks, just use the standard markdown renderer
        jcode::tui::markdown::render_markdown_with_width(&self.streaming_text, Some(width))
    }

    fn centered_mode(&self) -> bool {
        false
    }

    fn auth_status(&self) -> jcode::auth::AuthStatus {
        jcode::auth::AuthStatus::default()
    }

    fn diagram_mode(&self) -> jcode::config::DiagramDisplayMode {
        jcode::config::DiagramDisplayMode::Pinned
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
        40
    }

    fn diagram_pane_animating(&self) -> bool {
        false
    }

    fn diagram_pane_enabled(&self) -> bool {
        true
    }

    fn diagram_pane_position(&self) -> jcode::config::DiagramPanePosition {
        jcode::config::DiagramPanePosition::default()
    }

    fn diagram_zoom(&self) -> u8 {
        100
    }
    fn diff_pane_scroll(&self) -> usize {
        self.diff_pane_scroll
    }
    fn diff_pane_scroll_x(&self) -> i32 {
        self.diff_pane_scroll_x
    }
    fn diff_pane_focus(&self) -> bool {
        self.diff_pane_focus
    }
    fn side_panel(&self) -> &jcode::side_panel::SidePanelSnapshot {
        &self.side_panel
    }
    fn pin_images(&self) -> bool {
        false
    }

    fn chat_native_scrollbar(&self) -> bool {
        jcode::config::config().display.native_scrollbars.chat
    }

    fn side_panel_native_scrollbar(&self) -> bool {
        jcode::config::config().display.native_scrollbars.side_panel
    }

    fn diff_line_wrap(&self) -> bool {
        true
    }
    fn inline_interactive_state(&self) -> Option<&jcode::tui::InlineInteractiveState> {
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
    ) -> Option<&std::cell::RefCell<jcode::tui::session_picker::SessionPicker>> {
        None
    }

    fn login_picker_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<jcode::tui::login_picker::LoginPicker>> {
        None
    }

    fn account_picker_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<jcode::tui::account_picker::AccountPicker>> {
        None
    }

    fn usage_overlay(
        &self,
    ) -> Option<&std::cell::RefCell<jcode::tui::usage_overlay::UsageOverlay>> {
        None
    }

    fn working_dir(&self) -> Option<String> {
        None
    }

    fn now_millis(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    fn copy_badge_ui(&self) -> jcode::tui::CopyBadgeUiState {
        jcode::tui::CopyBadgeUiState::default()
    }

    fn copy_selection_mode(&self) -> bool {
        false
    }

    fn copy_selection_range(&self) -> Option<jcode::tui::CopySelectionRange> {
        None
    }

    fn copy_selection_status(&self) -> Option<jcode::tui::CopySelectionStatus> {
        None
    }

    fn suggestion_prompts(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn cache_ttl_status(&self) -> Option<jcode::tui::CacheTtlInfo> {
        None
    }
}

fn make_text(len: usize) -> String {
    let base = "lorem ipsum dolor sit amet consectetur adipiscing elit";
    let mut out = String::with_capacity(len + base.len());
    while out.len() < len {
        out.push_str(base);
        out.push(' ');
    }
    out.truncate(len);
    out
}

fn main() -> Result<()> {
    if std::env::var("JCODE_TUI_PROFILE").is_ok() {
        jcode::logging::init();
        if let Some(path) = jcode::logging::log_path() {
            println!("profile_log: {}", path.display());
        }
    }
    let args = Args::parse();
    let mut state = if let Some(session) = args.session.as_deref() {
        BenchState::from_session(
            session,
            args.mode,
            args.side_panel_page.as_deref(),
            args.session_max_messages,
        )?
    } else {
        BenchState::new(
            args.turns,
            args.user_len,
            args.assistant_len,
            args.mode,
            args.side_panel_source,
            args.side_panel_mermaids,
        )?
    };
    let stream_text = make_text(args.assistant_len.max(args.stream_chunk));

    if matches!(args.mode, BenchMode::MermaidFlicker) {
        let result = jcode::tui::mermaid::debug_flicker_benchmark(args.frames.max(4));
        println!("mode: {:?}", args.mode);
        println!("steps: {}", result.steps);
        println!("protocol_supported: {}", result.protocol_supported);
        if let Some(protocol) = &result.protocol {
            println!("protocol: {}", protocol);
        }
        println!("fit_avg_ms: {:.2}", result.fit_timing.avg_ms);
        println!("fit_p95_ms: {:.2}", result.fit_timing.p95_ms);
        println!("viewport_avg_ms: {:.2}", result.viewport_timing.avg_ms);
        println!("viewport_p95_ms: {:.2}", result.viewport_timing.p95_ms);
        println!(
            "viewport_protocol_rebuilds: {}",
            result.deltas.viewport_protocol_rebuilds
        );
        println!(
            "viewport_state_reuse_hits: {}",
            result.deltas.viewport_state_reuse_hits
        );
        println!(
            "fit_protocol_rebuilds: {}",
            result.deltas.fit_protocol_rebuilds
        );
        println!(
            "fit_state_reuse_hits: {}",
            result.deltas.fit_state_reuse_hits
        );
        println!("clear_operations: {}", result.deltas.clear_operations);
        println!(
            "viewport_protocol_rebuild_rate: {:.4}",
            result.viewport_protocol_rebuild_rate
        );
        println!(
            "fit_protocol_rebuild_rate: {:.4}",
            result.fit_protocol_rebuild_rate
        );
        return Ok(());
    }

    if matches!(args.mode, BenchMode::FileDiff) {
        state.diff_mode = jcode::config::DiffDisplayMode::File;
    }

    let profile_mermaid_ui = matches!(args.mode, BenchMode::MermaidUi);
    let profile_side_panel = matches!(args.mode, BenchMode::SidePanel | BenchMode::MermaidUi);
    if profile_side_panel {
        jcode::tui::mermaid::init_picker();
        jcode::tui::mermaid::clear_active_diagrams();
        jcode::tui::mermaid::clear_streaming_preview_diagram();
        jcode::tui::clear_side_panel_render_caches();
        jcode::tui::reset_side_panel_debug_stats();
        jcode::tui::markdown::reset_debug_stats();
        jcode::tui::mermaid::reset_debug_stats();
        if !args.keep_mermaid_cache {
            let _ = jcode::tui::mermaid::clear_cache();
        }
        if !args.no_side_panel_prewarm {
            let _ = state.prewarm_side_panel(args.width, args.height);
        }
    }

    let backend = TestBackend::new(args.width, args.height);
    let mut terminal = Terminal::new(backend)?;

    let start = Instant::now();
    let mut frame_times_ms: Vec<f64> = Vec::with_capacity(args.frames);
    let mut side_panel_profiles: Vec<SidePanelFrameProfile> = Vec::new();
    for frame in 0..args.frames {
        if args.scroll_cycle > 0 {
            state.scroll_offset = frame % args.scroll_cycle;
            if matches!(args.mode, BenchMode::FileDiff) {
                state.diff_pane_scroll = (frame * 3) % args.scroll_cycle.max(1);
            } else if matches!(args.mode, BenchMode::SidePanel) {
                state.diff_pane_scroll = (frame * 3) % args.scroll_cycle.max(1);
                state.diff_pane_scroll_x = if frame % 2 == 0 { 0 } else { 2 };
            }
        }
        if matches!(args.mode, BenchMode::SidePanel)
            && args.linked_refresh_every > 0
            && frame > 0
            && frame % args.linked_refresh_every == 0
        {
            state.simulate_linked_refresh()?;
        }
        if matches!(args.mode, BenchMode::Streaming) {
            let chunk_len = ((frame + 1) * args.stream_chunk).min(stream_text.len());
            state.streaming_text = stream_text[..chunk_len].to_string();
            state.is_processing = true;
            state.status = ProcessingStatus::Streaming;
        }
        let markdown_before = profile_side_panel.then(jcode::tui::markdown::debug_stats);
        let mermaid_before = profile_side_panel.then(jcode::tui::mermaid::debug_stats);
        let side_panel_before = profile_side_panel.then(jcode::tui::side_panel_debug_stats);
        let frame_start = Instant::now();
        terminal.draw(|f| jcode::tui::render_frame(f, &state))?;
        let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.0;
        frame_times_ms.push(frame_ms);
        if let (Some(markdown_before), Some(mermaid_before), Some(side_panel_before)) =
            (markdown_before, mermaid_before, side_panel_before)
        {
            let markdown_after = jcode::tui::markdown::debug_stats();
            let mermaid_after = jcode::tui::mermaid::debug_stats();
            let side_panel_after = jcode::tui::side_panel_debug_stats();
            side_panel_profiles.push(SidePanelFrameProfile {
                frame,
                ms: frame_ms,
                markdown_renders: markdown_after
                    .total_renders
                    .saturating_sub(markdown_before.total_renders),
                mermaid_requests: mermaid_after
                    .total_requests
                    .saturating_sub(mermaid_before.total_requests),
                mermaid_cache_hits: mermaid_after
                    .cache_hits
                    .saturating_sub(mermaid_before.cache_hits),
                mermaid_cache_misses: mermaid_after
                    .cache_misses
                    .saturating_sub(mermaid_before.cache_misses),
                mermaid_render_success: mermaid_after
                    .render_success
                    .saturating_sub(mermaid_before.render_success),
                side_panel_markdown_hits: side_panel_after
                    .markdown_cache_hits
                    .saturating_sub(side_panel_before.markdown_cache_hits),
                side_panel_markdown_misses: side_panel_after
                    .markdown_cache_misses
                    .saturating_sub(side_panel_before.markdown_cache_misses),
                side_panel_render_hits: side_panel_after
                    .render_cache_hits
                    .saturating_sub(side_panel_before.render_cache_hits),
                side_panel_render_misses: side_panel_after
                    .render_cache_misses
                    .saturating_sub(side_panel_before.render_cache_misses),
                deferred_pending_after: mermaid_after.deferred_pending,
                deferred_enqueued: mermaid_after
                    .deferred_enqueued
                    .saturating_sub(mermaid_before.deferred_enqueued),
                deferred_deduped: mermaid_after
                    .deferred_deduped
                    .saturating_sub(mermaid_before.deferred_deduped),
                deferred_worker_renders: mermaid_after
                    .deferred_worker_renders
                    .saturating_sub(mermaid_before.deferred_worker_renders),
                image_state_hits: mermaid_after
                    .image_state_hits
                    .saturating_sub(mermaid_before.image_state_hits),
                image_state_misses: mermaid_after
                    .image_state_misses
                    .saturating_sub(mermaid_before.image_state_misses),
                fit_state_reuse_hits: mermaid_after
                    .fit_state_reuse_hits
                    .saturating_sub(mermaid_before.fit_state_reuse_hits),
                fit_protocol_rebuilds: mermaid_after
                    .fit_protocol_rebuilds
                    .saturating_sub(mermaid_before.fit_protocol_rebuilds),
                viewport_state_reuse_hits: mermaid_after
                    .viewport_state_reuse_hits
                    .saturating_sub(mermaid_before.viewport_state_reuse_hits),
                viewport_protocol_rebuilds: mermaid_after
                    .viewport_protocol_rebuilds
                    .saturating_sub(mermaid_before.viewport_protocol_rebuilds),
            });
        }
    }
    let elapsed = start.elapsed();

    let total_ms = elapsed.as_secs_f64() * 1000.0;
    let avg_ms = total_ms / args.frames.max(1) as f64;
    let fps = if elapsed.as_secs_f64() > 0.0 {
        args.frames as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    let total_summary = summarize_timing(&frame_times_ms);
    let warm_start = args.warmup_frames.min(frame_times_ms.len());
    let warm_summary = summarize_timing(&frame_times_ms[warm_start..]);
    let first_frame_ms = frame_times_ms.first().copied().unwrap_or(0.0);
    let side_panel_final_stats = profile_side_panel.then(jcode::tui::side_panel_debug_stats);
    let markdown_final_stats = profile_side_panel.then(jcode::tui::markdown::debug_stats);
    let mermaid_final_stats = profile_side_panel.then(jcode::tui::mermaid::debug_stats);
    let mermaid_ui_summary = if profile_mermaid_ui {
        Some(summarize_mermaid_ui(
            &side_panel_profiles,
            jcode::tui::mermaid::protocol_type().is_some(),
            jcode::tui::mermaid::protocol_type().map(|p| format!("{:?}", p)),
        ))
    } else {
        None
    };
    let actual_policy = summarize_policy("detected", jcode::perf::tui_policy());
    let synthetic_policy = args.synthetic_profile.map(|kind| {
        let synthetic = jcode::perf::synthetic_profile(kind.to_system_profile());
        summarize_policy(
            kind.to_system_profile().label(),
            tui_policy_for(&synthetic, &jcode::config::config().display),
        )
    });
    let cold_frame_count = side_panel_profiles
        .iter()
        .filter(|frame| {
            frame.markdown_renders > 0
                || frame.mermaid_cache_misses > 0
                || frame.mermaid_render_success > 0
                || frame.side_panel_markdown_misses > 0
                || frame.side_panel_render_misses > 0
        })
        .count();

    if args.json {
        let report = json!({
            "mode": format!("{:?}", args.mode),
            "width": args.width,
            "height": args.height,
            "frames": args.frames,
            "warmup_frames": args.warmup_frames,
            "prewarm_side_panel": profile_side_panel && !args.no_side_panel_prewarm,
            "keep_mermaid_cache": args.keep_mermaid_cache,
            "session": state.session_source,
            "session_messages": if !state.messages.is_empty() { Some(state.messages.len()) } else { None },
            "tui_policy": {
                "detected": actual_policy,
                "synthetic": synthetic_policy,
            },
            "side_panel": if profile_side_panel {
                Some(json!({
                    "pages": state.side_panel.pages.len(),
                    "focused_page": state.side_panel.focused_page_id,
                    "final_cache_stats": side_panel_final_stats,
                    "markdown_stats": markdown_final_stats,
                    "mermaid_stats": mermaid_final_stats,
                    "mermaid_ui_summary": mermaid_ui_summary,
                    "cold_frame_count": cold_frame_count,
                    "frame_profiles": side_panel_profiles,
                }))
            } else {
                None
            },
            "timing": {
                "first_frame_ms": first_frame_ms,
                "total": total_summary,
                "warm": warm_summary,
                "fps": fps,
                "avg_total_ms": avg_ms,
            }
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("mode: {:?}", args.mode);
    println!("tui_policy_source: {}", actual_policy.source);
    println!("tui_policy_tier: {}", actual_policy.tier);
    println!("tui_policy_redraw_fps: {}", actual_policy.redraw_fps);
    println!("tui_policy_animation_fps: {}", actual_policy.animation_fps);
    println!(
        "tui_policy_decorative_animations: {}",
        actual_policy.enable_decorative_animations
    );
    println!(
        "tui_policy_focus_change: {}",
        actual_policy.enable_focus_change
    );
    println!(
        "tui_policy_keyboard_enhancement: {}",
        actual_policy.enable_keyboard_enhancement
    );
    println!(
        "tui_policy_simplified_model_picker: {}",
        actual_policy.simplified_model_picker
    );
    println!(
        "tui_policy_linked_refresh_ms: {}",
        actual_policy.linked_side_panel_refresh_ms
    );
    if let Some(synthetic_policy) = &synthetic_policy {
        println!("synthetic_tui_policy_source: {}", synthetic_policy.source);
        println!("synthetic_tui_policy_tier: {}", synthetic_policy.tier);
        println!(
            "synthetic_tui_policy_redraw_fps: {}",
            synthetic_policy.redraw_fps
        );
        println!(
            "synthetic_tui_policy_animation_fps: {}",
            synthetic_policy.animation_fps
        );
        println!(
            "synthetic_tui_policy_decorative_animations: {}",
            synthetic_policy.enable_decorative_animations
        );
        println!(
            "synthetic_tui_policy_focus_change: {}",
            synthetic_policy.enable_focus_change
        );
        println!(
            "synthetic_tui_policy_keyboard_enhancement: {}",
            synthetic_policy.enable_keyboard_enhancement
        );
        println!(
            "synthetic_tui_policy_simplified_model_picker: {}",
            synthetic_policy.simplified_model_picker
        );
        println!(
            "synthetic_tui_policy_linked_refresh_ms: {}",
            synthetic_policy.linked_side_panel_refresh_ms
        );
    }
    if let Some(session_source) = &state.session_source {
        println!("session: {}", session_source);
        println!("session_messages: {}", state.messages.len());
    }
    if matches!(args.mode, BenchMode::SidePanel | BenchMode::MermaidUi) {
        println!("side_panel_source: {:?}", args.side_panel_source);
        println!("side_panel_mermaids: {}", args.side_panel_mermaids);
        println!("side_panel_pages: {}", state.side_panel.pages.len());
        println!(
            "focused_side_panel_page: {}",
            state
                .side_panel
                .focused_page_id
                .as_deref()
                .unwrap_or("none")
        );
        println!("side_panel_prewarm: {}", !args.no_side_panel_prewarm);
        println!("mermaid_cache_cold_start: {}", !args.keep_mermaid_cache);
        if let Some(summary) = &mermaid_ui_summary {
            println!("protocol_supported: {}", summary.protocol_supported);
            if let Some(protocol) = &summary.protocol {
                println!("protocol: {}", protocol);
            }
        }
    }
    println!("frames: {}", args.frames);
    println!("warmup_frames: {}", args.warmup_frames);
    println!("total_ms: {:.2}", total_ms);
    println!("avg_ms: {:.2}", avg_ms);
    println!("first_frame_ms: {:.2}", first_frame_ms);
    println!("p50_ms: {:.2}", total_summary.p50_ms);
    println!("p95_ms: {:.2}", total_summary.p95_ms);
    println!("p99_ms: {:.2}", total_summary.p99_ms);
    println!("max_ms: {:.2}", total_summary.max_ms);
    println!("warm_avg_ms: {:.2}", warm_summary.avg_ms);
    println!("warm_p95_ms: {:.2}", warm_summary.p95_ms);
    println!("warm_p99_ms: {:.2}", warm_summary.p99_ms);
    println!("fps: {:.1}", fps);
    if profile_side_panel {
        let markdown_frames = side_panel_profiles
            .iter()
            .filter(|frame| frame.markdown_renders > 0)
            .count();
        let mermaid_miss_frames = side_panel_profiles
            .iter()
            .filter(|frame| frame.mermaid_cache_misses > 0)
            .count();
        let render_miss_frames = side_panel_profiles
            .iter()
            .filter(|frame| frame.side_panel_render_misses > 0)
            .count();
        println!("cold_frames: {}", cold_frame_count);
        println!("frames_with_markdown_render: {}", markdown_frames);
        println!("frames_with_mermaid_cache_miss: {}", mermaid_miss_frames);
        println!("frames_with_render_cache_miss: {}", render_miss_frames);
        if let Some(stats) = side_panel_final_stats {
            println!(
                "side_panel_markdown_cache_hits: {}",
                stats.markdown_cache_hits
            );
            println!(
                "side_panel_markdown_cache_misses: {}",
                stats.markdown_cache_misses
            );
            println!("side_panel_render_cache_hits: {}", stats.render_cache_hits);
            println!(
                "side_panel_render_cache_misses: {}",
                stats.render_cache_misses
            );
            println!(
                "side_panel_markdown_cache_entries: {}",
                stats.markdown_cache_entries
            );
            println!(
                "side_panel_render_cache_entries: {}",
                stats.render_cache_entries
            );
        }
        if let Some(stats) = markdown_final_stats {
            println!("markdown_total_renders: {}", stats.total_renders);
        }
        if let Some(stats) = mermaid_final_stats {
            println!("mermaid_total_requests: {}", stats.total_requests);
            println!("mermaid_cache_hits: {}", stats.cache_hits);
            println!("mermaid_cache_misses: {}", stats.cache_misses);
            println!("mermaid_render_success: {}", stats.render_success);
            println!("mermaid_deferred_enqueued: {}", stats.deferred_enqueued);
            println!("mermaid_deferred_deduped: {}", stats.deferred_deduped);
            println!(
                "mermaid_deferred_worker_renders: {}",
                stats.deferred_worker_renders
            );
            println!("mermaid_image_state_hits: {}", stats.image_state_hits);
            println!("mermaid_image_state_misses: {}", stats.image_state_misses);
            println!(
                "mermaid_fit_protocol_rebuilds: {}",
                stats.fit_protocol_rebuilds
            );
            println!(
                "mermaid_viewport_protocol_rebuilds: {}",
                stats.viewport_protocol_rebuilds
            );
        }
        if let Some(summary) = mermaid_ui_summary {
            println!("mermaid_pending_frames: {}", summary.pending_frames);
            println!(
                "mermaid_protocol_render_frames: {}",
                summary.protocol_render_frames
            );
            println!(
                "mermaid_protocol_rebuild_frames: {}",
                summary.protocol_rebuild_frames
            );
            if let Some(frame) = summary.first_worker_render_frame {
                println!("mermaid_first_worker_render_frame: {}", frame);
            }
            if let Some(ms) = summary.time_to_first_worker_render_ms {
                println!("mermaid_time_to_first_worker_render_ms: {:.2}", ms);
            }
            if let Some(frame) = summary.first_protocol_render_frame {
                println!("mermaid_first_protocol_render_frame: {}", frame);
            }
            if let Some(ms) = summary.time_to_first_protocol_render_ms {
                println!("mermaid_time_to_first_protocol_render_ms: {:.2}", ms);
            }
            if let Some(frame) = summary.first_deferred_idle_frame {
                println!("mermaid_first_deferred_idle_frame: {}", frame);
            }
            if let Some(ms) = summary.time_to_deferred_idle_ms {
                println!("mermaid_time_to_deferred_idle_ms: {:.2}", ms);
            }
        }
    }

    Ok(())
}
