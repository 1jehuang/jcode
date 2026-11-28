mod overview_summary {
    use super::*;

    fn summary_counts(summary: crate::tui::info_widget::TodosSummary) -> [usize; 5] {
        [
            summary.total,
            summary.completed,
            summary.in_progress,
            summary.pending,
            summary.blocked,
        ]
    }

    #[test]
    fn todo_summary_counts_statuses_and_only_incomplete_blockers() {
        assert!(remote_cache::summarize_todos(&[]).is_none());
        let todos: Vec<_> = ["completed", "in_progress", "pending", "pending"]
            .into_iter()
            .enumerate()
            .map(|(index, status)| {
                let mut item = todo(&index.to_string(), "Task", status, "low", None, None);
                if index < 3 {
                    item.blocked_by = vec!["dependency".into()];
                }
                item
            })
            .collect();
        assert_eq!(
            summary_counts(remote_cache::summarize_todos(&todos).unwrap()),
            [4, 1, 1, 2, 2]
        );
    }

    #[test]
    fn remote_todo_summary_filters_cancelled_and_preserves_unchanged_cache() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some("summary-session".into());
        let todos: Vec<_> = ["completed", "in_progress", "pending", "cancelled"]
            .into_iter()
            .enumerate()
            .map(|(index, status)| {
                let mut item = todo(&index.to_string(), "Task", status, "low", None, None);
                if status == "pending" || status == "cancelled" {
                    item.blocked_by = vec!["dependency".into()];
                }
                item
            })
            .collect();
        let payload = todo_card_payload_json(&todos, &plan(), &[]);
        assert!(app.receive_remote_todos("summary-session".into(), payload.clone()));
        let state = app.current_todo_state();
        assert_eq!(state.0.len(), 3);
        assert!(state.0.iter().all(|todo| todo.status != "cancelled"));
        assert_eq!(
            summary_counts(app.overview_todos_summary().unwrap()),
            [3, 1, 1, 1, 1]
        );
        assert!(!app.receive_remote_todos("summary-session".into(), payload.clone()));
        assert!(std::sync::Arc::ptr_eq(&state, &app.current_todo_state()));
        assert_eq!(app.remote_todos_payload.as_ref().unwrap().1, payload);
        assert_eq!(
            summary_counts(app.overview_todos_summary().unwrap()),
            [3, 1, 1, 1, 1]
        );
        assert!(!app.receive_remote_todos("other-session".into(), "{}".into()));
        assert!(std::sync::Arc::ptr_eq(&state, &app.current_todo_state()));
        assert_eq!(app.remote_todos_payload.as_ref().unwrap().1, payload);
        assert_eq!(
            summary_counts(app.overview_todos_summary().unwrap()),
            [3, 1, 1, 1, 1]
        );
    }

    #[test]
    fn remote_todo_replacements_update_and_clear_summary() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some("summary-session".into());
        for empty in [r#"{"todos":[],"goals":[]}"#, "{", r#"{"todos":false}"#] {
            let pending = todo("1", "Task", "pending", "low", None, None);
            let payload = todo_card_payload_json(&[pending], &plan(), &[]);
            assert!(app.receive_remote_todos("summary-session".into(), payload));
            assert_eq!(
                summary_counts(app.overview_todos_summary().unwrap()),
                [1, 0, 0, 1, 0]
            );
            let mut completed = todo("1", "Task", "completed", "low", None, None);
            completed.blocked_by = vec!["old dependency".into()];
            let payload = todo_card_payload_json(&[completed], &plan(), &[]);
            assert!(app.receive_remote_todos("summary-session".into(), payload));
            assert_eq!(
                summary_counts(app.overview_todos_summary().unwrap()),
                [1, 1, 0, 0, 0]
            );
            assert!(app.receive_remote_todos("summary-session".into(), empty.into()));
            assert!(app.current_todo_state().0.is_empty());
            assert!(app.remote_todos_cache.borrow().summary.is_none());
            assert!(app.overview_todos_summary().is_none());
        }
    }

    #[test]
    fn resume_todo_snapshot_supplies_overview_before_history() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_remote = true;
        app.remote_session_id = None;
        app.resume_session_id = Some("resume-summary".into());
        let item = todo("1", "Resume task", "in_progress", "low", None, None);
        let payload = todo_card_payload_json(&[item], &plan(), &[]);
        assert!(app.receive_remote_todos("resume-summary".into(), payload));
        assert!(app.remote_session_id.is_none());
        assert_eq!(
            summary_counts(app.overview_todos_summary().unwrap()),
            [1, 0, 1, 0, 0]
        );
        app.resume_session_id = Some("different-session".into());
        assert!(app.overview_todos_summary().is_none());
        assert!(app.current_todo_state().0.is_empty());
    }

    #[test]
    fn live_status_elapsed_and_tokens_invalidate_pinned_payload() {
        let mut agent = crate::tui::ui::TodoCardAgent {
            live_status: Some(crate::protocol::SwarmLiveStatus {
                phase: "running_tool".into(),
                elapsed_ms: 12_000,
                input_tokens: Some(123),
                ..Default::default()
            }),
            activity_age_secs: Some(1),
            ..Default::default()
        };
        let before = hash_todo_card_agents(&[agent.clone()]);
        advance_todo_agent_elapsed(&mut agent, std::time::Duration::from_secs(5));
        assert_eq!(agent.live_status.as_ref().unwrap().elapsed_ms, 17_000);
        assert_eq!(agent.activity_age_secs, Some(6));
        let later = hash_todo_card_agents(&[agent.clone()]);
        assert_ne!(before, later);
        agent.live_status.as_mut().unwrap().output_tokens = Some(456);
        assert_ne!(later, hash_todo_card_agents(&[agent]));
    }

    #[test]
    fn shortcut_changes_invalidate_pinned_payload() {
        let mut agent = crate::tui::ui::TodoCardAgent {
            shortcut: Some(1),
            ..Default::default()
        };
        let before = hash_todo_card_agents(&[agent.clone()]);
        agent.shortcut = Some(0);
        assert_ne!(before, hash_todo_card_agents(&[agent]));
    }

    #[test]
    fn worker_item_status_changes_invalidate_pinned_payload() {
        let mut agent = crate::tui::ui::TodoCardAgent {
            todo_statuses: vec!["pending".into(), "in_progress".into()],
            ..Default::default()
        };
        let before = hash_todo_card_agents(&[agent.clone()]);
        agent.todo_statuses.swap(0, 1);
        assert_ne!(before, hash_todo_card_agents(&[agent]));
    }

    #[test]
    fn wait_and_activity_changes_invalidate_pinned_payload() {
        let mut agent = crate::tui::ui::TodoCardAgent::default();
        let before = hash_todo_card_agents(&[agent.clone()]);
        agent.wait_reason = Some("Waiting for build".into());
        let waiting = hash_todo_card_agents(&[agent.clone()]);
        assert_ne!(before, waiting);
        agent.wait_age_secs = Some(120);
        let aged = hash_todo_card_agents(&[agent.clone()]);
        assert_ne!(waiting, aged);
        agent.wait_age_secs = Some(125);
        assert_eq!(aged, hash_todo_card_agents(&[agent.clone()]));
        agent.activity_age_secs = Some(180);
        assert_ne!(aged, hash_todo_card_agents(&[agent.clone()]));
        agent.wait_reason = None;
        assert_ne!(waiting, hash_todo_card_agents(&[agent]));
    }
}
