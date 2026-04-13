use super::*;
use std::sync::{Mutex, OnceLock};

fn clear_side_panel_render_caches() {
    super::clear_side_panel_render_caches();
}

fn mermaid_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_mermaid_placeholder_mode<T>(f: impl FnOnce() -> T) -> T {
    struct ResetVideoExportMode;
    impl Drop for ResetVideoExportMode {
        fn drop(&mut self) {
            crate::tui::mermaid::set_video_export_mode(false);
        }
    }

    let _guard = mermaid_test_lock()
        .lock()
        .expect("mermaid placeholder test lock");
    crate::tui::mermaid::set_video_export_mode(true);
    let _reset = ResetVideoExportMode;
    let result = f();
    result
}

fn with_serialized_mermaid_state<T>(f: impl FnOnce() -> T) -> T {
    let _guard = mermaid_test_lock().lock().expect("mermaid test lock");
    f()
}

fn sample_mermaid_page(content: impl Into<String>) -> crate::side_panel::SidePanelPage {
    use std::hash::{Hash as _, Hasher as _};

    let content = content.into();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    let content_hash = hasher.finish();

    crate::side_panel::SidePanelPage {
        id: format!("mermaid_demo_{content_hash:016x}"),
        title: format!("Mermaid Demo {content_hash:016x}"),
        file_path: format!("mermaid_demo_{content_hash:016x}.md"),
        format: crate::side_panel::SidePanelPageFormat::Markdown,
        source: crate::side_panel::SidePanelPageSource::Managed,
        content,
        updated_at_ms: content_hash,
    }
}

#[test]
fn clamp_side_panel_image_rows_leaves_room_for_following_content() {
    let rows = clamp_side_panel_image_rows(18, 16, 2, true);
    assert_eq!(rows, 10);
}

#[test]
fn clamp_side_panel_image_rows_preserves_estimate_without_following_content() {
    let rows = clamp_side_panel_image_rows(18, 16, 2, false);
    assert_eq!(rows, 16);
}

#[test]
fn clamp_side_panel_image_rows_keeps_minimum_image_presence() {
    let rows = clamp_side_panel_image_rows(10, 5, 1, true);
    assert_eq!(rows, 4);
}

#[test]
fn clamp_side_panel_image_rows_ignores_preceding_document_length() {
    let near_top = clamp_side_panel_image_rows(18, 16, 2, true);
    let far_down_page = clamp_side_panel_image_rows(18, 16, 200, true);
    assert_eq!(near_top, 10);
    assert_eq!(far_down_page, near_top);
}

#[test]
fn estimate_side_panel_image_rows_uses_actual_inner_width() {
    let rows = estimate_side_panel_image_rows_with_font(999, 1454, 36, Some((8, 16)));
    assert_eq!(rows, 27);
}

#[test]
fn side_panel_mermaid_switches_to_scrollable_viewport_when_fit_would_be_too_small() {
    let layout =
        estimate_side_panel_image_layout_with_font(4000, 2000, 24, 20, 0, false, Some((8, 16)));

    assert_eq!(
        layout.render_mode,
        SidePanelImageRenderMode::ScrollableViewport {
            zoom_percent: SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT,
        }
    );
    assert!(layout.rows > 20, "expected tall scrollable diagram rows");
    assert!(layout.render_mode.is_scrollable());
}

#[test]
fn side_panel_mermaid_keeps_fit_mode_when_zoom_stays_readable() {
    let layout =
        estimate_side_panel_image_layout_with_font(300, 480, 36, 30, 0, true, Some((8, 16)));

    assert_eq!(layout.render_mode, SidePanelImageRenderMode::Fit);
    assert_eq!(layout.rows, 20);
    assert!(!layout.render_mode.is_scrollable());
}

#[test]
fn side_panel_viewport_scroll_x_applies_horizontal_pan_around_center() {
    let centered = side_panel_viewport_scroll_x(4000, 24, 70, true, Some((8, 16)), 0);
    let panned_right = side_panel_viewport_scroll_x(4000, 24, 70, true, Some((8, 16)), 6);
    let panned_left = side_panel_viewport_scroll_x(4000, 24, 70, true, Some((8, 16)), -6);

    assert!(centered > 0, "expected oversized diagram to start centered");
    assert!(
        panned_right > centered,
        "expected positive pan to move viewport right"
    );
    assert!(
        panned_left < centered,
        "expected negative pan to move viewport left"
    );
}

#[test]
fn fit_side_panel_image_area_centers_constrained_image_horizontally() {
    let area = Rect::new(10, 4, 36, 12);
    let fitted = fit_image_area_with_font(area, 999, 1454, Some((8, 16)), true, false);

    assert!(fitted.width < area.width);
    assert!(
        fitted.x > area.x,
        "expected horizontal centering: {:?} within {:?}",
        fitted,
        area
    );
    assert_eq!(
        fitted.y, area.y,
        "inline side-panel images should remain top-aligned"
    );
    assert_eq!(fitted.height, area.height);
}

#[test]
fn fit_side_panel_image_area_preserves_full_width_when_width_constrained() {
    let area = Rect::new(0, 0, 36, 30);
    let fitted = fit_image_area_with_font(area, 999, 1454, Some((8, 16)), true, false);

    assert_eq!(fitted.x, area.x);
    assert_eq!(fitted.width, area.width);
    assert!(fitted.height < area.height);
}

#[test]
fn plan_fit_image_render_uses_clipped_viewport_for_partial_visibility() {
    let viewport = Rect::new(0, 10, 36, 12);
    let plan = plan_fit_image_render(viewport, 4, 0, 12, 720, 1440, true).expect("fit render plan");

    match plan {
        FitImageRenderPlan::ClippedViewport {
            area,
            scroll_y,
            zoom_percent,
        } => {
            assert!(
                area.height < 12,
                "expected clipped visible height: {area:?}"
            );
            assert!(scroll_y > 0, "expected positive vertical clip offset");
            assert!(zoom_percent > 0);
        }
        other => panic!("expected clipped viewport plan, got {other:?}"),
    }
}

#[test]
fn plan_fit_image_render_uses_full_fit_when_fully_visible() {
    let viewport = Rect::new(0, 10, 36, 12);
    let plan = plan_fit_image_render(viewport, 0, 0, 12, 720, 1440, true).expect("fit render plan");

    match plan {
        FitImageRenderPlan::Full { area } => {
            assert_eq!(area.y, viewport.y);
            assert_eq!(area.height, viewport.height);
        }
        other => panic!("expected full fit plan, got {other:?}"),
    }
}

#[test]
fn render_side_panel_markdown_keeps_text_after_mermaid_block() {
    let page = sample_mermaid_page(
        "This is some text above the diagram.\n\n```mermaid\nflowchart TD\n    A[Start] --> B[Do the thing]\n    B --> C[Done]\n```\n\nThis is some text below the diagram.",
    );

    let rendered = with_mermaid_placeholder_mode(|| {
        render_side_panel_markdown_cached(&page, Rect::new(0, 0, 36, 30), true, true)
    });
    let text: Vec<String> = rendered
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();

    assert!(
        text.iter()
            .any(|line| line.contains("This is some text above the diagram.")),
        "expected text above mermaid block in rendered lines: {:?}",
        text
    );
    assert!(
        text.iter()
            .any(|line| line.contains("This is some text below the diagram.")),
        "expected text below mermaid block in rendered lines: {:?}",
        text
    );
    if let Some(placement) = rendered.image_placements.first() {
        assert!(
            placement.rows < 30,
            "image should not consume the full side-panel height when trailing text exists"
        );
    }
}

#[test]
fn render_side_panel_markdown_late_mermaid_keeps_reasonable_rows() {
    let mut content = String::new();
    for i in 0..24 {
        content.push_str(&format!("Paragraph {} before chart.\n\n", i + 1));
    }
    content.push_str(
            "```mermaid\nxychart-beta\n    title \"Volume\"\n    x-axis [A, B, C, D]\n    y-axis \"Count\" 0 --> 100\n    bar [10, 50, 80, 30]\n```\n\nTail text after chart.\n",
        );

    let page = sample_mermaid_page(content);

    let rendered = with_mermaid_placeholder_mode(|| {
        render_side_panel_markdown_cached(&page, Rect::new(0, 0, 48, 30), true, true)
    });

    let placement = rendered
        .image_placements
        .first()
        .expect("expected mermaid image placement");

    assert!(
        placement.rows >= 8,
        "late side-panel mermaid should not collapse to tiny height: {} rows",
        placement.rows
    );
}

#[test]
fn render_side_panel_markdown_reserves_blank_rows_for_mermaid_placement() {
    let page = sample_mermaid_page(
        "Intro text.\n\n```mermaid\nflowchart TD\n    A[Start] --> B[Done]\n```\n",
    );

    let rendered = with_mermaid_placeholder_mode(|| {
        render_side_panel_markdown_cached(&page, Rect::new(0, 0, 36, 24), true, true)
    });

    assert_eq!(
        rendered.image_placements.len(),
        1,
        "expected one mermaid image placement"
    );
    let placement = &rendered.image_placements[0];
    assert!(placement.rows >= SIDE_PANEL_INLINE_IMAGE_MIN_ROWS);
    let reserved = &rendered.lines
        [placement.after_text_line..placement.after_text_line + placement.rows as usize];
    assert!(
        reserved.iter().all(|line| line.width() == 0),
        "expected reserved side-panel image rows to remain blank placeholders: {:?}",
        reserved
    );
}

#[test]
fn render_side_panel_markdown_multiple_mermaids_create_ordered_placements() {
    let page = sample_mermaid_page(
        "Alpha\n\n```mermaid\nflowchart TD\n    A --> B\n```\n\nBetween\n\n```mermaid\nflowchart TD\n    C --> D\n```\n\nOmega\n",
    );

    let rendered = with_mermaid_placeholder_mode(|| {
        render_side_panel_markdown_cached(&page, Rect::new(0, 0, 40, 28), true, true)
    });

    assert_eq!(
        rendered.image_placements.len(),
        2,
        "expected two mermaid placements"
    );
    assert!(
        rendered.image_placements[0].after_text_line < rendered.image_placements[1].after_text_line,
        "expected mermaid placements to preserve document order: {:?}",
        rendered
            .image_placements
            .iter()
            .map(|p| (p.after_text_line, p.rows))
            .collect::<Vec<_>>()
    );
}

#[test]
fn render_side_panel_markdown_without_protocol_falls_back_to_text_placeholder() {
    let page = sample_mermaid_page("```mermaid\nflowchart TD\n    A --> B\n```\n");

    let rendered = with_serialized_mermaid_state(|| {
        render_side_panel_markdown_cached(&page, Rect::new(0, 0, 36, 20), false, true)
    });
    let text: Vec<String> = rendered
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();

    assert!(
        rendered.image_placements.is_empty(),
        "expected no image placement without protocol support: {:?}",
        rendered.image_placements.len()
    );
    assert!(
        text.iter().any(|line| line.contains("mermaid diagram")),
        "expected textual placeholder when image protocols are unavailable: {:?}",
        text
    );
}

#[test]
fn render_side_panel_markdown_trailing_text_reduces_mermaid_rows() {
    let chart = "```mermaid\nxychart-beta\n    title \"Volume\"\n    x-axis [A, B, C, D]\n    y-axis \"Count\" 0 --> 100\n    bar [10, 50, 80, 30]\n```\n";
    let page_without_tail = sample_mermaid_page(chart);
    let page_with_tail = sample_mermaid_page(format!("{chart}\nTail text after chart.\n"));

    let (without_tail, with_tail) = with_mermaid_placeholder_mode(|| {
        (
            render_side_panel_markdown_cached(
                &page_without_tail,
                Rect::new(0, 0, 48, 30),
                true,
                true,
            ),
            render_side_panel_markdown_cached(&page_with_tail, Rect::new(0, 0, 48, 30), true, true),
        )
    });

    let rows_without_tail = without_tail
        .image_placements
        .first()
        .expect("expected mermaid placement without trailing text")
        .rows;
    let rows_with_tail = with_tail
        .image_placements
        .first()
        .expect("expected mermaid placement with trailing text")
        .rows;

    assert!(
        rows_without_tail >= rows_with_tail,
        "trailing text should not increase image rows: without tail {}, with tail {}",
        rows_without_tail,
        rows_with_tail
    );
}

#[test]
fn render_side_panel_markdown_wraps_long_text_lines() {
    let page = crate::side_panel::SidePanelPage {
            id: "wrap_demo".to_string(),
            title: "Wrap Demo".to_string(),
            file_path: "wrap_demo.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "This is a deliberately long side panel line that should wrap instead of overflowing the pane.".to_string(),
            updated_at_ms: 1,
        };

    let rendered = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 18, 30), false, false);

    let non_empty: Vec<&Line<'_>> = rendered
        .lines
        .iter()
        .filter(|line| line.width() > 0)
        .collect();

    assert!(
        non_empty.len() >= 2,
        "expected long side panel text to wrap: {:?}",
        rendered.lines
    );
    assert!(
        non_empty.iter().all(|line| line.width() <= 18),
        "expected wrapped side panel lines to fit width 18: {:?}",
        rendered.lines
    );
}

#[test]
fn render_side_panel_markdown_keeps_table_rows_intact() {
    let page = crate::side_panel::SidePanelPage {
        id: "table_demo".to_string(),
        title: "Table Demo".to_string(),
        file_path: "table_demo.md".to_string(),
        format: crate::side_panel::SidePanelPageFormat::Markdown,
        source: crate::side_panel::SidePanelPageSource::Managed,
        content:
            "| # | Principle | Story Ready |\n| - | - | - |\n| 1 | Customer Obsession | unchecked |"
                .to_string(),
        updated_at_ms: 1,
    };

    let rendered = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
    let text: Vec<String> = rendered
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    assert!(
        text.iter().any(|line| line.contains("─┼─")),
        "expected separator line to remain intact: {:?}",
        text
    );
    assert!(
        text.iter()
            .any(|line| line.matches('│').count() == 2 && line.contains("Cust")),
        "expected a single intact table row line: {:?}",
        text
    );
}

#[test]
fn render_side_panel_markdown_live_syncs_file_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("live.md");
    std::fs::write(&file_path, "# First").expect("write initial content");

    let mut snapshot = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("live_demo".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "live_demo".to_string(),
            title: "Live Demo".to_string(),
            file_path: file_path.display().to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::LinkedFile,
            content: "# Stale".to_string(),
            updated_at_ms: 1,
        }],
    };

    clear_side_panel_render_caches();
    assert!(crate::side_panel::refresh_linked_page_content(
        &mut snapshot,
        None
    ));
    let page = snapshot.focused_page().expect("focused page");

    let first = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
    let first_text: Vec<String> = first
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert!(
        first_text.iter().any(|line| line.contains("First")),
        "expected first render to use file content: {:?}",
        first_text
    );

    std::fs::write(&file_path, "# Second").expect("write updated content");

    assert!(crate::side_panel::refresh_linked_page_content(
        &mut snapshot,
        None
    ));
    let page = snapshot.focused_page().expect("focused page");

    let second = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
    let second_text: Vec<String> = second
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert!(
        second_text.iter().any(|line| line.contains("Second")),
        "expected second render to reflect updated file content: {:?}",
        second_text
    );
}

#[test]
fn render_side_panel_height_change_reuses_markdown_render_cache() {
    clear_side_panel_render_caches();
    let before = markdown::debug_stats().total_renders;
    let page = crate::side_panel::SidePanelPage {
        id: "height_cache_demo".to_string(),
        title: "Height Cache Demo".to_string(),
        file_path: "height_cache_demo.md".to_string(),
        format: crate::side_panel::SidePanelPageFormat::Markdown,
        source: crate::side_panel::SidePanelPageSource::Managed,
        content: "# Demo\n\nThis side panel should only parse markdown once for a stable width."
            .to_string(),
        updated_at_ms: 9,
    };

    let _first = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 28, 18), false, false);
    let after_first = markdown::debug_stats().total_renders;
    let _second = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 28, 26), false, false);
    let after_second = markdown::debug_stats().total_renders;

    assert!(
        after_first > before,
        "expected initial render to parse markdown"
    );
    assert_eq!(
        after_second, after_first,
        "height-only cache miss should not trigger another markdown render"
    );
}

#[test]
fn render_side_panel_content_change_with_same_revision_invalidates_cache() {
    clear_side_panel_render_caches();

    let first_page = crate::side_panel::SidePanelPage {
        id: "cache_invalidation_demo".to_string(),
        title: "Cache Invalidation Demo".to_string(),
        file_path: "cache_invalidation_demo.md".to_string(),
        format: crate::side_panel::SidePanelPageFormat::Markdown,
        source: crate::side_panel::SidePanelPageSource::Managed,
        content: "# First version".to_string(),
        updated_at_ms: 1,
    };
    let second_page = crate::side_panel::SidePanelPage {
        content: "# Second version".to_string(),
        ..first_page.clone()
    };

    let first =
        render_side_panel_markdown_cached(&first_page, Rect::new(0, 0, 28, 12), false, false);
    let second =
        render_side_panel_markdown_cached(&second_page, Rect::new(0, 0, 28, 12), false, false);

    let first_text: Vec<String> = first
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    let second_text: Vec<String> = second
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    assert!(
        first_text.iter().any(|line| line.contains("First version")),
        "expected first render to contain the original content: {:?}",
        first_text
    );
    assert!(
        second_text
            .iter()
            .any(|line| line.contains("Second version")),
        "expected second render to invalidate the stale cache entry: {:?}",
        second_text
    );
}

#[test]
fn prewarm_focused_side_panel_reuses_markdown_cache_on_first_draw() {
    clear_side_panel_render_caches();
    let before = markdown::debug_stats().total_renders;
    let snapshot = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("prewarm_demo".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "prewarm_demo".to_string(),
            title: "Prewarm Demo".to_string(),
            file_path: "prewarm_demo.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Demo\n\nThis should be warm before first draw.".to_string(),
            updated_at_ms: 7,
        }],
    };

    assert!(prewarm_focused_side_panel(
        &snapshot, 120, 40, 40, false, false
    ));
    let after_prewarm = markdown::debug_stats().total_renders;
    let page = snapshot.focused_page().expect("focused page");
    let pane_area = estimate_side_panel_pane_area(120, 40, 40).expect("side panel area");
    let inner = side_panel_content_area(pane_area).expect("side panel content area");
    let _ = render_side_panel_markdown_cached(&page, inner, false, false);
    let after_draw = markdown::debug_stats().total_renders;

    assert!(
        after_prewarm > before,
        "expected prewarm to render markdown once"
    );
    assert_eq!(
        after_draw, after_prewarm,
        "expected first draw to reuse prewarmed markdown cache"
    );
}

#[test]
fn render_side_panel_managed_pages_ignore_disk_file_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("managed.md");
    std::fs::write(&file_path, "# Disk Version").expect("write disk content");

    let page = crate::side_panel::SidePanelPage {
        id: "managed_demo".to_string(),
        title: "Managed Demo".to_string(),
        file_path: file_path.display().to_string(),
        format: crate::side_panel::SidePanelPageFormat::Markdown,
        source: crate::side_panel::SidePanelPageSource::Managed,
        content: "# In Memory".to_string(),
        updated_at_ms: 42,
    };

    let rendered = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
    let text: Vec<String> = rendered
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    assert!(
        text.iter().any(|line| line.contains("In Memory")),
        "expected managed side panel to render snapshot content: {:?}",
        text
    );
    assert!(
        !text.iter().any(|line| line.contains("Disk Version")),
        "managed side panel should not re-read disk content: {:?}",
        text
    );
}

#[test]
fn render_side_panel_linked_file_missing_file_falls_back_to_snapshot_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("linked.md");

    let page = crate::side_panel::SidePanelPage {
        id: "linked_missing_demo".to_string(),
        title: "Linked Missing Demo".to_string(),
        file_path: file_path.display().to_string(),
        format: crate::side_panel::SidePanelPageFormat::Markdown,
        source: crate::side_panel::SidePanelPageSource::LinkedFile,
        content: "# Snapshot Fallback".to_string(),
        updated_at_ms: 7,
    };

    let rendered = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
    let text: Vec<String> = rendered
        .lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    assert!(
        text.iter().any(|line| line.contains("Snapshot Fallback")),
        "expected linked side panel to fall back to snapshot content when file is missing: {:?}",
        text
    );
}
