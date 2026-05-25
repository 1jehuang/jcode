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
