use anyhow::Result;
use clap::{Parser, ValueEnum};
use jcode::message::ToolCall;
use jcode::prompt::ContextInfo;
use jcode::tui::{info_widget::InfoWidgetData, DisplayMessage, ProcessingStatus, TuiState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

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
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BenchMode {
    Idle,
    Streaming,
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
}

impl BenchState {
    fn new(turns: usize, user_len: usize, assistant_len: usize, mode: BenchMode) -> Self {
        let mut messages = Vec::with_capacity(turns * 2);
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
        }

        let is_processing = matches!(mode, BenchMode::Streaming);
        let status = if is_processing {
            ProcessingStatus::Streaming
        } else {
            ProcessingStatus::Idle
        };

        Self {
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
        }
    }
}

impl TuiState for BenchState {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.messages
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

    fn pending_soft_interrupt(&self) -> Option<&str> {
        None
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

    fn server_sessions(&self) -> Vec<String> {
        Vec::new()
    }

    fn connected_clients(&self) -> Option<usize> {
        None
    }

    fn status_notice(&self) -> Option<String> {
        None
    }

    fn animation_elapsed(&self) -> f32 {
        let elapsed = self.started_at.elapsed().as_secs_f32();
        if elapsed > 2.0 {
            2.0
        } else {
            elapsed
        }
    }

    fn rate_limit_remaining(&self) -> Option<Duration> {
        None
    }

    fn queue_mode(&self) -> bool {
        self.queue_mode
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
        0
    }
    fn pin_images(&self) -> bool {
        false
    }
    fn picker_state(&self) -> Option<&jcode::tui::PickerState> {
        None
    }

    fn working_dir(&self) -> Option<String> {
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
    let mut state = BenchState::new(args.turns, args.user_len, args.assistant_len, args.mode);
    let stream_text = make_text(args.assistant_len.max(args.stream_chunk));

    let backend = TestBackend::new(args.width, args.height);
    let mut terminal = Terminal::new(backend)?;

    let start = Instant::now();
    for frame in 0..args.frames {
        if args.scroll_cycle > 0 {
            state.scroll_offset = frame % args.scroll_cycle;
        }
        if matches!(args.mode, BenchMode::Streaming) {
            let chunk_len = ((frame + 1) * args.stream_chunk).min(stream_text.len());
            state.streaming_text = stream_text[..chunk_len].to_string();
            state.is_processing = true;
            state.status = ProcessingStatus::Streaming;
        }
        terminal.draw(|f| jcode::tui::render_frame(f, &state))?;
    }
    let elapsed = start.elapsed();

    let total_ms = elapsed.as_secs_f64() * 1000.0;
    let avg_ms = total_ms / args.frames.max(1) as f64;
    let fps = if elapsed.as_secs_f64() > 0.0 {
        args.frames as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    println!("mode: {:?}", args.mode);
    println!("frames: {}", args.frames);
    println!("total_ms: {:.2}", total_ms);
    println!("avg_ms: {:.2}", avg_ms);
    println!("fps: {:.1}", fps);

    Ok(())
}
