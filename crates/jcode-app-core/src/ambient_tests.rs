use super::*;
use chrono::Duration;

/// Restores an env var on drop. Mirrors the helper in runner_tests.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

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

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

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

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");
    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

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

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

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

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

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
            repeat: None,
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
        repeat: None,
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

    queue
        .push(ScheduledItem {
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
            repeat: None,
        })
        .expect("queue persists in tests");

    let items = queue.items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "s1");
}

// ---------------------------------------------------------------------------
// Recurrence
// ---------------------------------------------------------------------------

/// Guards a directory's writability across a test: read-only for the body,
/// restored on drop so the temp dir cleans up.
#[cfg(unix)]
struct ReadOnlyDir {
    path: std::path::PathBuf,
}
impl ReadOnlyDir {
    fn lock(dir: &std::path::Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).expect("lock dir");
        Self {
            path: dir.to_path_buf(),
        }
    }
}
impl Drop for ReadOnlyDir {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn test_stale_snapshot_cancel_cannot_resurrect_series() {
    // Two loads of the same file: A pops (advancing the chain), then B —
    // holding the pre-pop snapshot — cancels the series. The cancel must
    // win; B's stale write must not restore the popped occurrence.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("queue.json");
    let mut a = ScheduledQueue::load(path.clone());
    a.push(recurring_item("first", Some(3)))
        .expect("queue persists in tests");
    let mut b = ScheduledQueue::load(path.clone());
    let popped = a.pop_ready();
    assert_eq!(popped.len(), 1);
    let removed = b
        .remove_by_recurrence("recur_test")
        .expect("cancel retries");
    assert_eq!(removed, 1, "cancel sees the requeued occurrence");
    let disk = ScheduledQueue::load(path);
    assert!(
        disk.items().iter().all(|item| item
            .repeat
            .as_ref()
            .is_none_or(|repeat| repeat.recurrence_id != "recur_test")),
        "no series item survives on disk"
    );
}

#[cfg(unix)]
#[test]
fn test_failed_dequeue_save_delivers_nothing() {
    // Save failures must not deliver: the popped items stay queued and the
    // runner retries on a later poll instead of redelivering after restart.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("queue.json");
    let mut queue = ScheduledQueue::load(path);
    queue
        .push(recurring_item("first", Some(3)))
        .expect("queue persists in tests");
    let _lock = ReadOnlyDir::lock(tmp.path());
    let ready = queue.pop_ready();
    assert!(
        ready.is_empty(),
        "nothing delivered without a durable dequeue"
    );
    assert_eq!(queue.len(), 1, "the popped item stays queued via reload");
    assert_eq!(queue.items()[0].id, "first");
}

#[cfg(unix)]
#[test]
fn test_push_reports_unpersistable_work() {
    // Scheduling must not return success for work that is in neither
    // memory nor durable storage.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("queue.json");
    let mut queue = ScheduledQueue::load(path);
    let _lock = ReadOnlyDir::lock(tmp.path());
    let result = queue.push(recurring_item("lost", Some(2)));
    assert!(result.is_err(), "unpersistable push must error, not vanish");
}

#[test]
fn test_legacy_bare_array_loads_as_v0() {
    // Pre-version queue files (a bare JSON array) load with version 0 and
    // upgrade to the envelope on the next save.
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("queue.json");
    let legacy = serde_json::to_string(&vec![recurring_item("old", Some(2))]).unwrap();
    std::fs::write(&path, legacy).unwrap();
    let mut queue = ScheduledQueue::load(path.clone());
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].id, "old");
    queue
        .push(recurring_item("new", None))
        .expect("queue persists in tests");
    let disk = ScheduledQueue::load(path);
    assert_eq!(disk.len(), 2);
}

fn recurring_item(id: &str, remaining: Option<u32>) -> ScheduledItem {
    ScheduledItem {
        id: id.into(),
        scheduled_for: Utc::now() - Duration::minutes(5),
        context: "garden".into(),
        priority: Priority::Normal,
        target: ScheduleTarget::Spawn {
            parent_session_id: "parent".into(),
        },
        created_by_session: "test".into(),
        created_at: Utc::now(),
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
        repeat: Some(RepeatState {
            every_minutes: 60,
            remaining,
            recurrence_id: "recur_test".into(),
        }),
    }
}

#[test]
fn test_recurring_pop_requeues_next_occurrence() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut queue = ScheduledQueue::load(tmp.path().to_path_buf());
    queue
        .push(recurring_item("first", Some(3)))
        .expect("queue persists in tests");

    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, "first");

    // Chain survives the pop: next occurrence already queued.
    assert_eq!(queue.len(), 1);
    let next = &queue.items()[0];
    assert_ne!(next.id, "first");
    let repeat = next.repeat.as_ref().expect("next keeps series");
    assert_eq!(repeat.recurrence_id, "recur_test");
    assert_eq!(repeat.remaining, Some(2));
    let gap = next.scheduled_for - Utc::now();
    assert!(
        gap >= Duration::minutes(55),
        "next due about one interval out, got {:?}",
        gap
    );
}

#[test]
fn test_recurrence_exhausts_at_last_iteration() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut queue = ScheduledQueue::load(tmp.path().to_path_buf());
    queue
        .push(recurring_item("last", Some(1)))
        .expect("queue persists in tests");

    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 1);
    assert!(
        queue.is_empty(),
        "remaining=1 means this pop was the final run"
    );
}

#[test]
fn test_recurrence_without_limit_repeats_forever() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut queue = ScheduledQueue::load(tmp.path().to_path_buf());
    queue
        .push(recurring_item("forever", None))
        .expect("queue persists in tests");

    for _ in 0..3 {
        let ready = queue.pop_ready();
        assert_eq!(ready.len(), 1);
        // Force each requeued occurrence due again.
        for item in queue.items_mut() {
            item.scheduled_for = Utc::now() - Duration::minutes(5);
        }
    }
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].repeat.as_ref().unwrap().remaining, None);
}

#[test]
fn test_recurring_direct_items_requeue_on_take_ready_direct() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut queue = ScheduledQueue::load(tmp.path().to_path_buf());
    queue
        .push(recurring_item("direct", Some(2)))
        .expect("queue persists in tests");

    let ready = queue.take_ready_direct_items();
    assert_eq!(ready.len(), 1);
    assert_eq!(queue.len(), 1, "direct path requeues too");
    assert_eq!(queue.items()[0].repeat.as_ref().unwrap().remaining, Some(1));
}

#[test]
fn test_cancel_recurrence_removes_whole_series() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut queue = ScheduledQueue::load(tmp.path().to_path_buf());
    queue
        .push(recurring_item("a", Some(5)))
        .expect("queue persists in tests");
    queue
        .push(recurring_item("b", None))
        .expect("queue persists in tests");
    // One-shot bystander from another series must survive.
    let mut solo = recurring_item("solo", Some(2));
    solo.id = "solo".into();
    solo.repeat.as_mut().unwrap().recurrence_id = "recur_other".into();
    queue.push(solo).expect("queue persists in tests");

    let removed = queue.remove_by_recurrence("recur_test").unwrap();
    assert_eq!(removed, 2);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].id, "solo");
    assert_eq!(queue.remove_by_recurrence("recur_missing").unwrap(), 0);
}

#[test]
fn test_schedule_rejects_repeat_for_ambient_target() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut manager = AmbientManager::new().expect("manager");
    let err = manager
        .schedule(ScheduleRequest {
            wake_in_minutes: Some(60),
            wake_at: None,
            repeat: Some(RepeatSpec {
                every_minutes: 60,
                max_iterations: None,
            }),
            context: "garden".into(),
            priority: Priority::Normal,
            target: ScheduleTarget::Ambient,
            created_by_session: "test".into(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect_err("ambient+repeat must be rejected");
    assert!(err.to_string().contains("direct target"), "got: {err}");
}

#[test]
fn test_schedule_rejects_zero_interval() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut manager = AmbientManager::new().expect("manager");
    let err = manager
        .schedule(ScheduleRequest {
            wake_in_minutes: Some(60),
            wake_at: None,
            repeat: Some(RepeatSpec {
                every_minutes: 0,
                max_iterations: None,
            }),
            context: "garden".into(),
            priority: Priority::Normal,
            target: ScheduleTarget::Spawn {
                parent_session_id: "parent".into(),
            },
            created_by_session: "test".into(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect_err("zero interval must be rejected");
    assert!(err.to_string().contains(">= 1"), "got: {err}");
}

#[test]
fn test_schedule_rejects_zero_max_iterations() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut manager = AmbientManager::new().expect("manager");
    let err = manager
        .schedule(ScheduleRequest {
            wake_in_minutes: Some(60),
            wake_at: None,
            repeat: Some(RepeatSpec {
                every_minutes: 60,
                max_iterations: Some(0),
            }),
            context: "garden".into(),
            priority: Priority::Normal,
            target: ScheduleTarget::Spawn {
                parent_session_id: "parent".into(),
            },
            created_by_session: "test".into(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect_err("zero max_iterations must be rejected");
    assert!(err.to_string().contains(">= 1"), "got: {err}");
}

#[test]
fn test_schedule_stamps_series_and_manager_cancels_it() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut manager = AmbientManager::new().expect("manager");
    manager
        .schedule(ScheduleRequest {
            wake_in_minutes: Some(60),
            wake_at: None,
            repeat: Some(RepeatSpec {
                every_minutes: 60,
                max_iterations: Some(4),
            }),
            context: "garden".into(),
            priority: Priority::Normal,
            target: ScheduleTarget::Spawn {
                parent_session_id: "parent".into(),
            },
            created_by_session: "test".into(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
        })
        .expect("schedule");
    let repeat = manager.queue().items()[0]
        .repeat
        .clone()
        .expect("series stamped");
    assert_eq!(repeat.remaining, Some(4));
    assert!(repeat.recurrence_id.starts_with("recur_"));
    assert_eq!(manager.cancel_recurrence(&repeat.recurrence_id).unwrap(), 1);
    assert!(manager.queue().is_empty());
}
