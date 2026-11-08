use super::*;
use ratatui::backend::TestBackend;

#[test]
fn cloud_directories_visible_in_fresh_roomy_tui() {
    let _env = crate::storage::lock_test_env();
    let _lock = viewport_snapshot_test_lock();
    crate::tui::ui::clear_test_render_state_for_tests();
    info_widget::clear_widget_placements_for_tests();
    let now = std::time::SystemTime::now();
    let state = TestState {
        info_widget_data: info_widget::InfoWidgetData {
            model: Some("cloud-model".into()),
            session_name: Some("cloud acceptance".into()),
            reasoning_effort: Some("high".into()),
            context_limit: Some(200_000),
            observed_context_tokens: Some(12500),
            provider_name: Some("openai".into()),
            usage_info: Some(info_widget::UsageInfo {
                provider: info_widget::UsageProvider::OpenAI,
                available: true,
                ..Default::default()
            }),
            dual_git_info: Some(info_widget::DualGitInfo::default()),
            cloud_info: Some(info_widget::CloudStatusInfo {
                host: "cloud-fixture".into(),
                remote_dir: "/current".into(),
                local_dir: Some("/local".into()),
                local_is_source: true,
                started_at: now,
                last_success: Some(now),
                probe_duration_ms: Some(20),
                state: info_widget::CloudProbeState::Ready,
                machine: None,
                last_transfer_at: None,
                directories_stale: false,
                directories: Some(info_widget::CloudDirectorySnapshot {
                    current_path: Some("/current".into()),
                    entries: (0..8)
                        .map(|i| info_widget::CloudDirectoryEntry {
                            path: if i == 0 {
                                "/current".into()
                            } else {
                                format!("/sibling-{i}")
                            },
                            agents: if i == 0 { 3 } else { 0 },
                            busy: if i == 0 { 1 } else { 0 },
                        })
                        .collect(),
                    truncated: false,
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    for centered in [false, true] {
        let state = TestState {
            centered_mode: centered,
            ..state.clone()
        };
        crate::tui::ui::clear_test_render_state_for_tests();
        info_widget::clear_widget_placements_for_tests();
        let mut terminal = Terminal::new(TestBackend::new(180, 60)).unwrap();
        terminal.draw(|frame| draw(frame, &state)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("Cloud Directories"), "{screen}");
        assert!(screen.contains("Cloud VM"), "{screen}");
        assert!(
            screen.contains("Agents") && screen.contains("Busy"),
            "{screen}"
        );
        assert!(screen.contains("> /current"), "{screen}");
    }
}
