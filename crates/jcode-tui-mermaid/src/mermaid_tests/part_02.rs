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
fn mmdr_size_api_reports_explicit_png_canvas() {
    super::reset_debug_stats();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let content = format!("flowchart TD\nA[Start {unique}] --> B[End]");

    let result = super::render_mermaid_untracked(&content, Some(100));
    let (width, height) = match result {
        super::RenderResult::Image { width, height, .. } => (width, height),
        super::RenderResult::Error(error) => panic!("render failed: {error}"),
    };
    let stats = super::debug_stats();

    assert_eq!(stats.last_measured_width, stats.last_target_width);
    assert_eq!(stats.last_measured_height, stats.last_target_height);
    assert_eq!(Some(width), stats.last_measured_width);
    assert_eq!(Some(height), stats.last_measured_height);
    assert!(stats.last_viewbox_width.unwrap_or_default() > 0);
    assert!(stats.last_viewbox_height.unwrap_or_default() > 0);
}
