use super::*;

/// Test ambient state: load, save, record_cycle
#[test]
fn test_ambient_state_lifecycle() {
    use jcode::ambient::{AmbientCycleResult, AmbientState, AmbientStatus, CycleStatus};

    let mut state = AmbientState::default();
    assert!(matches!(state.status, AmbientStatus::Idle));
    assert_eq!(state.total_cycles, 0);
    assert!(state.last_run.is_none());

    // Record a cycle
    let result = AmbientCycleResult {
        summary: "Gardened 3 memories".to_string(),
        memories_modified: 3,
        compactions: 0,
        proactive_work: None,
        next_schedule: None,
        started_at: chrono::Utc::now(),
        ended_at: chrono::Utc::now(),
        status: CycleStatus::Complete,
        conversation: None,
    };

    state.record_cycle(&result);
    assert_eq!(state.total_cycles, 1);
    assert!(state.last_run.is_some());
    assert_eq!(state.last_summary.as_deref(), Some("Gardened 3 memories"));
    assert_eq!(state.last_memories_modified, Some(3));
    assert_eq!(state.last_compactions, Some(0));
    // No next_schedule → should be Idle
    assert!(matches!(state.status, AmbientStatus::Idle));
}

/// Test ambient scheduled queue: push, pop, priority ordering
#[test]
fn test_ambient_scheduled_queue() {
    use jcode::ambient::{Priority, ScheduledItem, ScheduledQueue};

    let tmp = std::env::temp_dir().join("jcode-test-queue.json");
    let _ = std::fs::remove_file(&tmp); // Clean up from previous runs
    let mut queue = ScheduledQueue::load(tmp);
    assert!(queue.is_empty());

    // Push items with different priorities
    let now = chrono::Utc::now();
    queue.push(ScheduledItem {
        id: "low_1".to_string(),
        scheduled_for: now - chrono::Duration::minutes(5),
        context: "low priority task".to_string(),
        priority: Priority::Low,
        target: jcode::ambient::ScheduleTarget::Ambient,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "high_1".to_string(),
        scheduled_for: now - chrono::Duration::minutes(5),
        context: "high priority task".to_string(),
        priority: Priority::High,
        target: jcode::ambient::ScheduleTarget::Ambient,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    queue.push(ScheduledItem {
        id: "future_1".to_string(),
        scheduled_for: now + chrono::Duration::hours(1),
        context: "future task".to_string(),
        priority: Priority::Normal,
        target: jcode::ambient::ScheduleTarget::Ambient,
        created_by_session: "test".to_string(),
        created_at: now,
        working_dir: None,
        task_description: None,
        relevant_files: Vec::new(),
        git_branch: None,
        additional_context: None,
    });

    assert_eq!(queue.len(), 3);

    // Pop ready items: should get high priority first, then low (future not ready)
    let ready = queue.pop_ready();
    assert_eq!(ready.len(), 2);
    assert_eq!(ready[0].id, "high_1"); // High priority first
    assert_eq!(ready[1].id, "low_1"); // Low priority second

    // Future item still in queue
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.items()[0].id, "future_1");
}

/// Test adaptive scheduler: interval calculation
#[test]
fn test_adaptive_scheduler_intervals() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        ..Default::default()
    };

    let scheduler = AdaptiveScheduler::new(config);

    // With no rate limit info, should return max interval
    let interval = scheduler.calculate_interval(None);
    assert!(interval.as_secs() >= 120 * 60 - 1); // Allow 1s tolerance
}

/// Test adaptive scheduler: backoff on rate limit
#[test]
fn test_adaptive_scheduler_backoff() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        ..Default::default()
    };

    let mut scheduler = AdaptiveScheduler::new(config);

    let base_interval = scheduler.calculate_interval(None);

    // Hit rate limit
    scheduler.on_rate_limit_hit();
    let backed_off = scheduler.calculate_interval(None);
    assert!(backed_off >= base_interval);

    // Reset on success
    scheduler.on_successful_cycle();
    let after_reset = scheduler.calculate_interval(None);
    assert!(after_reset <= backed_off);
}

/// Test adaptive scheduler: pause on active session
#[test]
fn test_adaptive_scheduler_pause() {
    use jcode::ambient_scheduler::{AdaptiveScheduler, AmbientSchedulerConfig};

    let config = AmbientSchedulerConfig {
        min_interval_minutes: 5,
        max_interval_minutes: 120,
        pause_on_active_session: true,
        ..Default::default()
    };

    let mut scheduler = AdaptiveScheduler::new(config);

    assert!(!scheduler.should_pause());
    scheduler.set_user_active(true);
    assert!(scheduler.should_pause());
    scheduler.set_user_active(false);
    assert!(!scheduler.should_pause());
}

/// Test ambient tools: end_ambient_cycle via mock agent
#[tokio::test]
async fn test_ambient_end_cycle_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Mock: agent calls end_ambient_cycle tool
    let tool_input = serde_json::json!({
        "summary": "Merged 2 duplicate memories, pruned 1 stale memory",
        "memories_modified": 3,
        "compactions": 0
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_001".to_string(),
            name: "end_ambient_cycle".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, the agent calls the provider again — mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Cycle complete.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Begin ambient cycle").await?;
    assert_eq!(response, "Cycle complete.");

    // The tool should have stored a cycle result
    let result = jcode::tool::ambient::take_cycle_result();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(
        result.summary,
        "Merged 2 duplicate memories, pruned 1 stale memory"
    );
    assert_eq!(result.memories_modified, 3);
    assert_eq!(result.compactions, 0);

    Ok(())
}

/// Test ambient tools: request_permission via mock agent
#[tokio::test]
async fn test_ambient_request_permission_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    let tool_input = serde_json::json!({
        "action": "create_pull_request",
        "description": "Create PR for test fixes",
        "rationale": "Found 3 failing tests in auth module",
        "urgency": "high",
        "wait": false
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_perm_001".to_string(),
            name: "request_permission".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Permission requested.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);
    let ambient_session_id = agent.session_id().to_string();
    jcode::tool::ambient::register_ambient_session(ambient_session_id.clone());

    let response = agent.run_once_capture("Request permission").await?;
    jcode::tool::ambient::unregister_ambient_session(&ambient_session_id);
    assert_eq!(response, "Permission requested.");

    Ok(())
}

/// Test ambient tools: schedule_ambient via mock agent
#[tokio::test]
async fn test_ambient_schedule_tool() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    let tool_input = serde_json::json!({
        "wake_in_minutes": 30,
        "context": "Check CI results and verify test fixes",
        "priority": "normal"
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::ToolUseStart {
            id: "tool_sched_001".to_string(),
            name: "schedule_ambient".to_string(),
        },
        StreamEvent::ToolInputDelta(tool_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // After tool execution, mock a final response
    provider.queue_response(vec![
        StreamEvent::TextDelta("Scheduled next cycle.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider, registry);

    let response = agent.run_once_capture("Schedule next cycle").await?;
    assert_eq!(response, "Scheduled next cycle.");

    Ok(())
}

/// Test ambient system prompt builder
#[test]
fn test_ambient_system_prompt_builder() {
    use jcode::ambient::{
        AmbientState, MemoryGraphHealth, ResourceBudget, build_ambient_system_prompt,
    };

    let state = AmbientState::default();
    let queue_items = vec![];
    let health = MemoryGraphHealth {
        total: 42,
        active: 38,
        inactive: 4,
        low_confidence: 2,
        contradictions: 1,
        missing_embeddings: 0,
        duplicate_candidates: 3,
        last_consolidation: None,
    };
    let recent_sessions = vec![];
    let feedback: Vec<String> = vec![];
    let budget = ResourceBudget {
        provider: "mock".to_string(),
        tokens_remaining_desc: "50k tokens".to_string(),
        window_resets_desc: "2h".to_string(),
        user_usage_rate_desc: "5k/min".to_string(),
        cycle_budget_desc: "stay under 50k".to_string(),
    };

    let prompt = build_ambient_system_prompt(
        &state,
        &queue_items,
        &health,
        &recent_sessions,
        &feedback,
        &budget,
        0,
    );

    // Verify key sections exist
    assert!(
        prompt.contains("ambient agent"),
        "Prompt missing 'ambient agent'"
    );
    assert!(
        prompt.contains("Memory Graph Health"),
        "Prompt missing 'Memory Graph Health'"
    );
    assert!(
        prompt.contains("Total memories: 42"),
        "Prompt missing memory count"
    );
    assert!(
        prompt.contains("Resource Budget"),
        "Prompt missing 'Resource Budget'"
    );
    assert!(
        prompt.contains("end_ambient_cycle"),
        "Prompt missing end_ambient_cycle instruction"
    );
}

/// Test ambient runner handle: status_json
#[tokio::test]
async fn test_ambient_runner_status() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let status_json = handle.status_json().await;
    let status: serde_json::Value = serde_json::from_str(&status_json).unwrap();

    // Verify expected fields exist and have correct types
    assert!(status.get("status").is_some(), "Missing 'status' field");
    assert!(
        status.get("total_cycles").is_some(),
        "Missing 'total_cycles' field"
    );
    assert!(
        status.get("loop_running").is_some(),
        "Missing 'loop_running' field"
    );
    assert_eq!(
        status["loop_running"], false,
        "Runner loop should not be running"
    );
    assert!(
        status["total_cycles"].is_number(),
        "total_cycles should be a number"
    );
    assert!(
        status.get("queue_count").is_some(),
        "Missing 'queue_count' field"
    );
    assert!(
        status.get("active_user_sessions").is_some(),
        "Missing 'active_user_sessions' field"
    );
}

/// Test ambient runner handle: trigger and stop
#[tokio::test]
async fn test_ambient_runner_trigger_and_stop() {
    use jcode::ambient::AmbientStatus;
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    // Stop (sets status to disabled)
    handle.stop().await;
    let state = handle.state().await;
    assert!(
        matches!(state.status, AmbientStatus::Disabled),
        "After stop(), status should be Disabled, got: {:?}",
        state.status
    );

    // Runner should not be running (no loop was started)
    assert!(!handle.is_running().await, "Runner should not be active");
}

/// Test ambient runner handle: queue_json
#[tokio::test]
async fn test_ambient_runner_queue_json() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let json = handle.queue_json().await;
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

/// Test ambient runner handle: log_json
#[tokio::test]
async fn test_ambient_runner_log_json() {
    use jcode::ambient_runner::AmbientRunnerHandle;
    use jcode::safety::SafetySystem;

    let safety = Arc::new(SafetySystem::new());
    let handle = AmbientRunnerHandle::new(safety);

    let json = handle.log_json().await;
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

/// Test memory reinforcement provenance
#[test]
fn test_memory_reinforcement_provenance() {
    use jcode::memory::{MemoryCategory, MemoryEntry};

    let mut entry = MemoryEntry::new(MemoryCategory::Preference, "User prefers dark mode");
    assert!(entry.reinforcements.is_empty());
    assert_eq!(entry.strength, 1); // Initial strength

    // Reinforce with provenance
    entry.reinforce("session_abc123", 42);
    assert_eq!(entry.strength, 2);
    assert_eq!(entry.reinforcements.len(), 1);
    assert_eq!(entry.reinforcements[0].session_id, "session_abc123");
    assert_eq!(entry.reinforcements[0].message_index, 42);

    // Reinforce again from different session
    entry.reinforce("session_def456", 10);
    assert_eq!(entry.strength, 3);
    assert_eq!(entry.reinforcements.len(), 2);
    assert_eq!(entry.reinforcements[1].session_id, "session_def456");
    assert_eq!(entry.reinforcements[1].message_index, 10);
}

/// Test ambient config defaults
#[test]
fn test_ambient_config_defaults() {
    use jcode::config::AmbientConfig;

    let config = AmbientConfig::default();
    assert!(!config.enabled);
    assert!(!config.allow_api_keys);
    assert_eq!(config.min_interval_minutes, 5);
    assert_eq!(config.max_interval_minutes, 120);
    assert!(config.pause_on_active_session);
    assert!(config.proactive_work);
    assert_eq!(config.work_branch_prefix, "ambient/");
    assert!(config.provider.is_none());
    assert!(config.model.is_none());
    assert!(config.api_daily_budget.is_none());
}

/// Test ambient lock acquisition and release
#[test]
fn test_ambient_lock() {
    use jcode::ambient::AmbientLock;
    let _env = setup_test_env().expect("failed to setup isolated JCODE_HOME");

    // First acquisition should succeed
    let lock1 = AmbientLock::try_acquire();
    assert!(lock1.is_ok());
    let lock1 = lock1.unwrap();
    assert!(lock1.is_some());
    let lock1 = lock1.unwrap();

    // Second acquisition should fail (lock held)
    let lock2 = AmbientLock::try_acquire();
    assert!(lock2.is_ok());
    assert!(lock2.unwrap().is_none());

    // Release
    let _ = lock1.release();

    // Now should succeed again
    let lock3 = AmbientLock::try_acquire();
    assert!(lock3.is_ok());
    assert!(lock3.unwrap().is_some());
}

/// Test full ambient cycle simulation with mock provider
/// Simulates: agent receives prompt → uses tools → calls end_ambient_cycle
#[tokio::test]
async fn test_full_ambient_cycle_simulation() -> Result<()> {
    let _env = setup_test_env()?;
    let provider = MockProvider::new();

    // Turn 1: Agent calls end_ambient_cycle with full data
    let end_cycle_input = serde_json::json!({
        "summary": "Gardened memory graph: merged 2 duplicates about dark mode preference, pruned 1 stale memory with confidence 0.02, verified 5 facts against codebase.",
        "memories_modified": 6,
        "compactions": 1,
        "proactive_work": null,
        "next_schedule": {
            "wake_in_minutes": 45,
            "context": "Follow up on memory verification",
            "priority": "normal"
        }
    })
    .to_string();

    provider.queue_response(vec![
        StreamEvent::TextDelta("Starting ambient cycle...\n".to_string()),
        StreamEvent::ToolUseStart {
            id: "call_end".to_string(),
            name: "end_ambient_cycle".to_string(),
        },
        StreamEvent::ToolInputDelta(end_cycle_input),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: Some("tool_use".to_string()),
        },
    ]);

    // Turn 2: After end_ambient_cycle tool result, agent responds
    provider.queue_response(vec![
        StreamEvent::TextDelta("Ambient cycle completed successfully.".to_string()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        },
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    registry.register_ambient_tools().await;

    let mut agent = Agent::new(provider.clone(), registry);
    agent.set_system_prompt("You are the jcode ambient maintenance agent.");

    let response = agent.run_once_capture("Begin your ambient cycle.").await?;

    assert!(response.contains("Ambient cycle completed"));

    // Verify end_ambient_cycle stored the result
    let result = jcode::tool::ambient::take_cycle_result();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.memories_modified, 6);
    assert_eq!(result.compactions, 1);
    assert!(result.summary.contains("Gardened memory graph"));
    assert!(result.next_schedule.is_some());
    let sched = result.next_schedule.unwrap();
    assert_eq!(sched.wake_in_minutes, Some(45));
    assert!(sched.context.contains("Follow up"));

    Ok(())
}
