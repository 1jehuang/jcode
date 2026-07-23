pub use jcode_tui_markdown::{
    CopyTargetKind, IncrementalMarkdownRenderer, MERMAID_PENDING_PLACEHOLDER_TEXT,
    MarkdownDebugStats, MarkdownMemoryProfile, RawCopyTarget, center_code_blocks,
    debug_memory_profile, debug_stats, debug_stats_json, encode_handterm_latex_apc,
    extract_copy_targets_from_rendered_lines, handterm_native_latex_for_hash, highlight_file_lines,
    highlight_line, line_is_mermaid_pending_placeholder, mermaid_rendering_enabled, progress_bar,
    progress_line, recenter_structured_blocks_for_display, render_markdown, render_markdown_lazy,
    render_markdown_with_width, render_table_with_width, reset_debug_stats, set_center_code_blocks,
    thread_render_count, with_mermaid_rendering_override, wrap_line, wrap_lines,
};

fn to_markdown_diagram_mode(
    mode: crate::config::DiagramDisplayMode,
) -> jcode_tui_markdown::DiagramDisplayMode {
    match mode {
        crate::config::DiagramDisplayMode::None => jcode_tui_markdown::DiagramDisplayMode::None,
        crate::config::DiagramDisplayMode::Margin => jcode_tui_markdown::DiagramDisplayMode::Margin,
        crate::config::DiagramDisplayMode::Pinned => jcode_tui_markdown::DiagramDisplayMode::Pinned,
    }
}

fn from_markdown_diagram_mode(
    mode: jcode_tui_markdown::DiagramDisplayMode,
) -> crate::config::DiagramDisplayMode {
    match mode {
        jcode_tui_markdown::DiagramDisplayMode::None => crate::config::DiagramDisplayMode::None,
        jcode_tui_markdown::DiagramDisplayMode::Margin => crate::config::DiagramDisplayMode::Margin,
        jcode_tui_markdown::DiagramDisplayMode::Pinned => crate::config::DiagramDisplayMode::Pinned,
    }
}

fn to_markdown_spacing_mode(
    mode: crate::config::MarkdownSpacingMode,
) -> jcode_tui_markdown::MarkdownSpacingMode {
    match mode {
        crate::config::MarkdownSpacingMode::Compact => {
            jcode_tui_markdown::MarkdownSpacingMode::Compact
        }
        crate::config::MarkdownSpacingMode::Document => {
            jcode_tui_markdown::MarkdownSpacingMode::Document
        }
    }
}

fn to_markdown_latex_mode(
    mode: crate::config::LatexRenderingMode,
) -> jcode_tui_markdown::LatexRenderingMode {
    match mode {
        crate::config::LatexRenderingMode::None => jcode_tui_markdown::LatexRenderingMode::None,
        crate::config::LatexRenderingMode::Unicode => {
            jcode_tui_markdown::LatexRenderingMode::Unicode
        }
        crate::config::LatexRenderingMode::Image => jcode_tui_markdown::LatexRenderingMode::Image,
    }
}

pub fn install_jcode_markdown_hooks() {
    jcode_tui_markdown::set_latex_log_hook(|error| {
        crate::logging::warn(&format!(
            "LaTeX image rendering fell back to Unicode: {error}"
        ));
    });
    jcode_tui_markdown::set_config_snapshot_hook(|| {
        let cfg = crate::config::config();
        jcode_tui_markdown::MarkdownConfigSnapshot {
            diagram_mode: to_markdown_diagram_mode(cfg.display.diagram_mode),
            markdown_spacing: to_markdown_spacing_mode(cfg.display.markdown_spacing),
            mermaid_enabled: cfg.features.mermaid,
            latex_rendering: to_markdown_latex_mode(cfg.display.latex_rendering),
        }
    });
    jcode_tui_markdown::set_memory_snapshot_hook(|| {
        let snapshot = crate::process_memory::snapshot_with_source("client:markdown:memory");
        jcode_tui_markdown::ProcessMemorySnapshot {
            rss_bytes: snapshot.rss_bytes,
            peak_rss_bytes: snapshot.peak_rss_bytes,
            virtual_bytes: snapshot.virtual_bytes,
        }
    });
}

pub fn set_diagram_mode_override(mode: Option<crate::config::DiagramDisplayMode>) {
    jcode_tui_markdown::set_diagram_mode_override(mode.map(to_markdown_diagram_mode));
}

pub fn get_diagram_mode_override() -> Option<crate::config::DiagramDisplayMode> {
    jcode_tui_markdown::get_diagram_mode_override().map(from_markdown_diagram_mode)
}

/// Run `f` with the diagram display mode pinned on the current thread only.
/// Unlike `set_diagram_mode_override`, this never mutates process-global
/// state, so concurrent renders (and parallel tests) are unaffected.
pub fn with_diagram_mode_scope<T>(
    mode: crate::config::DiagramDisplayMode,
    f: impl FnOnce() -> T,
) -> T {
    jcode_tui_markdown::with_diagram_mode_scope(to_markdown_diagram_mode(mode), f)
}

pub fn with_deferred_mermaid_render_context<T>(f: impl FnOnce() -> T) -> T {
    jcode_tui_markdown::with_deferred_mermaid_render_context(f)
}
