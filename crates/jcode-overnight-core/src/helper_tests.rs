use super::*;
use chrono::Utc;

fn task_card(id: &str, title: &str, status: &str) -> OvernightTaskCard {
    OvernightTaskCard {
        id: id.to_string(),
        title: title.to_string(),
        status: status.to_string(),
        ..Default::default()
    }
}

fn test_manifest(now: DateTime<Utc>) -> OvernightManifest {
    let run_dir = PathBuf::from("/tmp/overnight-run");
    OvernightManifest {
        version: OVERNIGHT_VERSION,
        run_id: "run-1".to_string(),
        parent_session_id: "parent".to_string(),
        coordinator_session_id: "coord".to_string(),
        coordinator_session_name: "coordinator".to_string(),
        started_at: now - chrono::Duration::minutes(60),
        target_wake_at: now + chrono::Duration::minutes(60),
        handoff_ready_at: now - chrono::Duration::minutes(10),
        post_wake_grace_until: now + chrono::Duration::hours(2),
        morning_report_posted_at: None,
        completed_at: None,
        cancel_requested_at: None,
        status: OvernightRunStatus::Running,
        mission: Some("verify <things>".to_string()),
        working_dir: Some("/tmp/project".to_string()),
        provider_name: "provider".to_string(),
        model: "model".to_string(),
        max_agents_guidance: 1,
        process_id: 123,
        run_dir: run_dir.clone(),
        events_path: run_dir.join("events.jsonl"),
        human_log_path: run_dir.join("run.log"),
        review_path: run_dir.join("review.html"),
        review_notes_path: run_dir.join("notes.md"),
        preflight_path: run_dir.join("preflight.json"),
        task_cards_dir: run_dir.join("task-cards"),
        issue_drafts_dir: run_dir.join("issues"),
        validation_dir: run_dir.join("validation"),
        last_activity_at: now - chrono::Duration::minutes(5),
    }
}

#[test]
fn summarizes_task_card_statuses_and_validation() {
    let mut completed = task_card("1", "Done", "validated");
    completed.validation.result = Some("passed".to_string());
    completed.risk = Some("high".to_string());
    let active = task_card("2", "Active", "in progress");
    let blocked = task_card("3", "Blocked", "needs user");
    let summary = summarize_task_cards_slice(&[completed, active, blocked]);
    assert_eq!(summary.total, 3);
    assert_eq!(summary.counts.completed, 1);
    assert_eq!(summary.counts.active, 1);
    assert_eq!(summary.counts.blocked, 1);
    assert_eq!(summary.validated, 1);
    assert_eq!(summary.high_risk, 1);
    assert_eq!(summary.latest_title.as_deref(), Some("Blocked"));
}

#[test]
fn task_status_bucket_normalizes_common_labels() {
    assert_eq!(task_status_bucket("in-progress"), "active");
    assert_eq!(task_status_bucket("needs user"), "blocked");
    assert_eq!(task_status_bucket("not started"), "skipped");
}

#[test]
fn escape_and_event_class_helpers_are_stable() {
    assert_eq!(
        html_escape("<tag & 'quote'>"),
        "&lt;tag &amp; &#39;quote&#39;&gt;"
    );
    assert_eq!(event_class("task_failed"), "bad");
    assert_eq!(event_class("handoff_requested"), "warn");
    assert_eq!(event_class("run_completed"), "ok");
}

#[test]
fn resource_and_git_summaries_are_compact() {
    let resources = ResourceSnapshot {
        captured_at: Utc::now(),
        memory_used_percent: Some(42.0),
        load_one: Some(1.5),
        cpu_count: Some(8),
        battery_percent: Some(77),
        battery_status: Some("Discharging".to_string()),
        ..Default::default()
    };
    assert_eq!(
        resource_summary(&resources),
        "RAM 42%, load 1.5/8, battery 77% Discharging"
    );

    let git = GitSnapshot {
        captured_at: Utc::now(),
        branch: Some("master".to_string()),
        dirty_count: Some(2),
        dirty_summary: Vec::new(),
        error: None,
    };
    assert_eq!(git_summary(&git), "master with 2 dirty files");
}

#[test]
fn format_minutes_is_human_compact() {
    assert_eq!(format_minutes(45), "45m");
    assert_eq!(format_minutes(120), "2h");
    assert_eq!(format_minutes(125), "2h 5m");
}

#[test]
fn progress_card_builder_uses_supplied_runtime_parts() {
    let now = Utc::now();
    let manifest = test_manifest(now);
    let events = vec![OvernightEvent {
        timestamp: now,
        run_id: manifest.run_id.clone(),
        session_id: Some(manifest.coordinator_session_id.clone()),
        kind: "task_completed".to_string(),
        summary: "finished setup".to_string(),
        details: serde_json::json!({}),
        meaningful: true,
    }];
    let preflight = OvernightPreflight {
        captured_at: now,
        usage: UsageProjection {
            captured_at: now,
            risk: "medium".to_string(),
            confidence: "high".to_string(),
            projected_delta_min_percent: None,
            projected_delta_max_percent: None,
            projected_end_min_percent: Some(70.0),
            projected_end_max_percent: Some(80.0),
            providers: Vec::new(),
            notes: Vec::new(),
        },
        resources: ResourceSnapshot {
            captured_at: now,
            memory_used_percent: Some(42.0),
            load_one: Some(1.5),
            cpu_count: Some(8),
            ..Default::default()
        },
        git: GitSnapshot {
            captured_at: now,
            branch: Some("master".to_string()),
            dirty_count: Some(0),
            dirty_summary: Vec::new(),
            error: None,
        },
    };
    let cards = vec![task_card("1", "Active task", "in progress")];

    let card = build_progress_card_from_parts(&manifest, &events, Some(&preflight), &cards, now);
    assert_eq!(card.phase, "wind-down");
    assert_eq!(card.progress_percent, 50.0);
    assert_eq!(card.usage_risk, "medium");
    assert_eq!(card.usage_projection, "projected 70% to 80%");
    assert_eq!(
        card.resources_summary,
        "RAM 42%, load 1.5/8, battery unknown"
    );
    assert_eq!(card.latest_event_kind.as_deref(), Some("task_completed"));
    assert_eq!(card.active_task_title.as_deref(), Some("Active task"));
}

#[test]
fn status_and_log_markdown_builders_are_stable() {
    let now = Utc::now();
    let manifest = test_manifest(now);
    let summary = summarize_task_cards_slice(&[
        task_card("1", "Done", "complete"),
        task_card("2", "Blocked", "blocked"),
    ]);
    let status = format_status_markdown_from_summary(&manifest, &summary, now);
    assert!(status.contains("Overnight run `run-1`"));
    assert!(status.contains("Target wake time in 1h."));
    assert!(status.contains("**1 complete**, **0 active**, **1 blocked**"));

    let events = vec![OvernightEvent {
        timestamp: now,
        run_id: manifest.run_id.clone(),
        session_id: None,
        kind: "note".to_string(),
        summary: "hello".to_string(),
        details: serde_json::json!({}),
        meaningful: false,
    }];
    let log = format_log_markdown_from_events(&manifest, &events, 30);
    assert!(log.contains("**note**: hello"));
    assert!(log.contains("Full log: `/tmp/overnight-run/run.log`"));
}

#[test]
fn review_html_builder_includes_core_sections() {
    let now = Utc::now();
    let run_dir = PathBuf::from("/tmp/overnight-run");
    let manifest = OvernightManifest {
        version: OVERNIGHT_VERSION,
        run_id: "run-1".to_string(),
        parent_session_id: "parent".to_string(),
        coordinator_session_id: "coord".to_string(),
        coordinator_session_name: "coordinator".to_string(),
        started_at: now,
        target_wake_at: now,
        handoff_ready_at: now,
        post_wake_grace_until: now,
        morning_report_posted_at: None,
        completed_at: None,
        cancel_requested_at: None,
        status: OvernightRunStatus::Running,
        mission: Some("verify <things>".to_string()),
        working_dir: Some("/tmp/project".to_string()),
        provider_name: "provider".to_string(),
        model: "model".to_string(),
        max_agents_guidance: 1,
        process_id: 123,
        run_dir: run_dir.clone(),
        events_path: run_dir.join("events.jsonl"),
        human_log_path: run_dir.join("run.log"),
        review_path: run_dir.join("review.html"),
        review_notes_path: run_dir.join("notes.md"),
        preflight_path: run_dir.join("preflight.json"),
        task_cards_dir: run_dir.join("task-cards"),
        issue_drafts_dir: run_dir.join("issues"),
        validation_dir: run_dir.join("validation"),
        last_activity_at: now,
    };
    let events = vec![OvernightEvent {
        timestamp: now,
        run_id: "run-1".to_string(),
        session_id: Some("coord".to_string()),
        kind: "task_completed".to_string(),
        summary: "Finished <task>".to_string(),
        details: serde_json::json!({}),
        meaningful: true,
    }];
    let card = OvernightTaskCard {
        title: "Task <A>".to_string(),
        status: "completed".to_string(),
        ..Default::default()
    };

    let html = build_review_html(&manifest, &events, "notes", "preflight", &[card]);
    assert!(html.contains("Overnight run"));
    assert!(html.contains("Structured task cards"));
    assert!(html.contains("Task &lt;A&gt;"));
    assert!(html.contains("Finished &lt;task&gt;"));
    assert!(html.contains("verify &lt;things&gt;"));
}
