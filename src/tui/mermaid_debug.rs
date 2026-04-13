use super::*;

fn percentile_summary(samples_ms: &[f64]) -> MermaidTimingSummary {
    if samples_ms.is_empty() {
        return MermaidTimingSummary {
            avg_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            max_ms: 0.0,
        };
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile = |p: f64| {
        let rank = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[rank.min(sorted.len() - 1)]
    };
    MermaidTimingSummary {
        avg_ms: samples_ms.iter().sum::<f64>() / samples_ms.len() as f64,
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        p99_ms: percentile(0.99),
        max_ms: sorted.last().copied().unwrap_or(0.0),
    }
}

fn diff_counter(after: u64, before: u64) -> u64 {
    after.saturating_sub(before)
}

fn debug_stats_delta(
    before: &MermaidDebugStats,
    after: &MermaidDebugStats,
) -> MermaidDebugStatsDelta {
    MermaidDebugStatsDelta {
        image_state_hits: diff_counter(after.image_state_hits, before.image_state_hits),
        image_state_misses: diff_counter(after.image_state_misses, before.image_state_misses),
        skipped_renders: diff_counter(after.skipped_renders, before.skipped_renders),
        fit_state_reuse_hits: diff_counter(after.fit_state_reuse_hits, before.fit_state_reuse_hits),
        fit_protocol_rebuilds: diff_counter(
            after.fit_protocol_rebuilds,
            before.fit_protocol_rebuilds,
        ),
        viewport_state_reuse_hits: diff_counter(
            after.viewport_state_reuse_hits,
            before.viewport_state_reuse_hits,
        ),
        viewport_protocol_rebuilds: diff_counter(
            after.viewport_protocol_rebuilds,
            before.viewport_protocol_rebuilds,
        ),
        clear_operations: diff_counter(after.clear_operations, before.clear_operations),
    }
}

pub fn debug_stats() -> MermaidDebugStats {
    let mut out = if let Ok(state) = MERMAID_DEBUG.lock() {
        state.stats.clone()
    } else {
        MermaidDebugStats::default()
    };

    // Fill runtime fields
    if let Ok(cache) = RENDER_CACHE.lock() {
        out.cache_entries = cache.entries.len();
        out.cache_dir = Some(cache.cache_dir.to_string_lossy().to_string());
    }
    if let Ok(pending) = PENDING_RENDER_REQUESTS.lock() {
        out.deferred_pending = pending.len();
    }
    out.deferred_epoch = deferred_render_epoch();
    out.protocol = protocol_type().map(|p| format!("{:?}", p));
    out
}

pub fn reset_debug_stats() {
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats = MermaidDebugStats::default();
    }
}

pub fn debug_stats_json() -> Option<serde_json::Value> {
    serde_json::to_value(debug_stats()).ok()
}

pub fn debug_cache() -> Vec<MermaidCacheEntry> {
    if let Ok(cache) = RENDER_CACHE.lock() {
        return cache
            .entries
            .iter()
            .map(|(hash, diagram)| MermaidCacheEntry {
                hash: format!("{:016x}", hash),
                path: diagram.path.to_string_lossy().to_string(),
                width: diagram.width,
                height: diagram.height,
            })
            .collect();
    }
    Vec::new()
}

pub fn debug_memory_profile() -> MermaidMemoryProfile {
    let process_mem = crate::process_memory::snapshot_with_source("client:mermaid:memory");
    let mut out = MermaidMemoryProfile {
        process_rss_bytes: process_mem.rss_bytes,
        process_peak_rss_bytes: process_mem.peak_rss_bytes,
        process_virtual_bytes: process_mem.virtual_bytes,
        render_cache_limit: RENDER_CACHE_MAX,
        image_state_limit: IMAGE_STATE_MAX,
        source_cache_limit: SOURCE_CACHE_MAX,
        active_diagrams_limit: ACTIVE_DIAGRAMS_MAX,
        cache_disk_limit_bytes: CACHE_MAX_SIZE_BYTES,
        cache_disk_max_age_secs: CACHE_MAX_AGE_SECS,
        ..MermaidMemoryProfile::default()
    };

    let mut cache_dir: Option<PathBuf> = None;
    if let Ok(cache) = RENDER_CACHE.lock() {
        out.render_cache_entries = cache.entries.len();
        out.render_cache_metadata_estimate_bytes = cache
            .entries
            .values()
            .map(|diagram| {
                (std::mem::size_of::<CachedDiagram>() as u64)
                    .saturating_add(diagram.path.to_string_lossy().len() as u64)
                    .saturating_add(24)
            })
            .sum();
        cache_dir = Some(cache.cache_dir.clone());
    }

    if let Some(dir) = cache_dir.as_deref() {
        let (count, bytes) = scan_cache_dir_png_usage(dir);
        out.cache_disk_png_files = count;
        out.cache_disk_png_bytes = bytes;
    }

    if let Ok(state) = IMAGE_STATE.lock() {
        out.image_state_entries = state.entries.len();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        for (_, image_state) in state.iter() {
            if seen_paths.insert(image_state.source_path.clone())
                && let Some((w, h)) = get_png_dimensions(&image_state.source_path)
            {
                out.image_state_protocol_min_estimate_bytes = out
                    .image_state_protocol_min_estimate_bytes
                    .saturating_add(rgba_bytes_estimate(w, h));
            }
        }
    }

    if let Ok(source) = SOURCE_CACHE.lock() {
        out.source_cache_entries = source.entries.len();
        for entry in source.entries.values() {
            out.source_cache_decoded_estimate_bytes = out
                .source_cache_decoded_estimate_bytes
                .saturating_add(rgba_bytes_estimate(
                    entry.image.width(),
                    entry.image.height(),
                ));
        }
    }

    out.active_diagrams = active_diagram_count();

    out.mermaid_working_set_estimate_bytes = out
        .render_cache_metadata_estimate_bytes
        .saturating_add(out.image_state_protocol_min_estimate_bytes)
        .saturating_add(out.source_cache_decoded_estimate_bytes);

    out
}

pub fn debug_memory_benchmark(iterations: usize) -> MermaidMemoryBenchmark {
    let iterations = iterations.clamp(1, 256);
    let before = debug_memory_profile();
    let mut peak_rss = before.process_rss_bytes;
    let mut peak_working_set = before.mermaid_working_set_estimate_bytes;
    let mut errors = 0usize;

    for idx in 0..iterations {
        let content = format!(
            "flowchart TD\n    A{i}[Start {i}] --> B{i}{{Check}}\n    B{i} -->|yes| C{i}[Fast path]\n    B{i} -->|no| D{i}[Slow path]\n    C{i} --> E{i}[Done]\n    D{i} --> E{i}",
            i = idx
        );

        if matches!(
            render_mermaid_untracked(&content, Some(96)),
            RenderResult::Error(_)
        ) {
            errors += 1;
        }

        let sample = debug_memory_profile();
        peak_rss = max_opt_u64(peak_rss, sample.process_rss_bytes);
        peak_working_set = peak_working_set.max(sample.mermaid_working_set_estimate_bytes);
    }

    let after = debug_memory_profile();
    peak_rss = max_opt_u64(peak_rss, after.process_rss_bytes);
    peak_working_set = peak_working_set.max(after.mermaid_working_set_estimate_bytes);

    MermaidMemoryBenchmark {
        iterations,
        errors,
        rss_delta_bytes: diff_opt_u64(after.process_rss_bytes, before.process_rss_bytes),
        working_set_delta_bytes: diff_u64(
            after.mermaid_working_set_estimate_bytes,
            before.mermaid_working_set_estimate_bytes,
        ),
        peak_rss_bytes: peak_rss,
        peak_working_set_estimate_bytes: peak_working_set,
        before,
        after,
    }
}

pub fn debug_flicker_benchmark(steps: usize) -> MermaidFlickerBenchmark {
    init_picker();
    let protocol = protocol_type().map(|p| format!("{:?}", p));
    let protocol_supported = protocol.is_some();
    let steps = steps.clamp(4, 256);

    if !protocol_supported {
        return MermaidFlickerBenchmark {
            protocol_supported: false,
            protocol,
            steps,
            changed_viewports: 0,
            fit_frames: 0,
            viewport_frames: 0,
            fit_timing: percentile_summary(&[]),
            viewport_timing: percentile_summary(&[]),
            deltas: MermaidDebugStatsDelta::default(),
            viewport_protocol_rebuild_rate: 0.0,
            fit_protocol_rebuild_rate: 0.0,
        };
    }

    let sample = r#"flowchart LR
    A[Client] --> B[Side Panel]
    B --> C[Viewport Render]
    C --> D[Kitty Protocol]
    D --> E[Terminal]
    E --> F[Visible Frame]
    F --> G{Scroll?}
    G -->|Yes| C
    G -->|No| H[Stable]
    I[Wide diagram] --> B
    J[Large labels] --> B
    K[Resize] --> B
    L[Pan] --> C
"#;

    let hash = match render_mermaid_sized(sample, Some(140)) {
        RenderResult::Image { hash, .. } => hash,
        RenderResult::Error(_) => {
            return MermaidFlickerBenchmark {
                protocol_supported,
                protocol,
                steps,
                changed_viewports: 0,
                fit_frames: 0,
                viewport_frames: 0,
                fit_timing: percentile_summary(&[]),
                viewport_timing: percentile_summary(&[]),
                deltas: MermaidDebugStatsDelta::default(),
                viewport_protocol_rebuild_rate: 0.0,
                fit_protocol_rebuild_rate: 0.0,
            };
        }
    };

    let mut fit_samples = Vec::with_capacity(steps);
    let mut viewport_samples = Vec::with_capacity(steps);
    let before = debug_stats();
    let area = Rect::new(0, 0, 56, 18);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));

    for _ in 0..steps {
        let start = Instant::now();
        let _ = render_image_widget_scale(hash, area, &mut buf, false);
        fit_samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let mut changed_viewports = 0usize;
    let mut last_viewport: Option<(i32, i32)> = None;
    for idx in 0..steps {
        let scroll_x = (idx as i32) * 2;
        let scroll_y = (idx as i32) / 3;
        if last_viewport != Some((scroll_x, scroll_y)) {
            changed_viewports += 1;
            last_viewport = Some((scroll_x, scroll_y));
        }
        let start = Instant::now();
        let _ = render_image_widget_viewport(hash, area, &mut buf, scroll_x, scroll_y, 100, false);
        viewport_samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let after = debug_stats();
    let deltas = debug_stats_delta(&before, &after);

    MermaidFlickerBenchmark {
        protocol_supported,
        protocol,
        steps,
        changed_viewports,
        fit_frames: fit_samples.len(),
        viewport_frames: viewport_samples.len(),
        fit_timing: percentile_summary(&fit_samples),
        viewport_timing: percentile_summary(&viewport_samples),
        viewport_protocol_rebuild_rate: if changed_viewports == 0 {
            0.0
        } else {
            deltas.viewport_protocol_rebuilds as f64 / changed_viewports as f64
        },
        fit_protocol_rebuild_rate: if fit_samples.is_empty() {
            0.0
        } else {
            deltas.fit_protocol_rebuilds as f64 / fit_samples.len() as f64
        },
        deltas,
    }
}

fn scan_cache_dir_png_usage(cache_dir: &Path) -> (usize, u64) {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return (0, 0);
    };

    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "png") {
            file_count += 1;
            if let Ok(meta) = entry.metadata() {
                total_bytes = total_bytes.saturating_add(meta.len());
            }
        }
    }
    (file_count, total_bytes)
}

fn rgba_bytes_estimate(width: u32, height: u32) -> u64 {
    (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(4)
}

fn max_opt_u64(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

fn diff_u64(after: u64, before: u64) -> i64 {
    if after >= before {
        (after - before).min(i64::MAX as u64) as i64
    } else {
        -((before - after).min(i64::MAX as u64) as i64)
    }
}

fn diff_opt_u64(after: Option<u64>, before: Option<u64>) -> Option<i64> {
    match (after, before) {
        (Some(after), Some(before)) => Some(diff_u64(after, before)),
        _ => None,
    }
}

#[cfg(test)]
fn parse_proc_status_kib_line(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.trim();
    let value_kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(value_kib.saturating_mul(1024))
}

#[cfg(test)]
pub(super) fn parse_proc_status_value_bytes(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| parse_proc_status_kib_line(line, key))
}
