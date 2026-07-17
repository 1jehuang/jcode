#[test]
fn precise_viewport_accepts_high_auto_zoom_without_panicking() {
    let area = ratatui::prelude::Rect::new(0, 0, 40, 20);
    let mut buf = ratatui::buffer::Buffer::empty(area);

    // No picker/cache is installed in this unit test, so rendering returns 0.
    // The important regression coverage is that the high-zoom precise API is
    // accepted and follows the normal graceful early-return path.
    assert_eq!(
        super::render_image_widget_viewport_precise(0xdef, area, &mut buf, 12, 0, 1000, false),
        0
    );
}

#[test]
fn viewport_crop_resize_scales_complete_zoomed_crops_to_fill_destination() {
    // A high-zoom fit-fill viewport crops a small source rectangle, then must
    // scale that crop back up to the destination cell area. Rendering it with
    // Fit caused the pane to report fit-fill while visually staying tiny.
    assert!(super::viewport_render::viewport_crop_should_scale_to_area(
        280, 180, 280, 180
    ));

    // When the requested viewport is larger than the source on an axis, the
    // crop is the whole remaining source image. That case should keep aspect
    // ratio instead of stretching a non-cropped image.
    assert!(!super::viewport_render::viewport_crop_should_scale_to_area(
        280, 120, 280, 180
    ));
    assert!(!super::viewport_render::viewport_crop_should_scale_to_area(
        200, 180, 280, 180
    ));
}

#[test]
fn preferred_aspect_ratio_context_is_scoped_and_bucketed() {
    assert_eq!(super::current_preferred_aspect_ratio_bucket(), None);

    let outer = super::with_preferred_aspect_ratio(Some(0.75), || {
        assert_eq!(super::current_preferred_aspect_ratio_bucket(), Some(750));
        super::with_preferred_aspect_ratio(Some(1.25), || {
            assert_eq!(super::current_preferred_aspect_ratio_bucket(), Some(1250));
        });
        super::current_preferred_aspect_ratio_bucket()
    });

    assert_eq!(outer, Some(750));
    assert_eq!(super::current_preferred_aspect_ratio_bucket(), None);
}

#[test]
fn preferred_aspect_ratio_adjusts_render_height_without_changing_width_bucket() {
    let (default_width, default_height) = super::calculate_render_size(6, 5, Some(80));
    let (profile_width, profile_height) = super::with_preferred_aspect_ratio(Some(0.5), || {
        super::calculate_render_size(6, 5, Some(80))
    });

    assert_eq!(profile_width, default_width);
    assert!(
        profile_height > default_height,
        "portrait side-pane aspect should request a taller render: default={default_height}, profiled={profile_height}"
    );
    assert!((profile_width / profile_height - 0.5).abs() < 0.01);
}

#[test]
fn deferred_render_supersedes_prefix_stream_updates_only() {
    let partial = "flowchart TD\nA[Start] --> B[In progress]";
    let extended = "flowchart TD\nA[Start] --> B[In progress]\nB --> C[Done]";

    assert!(super::cache_render::is_likely_stream_update(
        partial, extended
    ));
    assert!(super::cache_render::is_likely_stream_update(
        extended, partial
    ));

    assert!(!super::cache_render::is_likely_stream_update(
        "flowchart TD\nA[Start] --> B[One]",
        "flowchart TD\nA[Start] --> C[Different]",
    ));
    assert!(!super::cache_render::is_likely_stream_update(
        "flowchart TD\nA",
        "flowchart TD\nA[short]",
    ));
}

#[cfg(all(feature = "mmdr-size-api", mmdr_size_api_available))]
#[test]
fn mmdr_size_api_fits_natural_aspect_into_target_canvas() {
    let _stats_guard = render_stats_test_lock();
    super::reset_debug_stats();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Wide linear chain: natural layout is much wider than the ~4:3 target
    // box, so forcing the raw target canvas would letterbox ~80% of the PNG
    // with transparent padding above and below the ink.
    let content =
        format!("flowchart LR\nA[Start {unique}] --> B[Step] --> C[Step] --> D[Step] --> E[End]");

    let result = super::render_mermaid_untracked(&content, Some(100));
    let (width, height) = match result {
        super::RenderResult::Image { width, height, .. } => (width, height),
        super::RenderResult::Error(error) => panic!("render failed: {error}"),
    };
    let stats = super::debug_stats();

    let target_width = stats.last_target_width.expect("target width");
    let target_height = stats.last_target_height.expect("target height");
    let measured_width = stats.last_measured_width.expect("measured width");
    let measured_height = stats.last_measured_height.expect("measured height");
    let viewbox_width = stats.last_viewbox_width.unwrap_or_default();
    let viewbox_height = stats.last_viewbox_height.unwrap_or_default();
    assert!(viewbox_width > 0);
    assert!(viewbox_height > 0);

    // Output canvas must fit inside the requested box (small rounding slack).
    assert!(
        measured_width <= target_width + 1 && measured_height <= target_height + 1,
        "measured {measured_width}x{measured_height} exceeds target {target_width}x{target_height}"
    );
    // The canvas must hug the ink: output aspect matches the natural viewbox
    // aspect instead of the target box aspect (no letterboxing).
    let measured_ratio = measured_width as f64 / measured_height.max(1) as f64;
    let natural_ratio = viewbox_width as f64 / viewbox_height.max(1) as f64;
    assert!(
        (measured_ratio - natural_ratio).abs() / natural_ratio < 0.05,
        "output aspect {measured_ratio:.3} should match natural aspect {natural_ratio:.3}"
    );
    // The binding axis should reach the target box (fit, not shrink-only).
    assert!(
        measured_width + 1 >= target_width || measured_height + 1 >= target_height,
        "fit should touch the target box on one axis: measured {measured_width}x{measured_height}, target {target_width}x{target_height}"
    );
    assert_eq!(Some(width), stats.last_measured_width);
    assert_eq!(Some(height), stats.last_measured_height);
}

/// Regression guard for inline-image scroll latency.
///
/// The transcript-scroll hot path must not pay a filesystem stat syscall per
/// visible/prefetched image per frame, and steady-state re-scrolling within the
/// cache working set must not trigger Kitty fit-state rebuilds (synchronous
/// decode + scale + base64 re-transmit). Both showed up as p95/max frame spikes
/// of 11-35ms while scrolling a screenshot-heavy transcript before the fix; this
/// test pins the corrected steady-state behavior via the image-scroll benchmark.
#[test]
fn image_scroll_steady_state_has_no_per_frame_stats_or_rebuilds() {
    let _stats_guard = render_stats_test_lock();
    // 60 images > the historical fit-state cap (24); 800 frames is plenty of
    // steady-state scrolling to surface any per-frame stat/rebuild regression.
    let result = super::debug_image_scroll_benchmark(60, 800, 3);

    // Only meaningful when the Kitty stable-fit path is active (it is, because
    // the benchmark forces a Kitty picker). If for some reason it is not, the
    // readiness path reports Unsupported and there is nothing to assert.
    if result.protocol.as_deref() != Some("Kitty") {
        return;
    }

    assert_eq!(
        result.cache_stat_syscalls, 0,
        "steady-state image scroll must perform zero render-cache stat syscalls, got {} ({:.2}/frame)",
        result.cache_stat_syscalls, result.cache_stat_syscalls_per_frame
    );
    assert_eq!(
        result.fit_protocol_rebuilds, 0,
        "steady-state image scroll must not rebuild Kitty fit-state (cache thrash), got {}",
        result.fit_protocol_rebuilds
    );
    // Every visible image should hit the cheap reuse path each frame.
    assert_eq!(
        result.fit_state_reuse_hits,
        (result.frames * result.visible_per_frame) as u64,
        "expected one cheap fit-state reuse per visible image per frame"
    );
    assert_eq!(
        result.retained_source_cache_decoded_bytes, 0,
        "Kitty owns transmitted pixels, so decoded full sources should be released"
    );
    assert!(
        result.retained_image_state_source_bytes <= crate::IMAGE_STATE_MAX_SOURCE_BYTES as u64,
        "protocol source memory must remain within its byte budget"
    );
}

#[test]
fn warm_fit_draws_do_not_stat_the_render_cache() {
    let _stats_guard = render_stats_test_lock();
    use base64::Engine as _;
    use image::ImageEncoder as _;

    crate::force_test_kitty_picker();
    let image = image::RgbaImage::from_pixel(32, 16, image::Rgba([17, 29, 43, 255]));
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(image.as_raw(), 32, 16, image::ExtendedColorType::Rgba8)
        .expect("encode fixture");
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let (hash, _, _) = crate::materialize_inline_image("image/png", &b64).expect("materialize");
    let area = ratatui::layout::Rect::new(0, 0, 20, 6);
    let mut buf = ratatui::buffer::Buffer::empty(area);

    // The first expanded draw may probe disk once for a wider cached rendition.
    // Measure after that preference decision has been memoized: steady-state
    // draws must never repeat the lookup.
    crate::render_image_widget_fit(hash, area, &mut buf, false, true);
    let before = crate::cache_stat_syscalls();
    crate::render_image_widget_fit(hash, area, &mut buf, false, true);
    crate::render_image_widget_fit(hash, area, &mut buf, false, true);
    assert_eq!(
        crate::cache_stat_syscalls() - before,
        0,
        "warm draw paths must stay entirely in memory"
    );
}

/// `evict_old_cache` used to look only at `*.png`, so inline images cached in
/// their source container format (`{hash}_inline.jpg` etc.) were never evicted
/// and leaked on disk forever. The recognized-extension list must cover every
/// extension `inline_image_extension` can produce.
#[test]
fn cache_eviction_recognizes_every_inline_extension() {
    for media_type in [
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/bmp",
        "image/x-icon",
        "application/octet-stream", // falls back to "img"
    ] {
        let ext = crate::inline_image::mermaid_inline_extension_for_test(media_type);
        assert!(
            crate::CACHE_FILE_EXTENSIONS.contains(&ext),
            "extension {ext:?} (from {media_type}) is written to the cache dir \
             but would never be evicted by evict_old_cache"
        );
    }
}

/// The bounded bookkeeping insert must clear-and-restart instead of growing
/// past its cap, while still recording the newest entry.
#[test]
fn bounded_bookkeeping_insert_caps_map_growth() {
    let mut map: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    for hash in 0..(crate::RENDER_BOOKKEEPING_MAX as u64 * 2) {
        crate::bounded_bookkeeping_insert(&mut map, hash, 0);
        assert!(
            map.len() <= crate::RENDER_BOOKKEEPING_MAX,
            "bookkeeping map exceeded its cap at {} entries",
            map.len()
        );
    }
    let last = crate::RENDER_BOOKKEEPING_MAX as u64 * 2 - 1;
    assert!(map.contains_key(&last), "newest entry must survive insert");
    // Re-inserting an existing key at the cap must not clear the map.
    let before = map.len();
    crate::bounded_bookkeeping_insert(&mut map, last, 1);
    assert_eq!(map.len(), before, "existing-key update must not clear");
}

#[test]
fn source_image_cache_enforces_decoded_byte_budget() {
    let mut cache = crate::SourceImageCache::new();
    let accounted = crate::SOURCE_CACHE_MAX_BYTES / 2 + 1;
    for hash in 1..=2u64 {
        cache.insert_with_decoded_bytes(
            hash,
            std::path::PathBuf::from(format!("source-{hash}.png")),
            image::DynamicImage::new_rgba8(1, 1),
            accounted,
        );
    }
    assert!(cache.total_decoded_bytes <= crate::SOURCE_CACHE_MAX_BYTES);
    assert_eq!(cache.entries.len(), 1, "oldest decoded source should evict");
    assert!(cache.entries.contains_key(&2));

    let mut oversized = crate::SourceImageCache::new();
    oversized.insert_with_decoded_bytes(
        9,
        std::path::PathBuf::from("oversized.png"),
        image::DynamicImage::new_rgba8(1, 1),
        crate::SOURCE_CACHE_MAX_BYTES + 1,
    );
    assert_eq!(
        oversized.entries.len(),
        1,
        "single oversized source must remain drawable"
    );
}

#[test]
fn fitted_source_cache_enforces_budget_and_replaces_resize_variants() {
    let mut cache = crate::FittedSourceCache::new();
    let accounted = crate::FITTED_SOURCE_CACHE_MAX_BYTES / 2 + 1;
    for hash in 1..=2u64 {
        let key = crate::FittedSourceKey {
            hash,
            target_cols: 80,
            target_rows: 16,
            font_size: (8, 16),
        };
        cache.insert_with_decoded_bytes(
            key,
            std::path::PathBuf::from(format!("fitted-{hash}.png")),
            image::DynamicImage::new_rgba8(1, 1),
            accounted,
        );
    }
    assert!(cache.total_decoded_bytes <= crate::FITTED_SOURCE_CACHE_MAX_BYTES);
    assert_eq!(cache.entries.len(), 1, "oldest fitted source should evict");
    assert!(cache.entries.keys().any(|key| key.hash == 2));

    let replacement = crate::FittedSourceKey {
        hash: 2,
        target_cols: 96,
        target_rows: 20,
        font_size: (8, 16),
    };
    cache.insert_with_decoded_bytes(
        replacement,
        std::path::PathBuf::from("fitted-2.png"),
        image::DynamicImage::new_rgba8(1, 1),
        4,
    );
    assert_eq!(
        cache.entries.keys().filter(|key| key.hash == 2).count(),
        1,
        "terminal resize must replace rather than accumulate fitted variants"
    );
    assert!(cache.entries.contains_key(&replacement));
}

#[test]
fn fitted_scroll_crop_tracks_arbitrary_middle_rows_without_rescaling() {
    // 800x320 at 10x20 pixels/cell occupies 80x16 cells. Scrolling four
    // rows down into a five-row viewport must crop source pixels 80..180, not
    // squeeze the full image or jump to its bottom edge.
    assert_eq!(
        super::viewport_render::fitted_scroll_crop(800, 320, (10, 20), 80, 5, 4),
        Some((80, 100, 80, 5))
    );
    // The final partial row is bounded by the actual source height.
    assert_eq!(
        super::viewport_render::fitted_scroll_crop(800, 305, (10, 20), 80, 4, 15),
        Some((300, 5, 80, 1))
    );
    assert_eq!(
        super::viewport_render::fitted_scroll_crop(800, 320, (10, 20), 80, 5, 16),
        None,
        "rows entirely below the fitted image should remain blank"
    );
}

#[test]
fn image_state_cache_enforces_source_byte_budget() {
    crate::force_test_kitty_picker();
    let picker = crate::PICKER
        .get()
        .and_then(|picker| picker.as_ref())
        .expect("test picker");
    let mut cache = crate::ImageStateCache::new();
    let accounted = crate::IMAGE_STATE_MAX_SOURCE_BYTES / 2 + 1;
    for hash in 1..=2u64 {
        let protocol = picker.new_resize_protocol(image::DynamicImage::new_rgba8(1, 1));
        cache.insert(
            hash,
            crate::ImageState {
                protocol,
                source_path: std::path::PathBuf::from(format!("state-{hash}.png")),
                source_bytes: accounted,
                last_area: None,
                resize_mode: crate::ResizeMode::Fit,
                last_crop_top: false,
                last_viewport: None,
            },
        );
    }
    assert!(cache.total_source_bytes <= crate::IMAGE_STATE_MAX_SOURCE_BYTES);
    assert_eq!(cache.entries.len(), 1, "oldest protocol state should evict");
    assert!(cache.entries.contains_key(&2));
}

#[test]
fn png_dimension_probe_needs_only_the_header() {
    let path = std::env::temp_dir().join(format!(
        "jcode-png-header-{}-{}.png",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let mut header = [0u8; 24];
    header[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    header[16..20].copy_from_slice(&123u32.to_be_bytes());
    header[20..24].copy_from_slice(&456u32.to_be_bytes());
    std::fs::write(&path, header).expect("write header fixture");
    assert_eq!(crate::get_png_dimensions(&path), Some((123, 456)));
    let _ = std::fs::remove_file(path);
}

/// Inline-fit geometry must preserve aspect ratio, respect the row cap, and
/// return a marker-parsable placeholder that survives leading padding spans
/// (centered mode inserts one).
#[test]
fn inline_fit_geometry_and_marker_roundtrip() {
    use ratatui::style::Style;
    use ratatui::text::Span;

    // Wide image at 80 cells: width-bound, well under the cap.
    let (rows, cols) = crate::inline_fit_geometry(1600, 400, 80, crate::INLINE_DIAGRAM_MAX_ROWS);
    assert!(rows >= crate::INLINE_FIT_MIN_ROWS);
    assert!(rows < crate::INLINE_DIAGRAM_MAX_ROWS);
    assert!(cols <= 80);

    // Tall image: height-bound by the cap.
    let (tall_rows, _) = crate::inline_fit_geometry(400, 40_000, 80, 20);
    assert_eq!(tall_rows, 20);

    // Placeholder lines round-trip through the parser.
    let lines = crate::inline_image_placeholder_lines(0xabcdef, rows, cols);
    assert_eq!(lines.len(), rows as usize);
    let parsed = crate::parse_inline_image_placeholder(&lines[0]);
    assert_eq!(parsed, Some((0xabcdef, rows, cols)));

    // A leading whitespace span (centered-mode padding) must not break parsing.
    let mut padded = lines[0].clone();
    padded
        .spans
        .insert(0, Span::styled("    ", Style::default()));
    assert_eq!(
        crate::parse_inline_image_placeholder(&padded),
        Some((0xabcdef, rows, cols)),
        "padded marker line must still parse"
    );
}

/// Inline transcript renders get a terminal-friendly aspect goal derived from
/// the chat geometry, bucketed through the existing per-mille RenderProfile
/// machinery so cache keys stay coarse.
#[test]
fn inline_transcript_aspect_goal_produces_expected_bucketed_profile() {
    // Wide terminal: 120 cols x 40 rows at an 8x16 cell.
    // width_px = (120-2)*8 = 944; goal_rows = (40-4)=36 -> 36*16 = 576 px.
    // raw = 944/576 ~= 1.639 -> quantized to 1.75.
    let goal = crate::inline_transcript_aspect_goal_with_font(120, 40, Some((8, 16)));
    assert_eq!(goal, Some(1.75));

    // The goal flows through the standard profile bucketing (per-mille).
    let bucket =
        crate::with_preferred_aspect_ratio(goal, || crate::current_preferred_aspect_ratio_bucket());
    assert_eq!(bucket, Some(1750));

    // Narrow terminals floor at the 4:3 sizing default instead of requesting
    // portrait renders.
    let narrow = crate::inline_transcript_aspect_goal_with_font(40, 50, Some((8, 16)));
    assert!(
        (narrow.unwrap() - 4.0 / 3.0).abs() < 1e-6,
        "narrow terminal must floor at 4:3, got {narrow:?}"
    );

    // Very wide, short terminals cap at the flatness limit.
    let flat = crate::inline_transcript_aspect_goal_with_font(500, 12, Some((8, 16)));
    assert_eq!(flat, Some(6.0));
}

/// Coarse 0.25-step quantization means one-cell resize jitter does not mint a
/// new aspect bucket (and thus does not re-render cached diagrams). Jitter can
/// still cross a step boundary occasionally, but within a step it is stable;
/// these cases sit inside one step.
#[test]
fn inline_transcript_aspect_goal_is_stable_under_resize_jitter() {
    let a = crate::inline_transcript_aspect_goal_with_font(120, 40, Some((8, 16)));
    let b = crate::inline_transcript_aspect_goal_with_font(121, 40, Some((8, 16)));
    let c = crate::inline_transcript_aspect_goal_with_font(120, 39, Some((8, 16)));
    assert_eq!(a, b, "1-col width jitter must stay in the same aspect step");
    assert_eq!(
        a, c,
        "1-row height jitter must stay in the same aspect step"
    );

    let bucket_a = crate::preferred_aspect_ratio_bucket(a);
    let bucket_b = crate::preferred_aspect_ratio_bucket(b);
    assert_eq!(bucket_a, bucket_b);
}

/// Unknown font/terminal geometry keeps today's behavior: no aspect goal.
#[test]
fn inline_transcript_aspect_goal_no_geometry_yields_none() {
    assert_eq!(
        crate::inline_transcript_aspect_goal_with_font(120, 40, None),
        None,
        "unknown font size must not invent an aspect goal"
    );
    assert_eq!(
        crate::inline_transcript_aspect_goal_with_font(0, 40, Some((8, 16))),
        None,
        "zero-width chat area must not produce a goal"
    );
    assert_eq!(
        crate::inline_transcript_aspect_goal_with_font(120, 0, Some((8, 16))),
        None,
        "zero-height chat area must not produce a goal"
    );
}

/// The transcript profile must keep an explicit pinned-pane aspect when one is
/// set (inline + pane share a single cached PNG), and only fall back to the
/// inline goal otherwise. Side-panel/pinned call sites set their own profile
/// directly and are unaffected by the inline goal.
#[test]
fn transcript_profile_keeps_pinned_pane_aspect_when_present() {
    let pane_aspect = Some(0.5);
    let combined =
        crate::transcript_preferred_aspect_ratio_with_font(pane_aspect, 120, 40, Some((8, 16)));
    assert_eq!(
        combined, pane_aspect,
        "pinned pane aspect must win over the inline goal"
    );

    let fallback = crate::transcript_preferred_aspect_ratio_with_font(None, 120, 40, Some((8, 16)));
    assert_eq!(fallback, Some(1.75), "no pane -> inline goal");

    let no_geometry = crate::transcript_preferred_aspect_ratio_with_font(None, 120, 40, None);
    assert_eq!(no_geometry, None, "no pane + no geometry -> None");
}

/// Serialize tests that render diagrams and assert on the global
/// `MermaidDebugStats` (`last_*` fields and counters), which concurrent
/// renders in sibling tests would otherwise clobber.
fn render_stats_test_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::IMAGE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Layout is terminal-width independent: rendering the same source at two
/// terminal widths that map to different PNG width buckets must compute the
/// layout exactly once, with the second render taking the rasterize-only
/// layout-cache hit path.
#[cfg(feature = "renderer")]
#[test]
fn layout_cache_reuses_layout_across_terminal_widths() {
    let _stats_guard = render_stats_test_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let content = format!(
        "flowchart TD\n    A{unique}[Start] --> B{unique}{{Check}}\n    B{unique} -->|yes| C{unique}[Fast]\n    B{unique} -->|no| D{unique}[Slow]\n    C{unique} --> E{unique}[Done]\n    D{unique} --> E{unique}"
    );
    let hash = super::hash_content(&content);
    let hits_before = super::debug_stats().layout_cache_hits;

    // Narrow terminal: small width bucket.
    let first = super::render_mermaid_untracked(&content, Some(60));
    let first_width = match first {
        super::RenderResult::Image { width, .. } => width,
        super::RenderResult::Error(error) => panic!("first render failed: {error}"),
    };
    assert_eq!(
        super::cache_render::layout_computations_for_test(hash),
        1,
        "first render must compute the layout"
    );

    // Wide terminal: crosses the PNG width bucket (85% reuse threshold), so
    // the PNG cache misses but the layout tier must hit.
    let second = super::render_mermaid_untracked(&content, Some(200));
    let second_width = match second {
        super::RenderResult::Image { width, .. } => width,
        super::RenderResult::Error(error) => panic!("second render failed: {error}"),
    };
    assert!(
        second_width > first_width,
        "test setup: widths must land in different buckets ({first_width} vs {second_width})"
    );
    assert_eq!(
        super::cache_render::layout_computations_for_test(hash),
        1,
        "bucket-crossing resize must reuse the cached layout (rasterize only)"
    );
    assert!(
        super::debug_stats().layout_cache_hits > hits_before,
        "layout cache hit must be visible in debug stats"
    );
}

/// Different aspect profiles produce different layouts (the aspect goal feeds
/// `LayoutConfig::preferred_aspect_ratio`), so each profile is its own layout
/// cache entry: two profiles -> exactly two layout computations.
#[cfg(feature = "renderer")]
#[test]
fn layout_cache_keys_on_aspect_profile() {
    let _stats_guard = render_stats_test_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let content = format!(
        "flowchart LR\n    P{unique}[Input] --> Q{unique}[Transform] --> R{unique}[Output]"
    );
    let hash = super::hash_content(&content);

    super::with_preferred_aspect_ratio(Some(2.0), || {
        assert!(matches!(
            super::render_mermaid_untracked(&content, Some(80)),
            super::RenderResult::Image { .. }
        ));
    });
    assert_eq!(super::cache_render::layout_computations_for_test(hash), 1);

    super::with_preferred_aspect_ratio(Some(1.0), || {
        assert!(matches!(
            super::render_mermaid_untracked(&content, Some(80)),
            super::RenderResult::Image { .. }
        ));
    });
    assert_eq!(
        super::cache_render::layout_computations_for_test(hash),
        2,
        "a different aspect profile must re-run layout"
    );

    // Same profile again: layout tier hit, still two computations.
    super::with_preferred_aspect_ratio(Some(2.0), || {
        assert!(matches!(
            super::render_mermaid_untracked(&content, Some(80)),
            super::RenderResult::Image { .. }
        ));
    });
    assert_eq!(super::cache_render::layout_computations_for_test(hash), 2);
}

/// A layout-cache hit must rasterize a byte-identical PNG to the uncached
/// (parse+layout) path for the same source and target size.
#[cfg(feature = "renderer")]
#[test]
fn layout_cache_hit_renders_byte_identical_png() {
    let _stats_guard = render_stats_test_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let content = format!(
        "flowchart TD\n    X{unique}[Request] --> Y{unique}{{Cache?}}\n    Y{unique} -->|hit| Z{unique}[Serve]\n    Y{unique} -->|miss| W{unique}[Render]\n    W{unique} --> Z{unique}"
    );
    let hash = super::hash_content(&content);

    let first = super::render_mermaid_untracked(&content, Some(90));
    let first_path = match first {
        super::RenderResult::Image { path, .. } => path,
        super::RenderResult::Error(error) => panic!("first render failed: {error}"),
    };
    let uncached_bytes = std::fs::read(&first_path).expect("read uncached png");
    assert_eq!(super::cache_render::layout_computations_for_test(hash), 1);

    // Drop the PNG tier (memory entries + on-disk files) so the next render
    // must rasterize, while the layout tier stays warm.
    super::cache_render::evict_render_cache_for_test(hash);

    let second = super::render_mermaid_untracked(&content, Some(90));
    let second_path = match second {
        super::RenderResult::Image { path, .. } => path,
        super::RenderResult::Error(error) => panic!("second render failed: {error}"),
    };
    assert_eq!(
        super::cache_render::layout_computations_for_test(hash),
        1,
        "second render must be a layout-cache hit"
    );
    let cached_bytes = std::fs::read(&second_path).expect("read cached-layout png");
    assert_eq!(
        uncached_bytes, cached_bytes,
        "layout-cache hit must produce a byte-identical PNG"
    );
}

/// LRU eviction and theme-change clearing for the layout tier, exercised
/// directly on the cache struct so the test does not need 33 real renders.
#[cfg(feature = "renderer")]
#[test]
fn layout_cache_evicts_lru_and_clears_on_theme_change() {
    use mermaid_rs_renderer::ir::DiagramKind;
    use mermaid_rs_renderer::layout::{DiagramData, Layout};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let empty_layout = || {
        Arc::new(Layout {
            kind: DiagramKind::Flowchart,
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            subgraphs: Vec::new(),
            width: 10.0,
            height: 10.0,
            diagram: DiagramData::Graph {
                state_notes: Vec::new(),
            },
        })
    };
    let key = |source_hash: u64, theme_fingerprint: u64| super::cache_render::LayoutCacheKey {
        source_hash,
        theme_fingerprint,
        profile: super::RenderProfile::default(),
        layout_config_fingerprint: 7,
    };

    let mut cache = super::cache_render::LayoutCache::new();
    for idx in 0..super::cache_render::LAYOUT_CACHE_MAX as u64 {
        cache.insert(key(idx, 1), empty_layout());
    }
    assert_eq!(cache.entries.len(), super::cache_render::LAYOUT_CACHE_MAX);

    // Touch entry 0 so it becomes most-recently used, then overflow: entry 1
    // (now the LRU) must be evicted, entry 0 retained.
    assert!(cache.get(&key(0, 1)).is_some());
    cache.insert(
        key(super::cache_render::LAYOUT_CACHE_MAX as u64, 1),
        empty_layout(),
    );
    assert_eq!(cache.entries.len(), super::cache_render::LAYOUT_CACHE_MAX);
    assert!(
        cache.get(&key(0, 1)).is_some(),
        "recently used entry survives"
    );
    assert!(cache.get(&key(1, 1)).is_none(), "LRU entry is evicted");

    // Theme change: a lookup with a new theme fingerprint clears stale entries.
    assert!(cache.get(&key(0, 2)).is_none());
    assert_eq!(
        cache.entries.len(),
        0,
        "theme change must clear the layout cache"
    );
    cache.insert(key(0, 2), empty_layout());
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.get(&key(0, 2)).is_some());
}

#[test]
fn streaming_preview_then_final_registration_does_not_double_count() {
    // Pin the dedupe behavior between STREAMING_PREVIEW_DIAGRAM and
    // ACTIVE_DIAGRAMS when the same content hash finishes streaming and is
    // registered as a final diagram (mermaid_active.rs).
    let saved = super::snapshot_active_diagrams();
    super::clear_active_diagrams(); // also clears the streaming preview

    const HASH: u64 = 0xDEAD_BEEF_CAFE_0001;

    // 1. Streaming preview only: visible in the combined list, but NOT in the
    //    registered snapshot (preview is ephemeral, never persisted).
    super::set_streaming_preview_diagram(HASH, 100, 80, None);
    let combined = super::get_active_diagrams();
    assert_eq!(combined.len(), 1, "preview alone yields one entry");
    assert_eq!(combined[0].hash, HASH);
    assert!(
        super::snapshot_active_diagrams().is_empty(),
        "preview must not appear in the registered ACTIVE_DIAGRAMS list"
    );
    assert_eq!(super::active_diagram_count(), 0);

    // 2. Final registration with the SAME content hash: get_active_diagrams
    //    filters registered entries whose hash matches the live preview, so
    //    the diagram is NOT double-counted.
    super::register_active_diagram(HASH, 200, 160, Some("final".to_string()));
    let combined = super::get_active_diagrams();
    assert_eq!(
        combined.len(),
        1,
        "same-hash preview + registration must dedupe to one entry, got {}",
        combined.len()
    );
    assert_eq!(combined[0].hash, HASH);
    // While the preview is still live, the preview entry wins (it is pushed
    // first and the registered duplicate is filtered by hash).
    assert_eq!(
        combined[0].width, 100,
        "live preview entry shadows the registered one"
    );
    let registered = super::snapshot_active_diagrams();
    assert_eq!(registered.len(), 1, "exactly one registered entry");
    assert_eq!(registered[0].hash, HASH);
    assert_eq!(registered[0].width, 200);
    assert_eq!(registered[0].label.as_deref(), Some("final"));

    // 3. Once the streaming preview is cleared (segment committed), only the
    //    registered final diagram remains, with its final size/label.
    super::clear_streaming_preview_diagram();
    let combined = super::get_active_diagrams();
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].hash, HASH);
    assert_eq!(combined[0].width, 200);
    assert_eq!(combined[0].label.as_deref(), Some("final"));

    // 4. Control: a preview with a DIFFERENT hash is a distinct diagram and
    //    is counted separately (no over-eager dedupe).
    super::set_streaming_preview_diagram(HASH ^ 1, 50, 40, None);
    assert_eq!(
        super::get_active_diagrams().len(),
        2,
        "different-hash preview must not be merged with the registered diagram"
    );

    super::clear_active_diagrams();
    super::restore_active_diagrams(saved);
}

/// Regression: a mermaid diagram rendered under an aspect-tagged profile
/// (the transcript render path wraps `with_preferred_aspect_ratio`) must be
/// visible to the profile-agnostic draw/probe paths that run OUTSIDE that
/// aspect scope. Before the fix, `inline_image_is_materialized` only checked
/// the default profile key and `get_cached_diagram_in_memory` only checked
/// current+default profiles, so the plan-graph placeholder stayed blank
/// forever: the prewarm worker kept "succeeding" without ever making the
/// probe true.
#[test]
fn aspect_profile_cache_entry_is_visible_to_profile_agnostic_probes() {
    const HASH: u64 = 0x51AB_1E5C_AFE0_0001;
    // Insert an entry under an aspect-tagged (non-default) profile, exactly
    // like a deferred transcript render with an inline aspect goal does.
    super::with_preferred_aspect_ratio(Some(1.333), || {
        super::cache_render::insert_render_cache_entry_for_test(
            HASH,
            std::path::PathBuf::from("/nonexistent/aspect_profile_probe.png"),
            1560,
            1170,
        );
    });

    assert!(
        super::inline_image_is_materialized(HASH),
        "materialization probe must see the aspect-profile cache entry"
    );
    let cached = super::cache_render::get_cached_diagram_in_memory_for_test(HASH);
    assert!(
        cached.is_some(),
        "in-memory draw-path lookup must find the aspect-profile entry \
         even outside the aspect render scope"
    );

    super::cache_render::evict_render_cache_for_test(HASH);
    assert!(!super::inline_image_is_materialized(HASH));
}
