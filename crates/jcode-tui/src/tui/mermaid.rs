pub use jcode_tui_mermaid::{
    DiagramBlock, DiagramCacheKey, DiagramId, DiagramInfo, DiagramOrigin, DiagramRenderProfile,
    DiagramRenderRequest, ImageStateInfo, InlineFitReadiness, MermaidCacheEntry, MermaidContent,
    MermaidDebugStats, MermaidDebugStatsDelta, MermaidFlickerBenchmark, MermaidMemoryBenchmark,
    MermaidMemoryProfile, MermaidTheme, MermaidTimingSummary, ProcessMemorySnapshot,
    RenderArtifact, RenderError, RenderMode, RenderPriority, RenderResult, RenderStatus,
    RenderTarget, ScrollFrameInfo, ScrollTestResult, TERMINAL_IMAGE_FALLBACK_NOTE,
    TestRenderResult, active_diagram_count, clear_active_diagrams, clear_cache, clear_image_state,
    clear_streaming_preview_diagram, current_preferred_aspect_ratio_bucket, debug_cache,
    debug_flicker_benchmark, debug_image_scroll_benchmark, debug_image_state,
    debug_memory_benchmark, debug_memory_profile, debug_render, debug_stats, debug_stats_json,
    debug_test_render, debug_test_resize_stability, debug_test_scroll, deferred_render_epoch,
    diagram_placeholder_lines, error_lines_for, error_to_lines, estimate_image_height,
    evict_old_cache, evict_render_cache_for_content, get_active_diagrams, get_cached_path,
    get_cached_png, get_font_size, image_protocol_available, image_widget_placeholder_markdown,
    init_picker, inline_fit_geometry, inline_fit_readiness, inline_image_dims, inline_image_id,
    inline_image_is_materialized, inline_image_placeholder_lines, inline_transcript_aspect_goal,
    inline_transcript_aspect_goal_with_font, invalidate_render_state, is_mermaid_lang,
    is_video_export_mode, materialize_inline_image, materialize_inline_image_by_id,
    normalize_aspect_ratio, parse_image_placeholder, parse_inline_image_placeholder,
    preferred_aspect_ratio_bucket, prewarm_inline_fit_state, protocol_type,
    rediscover_inline_image, register_active_diagram, register_external_image,
    register_inline_image, render_image_widget, render_image_widget_fit,
    render_image_widget_fit_stable, render_image_widget_scale, render_image_widget_viewport,
    render_image_widget_viewport_precise, render_mermaid, render_mermaid_deferred,
    render_mermaid_deferred_with_registration, render_mermaid_deferred_with_stream_scope,
    render_mermaid_sized, render_mermaid_untracked, render_pending_terminal_image_cleanup,
    reset_debug_stats, restore_active_diagrams, result_to_content, result_to_lines, set_log_hooks,
    set_memory_snapshot_hook, set_render_completed_hook, set_streaming_preview_diagram,
    set_video_export_mode, snapshot_active_diagrams, take_terminal_image_cleanup_payload,
    text_image_fallback_note_line, transcript_preferred_aspect_ratio,
    transcript_preferred_aspect_ratio_with_font, uses_text_image_fallback,
    with_image_protocol_override, with_preferred_aspect_ratio, write_video_export_marker,
};
pub use jcode_tui_mermaid::{ImageScrollBenchmark, cache_stat_syscalls};

#[cfg(feature = "mmdr-size-api")]
pub use jcode_tui_mermaid::terminal_theme;

pub fn install_jcode_mermaid_hooks() {
    jcode_tui_mermaid::set_log_hooks(crate::logging::info, crate::logging::warn);
    jcode_tui_mermaid::set_render_completed_hook(|| {
        crate::bus::Bus::global().publish(crate::bus::BusEvent::MermaidRenderCompleted);
    });
    jcode_tui_mermaid::set_memory_snapshot_hook(|| {
        let snapshot = crate::process_memory::snapshot_with_source("client:mermaid:memory");
        jcode_tui_mermaid::ProcessMemorySnapshot {
            rss_bytes: snapshot.rss_bytes,
            peak_rss_bytes: snapshot.peak_rss_bytes,
            virtual_bytes: snapshot.virtual_bytes,
        }
    });
}
