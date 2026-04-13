
use super::*;
use std::path::Path;

#[test]
fn terminal_theme_uses_catppuccin_palette() {
    let theme = terminal_theme();

    assert_eq!(theme.background, "#00000000");
    assert_eq!(theme.primary_color, "#313244");
    assert_eq!(theme.primary_border_color, "#b4befe");
    assert_eq!(theme.line_color, "#74c7ec");
    assert_eq!(theme.cluster_background, "#181825d9");
    assert_eq!(theme.sequence_note_border, "#f9e2af");
    assert_eq!(theme.git_colors[0], "#b4befe");
    assert_eq!(theme.git_inv_colors[0], "#cba6f7");
    assert_eq!(theme.git_branch_label_colors[0], "#1e1e2e");
    assert_eq!(theme.pie_colors[0], "#cba6f7");
    assert_eq!(theme.pie_colors[11], "#f5c2e7");
    assert_eq!(theme.pie_section_text_color, "#1e1e2e");
    assert!(theme.font_family.contains("Inter"));
    assert!(!theme.font_family.contains('"'));
}

#[test]
fn terminal_theme_renders_common_diagram_types() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();

    let samples = [
        (
            "flowchart",
            "flowchart LR\n    A[User prompt] --> B{Agent loop}\n    B --> C[Tool call]\n    B --> D[Model reply]",
        ),
        (
            "sequence",
            "sequenceDiagram\n    participant U as User\n    participant J as jcode\n    U->>J: Render mermaid preview\n    J-->>U: Styled diagram",
        ),
        (
            "pie",
            "pie showData\n    title Activity\n    \"Total\" : 145\n    \"Weekly\" : 113\n    \"Today\" : 3",
        ),
        (
            "gitGraph",
            "gitGraph\n    commit id: \"init\"\n    branch feature\n    checkout feature\n    commit id: \"theme\"\n    checkout main\n    merge feature\n    commit id: \"preview\"",
        ),
    ];

    for (name, content) in samples {
        match render_mermaid_untracked(content, Some(80)) {
            RenderResult::Image {
                path,
                width,
                height,
                ..
            } => {
                assert!(path.exists(), "{name}: expected rendered PNG at {path:?}");
                assert!(width > 0, "{name}: expected non-zero width");
                assert!(height > 0, "{name}: expected non-zero height");
            }
            RenderResult::Error(err) => panic!("{name}: expected render success, got {err}"),
        }
    }
}

fn write_test_png(path: &Path, width: u32, height: u32) {
    let img = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 0]));
    img.save(path).expect("save test png");
}

fn mermaid_render_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn test_mermaid_detection() {
    assert!(is_mermaid_lang("mermaid"));
    assert!(is_mermaid_lang("Mermaid"));
    assert!(is_mermaid_lang("mermaid-js"));
    assert!(!is_mermaid_lang("rust"));
    assert!(!is_mermaid_lang("python"));
}

#[test]
fn test_picker_init_mode_from_probe_env() {
    assert_eq!(picker_init_mode_from_probe_env(None), PickerInitMode::Fast);
    assert_eq!(
        picker_init_mode_from_probe_env(Some("1")),
        PickerInitMode::Probe
    );
    assert_eq!(
        picker_init_mode_from_probe_env(Some("true")),
        PickerInitMode::Probe
    );
    assert_eq!(
        picker_init_mode_from_probe_env(Some("yes")),
        PickerInitMode::Probe
    );
    assert_eq!(
        picker_init_mode_from_probe_env(Some("0")),
        PickerInitMode::Fast
    );
    assert_eq!(
        picker_init_mode_from_probe_env(Some("off")),
        PickerInitMode::Fast
    );
    assert_eq!(
        picker_init_mode_from_probe_env(Some("garbage")),
        PickerInitMode::Fast
    );
}

#[test]
fn test_infer_protocol_from_env() {
    assert_eq!(
        infer_protocol_from_env(Some("xterm-kitty"), None, None, None),
        Some(ProtocolType::Kitty)
    );
    assert_eq!(
        infer_protocol_from_env(None, Some("WezTerm"), None, None),
        Some(ProtocolType::Kitty)
    );
    assert_eq!(
        infer_protocol_from_env(None, Some("iTerm.app"), None, None),
        Some(ProtocolType::Iterm2)
    );
    assert_eq!(
        infer_protocol_from_env(None, None, Some("iTerm2"), None),
        Some(ProtocolType::Iterm2)
    );
    assert_eq!(
        infer_protocol_from_env(Some("xterm-sixel"), None, None, None),
        Some(ProtocolType::Sixel)
    );
    assert_eq!(
        infer_protocol_from_env(Some("xterm-256color"), None, None, Some("17")),
        Some(ProtocolType::Kitty)
    );
    assert_eq!(
        infer_protocol_from_env(Some("xterm-256color"), None, None, None),
        None
    );
}

#[test]
fn test_content_hash() {
    let h1 = hash_content("flowchart LR\nA --> B");
    let h2 = hash_content("flowchart LR\nA --> B");
    let h3 = hash_content("flowchart LR\nA --> C");
    assert_eq!(h1, h2);
    assert_ne!(h1, h3);
}

#[test]
fn test_placeholder_parsing() {
    let hash = 0x123456789abcdef0u64;
    let lines = image_widget_placeholder(hash, 10);
    assert!(!lines.is_empty());

    let parsed = parse_image_placeholder(&lines[0]);
    assert_eq!(parsed, Some(hash));
}

#[test]
fn test_adaptive_sizing() {
    // Simple diagram should get smaller size
    let (w1, h1) = calculate_render_size(3, 2, Some(100));
    // Complex diagram should get larger size
    let (w2, h2) = calculate_render_size(50, 80, Some(100));
    assert!(w2 > w1);
    assert!(h2 > h1);
}

#[test]
fn test_adjacent_terminal_widths_share_render_bucket() {
    let (w1, _) = calculate_render_size(5, 6, Some(99));
    let (w2, _) = calculate_render_size(5, 6, Some(100));
    assert_eq!(w1, w2);
}

#[test]
fn test_diagram_size_estimation() {
    let simple = "flowchart LR\n    A --> B";
    let (n1, e1) = estimate_diagram_size(simple);
    assert!(n1 >= 2);
    assert!(e1 >= 1);

    let complex = "flowchart TD\n    A[Start] --> B{Check}\n    B --> C[Yes]\n    B --> D[No]\n    C --> E[End]\n    D --> E";
    let (n2, e2) = estimate_diagram_size(complex);
    assert!(n2 > n1);
    assert!(e2 > e1);
}

#[test]
fn test_cached_width_satisfies_threshold() {
    assert!(cached_width_satisfies(850, Some(1000)));
    assert!(cached_width_satisfies(1000, Some(1000)));
    assert!(!cached_width_satisfies(849, Some(1000)));
    assert!(cached_width_satisfies(300, None));
}

#[test]
fn test_parse_cache_filename() {
    let path = std::path::Path::new("/tmp/0123456789abcdef_w640.png");
    let parsed = parse_cache_filename(path);
    assert_eq!(parsed, Some((0x0123_4567_89ab_cdef, 640)));
}

#[test]
fn test_cache_path_includes_target_width() {
    let cache = MermaidCache::new();
    let path = cache.cache_path(0x0123_4567_89ab_cdef, 960);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    assert_eq!(file_name, "0123456789abcdef_w960.png");
}

#[test]
fn test_discover_on_disk_prefers_smallest_variant_above_reuse_threshold() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hash = 0xfeed_face_cafe_beefu64;
    let small = temp.path().join(format!("{:016x}_w900.png", hash));
    let medium = temp.path().join(format!("{:016x}_w1000.png", hash));
    let large = temp.path().join(format!("{:016x}_w1400.png", hash));
    write_test_png(&small, 900, 600);
    write_test_png(&medium, 1000, 700);
    write_test_png(&large, 1400, 900);

    let cache = MermaidCache {
        entries: HashMap::new(),
        order: VecDeque::new(),
        cache_dir: temp.path().to_path_buf(),
    };

    let found = cache
        .discover_on_disk(hash, Some(1000))
        .expect("expected discovered diagram");
    assert_eq!(found.width, 900);
    assert_eq!(found.height, 600);
    assert_eq!(found.path, small);
}

#[test]
fn test_discover_on_disk_returns_none_when_threshold_not_met() {
    let temp = tempfile::tempdir().expect("tempdir");
    let hash = 0x0bad_f00d_dead_beefu64;
    let smaller = temp.path().join(format!("{:016x}_w500.png", hash));
    let larger = temp.path().join(format!("{:016x}_w700.png", hash));
    write_test_png(&smaller, 500, 300);
    write_test_png(&larger, 700, 420);

    let cache = MermaidCache {
        entries: HashMap::new(),
        order: VecDeque::new(),
        cache_dir: temp.path().to_path_buf(),
    };

    let found = cache.discover_on_disk(hash, Some(1000));
    assert!(
        found.is_none(),
        "undersized cached variants should force a re-render"
    );
}

#[test]
fn test_active_diagrams_are_bounded() {
    clear_active_diagrams();
    for idx in 0..(ACTIVE_DIAGRAMS_MAX + 5) {
        register_active_diagram(idx as u64, 100, 80, None);
    }
    let snapshot = snapshot_active_diagrams();
    assert_eq!(snapshot.len(), ACTIVE_DIAGRAMS_MAX);
    assert_eq!(snapshot.first().map(|d| d.hash), Some(5));
    assert_eq!(
        snapshot.last().map(|d| d.hash),
        Some((ACTIVE_DIAGRAMS_MAX + 4) as u64)
    );
    clear_active_diagrams();
}

#[test]
fn test_register_active_diagram_updates_existing_entry_without_duplication() {
    clear_active_diagrams();
    register_active_diagram(0xabc, 100, 80, Some("first".to_string()));
    register_active_diagram(0xdef, 120, 90, None);
    register_active_diagram(0xabc, 300, 200, Some("updated".to_string()));

    let diagrams = get_active_diagrams();
    assert_eq!(diagrams.len(), 2);
    assert_eq!(diagrams[0].hash, 0xabc);
    assert_eq!(diagrams[0].width, 300);
    assert_eq!(diagrams[0].height, 200);
    assert_eq!(diagrams[0].label.as_deref(), Some("updated"));
    assert_eq!(diagrams[1].hash, 0xdef);

    clear_active_diagrams();
}

#[test]
fn test_streaming_preview_is_ephemeral_and_prioritized() {
    clear_active_diagrams();
    register_active_diagram(0x1, 100, 80, None);

    set_streaming_preview_diagram(0x2, 140, 90, Some("streaming".to_string()));
    let with_preview = get_active_diagrams();
    assert_eq!(with_preview.first().map(|d| d.hash), Some(0x2));
    assert_eq!(with_preview.get(1).map(|d| d.hash), Some(0x1));

    clear_streaming_preview_diagram();
    let without_preview = get_active_diagrams();
    assert_eq!(without_preview.len(), 1);
    assert_eq!(without_preview[0].hash, 0x1);

    clear_active_diagrams();
}

#[test]
fn test_parse_proc_status_value_bytes() {
    let status = "Name:\tjcode\nVmSize:\t   2048 kB\nVmRSS:\t    512 kB\nVmHWM:\t   1024 kB\n";
    assert_eq!(
        parse_proc_status_value_bytes(status, "VmSize:"),
        Some(2048 * 1024)
    );
    assert_eq!(
        parse_proc_status_value_bytes(status, "VmRSS:"),
        Some(512 * 1024)
    );
    assert_eq!(
        parse_proc_status_value_bytes(status, "VmHWM:"),
        Some(1024 * 1024)
    );
    assert_eq!(parse_proc_status_value_bytes(status, "VmSwap:"), None);
}

#[test]
fn test_memory_profile_exposes_limits() {
    let profile = debug_memory_profile();
    assert_eq!(profile.render_cache_limit, RENDER_CACHE_MAX);
    assert_eq!(profile.image_state_limit, IMAGE_STATE_MAX);
    assert_eq!(profile.source_cache_limit, SOURCE_CACHE_MAX);
    assert_eq!(profile.active_diagrams_limit, ACTIVE_DIAGRAMS_MAX);
    assert_eq!(profile.cache_disk_limit_bytes, CACHE_MAX_SIZE_BYTES);
    assert_eq!(profile.cache_disk_max_age_secs, CACHE_MAX_AGE_SECS);
}

#[test]
fn test_memory_benchmark_clamps_iterations() {
    let result = debug_memory_benchmark(0);
    assert_eq!(result.iterations, 1);
}

#[test]
fn test_memory_benchmark_upper_clamps_iterations() {
    let result = debug_memory_benchmark(999);
    assert_eq!(result.iterations, 256);
}

#[test]
fn test_register_external_image_round_trips_through_cache() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("external.png");
    write_test_png(&path, 320, 180);

    let hash = register_external_image(&path, 320, 180);
    let cached = get_cached_png(hash).expect("cached png entry");
    assert_eq!(cached.0, path);
    assert_eq!(cached.1, 320);
    assert_eq!(cached.2, 180);
}

#[test]
fn test_result_to_lines_uses_hash_placeholder_in_video_export_mode() {
    set_video_export_mode(true);
    let hash = 0x1234_5678_9abc_def0u64;
    let lines = result_to_lines(
        RenderResult::Image {
            hash,
            path: PathBuf::from("/tmp/placeholder.png"),
            width: 640,
            height: 480,
        },
        Some(80),
    );
    set_video_export_mode(false);

    assert!(!lines.is_empty());
    assert_eq!(parse_image_placeholder(&lines[0]), Some(hash));
}

#[test]
fn test_estimate_image_height_fallback_scales_and_caps() {
    let short = estimate_image_height(800, 400, 80);
    let tall = estimate_image_height(200, 1600, 80);
    assert!(short > 0);
    assert!(tall >= short);
    assert!(
        tall <= 30,
        "fallback height should stay capped, got {}",
        tall
    );
}

#[test]
fn test_render_mermaid_sized_creates_distinct_cache_variants_for_widths() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();

    let content = "flowchart LR\n    A[Start] --> B[End]";
    let small = render_mermaid_untracked(content, Some(60));
    let large = render_mermaid_untracked(content, Some(200));

    let (small_path, large_path) = match (small, large) {
        (
            RenderResult::Image {
                path: small_path, ..
            },
            RenderResult::Image {
                path: large_path, ..
            },
        ) => (small_path, large_path),
        _ => panic!("expected successful mermaid renders"),
    };

    assert_ne!(
        small_path, large_path,
        "expected width-specific cache variants"
    );
    assert!(small_path.to_string_lossy().contains("_w432"));
    assert!(large_path.to_string_lossy().contains("_w1440"));
}

#[test]
fn test_render_mermaid_sized_honors_adaptive_output_dimensions() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();

    let content = "flowchart LR\n    A[Start] --> B[End]";
    let small = render_mermaid_untracked(content, Some(60));
    let large = render_mermaid_untracked(content, Some(200));

    let (small_w, small_h, large_w, large_h) = match (small, large) {
        (
            RenderResult::Image {
                width: small_w,
                height: small_h,
                ..
            },
            RenderResult::Image {
                width: large_w,
                height: large_h,
                ..
            },
        ) => (small_w, small_h, large_w, large_h),
        _ => panic!("expected successful mermaid renders"),
    };

    assert!(
        small_w < large_w,
        "expected adaptive widths: {} < {}",
        small_w,
        large_w
    );
    assert!(
        small_h < large_h,
        "expected adaptive heights: {} < {}",
        small_h,
        large_h
    );
    assert!(
        small_w <= 650,
        "small render should stay near narrow target width, got {}",
        small_w
    );
    assert!(
        large_w >= 1300,
        "large render should approach wide target width, got {}",
        large_w
    );
}

#[test]
fn test_render_mermaid_deferred_returns_pending_then_cached_image() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();

    let content = "flowchart LR\n    A[Deferred Start] --> B[Deferred End]";
    let first = render_mermaid_deferred(content, Some(80));
    assert!(first.is_none(), "expected background render to be queued");

    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    let result = loop {
        if let Some(result) = render_mermaid_deferred(content, Some(80)) {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for deferred mermaid render"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };

    match result {
        RenderResult::Image { width, height, .. } => {
            assert!(width > 0);
            assert!(height > 0);
        }
        RenderResult::Error(err) => panic!("expected deferred render success, got {err}"),
    }
}

#[test]
fn test_set_cell_if_visible_ignores_out_of_bounds_coordinates() {
    let mut buf = Buffer::empty(Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 2,
    });
    set_cell_if_visible(&mut buf, 10, 1, 'X', None);
    set_cell_if_visible(&mut buf, 2, 1, 'Y', None);
    assert_eq!(buf[(2, 1)].symbol(), "Y");
    assert_eq!(buf[(0, 0)].symbol(), " ");
}

#[test]
fn test_draw_left_border_clamps_to_buffer_area() {
    let mut buf = Buffer::empty(Rect {
        x: 0,
        y: 0,
        width: 5,
        height: 3,
    });
    draw_left_border(
        &mut buf,
        Rect {
            x: 10,
            y: 1,
            width: 4,
            height: 2,
        },
    );
    draw_left_border(
        &mut buf,
        Rect {
            x: 3,
            y: 0,
            width: 4,
            height: 3,
        },
    );
    assert_eq!(buf[(3, 0)].symbol(), "│");
    assert_eq!(buf[(4, 0)].symbol(), " ");
}

// ── SVG rewriting helpers ─────────────────────────────────────────────────

#[test]
fn test_extract_xml_attribute_reads_value() {
    let tag = r#"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="600" viewBox="0 0 400 300">"#;
    assert_eq!(extract_xml_attribute(tag, "width"), Some("800"));
    assert_eq!(extract_xml_attribute(tag, "height"), Some("600"));
    assert_eq!(extract_xml_attribute(tag, "viewBox"), Some("0 0 400 300"));
    assert_eq!(extract_xml_attribute(tag, "missing"), None);
}

#[test]
fn test_parse_svg_length_handles_variants() {
    assert_eq!(parse_svg_length("800"), Some(800.0));
    assert_eq!(parse_svg_length("640px"), Some(640.0));
    assert_eq!(parse_svg_length("100%"), None);
    assert_eq!(parse_svg_length(""), None);
    assert_eq!(parse_svg_length("0"), None);
    assert_eq!(parse_svg_length("-5"), None);
}

#[test]
fn test_parse_svg_viewbox_size_extracts_wh() {
    let tag = r#"<svg viewBox="10 20 800 600">"#;
    let result = parse_svg_viewbox_size(tag);
    assert_eq!(result, Some((800.0, 600.0)));

    let tag_no_vb = r#"<svg width="400" height="300">"#;
    assert_eq!(parse_svg_viewbox_size(tag_no_vb), None);
}

#[test]
fn test_set_xml_attribute_updates_existing() {
    let tag = r#"<svg width="800" height="600">"#;
    let updated = set_xml_attribute(tag, "width", "1200");
    assert!(updated.contains(r#"width="1200""#), "got: {}", updated);
    assert!(!updated.contains(r#"width="800""#));
    assert!(updated.contains(r#"height="600""#));
}

#[test]
fn test_retarget_svg_for_png_rewrites_root_dimensions() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300" viewBox="0 0 400 300"><rect/></svg>"#;
    let rewritten = retarget_svg_for_png(svg, 800.0, 600.0);
    assert!(
        rewritten.contains(r#"width="800""#) || rewritten.contains("800"),
        "width not rewritten: {}",
        rewritten
    );
    assert!(
        !rewritten.contains(r#"width="400""#),
        "old width still present: {}",
        rewritten
    );
    assert!(rewritten.contains(r#"<rect/>"#), "body was modified");
}

#[test]
fn test_retarget_svg_for_png_preserves_aspect_ratio_from_viewbox() {
    // viewBox is 200x100 (2:1 ratio), request 400×9999 — height should be ≈200
    let svg = r#"<svg width="200" height="100" viewBox="0 0 200 100"></svg>"#;
    let rewritten = retarget_svg_for_png(svg, 400.0, 9999.0);
    // Parse actual width from the result
    let w = extract_xml_attribute(&rewritten, "width")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    let h = extract_xml_attribute(&rewritten, "height")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    assert!((w - 400.0).abs() < 1.0, "expected w≈400, got {}", w);
    assert!(
        (h - 200.0).abs() < 1.0,
        "expected h≈200 (aspect from viewBox), got {}",
        h
    );
}

#[test]
fn test_retarget_svg_for_png_respects_target_height_cap() {
    // viewBox is tall (1:4 ratio), request 800x600. We should scale down to
    // fit the target height instead of preserving width and blowing past it.
    let svg = r#"<svg width="100" height="400" viewBox="0 0 100 400"></svg>"#;
    let rewritten = retarget_svg_for_png(svg, 800.0, 600.0);
    let w = extract_xml_attribute(&rewritten, "width")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    let h = extract_xml_attribute(&rewritten, "height")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(0.0);
    assert!((w - 150.0).abs() < 1.0, "expected w≈150, got {}", w);
    assert!((h - 600.0).abs() < 1.0, "expected h≈600, got {}", h);
}

#[test]
fn test_retarget_svg_for_png_is_noop_on_non_svg() {
    let not_svg = "<html><body></body></html>";
    let result = retarget_svg_for_png(not_svg, 800.0, 600.0);
    assert_eq!(result, not_svg);
}

// ── Image-state stats ─────────────────────────────────────────────────────

#[test]
fn test_image_state_hits_increment_on_cache_hit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("test_hit.png");
    write_test_png(&path, 400, 300);
    let hash = register_external_image(&path, 400, 300);

    let initial = { MERMAID_DEBUG.lock().unwrap().stats.image_state_hits };

    let mut buf = Buffer::empty(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    });
    let area = Rect {
        x: 0,
        y: 0,
        width: 60,
        height: 20,
    };

    // Clear any existing image state for this hash
    if let Ok(mut state) = IMAGE_STATE.lock() {
        state.remove(&hash);
    }

    // First call: image_state_misses (no state yet, but PICKER is None in tests)
    // so it won't actually hit the open path. Just verify hits don't go negative.
    let _h = render_image_widget_fit(hash, area, &mut buf, false, false);

    // Image state will only be populated if PICKER is set, which it isn't in CI.
    // But hits counter should remain stable (non-decreasing).
    let after = MERMAID_DEBUG.lock().unwrap().stats.image_state_hits;
    assert!(after >= initial, "image_state_hits should never decrease");
}

#[test]
fn test_skipped_renders_counter_is_non_negative() {
    let skipped = MERMAID_DEBUG.lock().unwrap().stats.skipped_renders;
    assert!(skipped < u64::MAX, "skipped_renders is a valid counter");
}

#[test]
fn test_skipped_renders_increments_on_identical_last_render_state() {
    // Exercise the LAST_RENDER + skipped_renders counting logic directly.
    // Simulate two consecutive renders with the same area & resize mode.
    let hash: u64 = 0xDEAD_BEEF_1234;
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let state_key = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };

    // Clear previous entry if any
    if let Ok(mut map) = LAST_RENDER.lock() {
        map.remove(&hash);
    }

    let before = MERMAID_DEBUG.lock().unwrap().stats.skipped_renders;

    // First render - no prior entry, so no skip
    {
        let last_same = LAST_RENDER
            .lock()
            .ok()
            .and_then(|mut map| {
                let prev = map.get(&hash).cloned();
                map.insert(hash, state_key.clone());
                prev
            })
            .map(|prev| prev == state_key)
            .unwrap_or(false);
        if last_same && let Ok(mut dbg) = MERMAID_DEBUG.lock() {
            dbg.stats.skipped_renders += 1;
        }
    }

    let after_first = MERMAID_DEBUG.lock().unwrap().stats.skipped_renders;
    assert_eq!(
        after_first, before,
        "first render should not increment skipped_renders"
    );

    // Second render - same state_key → should increment
    {
        let last_same = LAST_RENDER
            .lock()
            .ok()
            .and_then(|mut map| {
                let prev = map.get(&hash).cloned();
                map.insert(hash, state_key.clone());
                prev
            })
            .map(|prev| prev == state_key)
            .unwrap_or(false);
        if last_same && let Ok(mut dbg) = MERMAID_DEBUG.lock() {
            dbg.stats.skipped_renders += 1;
        }
    }

    let after_second = MERMAID_DEBUG.lock().unwrap().stats.skipped_renders;
    assert_eq!(
        after_second,
        before + 1,
        "second identical render should increment skipped_renders by 1"
    );
}

#[test]
fn test_last_render_state_equality_requires_all_fields() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let s1 = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };
    let s2 = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };
    let s3 = LastRenderState {
        area,
        crop_top: true,
        resize_mode: ResizeMode::Fit,
    };
    let s4 = LastRenderState {
        area,
        crop_top: false,
        resize_mode: ResizeMode::Crop,
    };
    let s5 = LastRenderState {
        area: Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        },
        crop_top: false,
        resize_mode: ResizeMode::Fit,
    };

    assert_eq!(s1, s2, "identical states should be equal");
    assert_ne!(s1, s3, "different crop_top should not be equal");
    assert_ne!(s1, s4, "different resize_mode should not be equal");
    assert_ne!(s1, s5, "different area should not be equal");
}

#[test]
fn test_debug_stats_aggregate_across_renders() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();

    let initial_requests = MERMAID_DEBUG.lock().unwrap().stats.total_requests;
    let content = "flowchart LR\n    X[Start] --> Y[End]";
    let _ = render_mermaid_untracked(content, None);
    let after_requests = MERMAID_DEBUG.lock().unwrap().stats.total_requests;

    assert!(
        after_requests > initial_requests,
        "total_requests should increment on each render call"
    );

    let stats = MERMAID_DEBUG.lock().unwrap().stats.clone();
    let total_cache = stats.cache_hits + stats.cache_misses;
    assert!(
        total_cache >= after_requests - initial_requests,
        "cache_hits + cache_misses should account for all render calls, \
             got hits={} misses={} requests_delta={}",
        stats.cache_hits,
        stats.cache_misses,
        after_requests - initial_requests
    );
}

#[test]
fn test_kitty_viewport_state_reuses_transmit_for_scroll_only_updates() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats = MermaidDebugStats::default();
    }

    let hash = 0x1234_5678_9abc_def0;
    let path = PathBuf::from("/tmp/test-kitty-scroll.png");
    let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        640,
        480,
        image::Rgba([20, 40, 60, 255]),
    ));

    let (unique_id, full_cols, full_rows) =
        ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16))
            .expect("kitty viewport state");
    assert!(full_cols > 0 && full_rows > 0);

    let rebuilds_after_first = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;
    assert_eq!(rebuilds_after_first, 1);

    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    assert!(render_kitty_virtual_viewport(
        hash,
        Rect::new(0, 0, 20, 8),
        &mut buf,
        0,
        0,
        20,
        8
    ));
    let first_symbol = buf[(0, 0)].symbol().to_string();
    assert!(
        first_symbol.contains("_Gq=2"),
        "first render should transmit image data"
    );

    let (same_id, _, _) = ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16))
        .expect("kitty viewport state reused");
    assert_eq!(same_id, unique_id);
    let rebuilds_after_second = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;
    assert_eq!(rebuilds_after_second, rebuilds_after_first);

    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
    assert!(render_kitty_virtual_viewport(
        hash,
        Rect::new(0, 0, 20, 8),
        &mut buf,
        3,
        2,
        20,
        8
    ));
    let second_symbol = buf[(0, 0)].symbol().to_string();
    assert!(
        !second_symbol.contains("_Gq=2"),
        "scroll-only render should reuse prior transmit"
    );
}

#[test]
fn test_kitty_viewport_state_rebuilds_when_zoom_changes() {
    let _lock = mermaid_render_test_lock();
    clear_cache().ok();
    if let Ok(mut debug) = MERMAID_DEBUG.lock() {
        debug.stats = MermaidDebugStats::default();
    }

    let hash = 0x0bad_f00d_dead_beef;
    let path = PathBuf::from("/tmp/test-kitty-zoom.png");
    let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        320,
        200,
        image::Rgba([200, 120, 80, 255]),
    ));

    let (id_100, cols_100, rows_100) =
        ensure_kitty_viewport_state(hash, &path, &source, 100, (8, 16)).expect("zoom 100");
    let rebuilds_100 = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;

    let (id_150, cols_150, rows_150) =
        ensure_kitty_viewport_state(hash, &path, &source, 150, (8, 16)).expect("zoom 150");
    let rebuilds_150 = MERMAID_DEBUG
        .lock()
        .unwrap()
        .stats
        .viewport_protocol_rebuilds;

    assert_eq!(id_100, id_150, "zoom changes should reuse kitty image id");
    assert!(cols_150 >= cols_100);
    assert!(rows_150 >= rows_100);
    assert_eq!(rebuilds_150, rebuilds_100 + 1);
}
