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
    out.render_size_backend = render_size_backend();
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
            .map(|((hash, _profile), diagram)| MermaidCacheEntry {
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
    let process_mem = crate::process_memory_snapshot();
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

/// Synthesize a base64 PNG payload of `(w, h)` pixels with a per-image gradient
/// so each benchmark image is a distinct content hash.
fn synth_png_b64(seed: u32, w: u32, h: u32) -> String {
    let mut img = image::RgbaImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let r = ((x.wrapping_add(seed)) % 256) as u8;
        let g = ((y.wrapping_add(seed.wrapping_mul(7))) % 256) as u8;
        let b = ((x ^ y).wrapping_add(seed.wrapping_mul(13)) % 256) as u8;
        *px = image::Rgba([r, g, b, 255]);
    }
    let dynimg = image::DynamicImage::ImageRgba8(img);
    let mut bytes: Vec<u8> = Vec::new();
    {
        use image::ImageEncoder as _;
        let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        let _ = encoder.write_image(dynimg.as_bytes(), w, h, image::ExtendedColorType::Rgba8);
    }
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// Headless image-scroll benchmark. Reproduces the transcript-scroll cost for a
/// session full of inline screenshots without a real terminal: it materializes
/// `images` synthetic PNGs and then drives the exact per-frame UI hot path the
/// viewport uses (`inline_fit_readiness` + `render_image_widget_fit_stable` for
/// every visible image, plus a readiness probe for every image in the
/// +/-2-viewport look-ahead prefetch band).
///
/// It is intentionally single-threaded and synchronous so the measured numbers
/// are attributable purely to the UI thread (the real app off-threads the cold
/// decode via a prewarm worker, but the *steady-state* per-frame cost measured
/// here is identical and is what "smooth scrolling" depends on).
///
/// Headline metric: `cache_stat_syscalls_per_frame`. Every render-cache lookup
/// that hits the filesystem (`path.exists()` / `read_dir`) is counted, so the
/// regression this benchmark guards - a per-frame stat syscall for every visible
/// and prefetched image - is caught objectively. It must stay ~0 in steady
/// state. `fit_protocol_rebuilds` catches cache thrash (re-decode / re-encode
/// storms) when the scroll working set exceeds the bounded caches.
pub fn debug_image_scroll_benchmark(
    images: usize,
    frames: usize,
    visible_per_frame: usize,
) -> ImageScrollBenchmark {
    // Force a Kitty picker so the stable-fit fast path (the one used for real
    // inline screenshots) is exercised even in a headless benchmark process.
    force_test_kitty_picker();

    let images = images.clamp(1, 4096);
    let frames = frames.clamp(1, 100_000);
    let visible_per_frame = visible_per_frame.clamp(1, images);

    // Placeholder geometry the viewport would compute for a fit image.
    let content_width: u16 = 100;
    let image_rows: u16 = 16;
    let image_area = Rect::new(0, 0, content_width, image_rows);
    let mut buf = Buffer::empty(image_area);

    // Materialize the synthetic transcript: each image is decoded + cached once,
    // exactly like the first time a screenshot scrolls into view.
    let mut ids: Vec<u64> = Vec::with_capacity(images);
    for seed in 0..images as u32 {
        let b64 = synth_png_b64(seed, 1280, 800);
        if let Some((id, _, _)) = materialize_inline_image("image/png", &b64) {
            ids.push(id);
        }
    }

    // A single synchronous "draw a visible image" step: the readiness probe the
    // viewport runs via ensure_drawable, then the stable-fit draw. On a cold
    // image this also performs the decode+scale+transmit inline (the work the
    // real app pushes to its prewarm worker); on a warm image it is the cheap
    // placeholder re-address that every steady-state scroll frame pays.
    let draw_visible = |buf: &mut Buffer, id: u64| {
        let _ = inline_fit_readiness(id, content_width, image_rows, true);
        let _ = render_image_widget_fit_stable(
            id,
            image_area,
            buf,
            content_width,
            image_rows,
            0,
            false,
            true,
        );
    };

    // Rows occupied per image in the simulated transcript (image + label + gaps).
    let row_stride: usize = image_rows as usize + 3;
    let viewport_rows: usize = visible_per_frame * row_stride;
    let prefetch_margin_images: usize = visible_per_frame * 2;
    let total_rows = images.saturating_mul(row_stride);

    // Warm the entire transcript once so the steady-state measurement below is
    // not polluted by unavoidable first-touch decodes. This mirrors scrolling
    // through the whole history once before settling.
    for &id in &ids {
        draw_visible(&mut buf, id);
    }

    let stat_before = super::cache_stat_syscalls();
    let stats_before = debug_stats();
    let mut frame_samples = Vec::with_capacity(frames);

    // Steady-state slow scroll: advance one transcript row per frame. Most
    // images stay visible across many frames (a real reading-speed scroll), so
    // this measures the warm per-frame hot path - the cost a regression in the
    // stat-syscall or fit-state-reuse path would inflate.
    for frame in 0..frames {
        let top_row = if total_rows > viewport_rows {
            frame % (total_rows - viewport_rows + 1)
        } else {
            0
        };
        let first_visible = top_row / row_stride;
        let last_visible = (first_visible + visible_per_frame).min(images);
        let band_start = first_visible.saturating_sub(prefetch_margin_images);
        let band_end = (last_visible + prefetch_margin_images).min(images);

        let start = Instant::now();

        // Visible images: probe + draw.
        for &id in &ids[first_visible..last_visible] {
            draw_visible(&mut buf, id);
        }
        // Prefetch band: readiness probe only (mirrors ui_viewport.rs prefetch).
        for &id in ids[band_start..first_visible]
            .iter()
            .chain(ids[last_visible..band_end].iter())
        {
            let _ = inline_fit_readiness(id, content_width, image_rows, true);
        }

        frame_samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let stat_after = super::cache_stat_syscalls();
    let stats_after = debug_stats();
    let stat_syscalls = stat_after.saturating_sub(stat_before);

    ImageScrollBenchmark {
        protocol: protocol_type().map(|p| format!("{:?}", p)),
        images,
        frames,
        visible_per_frame,
        frame_timing: percentile_summary(&frame_samples),
        cache_stat_syscalls: stat_syscalls,
        cache_stat_syscalls_per_frame: stat_syscalls as f64 / frames as f64,
        visible_draw_skips: 0,
        fit_protocol_rebuilds: stats_after
            .fit_protocol_rebuilds
            .saturating_sub(stats_before.fit_protocol_rebuilds),
        fit_state_reuse_hits: stats_after
            .fit_state_reuse_hits
            .saturating_sub(stats_before.fit_state_reuse_hits),
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
#[cfg_attr(test, allow(dead_code))]
fn parse_proc_status_kib_line(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.trim();
    let value_kib = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(value_kib.saturating_mul(1024))
}

#[cfg(test)]
#[cfg_attr(test, allow(dead_code))]
pub(super) fn parse_proc_status_value_bytes(status: &str, key: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| parse_proc_status_kib_line(line, key))
}
