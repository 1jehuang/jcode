use super::*;
use chrono::Duration;

#[test]
fn test_ambient_status_default() {
    let status = AmbientStatus::default();
    assert_eq!(status, AmbientStatus::Idle);
}

#[test]
fn test_priority_ordering() {
    assert!(Priority::High > Priority::Normal);
    assert!(Priority::Normal > Priority::Low);
}

#[test]
fn test_scheduled_queue_push_and_pop() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    let mut queue = ScheduledQueue::load(path);
    assert!(queue.is_empty());

    let past = Utc::now() - Duration::minutes(5);
    let future = Utc::now() + Duration::hours(1);

    queue.push(ScheduledItem {
        id: "s1".into(),
        scheduled_for: past,
        context: "past item".into(),
        priority: Priority::Low,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "s2".into(),
        scheduled_for: future,
        context: "future item".into(),
        priority: Priority::High,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    assert_eq!(queue.len(), 2);

    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "s1");

    // Future item still in queue
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.peek_next().unwrap().id, "s2");
}

#[test]
fn test_scheduled_queue_remove_by_id_persists_remaining_items() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    let mut queue = ScheduledQueue::load(path.clone());
    let future = Utc::now() + Duration::hours(1);

    queue.push(ScheduledItem {
        id: "keep".into(),
        scheduled_for: future,
        context: "keep item".into(),
        priority: Priority::Normal,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });
    queue.push(ScheduledItem {
        id: "cancel".into(),
        scheduled_for: future,
        context: "cancel item".into(),
        priority: Priority::High,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    let removed = queue.remove_by_id("cancel").unwrap().unwrap();
    assert_eq!(removed.id, "cancel");
    assert!(queue.remove_by_id("missing").unwrap().is_none());

    let reloaded = ScheduledQueue::load(path);
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded.items()[0].id, "keep");
}

#[test]
fn test_pop_ready_sorts_by_priority_then_time() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    let mut queue = ScheduledQueue::load(path);
    let past1 = Utc::now() - Duration::minutes(10);
    let past2 = Utc::now() - Duration::minutes(5);

    queue.push(ScheduledItem {
        id: "low_early".into(),
        scheduled_for: past1,
        context: "low early".into(),
        priority: Priority::Low,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "high_late".into(),
        scheduled_for: past2,
        context: "high late".into(),
        priority: Priority::High,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 2);
    // High priority should come first
    assert_eq!(ready[0].id, "high_late");
    assert_eq!(ready[1].id, "low_early");
}

#[test]
fn test_take_ready_direct_items_only_removes_direct_targets() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();

    let mut queue = ScheduledQueue::load(path);
    let past = Utc::now() - Duration::minutes(5);

    queue.push(ScheduledItem {
        id: "session_due".into(),
        scheduled_for: past,
        context: "scheduled session task".into(),
        priority: Priority::Normal,
        target: ScheduleTarget::Session {
            session_id: "session_123".into(),
        },
        created_by_session: "session_123".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "spawn_due".into(),
        scheduled_for: past,
        context: "spawned session task".into(),
        priority: Priority::High,
        target: ScheduleTarget::Spawn {
            parent_session_id: "session_123".into(),
        },
        created_by_session: "session_123".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "ambient_due".into(),
        scheduled_for: past,
        context: "scheduled ambient task".into(),
        priority: Priority::High,
        target: ScheduleTarget::Ambient,
        created_by_session: "ambient".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    let ready_direct = queue.take_ready_direct_items();
    assert_eq!(ready_direct.len(), 2);
    assert_eq!(ready_direct[0].id, "spawn_due");
    assert_eq!(ready_direct[1].id, "session_due");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].id, "ambient_due");
}

#[test]
fn test_ambient_state_record_cycle() {
    let mut state = AmbientState::default();
    assert_eq!(state.total_cycles, 0);

    let result = AmbientCycleResult {
        summary: "Merged 2 duplicates".into(),
        memories_modified: 3,
        compactions: 1,
        proactive_work: None,
        next_schedule: None,
        started_at: Utc::now() - Duration::seconds(30),
        ended_at: Utc::now(),
        status: CycleStatus::Complete,
        conversation: None,
    };

    state.record_cycle(&result);
    assert_eq!(state.total_cycles, 1);
    assert_eq!(state.last_summary.as_deref(), Some("Merged 2 duplicates"));
    assert_eq!(state.last_compactions, Some(1));
    assert_eq!(state.last_memories_modified, Some(3));
    assert_eq!(state.status, AmbientStatus::Idle);
}

#[test]
fn test_ambient_state_record_cycle_with_schedule() {
    let mut state = AmbientState::default();

    let result = AmbientCycleResult {
        summary: "Done".into(),
        memories_modified: 0,
        compactions: 0,
        proactive_work: None,
        next_schedule: Some(ScheduleRequest {
            wake_in_minutes: Some(15),
            wake_at: None,
            context: "check CI".into(),
            priority: Priority::Normal,
            target: ScheduleTarget::Ambient,
            created_by_session: "ambient_test".into(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        }),
        started_at: Utc::now() - Duration::seconds(10),
        ended_at: Utc::now(),
        status: CycleStatus::Complete,
        conversation: None,
    };

    state.record_cycle(&result);
    assert!(matches!(state.status, AmbientStatus::Scheduled { .. }));
}

#[test]
fn test_ambient_lock_release() {
    // Use a temp dir so we don't conflict with real state
    let tmp_dir = tempfile::tempdir().unwrap();
    let lock_file = tmp_dir.path().join("test.lock");

    // Manually create a lock to test release/drop
    std::fs::write(&lock_file, std::process::id().to_string()).unwrap();
    let lock = AmbientLock {
        lock_path: lock_file.clone(),
    };
    lock.release().unwrap();
    assert!(!lock_file.exists());
}

#[test]
fn test_schedule_id_format() {
    let id = format!("sched_{:08x}", rand::random::<u32>());
    assert!(id.starts_with("sched_"));
    assert_eq!(id.len(), 6 + 8); // "sched_" + 8 hex chars
}

#[test]
fn test_format_duration_rough() {
    assert_eq!(format_duration_rough(Duration::seconds(30)), "30s");
    assert_eq!(format_duration_rough(Duration::minutes(5)), "5m");
    assert_eq!(format_duration_rough(Duration::hours(2)), "2h");
    assert_eq!(
        format_duration_rough(Duration::hours(2) + Duration::minutes(30)),
        "2h 30m"
    );
    assert_eq!(format_duration_rough(Duration::days(3)), "3d");
    assert_eq!(format_duration_rough(Duration::seconds(-5)), "0s");
}

#[test]
fn test_build_ambient_system_prompt_minimal() {
    let state = AmbientState::default();
    let queue = vec![];
    let health = MemoryGraphHealth::default();
    let sessions = vec![];
    let feedback: Vec<String> = vec![];
    let budget = ResourceBudget {
        provider: "anthropic-oauth".into(),
        tokens_remaining_desc: "unknown".into(),
        window_resets_desc: "unknown".into(),
        user_usage_rate_desc: "0 tokens/min".into(),
        cycle_budget_desc: "stay under 50k tokens".into(),
    };

    let prompt =
        build_ambient_system_prompt(&state, &queue, &health, &sessions, &feedback, &budget, 0);

    assert!(prompt.contains("ambient agent for jcode"));
    assert!(prompt.contains("## Current State"));
    assert!(prompt.contains("never (first run)"));
    assert!(prompt.contains("Active user sessions: none"));
    assert!(prompt.contains("## Scheduled Queue"));
    assert!(prompt.contains("Empty"));
    assert!(prompt.contains("## Memory Graph Health"));
    assert!(prompt.contains("Total memories: 0"));
    assert!(prompt.contains("## User Feedback History"));
    assert!(prompt.contains("No feedback memories"));
    assert!(prompt.contains("## Resource Budget"));
    assert!(prompt.contains("anthropic-oauth"));
    assert!(prompt.contains("## Instructions"));
    assert!(prompt.contains("end_ambient_cycle"));
    assert!(prompt.contains("reviewer-ready"));
    assert!(prompt.contains("context.why_permission_needed"));
}

#[test]
fn test_build_ambient_system_prompt_with_data() {
    let state = AmbientState {
        last_run: Some(Utc::now() - Duration::minutes(15)),
        total_cycles: 7,
        ..Default::default()
    };

    let queue = vec![ScheduledItem {
        id: "sched_001".into(),
        scheduled_for: Utc::now(),
        context: "Check CI status".into(),
        priority: Priority::High,
        target: ScheduleTarget::Ambient,
        created_by_session: "session_abc".into(),
        created_at: Utc::now() - Duration::minutes(10),
        working_dir: Some("/home/user/project".into()),
        task_description: Some("Check CI status for the main branch".into()),
        relevant_files: vec!["src/main.rs".into()],
        git_branch: Some("main".into()),
        additional_context: Some("Background: Tests were flaky yesterday".into()),
    }];

    let health = MemoryGraphHealth {
        total: 42,
        active: 38,
        inactive: 4,
        low_confidence: 3,
        contradictions: 1,
        missing_embeddings: 5,
        duplicate_candidates: 0,
        last_consolidation: Some(Utc::now() - Duration::hours(2)),
    };

    let sessions = vec![RecentSessionInfo {
        id: "session_fox_123".into(),
        status: "closed".into(),
        topic: Some("Fix auth bug".into()),
        duration_secs: 900,
        extraction_status: "extracted".into(),
    }];

    let feedback = vec![
        "User approved ambient fixing typos in docs".into(),
        "User rejected ambient refactoring tests".into(),
    ];

    let budget = ResourceBudget {
        provider: "openai-oauth".into(),
        tokens_remaining_desc: "~85k".into(),
        window_resets_desc: "in 3h 20m".into(),
        user_usage_rate_desc: "120 tokens/min".into(),
        cycle_budget_desc: "stay under 15k tokens".into(),
    };

    let prompt =
        build_ambient_system_prompt(&state, &queue, &health, &sessions, &feedback, &budget, 2);

    assert!(prompt.contains("15m ago"));
    assert!(prompt.contains("Active user sessions: 2"));
    assert!(prompt.contains("Total cycles completed: 7"));
    assert!(prompt.contains("Check CI status"));
    assert!(prompt.contains("HIGH"));
    assert!(prompt.contains("42"));
    assert!(prompt.contains("38 active"));
    assert!(prompt.contains("confidence < 0.1: 3"));
    assert!(prompt.contains("contradictions: 1"));
    assert!(prompt.contains("without embeddings: 5"));
    assert!(prompt.contains("Fix auth bug"));
    assert!(prompt.contains("approved ambient fixing typos"));
    assert!(prompt.contains("rejected ambient refactoring"));
    assert!(prompt.contains("openai-oauth"));
    assert!(prompt.contains("~85k"));
    assert!(prompt.contains("Working dir: /home/user/project"));
    assert!(prompt.contains("Details: Check CI status for the main branch"));
    assert!(prompt.contains("Files: src/main.rs"));
    assert!(prompt.contains("Branch: main"));
    assert!(prompt.contains("Tests were flaky yesterday"));
}

#[test]
fn test_scheduled_queue_items_accessor() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let mut queue = ScheduledQueue::load(path);

    queue.push(ScheduledItem {
        id: "s1".into(),
        scheduled_for: Utc::now(),
        context: "test item".into(),
        priority: Priority::Normal,
        target: ScheduleTarget::Ambient,
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    let items = queue.items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "s1");
}
