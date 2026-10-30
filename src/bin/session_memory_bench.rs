use clap::{Parser, ValueEnum};
use jcode::message::{ContentBlock, Role};
use jcode::process_memory;
use jcode::session::Session;
use jcode::side_panel::{
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot,
};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Parser, Debug)]
#[command(about = "Benchmark heavy-session memory attribution and process footprint")]
struct Args {
    /// Measure remote server startup and history assembly in an isolated JCODE_HOME.
    #[arg(long)]
    remote_breakdown: bool,
    /// Scenario source
    #[arg(long, value_enum, default_value = "synthetic")]
    scenario: Scenario,

    /// Saved session id or path (required for --scenario saved)
    #[arg(long)]
    session: Option<String>,

    /// Memory mode to benchmark
    #[arg(long, value_enum, default_value = "local")]
    mode: BenchMode,

    /// Synthetic turns to generate
    #[arg(long, default_value_t = 24)]
    turns: usize,

    /// Synthetic tool input size in KiB per turn
    #[arg(long, default_value_t = 4)]
    tool_input_kib: usize,

    /// Synthetic tool output size in KiB per turn
    #[arg(long, default_value_t = 48)]
    tool_output_kib: usize,

    /// Synthetic side-panel page count
    #[arg(long, default_value_t = 0)]
    side_panel_pages: usize,

    /// Synthetic side-panel content size in KiB per page
    #[arg(long, default_value_t = 32)]
    side_panel_page_kib: usize,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Scenario {
    Synthetic,
    Saved,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BenchMode {
    /// Current local steady state: canonical session + display, provider view only transient
    Local,
    /// Simulated pre-refactor duplicate steady state: keep a resident provider copy too
    Duplicated,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.remote_breakdown {
        return remote_breakdown(&args);
    }

    let process_before = process_memory::snapshot_with_source("bench:session-memory:before");
    let session = load_or_build_session(&args)?;
    let display_messages = jcode::tui::display_messages_from_session(&session);
    let side_panel = build_side_panel(&args);

    let resident_provider_messages = match args.mode {
        BenchMode::Local => Vec::new(),
        BenchMode::Duplicated => session.messages_for_provider_uncached(),
    };

    let process_after_build =
        process_memory::snapshot_with_source("bench:session-memory:after-build");

    let materialized_provider_messages = match args.mode {
        BenchMode::Local => session.messages_for_provider_uncached(),
        BenchMode::Duplicated => resident_provider_messages.clone(),
    };
    let provider_view_source = match args.mode {
        BenchMode::Local => "session_materialized",
        BenchMode::Duplicated => "resident_ui",
    };

    let client_memory = jcode::tui::transcript_memory_profile(
        &session,
        &resident_provider_messages,
        &materialized_provider_messages,
        provider_view_source,
        &display_messages,
        &side_panel,
    );

    let process_after_profile =
        process_memory::snapshot_with_source("bench:session-memory:after-profile");

    drop(materialized_provider_messages);
    let process_after_drop_transient =
        process_memory::snapshot_with_source("bench:session-memory:after-drop-transient");

    let payload = serde_json::json!({
        "scenario": match args.scenario {
            Scenario::Synthetic => "synthetic",
            Scenario::Saved => "saved",
        },
        "mode": match args.mode {
            BenchMode::Local => "local",
            BenchMode::Duplicated => "duplicated",
        },
        "config": {
            "turns": args.turns,
            "tool_input_kib": args.tool_input_kib,
            "tool_output_kib": args.tool_output_kib,
            "side_panel_pages": args.side_panel_pages,
            "side_panel_page_kib": args.side_panel_page_kib,
            "session": args.session,
        },
        "counts": {
            "session_messages": session.messages.len(),
            "display_messages": display_messages.len(),
            "resident_provider_messages": resident_provider_messages.len(),
            "side_panel_pages": side_panel.pages.len(),
        },
        "process": {
            "before": process_before,
            "after_build": process_after_build,
            "after_profile": process_after_profile,
            "after_drop_transient": process_after_drop_transient,
        },
        "client_memory": client_memory,
    });

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

#[derive(Clone)]
struct BenchProvider;

#[async_trait::async_trait]
impl jcode::provider::Provider for BenchProvider {
    async fn complete(
        &self,
        _: &[jcode::message::Message],
        _: &[jcode::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<jcode::provider::EventStream> {
        anyhow::bail!("benchmark provider does not make requests")
    }
    fn name(&self) -> &str {
        "benchmark"
    }
    fn model(&self) -> String {
        "benchmark".into()
    }
    fn fork(&self) -> std::sync::Arc<dyn jcode::provider::Provider> {
        std::sync::Arc::new(self.clone())
    }
}

fn allocated() -> anyhow::Result<u64> {
    process_memory::snapshot()
        .allocator
        .stats
        .and_then(|stats| stats.allocated_bytes)
        .ok_or_else(|| anyhow::anyhow!("run with --features dev-bins,jemalloc"))
}

fn remote_breakdown(args: &Args) -> anyhow::Result<()> {
    let id = args
        .session
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--session ID required"))?;
    if std::env::var_os("JCODE_HOME").is_none() {
        anyhow::bail!("set JCODE_HOME to an isolated copy of the session before running");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let provider: std::sync::Arc<dyn jcode::provider::Provider> =
        std::sync::Arc::new(BenchProvider);
    let registry = runtime.block_on(jcode::tool::Registry::new(provider.clone()));
    let before = allocated()?;
    let mut session = Session::load_for_remote_startup(id)?;
    let loaded = allocated()?;
    let json_bytes = serde_json::to_vec(&session)?.len();
    let message_count = session.messages.len();
    let replay_count = session.replay_events.len();
    let replay_before = allocated()?;
    let replay = session.replay_events.clone();
    let replay_cloned = allocated()?;
    drop(replay);
    let cache_before = allocated()?;
    std::hint::black_box(session.provider_messages());
    let cache_after = allocated()?;
    session.release_provider_messages_cache();
    let cache_released = allocated()?;
    let agent_before = allocated()?;
    let agent = runtime.block_on(async {
        jcode::agent::Agent::new_with_session(provider, registry, session, None)
    });
    let agent_after = allocated()?;
    let history_before = allocated()?;
    let (history, images) = agent.get_history_and_rendered_images();
    let history_after = allocated()?;
    let wire = serde_json::to_vec(&history)?;
    let wire_after = allocated()?;
    println!(
        "{}",
        serde_json::json!({
            "session_id": id, "session_json_bytes": json_bytes, "messages": message_count,
            "replay_events": replay_count, "history_messages": history.len(), "images": images.len(),
            "allocated_bytes": {"baseline": before, "remote_load": loaded,
                "replay_before": replay_before, "replay_cloned": replay_cloned, "cache_before": cache_before,
                "cache_after": cache_after, "cache_released": cache_released,
                "agent_before": agent_before, "agent_after": agent_after,
                "history_before": history_before, "history_after": history_after,
                "history_wire_after": wire_after, "history_wire_len": wire.len()}
        })
    );
    std::hint::black_box((agent, history, images, wire));
    Ok(())
}

fn load_or_build_session(args: &Args) -> anyhow::Result<Session> {
    match args.scenario {
        Scenario::Synthetic => Ok(build_synthetic_session(
            args.turns,
            args.tool_input_kib,
            args.tool_output_kib,
        )),
        Scenario::Saved => {
            let Some(value) = args.session.as_deref() else {
                anyhow::bail!("--session is required with --scenario saved");
            };
            let path = std::path::Path::new(value);
            if path.exists() {
                Session::load_from_path(path)
            } else {
                Session::load(value)
            }
        }
    }
}

fn build_synthetic_session(turns: usize, tool_input_kib: usize, tool_output_kib: usize) -> Session {
    let mut session = Session::create_with_id(
        format!("session_memory_bench_{}", std::process::id()),
        None,
        Some("session memory bench".to_string()),
    );
    let tool_input_bytes = tool_input_kib * 1024;
    let tool_output_bytes = tool_output_kib * 1024;

    for idx in 0..turns {
        session.add_message(
            Role::User,
            vec![text_block(make_blob(
                &format!("user turn {idx} - "),
                768 + (idx % 5) * 64,
            ))],
        );
        session.add_message(
            Role::Assistant,
            vec![
                text_block(make_blob(
                    &format!("assistant summary {idx} - "),
                    1024 + (idx % 7) * 96,
                )),
                ContentBlock::ToolUse {
                    id: format!("tool_{idx}"),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": make_blob(&format!("printf 'turn {idx}' && # "), tool_input_bytes),
                        "description": format!("Synthetic tool call {idx}"),
                    }), thought_signature: None, },
            ],
        );
        session.add_message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: format!("tool_{idx}"),
                content: make_blob(&format!("tool output {idx} - "), tool_output_bytes),
                is_error: None,
            }],
        );
    }

    session
}

fn build_side_panel(args: &Args) -> SidePanelSnapshot {
    if args.side_panel_pages == 0 {
        return SidePanelSnapshot::default();
    }

    let mut pages = Vec::with_capacity(args.side_panel_pages);
    for idx in 0..args.side_panel_pages {
        pages.push(SidePanelPage {
            id: format!("bench_page_{idx}"),
            title: format!("Bench Page {idx}"),
            file_path: format!("/tmp/bench_page_{idx}.md"),
            format: SidePanelPageFormat::Markdown,
            pdf_data: None,
            source: SidePanelPageSource::Managed,
            content: make_blob(
                &format!("# Bench Page {idx}\n\n"),
                args.side_panel_page_kib * 1024,
            ),
            updated_at_ms: idx as u64,
        });
    }

    SidePanelSnapshot {
        focus_revision: 0,
        focused_page_id: pages.first().map(|page| page.id.clone()),
        pages,
    }
}

fn text_block(text: String) -> ContentBlock {
    ContentBlock::Text {
        text,
        cache_control: None,
    }
}

fn make_blob(prefix: &str, target_len: usize) -> String {
    if target_len <= prefix.len() {
        return prefix[..target_len].to_string();
    }
    let mut out = String::with_capacity(target_len);
    out.push_str(prefix);
    const CHUNK: &str = "abcdefghijklmnopqrstuvwxyz0123456789 ";
    while out.len() < target_len {
        out.push_str(CHUNK);
    }
    out.truncate(target_len);
    out
}
