use super::*;

#[test]
fn cloud_overview_reflows_when_git_loading_finishes_in_short_viewport() {
    use super::super::info_widget_layout::calculate_placements_anchored;
    let now = std::time::SystemTime::now();
    let mut data = InfoWidgetData {
        session_name: Some("meadow duckling".into()),
        usage_info: Some(UsageInfo {
            provider: UsageProvider::CostBased,
            available: true,
            ..Default::default()
        }),
        cloud_info: Some(CloudStatusInfo {
            host: "cloud-git-fixture".into(),
            remote_dir: "/remote-repo".into(),
            local_dir: Some("/local-repo".into()),
            local_is_source: true,
            started_at: now,
            last_success: Some(now),
            probe_duration_ms: Some(135),
            state: CloudProbeState::Ready,
            machine: None,
            last_transfer_at: None,
        }),
        dual_git_info: Some(DualGitInfo::default()),
        ..Default::default()
    };
    // The real 180x55 terminal's startup transcript leaves a 28-row messages
    // viewport. Both cards use its right margin, not the unused screen below.
    let area = Rect::new(0, 0, 180, 28);
    let margins = Margins {
        right_widths: vec![40; 28],
        ..Default::default()
    };
    let initial = calculate_placements_with_cloud_reflow(area, &margins, &data, true, &[]);
    let before = initial
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .unwrap();
    assert_eq!(before.rect.height, 7);
    assert!(
        initial
            .visible
            .iter()
            .any(|p| p.kind == WidgetKind::CloudVm)
    );
    let repo = |path: &str| {
        RepoState::Ready(GitInfo {
            branch: "main".into(),
            untracked: 1,
            dirty_total: 1,
            dirty_files: vec![DirtyFile::new('?', path)],
            ..Default::default()
        })
    };
    data.dual_git_info = Some(DualGitInfo {
        local_is_source: true,
        local: repo("local-only.txt"),
        cloud: repo("remote-only.txt"),
    });
    // Reproduce the regression: ordinary anchoring cannot grow into the cloud
    // card above it or past the messages viewport below it.
    let blocked = calculate_placements_anchored(area, &margins, &data, true, &initial.anchors);
    assert_eq!(
        blocked
            .visible
            .iter()
            .find(|p| p.kind == WidgetKind::Overview)
            .unwrap()
            .rect
            .height,
        7
    );
    let repaired =
        calculate_placements_with_cloud_reflow(area, &margins, &data, true, &initial.anchors);
    let overview = repaired
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .unwrap();
    assert_eq!(overview.rect.height, 9);
    let cloud = repaired
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::CloudVm)
        .unwrap();
    assert_eq!(cloud.rect.height, 16);
    assert!(overview.rect.intersection(cloud.rect).is_empty());
    let inner = Rect::new(0, 0, overview.rect.width - 2, overview.rect.height - 2);
    let lines = render_sections(&data, inner, None, now_secs());
    assert_eq!(lines.len(), inner.height as usize);
    let visible = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in ["remote-only.txt", "local-only.txt", "Cloud: Ready"] {
        assert!(visible.contains(expected), "missing {expected}: {visible}");
    }
    // Once sized, the next frame keeps stable resident positions.
    let stable =
        calculate_placements_with_cloud_reflow(area, &margins, &data, true, &repaired.anchors);
    for p in &repaired.visible {
        assert_eq!(
            stable
                .visible
                .iter()
                .find(|s| s.kind == p.kind)
                .unwrap()
                .rect,
            p.rect
        );
    }
    data.cloud_info = None;
    let ordinary = calculate_placements_anchored(area, &margins, &data, true, &repaired.anchors);
    let local =
        calculate_placements_with_cloud_reflow(area, &margins, &data, true, &repaired.anchors);
    assert_eq!(local.visible.len(), ordinary.visible.len());
    for (actual, expected) in local.visible.iter().zip(&ordinary.visible) {
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(actual.rect, expected.rect);
        assert_eq!(actual.side, expected.side);
    }
    assert!(!local.visible.iter().any(|p| p.kind == WidgetKind::CloudVm));
}
