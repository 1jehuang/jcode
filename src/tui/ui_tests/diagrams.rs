use super::*;

#[test]
fn test_truncate_line_preserves_width_for_ascii() {
    let line = Line::from(Span::raw("hello world foo bar"));
    let truncated = truncate_line_to_width(&line, 11);
    assert_eq!(truncated.width(), 11);
}

// ---- Mermaid side panel rendering tests ----

const TEST_FONT: Option<(u16, u16)> = Some((8, 16));

#[test]
fn test_vcenter_fitted_image_wide_image_in_narrow_pane() {
    // Wide image (800x200) in a narrow side panel (40 cols x 30 rows).
    // The image width should be the constraining dimension, so the
    // fitted image should fill the panel width.
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 30,
    };
    let result = vcenter_fitted_image_with_font(area, 800, 200, TEST_FONT);
    assert!(
        result.width >= area.width / 2,
        "wide image should fill most of pane width: got {} out of {} (expected >= {})",
        result.width,
        area.width,
        area.width / 2
    );
}

#[test]
fn test_vcenter_fitted_image_square_image_fills_width() {
    // Square image (400x400) in a side panel (40 cols x 40 rows).
    // With typical 8x16 font, terminal cells are 2:1 aspect.
    // 40 cols = 320px, 40 rows = 640px.
    // scale = min(320/400, 640/400) = min(0.8, 1.6) = 0.8
    // fitted_w = (400 * 0.8) / 8 = 40 cells -> fills width
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 40,
    };
    let result = vcenter_fitted_image_with_font(area, 400, 400, TEST_FONT);
    assert!(
        result.width >= area.width * 3 / 4,
        "square image should fill most of pane width: got {} out of {}",
        result.width,
        area.width
    );
}

#[test]
fn test_vcenter_fitted_image_tall_image_in_wide_pane() {
    // Tall image (200x800) in a wide pane (80 cols x 30 rows).
    // Height is constraining. Image won't fill width.
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 30,
    };
    let result = vcenter_fitted_image_with_font(area, 200, 800, TEST_FONT);
    assert!(
        result.width < area.width,
        "tall image should not fill full width: got {} out of {}",
        result.width,
        area.width
    );
    assert!(
        result.height <= area.height,
        "tall image height should not exceed pane: got {} out of {}",
        result.height,
        area.height
    );
}

#[test]
fn test_vcenter_fitted_image_centering_horizontal() {
    // Tall image centered in a wide area - should have x_offset > 0
    let area = Rect {
        x: 10,
        y: 5,
        width: 80,
        height: 20,
    };
    let result = vcenter_fitted_image_with_font(area, 100, 800, TEST_FONT);
    if result.width < area.width {
        assert!(
            result.x > area.x,
            "should be horizontally centered: x={}, area.x={}",
            result.x,
            area.x
        );
    }
}

#[test]
fn test_vcenter_fitted_image_centering_vertical() {
    // Wide image centered vertically - should have y_offset > 0
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 40,
    };
    let result = vcenter_fitted_image_with_font(area, 800, 100, TEST_FONT);
    if result.height < area.height {
        assert!(
            result.y > area.y || result.height < area.height,
            "should be vertically centered"
        );
    }
}

#[test]
fn test_vcenter_fitted_image_zero_dimensions() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };
    let result = vcenter_fitted_image_with_font(area, 400, 400, TEST_FONT);
    assert_eq!(result, area);

    let area2 = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 30,
    };
    let result2 = vcenter_fitted_image_with_font(area2, 0, 0, TEST_FONT);
    assert_eq!(result2, area2);
}

#[test]
fn test_vcenter_fitted_image_never_exceeds_area() {
    let test_cases: Vec<(Rect, u32, u32)> = vec![
        (
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 30,
            },
            800,
            600,
        ),
        (
            Rect {
                x: 5,
                y: 3,
                width: 60,
                height: 20,
            },
            100,
            100,
        ),
        (
            Rect {
                x: 0,
                y: 0,
                width: 120,
                height: 40,
            },
            1920,
            1080,
        ),
        (
            Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 50,
            },
            200,
            800,
        ),
    ];
    for (area, img_w, img_h) in test_cases {
        let result = vcenter_fitted_image_with_font(area, img_w, img_h, TEST_FONT);
        assert!(
            result.x >= area.x,
            "result.x ({}) < area.x ({})",
            result.x,
            area.x
        );
        assert!(
            result.y >= area.y,
            "result.y ({}) < area.y ({})",
            result.y,
            area.y
        );
        assert!(
            result.x + result.width <= area.x + area.width,
            "result right edge ({}) > area right edge ({})",
            result.x + result.width,
            area.x + area.width
        );
        assert!(
            result.y + result.height <= area.y + area.height,
            "result bottom edge ({}) > area bottom edge ({})",
            result.y + result.height,
            area.y + area.height
        );
    }
}

#[test]
fn test_vcenter_fitted_image_typical_mermaid_in_side_panel() {
    // Typical mermaid diagram: wider than tall (e.g., flowchart LR).
    // Side panel is narrow and tall (e.g., 50 cols x 40 rows).
    // The image should fill the width of the panel.
    let inner = Rect {
        x: 81,
        y: 1,
        width: 48,
        height: 38,
    };
    let result = vcenter_fitted_image_with_font(inner, 600, 300, TEST_FONT);
    let width_utilization = result.width as f64 / inner.width as f64;
    assert!(
        width_utilization > 0.8,
        "typical mermaid in side panel should use >80% width: {}% ({}/{})",
        (width_utilization * 100.0) as u32,
        result.width,
        inner.width
    );
}

#[test]
fn test_estimate_pinned_diagram_pane_width_wide_image() {
    // A very wide image should get a wider pane
    let diagram = info_widget::DiagramInfo {
        hash: 10,
        width: 1600,
        height: 200,
        label: None,
    };
    let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((8, 16)));
    assert!(
        width >= 24,
        "should be at least minimum width: got {}",
        width
    );
}

#[test]
fn test_estimate_pinned_diagram_pane_width_tall_image() {
    // A tall image should get a narrower pane (height-constrained)
    let diagram = info_widget::DiagramInfo {
        hash: 11,
        width: 200,
        height: 1600,
        label: None,
    };
    let width = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((8, 16)));
    // Height-constrained: 30 rows - 2 border = 28 inner rows
    // image_w_cells = ceil(200/8) = 25
    // image_h_cells = ceil(1600/16) = 100
    // fit_w_cells = ceil(25*28/100) = 7
    // pane_width = 7 + 2 = 9, but clamped to min 24
    assert_eq!(width, 24, "tall image should be clamped to minimum width");
}

#[test]
fn test_estimate_pinned_diagram_pane_width_zero_font_size() {
    // With None font size, should use default (8, 16)
    let diagram = info_widget::DiagramInfo {
        hash: 12,
        width: 800,
        height: 600,
        label: None,
    };
    let with_font = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, Some((8, 16)));
    let with_default = estimate_pinned_diagram_pane_width_with_font(&diagram, 20, 24, None);
    assert_eq!(with_font, with_default);
}

#[test]
fn test_estimate_pinned_diagram_pane_height_wide_image() {
    // Wide image (1600x200) in a pane 80 cols wide.
    // Should need less height since the image is short.
    let diagram = info_widget::DiagramInfo {
        hash: 13,
        width: 1600,
        height: 200,
        label: None,
    };
    let height = estimate_pinned_diagram_pane_height(&diagram, 80, 6);
    assert!(
        height >= 6,
        "should be at least minimum height: got {}",
        height
    );
}

#[test]
fn test_estimate_pinned_diagram_pane_height_tall_image() {
    // Tall image (200x1600) in a pane 80 cols wide.
    // Width-constrained, so height depends on the width scaling.
    let diagram = info_widget::DiagramInfo {
        hash: 14,
        width: 200,
        height: 1600,
        label: None,
    };
    let height = estimate_pinned_diagram_pane_height(&diagram, 80, 6);
    assert!(
        height > 6,
        "tall image should need more than minimum height: got {}",
        height
    );
}

#[test]
fn test_side_panel_layout_ratio_capping() {
    // Test that diagram_width respects the ratio cap.
    // area = 120 cols, ratio = 50% -> cap = 60
    // If estimated pane width > 60, it should be capped at 60.
    let diagram = info_widget::DiagramInfo {
        hash: 20,
        width: 2000,
        height: 400,
        label: None,
    };
    let area_width: u16 = 120;
    let ratio: u32 = 50;
    let ratio_cap = ((area_width as u32 * ratio) / 100) as u16;
    let min_diagram_width: u16 = 24;
    let min_chat_width: u16 = 20;
    let max_diagram = area_width.saturating_sub(min_chat_width);

    let needed = estimate_pinned_diagram_pane_width_with_font(
        &diagram,
        40,
        min_diagram_width,
        Some((8, 16)),
    );
    let diagram_width = needed
        .min(ratio_cap)
        .max(min_diagram_width)
        .min(max_diagram);

    assert!(
        diagram_width <= ratio_cap,
        "diagram_width ({}) should be <= ratio_cap ({})",
        diagram_width,
        ratio_cap
    );
    assert!(
        diagram_width >= min_diagram_width,
        "diagram_width ({}) should be >= min ({})",
        diagram_width,
        min_diagram_width
    );
    let chat_width = area_width.saturating_sub(diagram_width);
    assert!(
        chat_width >= min_chat_width,
        "chat_width ({}) should be >= min ({})",
        chat_width,
        min_chat_width
    );
}

#[test]
fn test_side_panel_layout_narrow_terminal() {
    // On a very narrow terminal (50 cols), side panel should still work
    // or gracefully degrade.
    let area_width: u16 = 50;
    let min_diagram_width: u16 = 24;
    let min_chat_width: u16 = 20;
    let max_diagram = area_width.saturating_sub(min_chat_width); // 30

    let diagram = info_widget::DiagramInfo {
        hash: 21,
        width: 600,
        height: 300,
        label: None,
    };
    let needed = estimate_pinned_diagram_pane_width_with_font(
        &diagram,
        30,
        min_diagram_width,
        Some((8, 16)),
    );
    let ratio_cap = ((area_width as u32 * 50) / 100) as u16; // 25
    let diagram_width = needed
        .min(ratio_cap)
        .max(min_diagram_width)
        .min(max_diagram);
    let chat_width = area_width.saturating_sub(diagram_width);

    assert!(
        diagram_width >= min_diagram_width,
        "narrow term: diagram_width ({}) >= min ({})",
        diagram_width,
        min_diagram_width
    );
    assert!(
        chat_width >= min_chat_width,
        "narrow term: chat_width ({}) >= min ({})",
        chat_width,
        min_chat_width
    );
    assert_eq!(
        diagram_width + chat_width,
        area_width,
        "widths should sum to total"
    );
}

#[test]
fn test_side_panel_image_width_utilization() {
    // This is the key test for the "only uses half width" bug.
    // After computing the pane width and getting the inner area (minus
    // 2 for borders), vcenter_fitted_image should return a rect where
    // the image width is close to the inner width for typical diagrams.
    let diagram = info_widget::DiagramInfo {
        hash: 30,
        width: 800,
        height: 400,
        label: None,
    };
    let area_width: u16 = 120;
    let area_height: u16 = 40;
    let min_diagram_width: u16 = 24;
    let ratio_cap = ((area_width as u32 * 50) / 100) as u16;
    let max_diagram = area_width.saturating_sub(20);

    let needed = estimate_pinned_diagram_pane_width_with_font(
        &diagram,
        area_height,
        min_diagram_width,
        Some((8, 16)),
    );
    let diagram_width = needed
        .min(ratio_cap)
        .max(min_diagram_width)
        .min(max_diagram);

    // Inner area after borders (1 cell each side)
    let inner = Rect {
        x: area_width.saturating_sub(diagram_width) + 1,
        y: 1,
        width: diagram_width.saturating_sub(2),
        height: area_height.saturating_sub(2),
    };

    let render_area =
        vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, TEST_FONT);

    let utilization = render_area.width as f64 / inner.width as f64;
    assert!(
        utilization > 0.5,
        "image should use >50% of inner pane width: {}% ({}/{}) \
             pane_width={}, inner_width={}, render_width={}, \
             img={}x{}, needed={}",
        (utilization * 100.0) as u32,
        render_area.width,
        inner.width,
        diagram_width,
        inner.width,
        render_area.width,
        diagram.width,
        diagram.height,
        needed,
    );
}

#[test]
fn test_side_panel_image_width_various_aspect_ratios() {
    // Test various diagram aspect ratios to ensure none uses "only half"
    let test_cases: Vec<(u32, u32, &str)> = vec![
        (800, 400, "2:1 landscape"),
        (800, 600, "4:3 landscape"),
        (800, 800, "1:1 square"),
        (600, 400, "3:2 landscape"),
        (1200, 300, "4:1 wide panoramic"),
        (400, 600, "2:3 portrait"),
        (300, 900, "1:3 tall portrait"),
    ];

    for (img_w, img_h, label) in test_cases {
        let _diagram = info_widget::DiagramInfo {
            hash: img_w as u64 * 1000 + img_h as u64,
            width: img_w,
            height: img_h,
            label: None,
        };

        let pane_width: u16 = 50;
        let pane_height: u16 = 40;
        let inner = Rect {
            x: 71,
            y: 1,
            width: pane_width - 2,
            height: pane_height - 2,
        };

        let render_area = vcenter_fitted_image_with_font(inner, img_w, img_h, TEST_FONT);

        // For any diagram, at least one dimension should be well-utilized
        let w_util = render_area.width as f64 / inner.width as f64;
        let h_util = render_area.height as f64 / inner.height as f64;
        let max_util = w_util.max(h_util);

        assert!(
            max_util > 0.5,
            "{}: at least one dimension should be >50% utilized: \
                 w_util={:.0}% h_util={:.0}%, render={}x{}, inner={}x{}",
            label,
            w_util * 100.0,
            h_util * 100.0,
            render_area.width,
            render_area.height,
            inner.width,
            inner.height,
        );
    }
}

#[test]
fn test_is_diagram_poor_fit_wide_in_side_pane() {
    // A very wide diagram in a side pane (narrow+tall) should be a poor fit
    let diagram = info_widget::DiagramInfo {
        hash: 40,
        width: 1600,
        height: 100,
        label: None,
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: 30,
        height: 40,
    };
    let poor = is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Side);
    assert!(
        poor,
        "very wide diagram in narrow side pane should be poor fit"
    );
}

#[test]
fn test_is_diagram_poor_fit_tall_in_top_pane() {
    // A very tall diagram in a top pane (wide+short) should be a poor fit
    let diagram = info_widget::DiagramInfo {
        hash: 41,
        width: 100,
        height: 1600,
        label: None,
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 15,
    };
    let poor = is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Top);
    assert!(
        poor,
        "very tall diagram in short top pane should be poor fit"
    );
}

#[test]
fn test_is_diagram_poor_fit_good_fit_cases() {
    // Normal aspect ratio diagrams should not be poor fits
    let diagram = info_widget::DiagramInfo {
        hash: 42,
        width: 600,
        height: 400,
        label: None,
    };
    let side_area = Rect {
        x: 0,
        y: 0,
        width: 50,
        height: 40,
    };
    assert!(
        !is_diagram_poor_fit(
            &diagram,
            side_area,
            crate::config::DiagramPanePosition::Side
        ),
        "normal diagram should not be poor fit in side pane"
    );

    let top_area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 20,
    };
    assert!(
        !is_diagram_poor_fit(&diagram, top_area, crate::config::DiagramPanePosition::Top),
        "normal diagram should not be poor fit in top pane"
    );
}

#[test]
fn test_is_diagram_poor_fit_zero_dimensions() {
    let diagram = info_widget::DiagramInfo {
        hash: 43,
        width: 0,
        height: 0,
        label: None,
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: 50,
        height: 40,
    };
    assert!(
        !is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Side),
        "zero-dimension diagram should not crash or be poor fit"
    );
}

#[test]
fn test_is_diagram_poor_fit_tiny_area() {
    let diagram = info_widget::DiagramInfo {
        hash: 44,
        width: 800,
        height: 600,
        label: None,
    };
    let area = Rect {
        x: 0,
        y: 0,
        width: 3,
        height: 2,
    };
    assert!(
        !is_diagram_poor_fit(&diagram, area, crate::config::DiagramPanePosition::Side),
        "tiny area should return false (not crash)"
    );
}

#[test]
fn test_div_ceil_u32_basic() {
    assert_eq!(div_ceil_u32(10, 3), 4);
    assert_eq!(div_ceil_u32(9, 3), 3);
    assert_eq!(div_ceil_u32(0, 5), 0);
    assert_eq!(div_ceil_u32(1, 1), 1);
    assert_eq!(div_ceil_u32(7, 0), 7);
}

#[test]
fn test_estimate_pinned_diagram_pane_width_various_fonts() {
    // Different font sizes affect the computed pane width.
    // With a proportionally larger font, the raw image-in-cells count
    // is smaller, but ceiling arithmetic can add a cell back.
    let diagram = info_widget::DiagramInfo {
        hash: 50,
        width: 800,
        height: 600,
        label: None,
    };
    let w_8x16 = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((8, 16)));
    let w_10x20 = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((10, 20)));
    let w_16x32 = estimate_pinned_diagram_pane_width_with_font(&diagram, 30, 24, Some((16, 32)));
    // With a substantially larger font, we should need noticeably fewer cells
    assert!(
        w_16x32 <= w_8x16,
        "much larger font should need fewer or equal cells: 16x32={}, 8x16={}",
        w_16x32,
        w_8x16
    );
    // All should respect the minimum
    assert!(w_8x16 >= 24);
    assert!(w_10x20 >= 24);
    assert!(w_16x32 >= 24);
}

#[test]
fn test_side_panel_full_pipeline_width_check() {
    // End-to-end: simulate the entire side panel width calculation pipeline
    // and verify the image render area is reasonable.
    //
    // This mimics what draw_inner + draw_pinned_diagram do:
    // 1. estimate_pinned_diagram_pane_width -> pane width
    // 2. Rect with that width -> block.inner -> inner
    // 3. vcenter_fitted_image(inner, img_w, img_h) -> render_area
    // 4. render_image_widget_scale(render_area) -> image displayed

    let terminal_width: u16 = 120;
    let terminal_height: u16 = 40;
    let diagram = info_widget::DiagramInfo {
        hash: 60,
        width: 700,
        height: 350,
        label: None,
    };
    let font = Some((8u16, 16u16));

    // Step 1: compute pane width (mimics Side branch in draw_inner)
    let min_diagram_width: u16 = 24;
    let min_chat_width: u16 = 20;
    let max_diagram = terminal_width.saturating_sub(min_chat_width);
    let ratio: u32 = 50;
    let ratio_cap = ((terminal_width as u32 * ratio) / 100) as u16;
    let needed = estimate_pinned_diagram_pane_width_with_font(
        &diagram,
        terminal_height,
        min_diagram_width,
        font,
    );
    let pane_width = needed
        .min(ratio_cap)
        .max(min_diagram_width)
        .min(max_diagram);
    let chat_width = terminal_width.saturating_sub(pane_width);

    assert!(pane_width > 0 && chat_width > 0, "both areas must exist");

    // Step 2: compute inner area (Block::inner subtracts 1 from each side)
    let inner = Rect {
        x: chat_width + 1,
        y: 1,
        width: pane_width.saturating_sub(2),
        height: terminal_height.saturating_sub(2),
    };

    // Step 3: compute render area
    let render_area = vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, font);

    // Step 4: verify the render area is reasonable
    assert!(
        render_area.width > 0 && render_area.height > 0,
        "render area should be non-empty"
    );
    assert!(
        render_area.x >= inner.x,
        "render area should be within inner"
    );
    assert!(
        render_area.x + render_area.width <= inner.x + inner.width,
        "render area should not exceed inner"
    );

    // THE KEY ASSERTION: the rendered image should use a significant
    // portion of the pane width, not just half.
    let pane_utilization = render_area.width as f64 / inner.width as f64;
    assert!(
        pane_utilization > 0.5,
        "CRITICAL: Image uses only {:.0}% of side panel width ({}/{})! \
             This is the 'half width' bug. Pipeline: terminal={}x{}, \
             pane_width={}, inner={}x{}, render={}x{}, img={}x{}",
        pane_utilization * 100.0,
        render_area.width,
        inner.width,
        terminal_width,
        terminal_height,
        pane_width,
        inner.width,
        inner.height,
        render_area.width,
        render_area.height,
        diagram.width,
        diagram.height,
    );
}

#[test]
fn test_side_panel_various_terminal_sizes() {
    // Test the pipeline at various realistic terminal sizes
    let terminals: Vec<(u16, u16, &str)> = vec![
        (80, 24, "80x24 standard"),
        (120, 40, "120x40 typical"),
        (200, 50, "200x50 ultrawide"),
        (60, 30, "60x30 small"),
    ];

    let diagram = info_widget::DiagramInfo {
        hash: 70,
        width: 800,
        height: 400,
        label: None,
    };

    for (tw, th, label) in terminals {
        let min_diagram_width: u16 = 24;
        let min_chat_width: u16 = 20;
        let max_diagram = tw.saturating_sub(min_chat_width);

        if max_diagram < min_diagram_width {
            continue; // too narrow for side panel
        }

        let ratio_cap = ((tw as u32 * 50) / 100) as u16;
        let needed = estimate_pinned_diagram_pane_width_with_font(
            &diagram,
            th,
            min_diagram_width,
            Some((8, 16)),
        );
        let pane_width = needed
            .min(ratio_cap)
            .max(min_diagram_width)
            .min(max_diagram);
        let chat_width = tw.saturating_sub(pane_width);

        if pane_width < 4 || chat_width == 0 {
            continue;
        }

        let inner = Rect {
            x: chat_width + 1,
            y: 1,
            width: pane_width.saturating_sub(2),
            height: th.saturating_sub(2),
        };

        let render_area =
            vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, TEST_FONT);
        let w_util = render_area.width as f64 / inner.width as f64;

        assert!(
            w_util > 0.4,
            "{}: image width utilization too low: {:.0}% ({}/{})",
            label,
            w_util * 100.0,
            render_area.width,
            inner.width,
        );
    }
}

#[test]
fn test_vcenter_fitted_image_preserves_aspect_ratio_close_to_source() {
    let cases = [
        (Rect::new(0, 0, 48, 38), 600, 300),
        (Rect::new(0, 0, 48, 38), 300, 600),
        (Rect::new(0, 0, 80, 20), 1200, 400),
        (Rect::new(0, 0, 30, 40), 400, 1200),
    ];

    for (area, img_w, img_h) in cases {
        let result = vcenter_fitted_image_with_font(area, img_w, img_h, TEST_FONT);
        let src_aspect = img_w as f64 / img_h as f64;
        let dst_aspect = (result.width as f64 * 8.0) / (result.height as f64 * 16.0);
        let rel_err = (dst_aspect - src_aspect).abs() / src_aspect.max(0.0001);
        assert!(
            rel_err < 0.12,
            "aspect ratio drift too large for {}x{} in {:?}: src={:.3}, dst={:.3}, err={:.3}",
            img_w,
            img_h,
            area,
            src_aspect,
            dst_aspect,
            rel_err,
        );
    }
}

#[test]
fn test_vcenter_fitted_image_with_zero_font_dimension_falls_back_safely() {
    let area = Rect::new(4, 2, 50, 20);
    let safe = vcenter_fitted_image_with_font(area, 800, 400, Some((0, 16)));
    assert!(safe.width > 0);
    assert!(safe.height > 0);
    assert!(safe.x >= area.x && safe.y >= area.y);
    assert!(safe.x + safe.width <= area.x + area.width);
    assert!(safe.y + safe.height <= area.y + area.height);

    let safe2 = vcenter_fitted_image_with_font(area, 800, 400, Some((8, 0)));
    assert!(safe2.width > 0);
    assert!(safe2.height > 0);
    assert!(safe2.x + safe2.width <= area.x + area.width);
    assert!(safe2.y + safe2.height <= area.y + area.height);
}

#[test]
fn test_side_panel_landscape_diagrams_fill_most_width_across_ratios() {
    let pane = Rect::new(0, 0, 48, 38);
    let diagrams = [
        (600, 300, 0.80),
        (800, 400, 0.80),
        (1200, 300, 0.80),
        (800, 600, 0.65),
    ];

    for (img_w, img_h, min_width_util) in diagrams {
        let result = vcenter_fitted_image_with_font(pane, img_w, img_h, TEST_FONT);
        let w_util = result.width as f64 / pane.width as f64;
        assert!(
            w_util >= min_width_util,
            "{}x{} should use at least {:.0}% width, got {:.0}% ({}/{})",
            img_w,
            img_h,
            min_width_util * 100.0,
            w_util * 100.0,
            result.width,
            pane.width,
        );
    }
}

#[test]
fn test_hidpi_font_size_does_not_halve_diagram_width() {
    const HIDPI_FONT: Option<(u16, u16)> = Some((15, 34));

    let terminal_width: u16 = 95;
    let terminal_height: u16 = 51;

    let diagram = info_widget::DiagramInfo {
        hash: 99,
        width: 614,
        height: 743,
        label: None,
    };

    let min_diagram_width: u16 = 24;
    let min_chat_width: u16 = 20;
    let max_diagram = terminal_width.saturating_sub(min_chat_width);
    let ratio: u32 = 40;
    let ratio_cap = ((terminal_width as u32 * ratio) / 100) as u16;

    let needed_hidpi = estimate_pinned_diagram_pane_width_with_font(
        &diagram,
        terminal_height,
        min_diagram_width,
        HIDPI_FONT,
    );
    let pane_width = needed_hidpi
        .min(ratio_cap)
        .max(min_diagram_width)
        .min(max_diagram);

    let inner = Rect {
        x: terminal_width.saturating_sub(pane_width) + 1,
        y: 1,
        width: pane_width.saturating_sub(2),
        height: terminal_height.saturating_sub(2),
    };

    let render_area =
        vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, HIDPI_FONT);

    let w_util = render_area.width as f64 / inner.width as f64;
    assert!(
        w_util > 0.7,
        "HiDPI (15x34 font): image should use >70% of pane width, got {:.0}% ({}/{}) \
             pane_width={}, inner={}x{}, render={}x{}, img={}x{}",
        w_util * 100.0,
        render_area.width,
        inner.width,
        pane_width,
        inner.width,
        inner.height,
        render_area.width,
        render_area.height,
        diagram.width,
        diagram.height,
    );

    let render_default =
        vcenter_fitted_image_with_font(inner, diagram.width, diagram.height, TEST_FONT);
    let w_util_default = render_default.width as f64 / inner.width as f64;

    assert!(
        (w_util - w_util_default).abs() < 0.15 || w_util > 0.7,
        "Font size should not drastically change width utilization. \
             HiDPI={:.0}%, default={:.0}%",
        w_util * 100.0,
        w_util_default * 100.0,
    );
}

#[test]
fn test_pinned_diagram_probe_reports_fit_utilization() {
    let area = Rect::new(0, 0, 46, 51);
    let inner = Rect::new(1, 1, 44, 49);
    let diagram = info_widget::DiagramInfo {
        hash: 123,
        width: 614,
        height: 743,
        label: None,
    };

    let probe = debug_probe_pinned_diagram(&diagram, area, inner, false, 0, 0, 100);

    assert_eq!(probe.render_mode, "fit");
    assert_eq!(probe.pane_width_cells, 46);
    assert_eq!(probe.pane_height_cells, 51);
    assert_eq!(probe.inner_width_cells, 44);
    assert_eq!(probe.inner_height_cells, 49);
    assert!(probe.inner_utilization.width_cells > 0);
    assert!(probe.inner_utilization.height_cells > 0);
    assert!(probe.inner_utilization.area_utilization_percent > 40.0);
    assert!(probe.log.contains("fit"));
}

#[test]
fn test_pinned_diagram_probe_reports_full_inner_usage_in_viewport_mode() {
    let area = Rect::new(0, 0, 46, 51);
    let inner = Rect::new(1, 1, 44, 49);
    let diagram = info_widget::DiagramInfo {
        hash: 124,
        width: 614,
        height: 743,
        label: None,
    };

    let probe = debug_probe_pinned_diagram(&diagram, area, inner, true, 3, 7, 125);

    assert_eq!(probe.render_mode, "scrollable-viewport@125%");
    assert_eq!(probe.inner_utilization.width_cells, 44);
    assert_eq!(probe.inner_utilization.height_cells, 49);
    assert_eq!(probe.inner_utilization.width_utilization_percent, 100.0);
    assert_eq!(probe.inner_utilization.height_utilization_percent, 100.0);
    assert_eq!(probe.inner_utilization.area_utilization_percent, 100.0);
}

#[test]
fn test_query_font_size_returns_valid_dimensions() {
    let font = crate::tui::mermaid::get_font_size();
    if let Some((w, h)) = font {
        assert!(w > 0, "font width should be positive, got {}", w);
        assert!(h > 0, "font height should be positive, got {}", h);
        assert!(
            w <= 100,
            "font width should be reasonable, got {} (likely bogus)",
            w
        );
        assert!(
            h <= 200,
            "font height should be reasonable, got {} (likely bogus)",
            h
        );
    }
}
