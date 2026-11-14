//! Cold-cache startup benchmarks (F-03 LOW finding).
//!
//! Measures the wall-clock cost of three initialization hot-paths that run
//! once per process start:
//!
//! 1. **TUI terminal setup** - `ratatui::Terminal<TestBackend>` at common sizes.
//! 2. **MCP tool registration** - individual `Tool::new()` constructors and
//!    the bulk `base_tools()` path that builds the full tool map.
//! 3. **Provider adapter creation** - constructing lightweight provider
//!    adapters that don't require network I/O.
//!
//! Run with:
//! ```sh
//! cargo bench --bench startup
//! cargo bench --bench startup -- --save-baseline before
//! ```

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// 1. TUI terminal initialization
// ---------------------------------------------------------------------------

fn bench_tui_terminal_init(c: &mut Criterion) {
    let mut group = c.benchmark_group("tui_terminal_init");

    for &(w, h) in &[(80, 24), (120, 40), (160, 50), (200, 60)] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                b.iter(|| {
                    let backend = ratatui::backend::TestBackend::new(w, h);
                    ratatui::Terminal::new(backend).unwrap()
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 2. MCP tool registration (individual constructors)
// ---------------------------------------------------------------------------

fn bench_tool_constructors(c: &mut Criterion) {
    let mut group = c.benchmark_group("tool_constructor");

    // Simple stateless tools
    group.bench_function("ReadTool::new", |b| {
        b.iter(|| jcode::tool::read::ReadTool::new());
    });
    group.bench_function("WriteTool::new", |b| {
        b.iter(|| jcode::tool::write::WriteTool::new());
    });
    group.bench_function("EditTool::new", |b| {
        b.iter(|| jcode::tool::edit::EditTool::new());
    });
    group.bench_function("MultiEditTool::new", |b| {
        b.iter(|| jcode::tool::multiedit::MultiEditTool::new());
    });
    group.bench_function("PatchTool::new", |b| {
        b.iter(|| jcode::tool::patch::PatchTool::new());
    });
    group.bench_function("ApplyPatchTool::new", |b| {
        b.iter(|| jcode::tool::apply_patch::ApplyPatchTool::new());
    });
    group.bench_function("LsTool::new", |b| {
        b.iter(|| jcode::tool::ls::LsTool::new());
    });
    group.bench_function("BashTool::new", |b| {
        b.iter(|| jcode::tool::bash::BashTool::new());
    });
    group.bench_function("BrowserTool::new", |b| {
        b.iter(|| jcode::tool::browser::BrowserTool::new());
    });
    group.bench_function("WebFetchTool::new", |b| {
        b.iter(|| jcode::tool::webfetch::WebFetchTool::new());
    });
    group.bench_function("WebSearchTool::new", |b| {
        b.iter(|| jcode::tool::websearch::WebSearchTool::new());
    });
    group.bench_function("SidePanelTool::new", |b| {
        b.iter(|| jcode::tool::side_panel::SidePanelTool::new());
    });
    group.bench_function("InvalidTool::new", |b| {
        b.iter(|| jcode::tool::invalid::InvalidTool::new());
    });
    group.bench_function("TodoTool::new", |b| {
        b.iter(|| jcode::tool::todo::TodoTool::new());
    });
    group.bench_function("BgTool::new", |b| {
        b.iter(|| jcode::tool::bg::BgTool::new());
    });
    group.bench_function("MemoryTool::new", |b| {
        b.iter(|| jcode::tool::memory::MemoryTool::new());
    });
    group.bench_function("OpenTool::new", |b| {
        b.iter(|| jcode::tool::open::OpenTool::new());
    });
    group.bench_function("JcodeDocsTool::new", |b| {
        b.iter(|| jcode::tool::jcode_docs::JcodeDocsTool::new());
    });
    group.bench_function("MaintainerFeedbackTool::new", |b| {
        b.iter(|| jcode::tool::feedback::MaintainerFeedbackTool::new());
    });

    // Tools that need extra state
    group.bench_function("SkillTool::new", |b| {
        let skills = jcode::skill::SkillRegistry::shared_registry();
        b.iter(|| jcode::tool::skill::SkillTool::new(skills.clone()));
    });

    group.bench_function("CommunicateTool::new", |b| {
        b.iter(|| jcode::tool::communicate::CommunicateTool::new());
    });

    // ToolSearchIndex (used during registry population)
    group.bench_function("ToolSearchIndex::new", |b| {
        b.iter(|| jcode::tool::tool_search::ToolSearchIndex::new());
    });

    // Simulate the search-index population loop for a typical tool set
    group.bench_function("search_index_population_30_tools", |b| {
        b.iter_batched(
            || {
                let idx = jcode::tool::tool_search::ToolSearchIndex::new();
                // 30 representative tool name+description pairs
                let entries: Vec<(String, String)> = (0..30)
                    .map(|i| (format!("tool_{i}"), format!("Description for tool number {i} that is moderately long to simulate real tool descriptions")))
                    .collect();
                (idx, entries)
            },
            |(idx, entries)| {
                for (name, desc) in &entries {
                    idx.register_name(name.clone());
                    idx.store_description(name.clone(), desc.clone());
                }
                idx
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// Aggregate benchmark: measure the cost of constructing ALL base tools
/// (the same set that `Registry::base_tools()` creates behind a OnceLock).
fn bench_all_base_tools(c: &mut Criterion) {
    c.bench_function("all_base_tools_constructors", |b| {
        b.iter(|| {
            let mut count = AtomicUsize::new(0);
            let skills = jcode::skill::SkillRegistry::shared_registry();

            let _ = jcode::tool::read::ReadTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::write::WriteTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::edit::EditTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::multiedit::MultiEditTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::patch::PatchTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::apply_patch::ApplyPatchTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::ls::LsTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::bash::BashTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::browser::BrowserTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::webfetch::WebFetchTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::websearch::WebSearchTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::invalid::InvalidTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::feedback::MaintainerFeedbackTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::jcode_docs::JcodeDocsTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::todo::TodoTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::bg::BgTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::session_search::SessionSearchTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::memory::MemoryTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::open::OpenTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::skill::SkillTool::new(skills.clone());
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::communicate::CommunicateTool::new();
            count.fetch_add(1, Ordering::Relaxed);
            let _ = jcode::tool::side_panel::SidePanelTool::new();
            count.fetch_add(1, Ordering::Relaxed);

            count.load(Ordering::Relaxed)
        });
    });
}

// ---------------------------------------------------------------------------
// 3. Provider adapter creation (no network I/O)
// ---------------------------------------------------------------------------

fn bench_provider_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("provider_creation");

    // GeminiProvider::new() is a cheap constructor (no network)
    group.bench_function("GeminiProvider::new", |b| {
        b.iter(|| jcode_provider_gemini_runtime::GeminiProvider::new());
    });

    // AnthropicProvider::new() is a cheap constructor
    group.bench_function("AnthropicProvider::new", |b| {
        b.iter(|| jcode_provider_anthropic_runtime::AnthropicProvider::new());
    });

    // OpenAIProvider::new_browser_only() avoids credential loading
    group.bench_function("OpenAIProvider::new_browser_only", |b| {
        b.iter(|| jcode_provider_openai_runtime::OpenAIProvider::new_browser_only());
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_tui_terminal_init,
    bench_tool_constructors,
    bench_all_base_tools,
    bench_provider_creation,
);
criterion_main!(benches);
