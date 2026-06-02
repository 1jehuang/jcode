use super::*;
use serde::Serialize;
use std::collections::{VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SLOW_DRAW_ATTRIBUTION_THRESHOLD_MS: f64 = 40.0;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct FramePerfStats {
    pub full_prep_requests: usize,
    pub full_prep_hits: usize,
    pub full_prep_oversized_hits: usize,
    pub full_prep_misses: usize,
    pub full_prep_cache_lookup_ms: f64,
    pub full_prep_build_ms: f64,
    pub full_prep_header_ms: f64,
    pub full_prep_body_ms: f64,
    pub full_prep_batch_ms: f64,
    pub full_prep_streaming_ms: f64,
    pub full_prep_compose_ms: f64,
    pub full_prep_last_path: String,
    pub full_prep_last_prepared_bytes: usize,
    pub full_prep_last_total_wrapped_lines: usize,
    pub full_prep_last_section_count: usize,
    pub body_requests: usize,
    pub body_hits: usize,
    pub body_oversized_hits: usize,
    pub body_misses: usize,
    pub body_incremental_reuses: usize,
    pub body_cache_lookup_ms: f64,
    pub body_build_ms: f64,
    pub body_incremental_build_ms: f64,
    pub body_last_path: String,
    pub body_last_incremental_base_messages: Option<usize>,
    pub body_last_prepared_bytes: usize,
    pub body_last_wrapped_lines: usize,
    pub body_last_copy_targets: usize,
    pub body_last_image_regions: usize,
    pub viewport_scroll: usize,
    pub viewport_visible_end: usize,
    pub viewport_visible_lines: usize,
    pub viewport_total_wrapped_lines: usize,
    pub viewport_prompt_preview_lines: u16,
    pub viewport_visible_user_prompts: usize,
    pub viewport_visible_copy_targets: usize,
    pub viewport_content_width: u16,
    pub viewport_stability_hash: u64,
    pub viewport_visible_streaming_hash: u64,
    pub viewport_visible_batch_progress_hash: u64,
    pub chat_area_width: u16,
    pub chat_area_height: u16,
    pub messages_area_width: u16,
    pub messages_area_height: u16,
    pub content_height: usize,
    pub initial_content_height: usize,
    pub chat_scrollbar_visible: bool,
    pub use_packed_layout: bool,
    pub has_side_panel_content: bool,
    pub has_pinned_content: bool,
    pub has_file_diff_edits: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct FrameResourceAttribution {
    pub wall_ms: f64,
    pub process_cpu_ms: Option<f64>,
    pub process_cpu_ratio: Option<f64>,
    pub process_rss_mb: Option<u64>,
    pub host_load_1m: Option<f64>,
    pub host_load_per_cpu: Option<f64>,
    pub host_cpu_count: Option<usize>,
    pub host_mem_available_mb: Option<u64>,
    pub host_mem_total_mb: Option<u64>,
    pub host_pressure: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SlowFrameSample {
    pub timestamp_ms: u64,
    pub threshold_ms: f64,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub status: String,
    pub diff_mode: String,
    pub centered: bool,
    pub is_processing: bool,
    pub auto_scroll_paused: bool,
    pub display_messages: usize,
    pub display_messages_version: u64,
    pub user_messages: usize,
    pub queued_messages: usize,
    pub streaming_text_len: usize,
    pub prepare_ms: f64,
    pub draw_ms: f64,
    pub total_ms: f64,
    pub messages_ms: Option<f64>,
    pub input_event: Option<String>,
    pub scroll_delta: Option<i32>,
    pub model_picker_open: bool,
    pub resources: FrameResourceAttribution,
    pub perf: FramePerfStats,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct FrameInputAttribution {
    pub event: Option<String>,
    pub scroll_delta: Option<i32>,
    pub model_picker_open: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DrawCallAttribution {
    pub timestamp_ms: u64,
    pub total_ms: f64,
    pub render_ms: f64,
    pub backend_flush_ms: f64,
    pub changed_cells: Option<usize>,
    pub total_cells: Option<usize>,
    pub force_full_redraw: bool,
    pub input: FrameInputAttribution,
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameResourceStart {
    process_cpu_ticks: Option<u64>,
    ticks_per_second: Option<f64>,
}

fn frame_input_attribution_slot() -> &'static Mutex<FrameInputAttribution> {
    static SLOT: OnceLock<Mutex<FrameInputAttribution>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(FrameInputAttribution::default()))
}

pub(crate) fn set_frame_input_attribution(attribution: FrameInputAttribution) {
    let mut guard = frame_input_attribution_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = attribution;
}

pub(crate) fn frame_input_attribution_snapshot() -> FrameInputAttribution {
    frame_input_attribution_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub(crate) fn record_draw_call_attribution(sample: DrawCallAttribution) {
    if sample.total_ms < SLOW_DRAW_ATTRIBUTION_THRESHOLD_MS {
        return;
    }
    if let Ok(payload) = serde_json::to_string(&sample) {
        crate::logging::warn(&format!("TUI_DRAW_CALL {}", payload));
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FullPrepPhaseMetrics {
    pub header_ms: f64,
    pub body_ms: f64,
    pub batch_ms: f64,
    pub streaming_ms: f64,
    pub compose_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FlickerFrameSample {
    pub timestamp_ms: u64,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub display_messages_version: u64,
    pub diff_mode: String,
    pub centered: bool,
    pub is_processing: bool,
    pub auto_scroll_paused: bool,
    pub scroll: usize,
    pub visible_end: usize,
    pub visible_lines: usize,
    pub total_wrapped_lines: usize,
    pub prompt_preview_lines: u16,
    pub messages_area_width: u16,
    pub messages_area_height: u16,
    pub content_width: u16,
    pub chat_scrollbar_visible: bool,
    pub visible_hash: u64,
    pub visible_streaming_hash: u64,
    pub visible_batch_progress_hash: u64,
    pub total_ms: f64,
    pub prepare_ms: f64,
    pub draw_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
struct FlickerEvent {
    pub timestamp_ms: u64,
    kind: String,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    previous: FlickerFrameSample,
    current: FlickerFrameSample,
}

#[derive(Clone, Debug)]
pub(crate) struct FlickerUiNotice {
    pub(crate) summary: String,
    pub(crate) hint: String,
}

// Keep this outside h/j/k/l for the same reason as COPY_BADGE_KEYS.
pub(super) const FLICKER_NOTICE_COPY_KEY: char = 'z';

#[derive(Default)]
struct SlowFrameHistory {
    samples: VecDeque<SlowFrameSample>,
    last_log_at_ms: Option<u64>,
}

#[derive(Default)]
struct FlickerFrameHistory {
    samples: VecDeque<FlickerFrameSample>,
    events: VecDeque<FlickerEvent>,
    last_log_at_ms: Option<u64>,
}

const SLOW_FRAME_HISTORY_MAX_SAMPLES: usize = 128;
const SLOW_FRAME_LOG_INTERVAL_MS: u64 = 1_000;
const FLICKER_HISTORY_MAX_SAMPLES: usize = 256;
const FLICKER_HISTORY_MAX_EVENTS: usize = 128;
const FLICKER_LOG_INTERVAL_MS: u64 = 500;
#[cfg(not(test))]
const FLICKER_UI_NOTICE_MAX_AGE_MS: u64 = 30_000;

static FRAME_PERF_STATS: OnceLock<Mutex<FramePerfStats>> = OnceLock::new();
static SLOW_FRAME_HISTORY: OnceLock<Mutex<SlowFrameHistory>> = OnceLock::new();
static FLICKER_FRAME_HISTORY: OnceLock<Mutex<FlickerFrameHistory>> = OnceLock::new();
static FRAME_RESOURCE_START: OnceLock<Mutex<Option<FrameResourceStart>>> = OnceLock::new();

fn frame_perf_stats() -> &'static Mutex<FramePerfStats> {
    FRAME_PERF_STATS.get_or_init(|| Mutex::new(FramePerfStats::default()))
}

fn slow_frame_history() -> &'static Mutex<SlowFrameHistory> {
    SLOW_FRAME_HISTORY.get_or_init(|| Mutex::new(SlowFrameHistory::default()))
}

fn flicker_frame_history() -> &'static Mutex<FlickerFrameHistory> {
    FLICKER_FRAME_HISTORY.get_or_init(|| Mutex::new(FlickerFrameHistory::default()))
}

fn frame_resource_start() -> &'static Mutex<Option<FrameResourceStart>> {
    FRAME_RESOURCE_START.get_or_init(|| Mutex::new(None))
}

pub(crate) fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn slow_frame_threshold_ms() -> f64 {
    static THRESHOLD_MS: OnceLock<f64> = OnceLock::new();
    *THRESHOLD_MS.get_or_init(|| {
        std::env::var("JCODE_TUI_SLOW_FRAME_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(40.0)
    })
}

fn flicker_detection_enabled() -> bool {
    #[cfg(test)]
    {
        true
    }

    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            std::env::var("JCODE_TUI_FLICKER_DETECTION")
                .ok()
                .map(|raw| {
                    matches!(
                        raw.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false)
        })
    }
}

fn with_frame_perf_stats_mut(f: impl FnOnce(&mut FramePerfStats)) {
    let mut stats = frame_perf_stats()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut stats);
}

pub(super) fn reset_frame_perf_stats() {
    with_frame_perf_stats_mut(|stats| *stats = FramePerfStats::default());
}

pub(super) fn begin_frame_resource_sample() {
    let mut start = frame_resource_start()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *start = Some(FrameResourceStart {
        process_cpu_ticks: process_cpu_ticks(),
        ticks_per_second: clock_ticks_per_second(),
    });
}

fn frame_perf_stats_snapshot() -> FramePerfStats {
    frame_perf_stats()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub(super) fn note_full_prep_request() {
    with_frame_perf_stats_mut(|stats| stats.full_prep_requests += 1);
}

pub(super) fn note_full_prep_cache_lookup(elapsed: Duration) {
    with_frame_perf_stats_mut(|stats| stats.full_prep_cache_lookup_ms += duration_ms(elapsed));
}

pub(super) fn note_full_prep_cache_hit(kind: CacheEntryKind, prepared: &PreparedChatFrame) {
    with_frame_perf_stats_mut(|stats| {
        stats.full_prep_hits += 1;
        if matches!(kind, CacheEntryKind::Oversized) {
            stats.full_prep_oversized_hits += 1;
        }
        stats.full_prep_last_path = format!("cache_hit_{}", cache_kind_label(kind));
        stats.full_prep_last_prepared_bytes = estimate_prepared_chat_frame_bytes(prepared);
        stats.full_prep_last_total_wrapped_lines = prepared.total_wrapped_lines();
        stats.full_prep_last_section_count = prepared.sections.len();
    });
}

pub(super) fn note_full_prep_cache_miss() {
    with_frame_perf_stats_mut(|stats| {
        stats.full_prep_misses += 1;
        stats.full_prep_last_path = "cache_miss".to_string();
    });
}

pub(super) fn note_full_prep_built(prepared: &PreparedChatFrame, elapsed: Duration) {
    with_frame_perf_stats_mut(|stats| {
        stats.full_prep_build_ms += duration_ms(elapsed);
        stats.full_prep_last_path = "built".to_string();
        stats.full_prep_last_prepared_bytes = estimate_prepared_chat_frame_bytes(prepared);
        stats.full_prep_last_total_wrapped_lines = prepared.total_wrapped_lines();
        stats.full_prep_last_section_count = prepared.sections.len();
    });
}

pub(super) fn note_full_prep_phase_metrics(metrics: FullPrepPhaseMetrics) {
    with_frame_perf_stats_mut(|stats| {
        stats.full_prep_header_ms += metrics.header_ms;
        stats.full_prep_body_ms += metrics.body_ms;
        stats.full_prep_batch_ms += metrics.batch_ms;
        stats.full_prep_streaming_ms += metrics.streaming_ms;
        stats.full_prep_compose_ms += metrics.compose_ms;
    });
}

pub(super) fn note_body_request() {
    with_frame_perf_stats_mut(|stats| stats.body_requests += 1);
}

pub(super) fn note_body_cache_lookup(elapsed: Duration) {
    with_frame_perf_stats_mut(|stats| stats.body_cache_lookup_ms += duration_ms(elapsed));
}

pub(super) fn note_body_cache_hit(kind: CacheEntryKind, prepared: &PreparedMessages) {
    with_frame_perf_stats_mut(|stats| {
        stats.body_hits += 1;
        if matches!(kind, CacheEntryKind::Oversized) {
            stats.body_oversized_hits += 1;
        }
        stats.body_last_path = format!("cache_hit_{}", cache_kind_label(kind));
        stats.body_last_prepared_bytes = estimate_prepared_messages_bytes(prepared);
        stats.body_last_wrapped_lines = prepared.wrapped_lines.len();
        stats.body_last_copy_targets = prepared.copy_targets.len();
        stats.body_last_image_regions = prepared.image_regions.len();
    });
}

pub(super) fn note_body_cache_miss() {
    with_frame_perf_stats_mut(|stats| {
        stats.body_misses += 1;
        stats.body_last_path = "cache_miss".to_string();
    });
}

pub(super) fn note_body_incremental_reuse(base_messages: usize) {
    with_frame_perf_stats_mut(|stats| {
        stats.body_incremental_reuses += 1;
        stats.body_last_incremental_base_messages = Some(base_messages);
    });
}

pub(super) fn note_body_built(
    prepared: &PreparedMessages,
    elapsed: Duration,
    build_path: &'static str,
) {
    with_frame_perf_stats_mut(|stats| {
        let elapsed_ms = duration_ms(elapsed);
        stats.body_build_ms += elapsed_ms;
        if build_path == "incremental" {
            stats.body_incremental_build_ms += elapsed_ms;
        }
        stats.body_last_path = build_path.to_string();
        stats.body_last_prepared_bytes = estimate_prepared_messages_bytes(prepared);
        stats.body_last_wrapped_lines = prepared.wrapped_lines.len();
        stats.body_last_copy_targets = prepared.copy_targets.len();
        stats.body_last_image_regions = prepared.image_regions.len();
    });
}

pub(super) struct ChatLayoutMetrics {
    pub chat_area: Rect,
    pub messages_area: Rect,
    pub initial_content_height: usize,
    pub content_height: usize,
    pub chat_scrollbar_visible: bool,
    pub use_packed_layout: bool,
    pub has_side_panel_content: bool,
    pub has_pinned_content: bool,
    pub has_file_diff_edits: bool,
}

pub(super) fn note_chat_layout(metrics: ChatLayoutMetrics) {
    let ChatLayoutMetrics {
        chat_area,
        messages_area,
        initial_content_height,
        content_height,
        chat_scrollbar_visible,
        use_packed_layout,
        has_side_panel_content,
        has_pinned_content,
        has_file_diff_edits,
    } = metrics;
    with_frame_perf_stats_mut(|stats| {
        stats.chat_area_width = chat_area.width;
        stats.chat_area_height = chat_area.height;
        stats.messages_area_width = messages_area.width;
        stats.messages_area_height = messages_area.height;
        stats.initial_content_height = initial_content_height;
        stats.content_height = content_height;
        stats.chat_scrollbar_visible = chat_scrollbar_visible;
        stats.use_packed_layout = use_packed_layout;
        stats.has_side_panel_content = has_side_panel_content;
        stats.has_pinned_content = has_pinned_content;
        stats.has_file_diff_edits = has_file_diff_edits;
    });
}

pub(super) struct ViewportMetrics {
    pub scroll: usize,
    pub visible_end: usize,
    pub visible_lines: usize,
    pub total_wrapped_lines: usize,
    pub prompt_preview_lines: u16,
    pub visible_user_prompts: usize,
    pub visible_copy_targets: usize,
    pub content_width: u16,
    pub stability_hash: u64,
    pub visible_streaming_hash: u64,
    pub visible_batch_progress_hash: u64,
}

pub(super) fn note_viewport_metrics(metrics: ViewportMetrics) {
    let ViewportMetrics {
        scroll,
        visible_end,
        visible_lines,
        total_wrapped_lines,
        prompt_preview_lines,
        visible_user_prompts,
        visible_copy_targets,
        content_width,
        stability_hash,
        visible_streaming_hash,
        visible_batch_progress_hash,
    } = metrics;
    with_frame_perf_stats_mut(|stats| {
        stats.viewport_scroll = scroll;
        stats.viewport_visible_end = visible_end;
        stats.viewport_visible_lines = visible_lines;
        stats.viewport_total_wrapped_lines = total_wrapped_lines;
        stats.viewport_prompt_preview_lines = prompt_preview_lines;
        stats.viewport_visible_user_prompts = visible_user_prompts;
        stats.viewport_visible_copy_targets = visible_copy_targets;
        stats.viewport_content_width = content_width;
        stats.viewport_stability_hash = stability_hash;
        stats.viewport_visible_streaming_hash = visible_streaming_hash;
        stats.viewport_visible_batch_progress_hash = visible_batch_progress_hash;
    });
}

pub(super) fn viewport_stability_hash(
    visible_lines: &[Line<'_>],
    visible_user_indices: &[usize],
    content_width: u16,
    prompt_preview_lines: u16,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    content_width.hash(&mut hasher);
    prompt_preview_lines.hash(&mut hasher);
    visible_lines.len().hash(&mut hasher);
    visible_user_indices.hash(&mut hasher);
    for line in visible_lines {
        line.alignment.hash(&mut hasher);
        line_plain_text(line).hash(&mut hasher);
    }
    hasher.finish()
}

fn same_flicker_state_key(a: &FlickerFrameSample, b: &FlickerFrameSample) -> bool {
    a.session_id == b.session_id
        && a.display_messages_version == b.display_messages_version
        && a.diff_mode == b.diff_mode
        && a.centered == b.centered
        && a.is_processing == b.is_processing
        && a.auto_scroll_paused == b.auto_scroll_paused
        && a.scroll == b.scroll
        && a.visible_end == b.visible_end
        && a.visible_lines == b.visible_lines
        && a.total_wrapped_lines == b.total_wrapped_lines
        && a.prompt_preview_lines == b.prompt_preview_lines
        && a.messages_area_width == b.messages_area_width
        && a.messages_area_height == b.messages_area_height
        && a.visible_streaming_hash == b.visible_streaming_hash
        && a.visible_batch_progress_hash == b.visible_batch_progress_hash
}

fn same_flicker_context_key(a: &FlickerFrameSample, b: &FlickerFrameSample) -> bool {
    a.session_id == b.session_id
        && a.display_messages_version == b.display_messages_version
        && a.diff_mode == b.diff_mode
        && a.centered == b.centered
        && a.is_processing == b.is_processing
        && a.auto_scroll_paused == b.auto_scroll_paused
        && a.messages_area_width == b.messages_area_width
        && a.messages_area_height == b.messages_area_height
}

fn sample_has_visible_transient_content(sample: &FlickerFrameSample) -> bool {
    sample.visible_streaming_hash != 0 || sample.visible_batch_progress_hash != 0
}

fn push_flicker_event(history: &mut FlickerFrameHistory, event: FlickerEvent) {
    history.events.push_back(event.clone());
    while history.events.len() > FLICKER_HISTORY_MAX_EVENTS {
        history.events.pop_front();
    }

    let severe = event.kind.contains("oscillation");
    let should_log = severe
        || history
            .last_log_at_ms
            .map(|last| event.timestamp_ms.saturating_sub(last) >= FLICKER_LOG_INTERVAL_MS)
            .unwrap_or(true);
    if should_log {
        history.last_log_at_ms = Some(event.timestamp_ms);
        if let Ok(payload) = serde_json::to_string(&event) {
            crate::logging::warn(&format!("TUI_FLICKER_EVENT {}", payload));
        } else {
            crate::logging::warn(&format!(
                "TUI_FLICKER_EVENT kind={} session={:?}",
                event.kind, event.session_name
            ));
        }
    }
}

fn duration_ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

fn cache_kind_label(kind: CacheEntryKind) -> &'static str {
    match kind {
        CacheEntryKind::Regular => "regular",
        CacheEntryKind::Oversized => "oversized",
    }
}

fn frame_resource_attribution(total_elapsed: Duration) -> FrameResourceAttribution {
    let start = frame_resource_start()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let end_ticks = process_cpu_ticks();
    let tick_hz = start
        .and_then(|start| start.ticks_per_second)
        .or_else(clock_ticks_per_second);
    let process_cpu_ms = match (
        start.and_then(|start| start.process_cpu_ticks),
        end_ticks,
        tick_hz,
    ) {
        (Some(start_ticks), Some(end_ticks), Some(tick_hz)) if tick_hz > 0.0 => {
            let diff = end_ticks.saturating_sub(start_ticks) as f64;
            Some((diff / tick_hz) * 1000.0)
        }
        _ => None,
    };
    let wall_ms = duration_ms(total_elapsed);
    let process_cpu_ratio = process_cpu_ms.and_then(|cpu_ms| {
        if wall_ms > 0.0 {
            Some(cpu_ms / wall_ms)
        } else {
            None
        }
    });
    let (host_load_1m, host_cpu_count) = host_load_and_cpu_count();
    let host_load_per_cpu = match (host_load_1m, host_cpu_count) {
        (Some(load), Some(cpus)) if cpus > 0 => Some(load / cpus as f64),
        _ => None,
    };
    let (host_mem_available_mb, host_mem_total_mb) = host_memory_mb();
    let host_pressure =
        classify_host_pressure(host_load_per_cpu, host_mem_available_mb, host_mem_total_mb);

    FrameResourceAttribution {
        wall_ms,
        process_cpu_ms,
        process_cpu_ratio,
        process_rss_mb: process_rss_mb(),
        host_load_1m,
        host_load_per_cpu,
        host_cpu_count,
        host_mem_available_mb,
        host_mem_total_mb,
        host_pressure,
    }
}

fn classify_host_pressure(
    load_per_cpu: Option<f64>,
    mem_available_mb: Option<u64>,
    mem_total_mb: Option<u64>,
) -> String {
    let cpu_pressure = load_per_cpu.is_some_and(|load| load >= 1.25);
    let memory_pressure = match (mem_available_mb, mem_total_mb) {
        (Some(available), Some(total)) if total > 0 => {
            available < 1024 || (available as f64 / total as f64) < 0.08
        }
        (Some(available), None) => available < 1024,
        _ => false,
    };

    match (cpu_pressure, memory_pressure) {
        (true, true) => "cpu+memory".to_string(),
        (true, false) => "cpu".to_string(),
        (false, true) => "memory".to_string(),
        (false, false) => {
            if load_per_cpu.is_none() && mem_available_mb.is_none() {
                "unknown".to_string()
            } else {
                "none".to_string()
            }
        }
    }
}

fn host_load_and_cpu_count() -> (Option<f64>, Option<usize>) {
    let load = read_loadavg_1m();
    let cpus = std::thread::available_parallelism().ok().map(|n| n.get());
    (load, cpus)
}

#[cfg(target_os = "linux")]
fn read_loadavg_1m() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn read_loadavg_1m() -> Option<f64> {
    None
}

#[cfg(target_os = "linux")]
fn host_memory_mb() -> (Option<u64>, Option<u64>) {
    let Ok(contents) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let mut available_kb = None;
    let mut total_kb = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available_kb = parse_meminfo_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = parse_meminfo_kb(rest);
        }
        if available_kb.is_some() && total_kb.is_some() {
            break;
        }
    }
    (
        available_kb.map(|kb| kb / 1024),
        total_kb.map(|kb| kb / 1024),
    )
}

#[cfg(not(target_os = "linux"))]
fn host_memory_mb() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(target_os = "linux")]
fn parse_meminfo_kb(rest: &str) -> Option<u64> {
    rest.split_whitespace().next()?.parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
fn process_rss_mb() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/self/status").ok()?;
    contents.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")
            .and_then(parse_meminfo_kb)
            .map(|kb| kb / 1024)
    })
}

#[cfg(not(target_os = "linux"))]
fn process_rss_mb() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn process_cpu_ticks() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after_comm = contents.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let user_ticks: u64 = fields.get(11)?.parse().ok()?;
    let system_ticks: u64 = fields.get(12)?.parse().ok()?;
    Some(user_ticks.saturating_add(system_ticks))
}

#[cfg(not(target_os = "linux"))]
fn process_cpu_ticks() -> Option<u64> {
    None
}

#[cfg(unix)]
fn clock_ticks_per_second() -> Option<f64> {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { Some(ticks as f64) } else { None }
}

#[cfg(not(unix))]
fn clock_ticks_per_second() -> Option<f64> {
    None
}

fn maybe_record_flicker_event(history: &mut FlickerFrameHistory, current: &FlickerFrameSample) {
    let Some(previous) = history.samples.back().cloned() else {
        return;
    };

    let len = history.samples.len();
    if len >= 2 {
        let earlier = history.samples.get(len - 2).cloned();
        if let Some(earlier) = earlier
            && same_flicker_state_key(&earlier, current)
            && same_flicker_state_key(&earlier, &previous)
            && earlier.visible_hash == current.visible_hash
            && earlier.chat_scrollbar_visible == current.chat_scrollbar_visible
            && earlier.content_width == current.content_width
            && (earlier.chat_scrollbar_visible != previous.chat_scrollbar_visible
                || earlier.content_width != previous.content_width)
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "layout_oscillation".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous,
                    current: current.clone(),
                },
            );
            return;
        }
    }

    if len >= 2 {
        let earlier = history.samples.get(len - 2).cloned();
        if let Some(earlier) = earlier
            && same_flicker_context_key(&earlier, current)
            && same_flicker_context_key(&earlier, &previous)
            && !current.auto_scroll_paused
            && earlier.visible_hash == current.visible_hash
            && earlier.content_width == current.content_width
            && earlier.chat_scrollbar_visible == current.chat_scrollbar_visible
            && (previous.visible_hash != current.visible_hash
                || previous.content_width != current.content_width
                || previous.chat_scrollbar_visible != current.chat_scrollbar_visible)
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "layout_feedback_oscillation".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous,
                    current: current.clone(),
                },
            );
            return;
        }
    }

    if same_flicker_state_key(&previous, current) {
        if previous.chat_scrollbar_visible != current.chat_scrollbar_visible
            || previous.content_width != current.content_width
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "layout_toggle_same_state".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous: previous.clone(),
                    current: current.clone(),
                },
            );
        } else if previous.visible_hash != current.visible_hash
            && !sample_has_visible_transient_content(&previous)
            && !sample_has_visible_transient_content(current)
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "visible_hash_changed_same_state".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous,
                    current: current.clone(),
                },
            );
        }
    }
}

pub(crate) fn record_flicker_frame_sample(sample: FlickerFrameSample) {
    if !flicker_detection_enabled() {
        return;
    }

    let mut history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    maybe_record_flicker_event(&mut history, &sample);
    history.samples.push_back(sample);
    while history.samples.len() > FLICKER_HISTORY_MAX_SAMPLES {
        history.samples.pop_front();
    }
}

pub(super) fn finalize_frame_metrics(
    app: &dyn TuiState,
    total_start: Instant,
    prep_elapsed: Duration,
    draw_elapsed: Duration,
    messages_ms: Option<f64>,
) {
    if profile_enabled() {
        record_profile(prep_elapsed, draw_elapsed, total_start.elapsed());
    }

    let total_elapsed = total_start.elapsed();
    let total_ms = total_elapsed.as_secs_f64() * 1000.0;
    let perf = frame_perf_stats_snapshot();
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: wall_clock_ms(),
        session_id: app.current_session_id(),
        session_name: app.session_display_name(),
        display_messages_version: app.display_messages_version(),
        diff_mode: format!("{:?}", app.diff_mode()),
        centered: app.centered_mode(),
        is_processing: app.is_processing(),
        auto_scroll_paused: app.auto_scroll_paused(),
        scroll: perf.viewport_scroll,
        visible_end: perf.viewport_visible_end,
        visible_lines: perf.viewport_visible_lines,
        total_wrapped_lines: perf.viewport_total_wrapped_lines,
        prompt_preview_lines: perf.viewport_prompt_preview_lines,
        messages_area_width: perf.messages_area_width,
        messages_area_height: perf.messages_area_height,
        content_width: perf.viewport_content_width,
        chat_scrollbar_visible: perf.chat_scrollbar_visible,
        visible_hash: perf.viewport_stability_hash,
        visible_streaming_hash: perf.viewport_visible_streaming_hash,
        visible_batch_progress_hash: perf.viewport_visible_batch_progress_hash,
        total_ms,
        prepare_ms: prep_elapsed.as_secs_f64() * 1000.0,
        draw_ms: draw_elapsed.as_secs_f64() * 1000.0,
    });

    let threshold_ms = slow_frame_threshold_ms();
    if total_ms >= threshold_ms {
        let input = frame_input_attribution_snapshot();
        record_slow_frame_sample(SlowFrameSample {
            timestamp_ms: wall_clock_ms(),
            threshold_ms,
            session_id: app.current_session_id(),
            session_name: app.session_display_name(),
            status: format!("{:?}", app.status()),
            diff_mode: format!("{:?}", app.diff_mode()),
            centered: app.centered_mode(),
            is_processing: app.is_processing(),
            auto_scroll_paused: app.auto_scroll_paused(),
            display_messages: app.display_messages().len(),
            display_messages_version: app.display_messages_version(),
            user_messages: app.display_user_message_count(),
            queued_messages: app.queued_messages().len(),
            streaming_text_len: app.streaming_text().len(),
            prepare_ms: prep_elapsed.as_secs_f64() * 1000.0,
            draw_ms: draw_elapsed.as_secs_f64() * 1000.0,
            total_ms,
            messages_ms,
            input_event: input.event,
            scroll_delta: input.scroll_delta,
            model_picker_open: input.model_picker_open,
            resources: frame_resource_attribution(total_elapsed),
            perf,
        });
    }
}

pub(crate) fn debug_flicker_frame_history(limit: usize) -> serde_json::Value {
    let history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let take_samples = limit.clamp(1, FLICKER_HISTORY_MAX_SAMPLES);
    let samples: Vec<FlickerFrameSample> = history
        .samples
        .iter()
        .rev()
        .take(take_samples)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let events: Vec<FlickerEvent> = history
        .events
        .iter()
        .rev()
        .take(limit.clamp(1, FLICKER_HISTORY_MAX_EVENTS))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    serde_json::json!({
        "enabled": flicker_detection_enabled(),
        "buffered_samples": history.samples.len(),
        "returned_samples": samples.len(),
        "buffered_events": history.events.len(),
        "returned_events": events.len(),
        "summary": {
            "layout_toggle_events": events.iter().filter(|event| event.kind == "layout_toggle_same_state").count(),
            "layout_oscillation_events": events.iter().filter(|event| event.kind == "layout_oscillation").count(),
            "layout_feedback_oscillation_events": events.iter().filter(|event| event.kind == "layout_feedback_oscillation").count(),
            "visible_hash_change_events": events.iter().filter(|event| event.kind == "visible_hash_changed_same_state").count(),
        },
        "events": events,
        "samples": samples,
    })
}

fn flicker_event_label(kind: &str) -> &str {
    match kind {
        "layout_toggle_same_state" => "layout toggle",
        "layout_oscillation" => "layout oscillation",
        "layout_feedback_oscillation" => "layout feedback oscillation",
        "visible_hash_changed_same_state" => "same-state redraw",
        _ => kind,
    }
}

fn abbreviate_flicker_log_path(path: &std::path::Path) -> String {
    let rendered = path.display().to_string();
    if let Some(home) = dirs::home_dir() {
        let home = home.display().to_string();
        if rendered == home {
            return "~".to_string();
        }
        if let Some(rest) = rendered.strip_prefix(&home) {
            return format!("~{}", rest);
        }
    }
    rendered
}

pub(crate) fn recent_flicker_ui_notice() -> Option<FlickerUiNotice> {
    if !flicker_detection_enabled() {
        return None;
    }

    let history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let event = history.events.back()?.clone();
    drop(history);

    #[cfg(not(test))]
    {
        let now = wall_clock_ms();
        if now.saturating_sub(event.timestamp_ms) > FLICKER_UI_NOTICE_MAX_AGE_MS {
            return None;
        }
    }

    let log_hint = crate::logging::log_path()
        .map(|path| abbreviate_flicker_log_path(&path))
        .unwrap_or_else(|| "~/.jcode/logs/".to_string());
    let summary = format!("⚠ flicker detected ({})", flicker_event_label(&event.kind));
    let hint = format!("logs: {} · debug: client:flicker-frames 32", log_hint);
    Some(FlickerUiNotice { summary, hint })
}

pub(crate) fn recent_flicker_copy_target_for_key(key: char) -> Option<VisibleCopyTarget> {
    if !key.eq_ignore_ascii_case(&FLICKER_NOTICE_COPY_KEY) {
        return None;
    }

    let notice = recent_flicker_ui_notice()?;
    Some(VisibleCopyTarget {
        key: FLICKER_NOTICE_COPY_KEY,
        kind_label: "flicker hint".to_string(),
        copied_notice: "Copied flicker hint".to_string(),
        content: notice.hint,
    })
}

pub(crate) fn record_slow_frame_sample(sample: SlowFrameSample) {
    let mut history = slow_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    history.samples.push_back(sample.clone());
    while history.samples.len() > SLOW_FRAME_HISTORY_MAX_SAMPLES {
        history.samples.pop_front();
    }

    let severe = sample.total_ms >= sample.threshold_ms * 2.0;
    let should_log = severe
        || history
            .last_log_at_ms
            .map(|last| sample.timestamp_ms.saturating_sub(last) >= SLOW_FRAME_LOG_INTERVAL_MS)
            .unwrap_or(true);
    if should_log {
        history.last_log_at_ms = Some(sample.timestamp_ms);
        if let Ok(payload) = serde_json::to_string(&sample) {
            crate::logging::warn(&format!("TUI_SLOW_FRAME {}", payload));
        } else {
            crate::logging::warn(&format!(
                "TUI_SLOW_FRAME total_ms={:.2} prepare_ms={:.2} draw_ms={:.2}",
                sample.total_ms, sample.prepare_ms, sample.draw_ms
            ));
        }
    }
}

pub(crate) fn debug_slow_frame_history(limit: usize) -> serde_json::Value {
    let history = slow_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let take = limit.clamp(1, SLOW_FRAME_HISTORY_MAX_SAMPLES);
    let samples: Vec<SlowFrameSample> = history
        .samples
        .iter()
        .rev()
        .take(take)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let max_total_ms = samples
        .iter()
        .map(|sample| sample.total_ms)
        .fold(0.0, f64::max);
    let max_prepare_ms = samples
        .iter()
        .map(|sample| sample.prepare_ms)
        .fold(0.0, f64::max);
    let max_draw_ms = samples
        .iter()
        .map(|sample| sample.draw_ms)
        .fold(0.0, f64::max);

    serde_json::json!({
        "threshold_ms": slow_frame_threshold_ms(),
        "buffered_samples": history.samples.len(),
        "returned_samples": samples.len(),
        "summary": {
            "max_total_ms": max_total_ms,
            "max_prepare_ms": max_prepare_ms,
            "max_draw_ms": max_draw_ms,
        },
        "samples": samples,
    })
}

#[cfg(test)]
pub(crate) fn clear_slow_frame_history_for_tests() {
    let mut history = slow_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    history.samples.clear();
    history.last_log_at_ms = None;
    reset_frame_perf_stats();
    set_last_chat_scrollbar_visible(false);
}

#[cfg(test)]
pub(crate) fn clear_flicker_frame_history_for_tests() {
    let mut history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    history.samples.clear();
    history.events.clear();
    history.last_log_at_ms = None;
    set_last_chat_scrollbar_visible(false);
}
