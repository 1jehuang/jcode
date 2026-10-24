// A dependency wake is delivered by the todo tool, never synthesized by the TUI.
fn waiting_dependency_test_app() -> App {
    let mut app = create_test_app();
    app.auto_poke_incomplete_todos = true;
    app.auto_poke_default_on = true;
    app
}

fn dependency_todo(id: &str, task_id: &str) -> crate::todo::TodoItem {
    crate::todo::TodoItem {
        id: id.into(),
        content: format!("Finish {id}"),
        status: "pending".into(),
        priority: "high".into(),
        waiting_on: vec![task_id.into()],
        ..Default::default()
    }
}

struct DependencyTaskFixture(std::path::PathBuf);

impl Drop for DependencyTaskFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn live_dependency_fixture(app: &App, task: &str) -> DependencyTaskFixture {
    let path = crate::background::global().status_path_for(task);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "task_id": task, "tool_name": "bash", "session_id": app.session.id,
            "status": "running", "started_at": "2026-10-24T09:00:00Z",
            "owner_pid": std::process::id()
        }))
        .unwrap(),
    )
    .unwrap();
    DependencyTaskFixture(path)
}

#[test]
fn background_dependency_waiting_only_suppresses_review_and_resets_budget() {
    with_temp_jcode_home(|| {
        let mut app = waiting_dependency_test_app();
        let task = format!("tui-wait-{}", app.session.id);
        let _fixture = live_dependency_fixture(&app, &task);
        app.upsert_running_background_task(task.clone(), "Build".into(), None);
        crate::todo::save_todos(&app.session.id, &[dependency_todo("waiting", &task)]).unwrap();
        app.auto_poke_attempts = crate::todo::INCOMPLETE_TODO_MAX_ATTEMPTS;
        app.last_auto_poke_fingerprint = Some("old state".into());
        app.todo_completion_gate_attempts = 2;
        let review_path = crate::todo::store::todo_file_path(
            &crate::storage::jcode_dir().unwrap(),
            &app.session.id,
            crate::todo::store::TodoFile::ReviewState,
        );
        crate::storage::write_json_fast(
            &review_path,
            &serde_json::json!({
                "cycle_started_at": "2000-01-01T00:00:00Z", "review_delivered": false
            }),
        )
        .unwrap();
        assert!(!app.schedule_auto_poke_followup_if_needed());
        assert!(!app.schedule_turn_end_followups());
        assert!(app.queued_messages.is_empty());
        assert!(!app.pending_queued_dispatch);
        assert_eq!(app.auto_poke_attempts, 0);
        assert!(app.last_auto_poke_fingerprint.is_none());
        assert_eq!(app.todo_completion_gate_attempts, 2);
        // The due timed review is still available after the dependency resolves.
        assert!(crate::todo::take_long_session_review_if_due(&app.session.id).unwrap());
    });
}

#[test]
fn background_dependency_mixed_poke_targets_only_actionable_ids() {
    with_temp_jcode_home(|| {
        let mut app = waiting_dependency_test_app();
        let task = format!("tui-wait-{}", app.session.id);
        let _fixture = live_dependency_fixture(&app, &task);
        app.upsert_running_background_task(task.clone(), "Build".into(), None);
        let mut actionable = dependency_todo("ready-id", &task);
        actionable.waiting_on.clear();
        crate::todo::save_todos(
            &app.session.id,
            &[dependency_todo("waiting-id", &task), actionable],
        )
        .unwrap();
        assert!(app.schedule_auto_poke_followup_if_needed());
        assert_eq!(app.queued_messages.len(), 1);
        assert!(super::commands::is_poke_message(&app.queued_messages[0]));
        assert!(app.queued_messages[0].contains("- ready-id:"));
        assert!(!app.queued_messages[0].contains("waiting-id"));
        assert_eq!(app.auto_poke_attempts, 1);
    });
}

#[test]
fn background_dependency_terminal_or_unknown_resumes_actionable_work() {
    with_temp_jcode_home(|| {
        for terminal in [
            None,
            Some(crate::tui::BackgroundTaskRowStatus::Completed),
            Some(crate::tui::BackgroundTaskRowStatus::Failed),
        ] {
            let mut app = waiting_dependency_test_app();
            let task = format!("tui-wait-{}", app.session.id);
            crate::todo::save_todos(&app.session.id, &[dependency_todo("resume-id", &task)])
                .unwrap();
            if let Some(status) = terminal {
                let _fixture = live_dependency_fixture(&app, &task);
                app.upsert_running_background_task(task.clone(), "Build".into(), None);
                assert!(!app.schedule_auto_poke_followup_if_needed());
                app.finish_background_task(task, "Build".into(), status);
                // The retained row itself must not enqueue a duplicate wake.
                assert!(app.queued_messages.is_empty());
            }
            assert!(app.schedule_auto_poke_followup_if_needed());
            assert_eq!(app.queued_messages.len(), 1);
            assert_eq!(app.auto_poke_attempts, 1);
        }
    });
}

#[test]
fn background_dependency_stale_row_missing_or_malformed_state_is_actionable() {
    with_temp_jcode_home(|| {
        for malformed in [false, true] {
            let mut app = waiting_dependency_test_app();
            let task = format!("tui-stale-{}", app.session.id);
            let fixture = live_dependency_fixture(&app, &task);
            if malformed {
                std::fs::write(&fixture.0, b"not valid task state").unwrap();
            } else {
                std::fs::remove_file(&fixture.0).unwrap();
            }
            app.upsert_running_background_task(task.clone(), "Stale build".into(), None);
            crate::todo::save_todos(&app.session.id, &[dependency_todo("recover", &task)]).unwrap();
            assert!(app.schedule_auto_poke_followup_if_needed());
            assert_eq!(app.queued_messages.len(), 1);
        }
    });
}

#[test]
fn background_dependency_dead_owner_or_detached_child_is_actionable() {
    with_temp_jcode_home(|| {
        // This PID is outside the OS PID range and cannot name a live process.
        let dead_pid = i32::MAX as u32;
        assert!(!crate::platform::is_process_running(dead_pid));
        for (detached, owner) in [
            (false, Some(dead_pid)),
            (true, Some(dead_pid)),
            (false, None),
        ] {
            let mut app = waiting_dependency_test_app();
            let task = format!("tui-orphan-{}", app.session.id);
            let fixture = live_dependency_fixture(&app, &task);
            let mut status: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&fixture.0).unwrap()).unwrap();
            status["detached"] = detached.into();
            status[if detached { "pid" } else { "owner_pid" }] = owner.into();
            std::fs::write(&fixture.0, serde_json::to_vec(&status).unwrap()).unwrap();
            app.upsert_running_background_task(task.clone(), "Orphan build".into(), None);
            crate::todo::save_todos(&app.session.id, &[dependency_todo("recover", &task)]).unwrap();
            assert!(app.schedule_auto_poke_followup_if_needed());
            assert_eq!(app.queued_messages.len(), 1);
        }
    });
}

#[test]
fn background_dependency_card_refresh_tracks_only_live_task_progress() {
    with_temp_jcode_home(|| {
        let mut app = waiting_dependency_test_app();
        let task = format!("tui-card-{}", app.session.id);
        let _fixture = live_dependency_fixture(&app, &task);
        let todo = dependency_todo("waiting", &task);
        crate::todo::save_todos(&app.session.id, &[todo.clone()]).unwrap();
        app.upsert_running_background_task(task.clone(), "Build 🦀".into(), Some(25.0));
        app.show_todo_card();
        let payload: serde_json::Value =
            serde_json::from_str(&app.display_messages.last().unwrap().content).unwrap();
        assert_eq!(payload["background_tasks"][0]["percent"], 25.0);
        app.upsert_running_background_task(task.clone(), "Build 🦀".into(), Some(65.0));
        assert!(app.refresh_todo_card_if_needed());
        let payload: serde_json::Value =
            serde_json::from_str(&app.display_messages.last().unwrap().content).unwrap();
        assert_eq!(payload["background_tasks"][0]["percent"], 65.0);
        app.finish_background_task(
            task,
            "Build 🦀".into(),
            crate::tui::BackgroundTaskRowStatus::Completed,
        );
        assert!(app.refresh_todo_card_if_needed());
        let payload: serde_json::Value =
            serde_json::from_str(&app.display_messages.last().unwrap().content).unwrap();
        assert_eq!(payload["background_tasks"], serde_json::json!([]));
        assert!(app.todo_card_background_tasks(&[todo]).is_empty());
    });
}

#[test]
fn background_dependency_reload_reads_session_scoped_task_state() {
    with_temp_jcode_home(|| {
        let mut app = waiting_dependency_test_app();
        let task = format!("tui-wait-{}", app.session.id);
        let path = crate::background::global().status_path_for(&task);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut status = serde_json::json!({
            "task_id": task, "tool_name": "bash", "session_id": app.session.id,
            "status": "running", "started_at": "2026-10-24T09:00:00Z",
            "owner_pid": std::process::id()
        });
        std::fs::write(&path, serde_json::to_vec(&status).unwrap()).unwrap();
        let todo = dependency_todo("reload-id", &task);
        crate::todo::save_todos(&app.session.id, &[todo.clone()]).unwrap();
        assert!(app.background_task_rows.is_empty());
        assert!(!app.schedule_auto_poke_followup_if_needed());
        app.finish_background_task(
            task.clone(),
            "Build".into(),
            crate::tui::BackgroundTaskRowStatus::Completed,
        );
        assert!(!app.todo_waits_on_background_task(&todo));
        app.upsert_running_background_task(task.clone(), "Build".into(), None);
        status["session_id"] = "other-session".into();
        std::fs::write(&path, serde_json::to_vec(&status).unwrap()).unwrap();
        assert!(!app.todo_waits_on_background_task(&todo));
        status["session_id"] = app.session.id.clone().into();
        status["status"] = "failed".into();
        std::fs::write(&path, serde_json::to_vec(&status).unwrap()).unwrap();
        // Persisted completion overrides a stale running row.
        app.upsert_running_background_task(task, "Build".into(), None);
        assert!(app.schedule_auto_poke_followup_if_needed());
        std::fs::remove_file(path).unwrap();
    });
}
