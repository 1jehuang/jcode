//! Tests for `#[path]`-attributed module `tests` of `overnight.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::*;

fn test_manifest(root: &Path, run_id: &str) -> OvernightManifest {
    let run_dir = root.join("run");
    let now = Utc::now();
    OvernightManifest {
        version: OVERNIGHT_VERSION,
        run_id: run_id.to_string(),
        parent_session_id: "parent".to_string(),
        coordinator_session_id: "coord".to_string(),
        coordinator_session_name: "coordinator".to_string(),
        started_at: now,
        target_wake_at: now + ChronoDuration::hours(7),
        handoff_ready_at: now + ChronoDuration::hours(6),
        post_wake_grace_until: now + ChronoDuration::hours(9),
        morning_report_posted_at: None,
        completed_at: None,
        cancel_requested_at: None,
        status: OvernightRunStatus::Running,
        mission: Some("verify things".to_string()),
        working_dir: Some("/tmp/project".to_string()),
        provider_name: "test-provider".to_string(),
        model: "test-model".to_string(),
        max_agents_guidance: 2,
        process_id: 123,
        run_dir: run_dir.clone(),
        events_path: run_dir.join("events.jsonl"),
        human_log_path: run_dir.join("run.log"),
        review_path: run_dir.join("review.html"),
        review_notes_path: run_dir.join("review-notes.md"),
        preflight_path: run_dir.join("preflight.json"),
        task_cards_dir: run_dir.join("task-cards"),
        issue_drafts_dir: run_dir.join("issue-drafts"),
        validation_dir: run_dir.join("validation"),
        last_activity_at: now,
    }
}

#[test]
fn parse_duration_accepts_hours_minutes_and_decimals() {
    assert_eq!(parse_duration("7").unwrap().minutes, 420);
    assert_eq!(parse_duration("7h").unwrap().minutes, 420);
    assert_eq!(parse_duration("90m").unwrap().minutes, 90);
    assert_eq!(parse_duration("1.5").unwrap().minutes, 90);
}

#[test]
fn parse_overnight_command_start_with_mission() {
    let parsed = parse_overnight_command("/overnight 7 fix verified bugs")
        .unwrap()
        .unwrap();
    match parsed {
        OvernightCommand::Start { duration, mission } => {
            assert_eq!(duration.minutes, 420);
            assert_eq!(mission.as_deref(), Some("fix verified bugs"));
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn parse_overnight_command_subcommands() {
    assert_eq!(
        parse_overnight_command("/overnight status")
            .unwrap()
            .unwrap(),
        OvernightCommand::Status
    );
    assert_eq!(
        parse_overnight_command("/overnight log").unwrap().unwrap(),
        OvernightCommand::Log
    );
    assert_eq!(
        parse_overnight_command("/overnight review")
            .unwrap()
            .unwrap(),
        OvernightCommand::Review
    );
    assert_eq!(
        parse_overnight_command("/overnight cancel")
            .unwrap()
            .unwrap(),
        OvernightCommand::Cancel
    );
}

#[test]
fn html_escape_escapes_basic_entities() {
    assert_eq!(html_escape("<a&b>\"'"), "&lt;a&amp;b&gt;&quot;&#39;");
}

#[test]
fn render_review_html_writes_required_sections() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = test_manifest(temp.path(), "overnight_test");
    write_initial_review_notes(&manifest).expect("write notes");
    render_review_html(&manifest).expect("render review");

    let html = std::fs::read_to_string(&manifest.review_path).expect("read review html");
    assert!(html.contains("Executive summary"));
    assert!(html.contains("Coordinator review notes"));
    assert!(html.contains("Timeline"));
    assert!(html.contains("Artifacts"));
    assert!(html.contains("Before"));
    assert!(html.contains("After"));
}

#[test]
fn task_card_summary_reads_structured_json_cards() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = test_manifest(temp.path(), "overnight_cards");
    std::fs::create_dir_all(&manifest.task_cards_dir).expect("task card dir");
    std::fs::write(
        manifest.task_cards_dir.join("task-001.json"),
        r#"{
          "id": "task-001",
          "title": "Fix deterministic bug",
          "status": "completed",
          "risk": "low",
          "validation": { "commands": ["cargo test bug"], "result": "passed" },
          "updated_at": "2026-05-01T08:00:00Z"
        }"#,
    )
    .expect("write completed card");
    std::fs::write(
        manifest.task_cards_dir.join("task-002.json"),
        r#"{
          "id": "task-002",
          "title": "Investigate static-analysis finding",
          "status": "active",
          "risk": "high",
          "updated_at": "2026-05-01T08:10:00Z"
        }"#,
    )
    .expect("write active card");

    let cards = read_task_cards(&manifest).expect("read cards");
    assert_eq!(cards.len(), 2);
    let summary = summarize_task_cards_slice(&cards);
    assert_eq!(summary.total, 2);
    assert_eq!(summary.counts.completed, 1);
    assert_eq!(summary.counts.active, 1);
    assert_eq!(summary.validated, 1);
    assert_eq!(summary.high_risk, 1);
    assert_eq!(
        summary.latest_title.as_deref(),
        Some("Investigate static-analysis finding")
    );
}

#[test]
fn progress_card_content_includes_task_summary_and_latest_event() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = test_manifest(temp.path(), "overnight_progress");
    std::fs::create_dir_all(&manifest.task_cards_dir).expect("task card dir");
    std::fs::create_dir_all(manifest.events_path.parent().unwrap()).expect("events dir");
    std::fs::write(
        manifest.task_cards_dir.join("task-001.json"),
        r#"{
          "id": "task-001",
          "title": "Verify reload race",
          "status": "completed",
          "validation": { "result": "passed" },
          "updated_at": "2026-05-01T08:00:00Z"
        }"#,
    )
    .expect("write card");
    let event = OvernightEvent {
        timestamp: Utc::now(),
        run_id: manifest.run_id.clone(),
        session_id: Some(manifest.coordinator_session_id.clone()),
        kind: "coordinator_turn_completed".to_string(),
        summary: "Coordinator turn completed".to_string(),
        details: json!({}),
        meaningful: true,
    };
    std::fs::write(
        &manifest.events_path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .expect("write event");

    let card: OvernightProgressCard =
        serde_json::from_str(&format_progress_card_content(&manifest).expect("progress card"))
            .expect("parse card");
    assert_eq!(card.task_summary.counts.completed, 1);
    assert_eq!(card.task_summary.validated, 1);
    assert_eq!(
        card.latest_event_kind.as_deref(),
        Some("coordinator_turn_completed")
    );
    assert_eq!(
        card.active_task_title.as_deref(),
        Some("Verify reload race")
    );
}

#[test]
fn render_review_html_includes_structured_task_cards() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest = test_manifest(temp.path(), "overnight_review_cards");
    write_initial_review_notes(&manifest).expect("write notes");
    std::fs::create_dir_all(&manifest.task_cards_dir).expect("task card dir");
    std::fs::write(
        manifest.task_cards_dir.join("task-001.json"),
        r#"{
          "id": "task-001",
          "title": "Fix deterministic bug",
          "status": "completed",
          "why_selected": "Reproducible failure",
          "before": { "problem": "Test failed before the fix" },
          "after": { "change": "Test passes after the fix", "files_changed": ["src/example.rs"] },
          "validation": { "commands": ["cargo test deterministic_bug"], "result": "passed" },
          "updated_at": "2026-05-01T08:00:00Z"
        }"#,
    )
    .expect("write card");

    render_review_html(&manifest).expect("render review");
    let html = std::fs::read_to_string(&manifest.review_path).expect("read html");
    assert!(html.contains("Structured task cards"));
    assert!(html.contains("Fix deterministic bug"));
    assert!(html.contains("Reproducible failure"));
    assert!(html.contains("cargo test deterministic_bug"));
}
