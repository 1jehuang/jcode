use super::*;
use crate::provider::{EventStream, Provider};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct MockSummaryProvider;

#[async_trait::async_trait]
impl Provider for MockSummaryProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn name(&self) -> &str {
        "mock-summary"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockSummaryProvider)
    }

    async fn complete_simple(&self, prompt: &str, _system: &str) -> Result<String> {
        Ok(format!("summary({} chars)", prompt.len()))
    }
}

fn make_text_message(role: Role, text: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn test_new_manager() {
    let manager = CompactionManager::new();
    assert_eq!(manager.compacted_count, 0);
    assert!(manager.active_summary.is_none());
    assert!(!manager.is_compacting());
}

#[test]
fn test_notify_message_added() {
    let mut manager = CompactionManager::new();
    manager.notify_message_added();
    manager.notify_message_added();
    assert_eq!(manager.total_turns, 2);
}

#[test]
fn test_restored_messages_do_not_trigger_compaction_immediately() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(Role::User, &format!("restored {}", i)));
    }
    manager.seed_restored_messages(messages.len());
    manager.update_observed_input_tokens(900);

    assert!(
        !manager.should_compact_with(&messages),
        "restored history should not compact until a new message is added"
    );
}

#[test]
fn test_new_message_after_restore_reenables_compaction() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(Role::User, &format!("restored {}", i)));
    }
    manager.seed_restored_messages(messages.len());
    manager.update_observed_input_tokens(900);
    assert!(!manager.should_compact_with(&messages));

    messages.push(make_text_message(Role::User, "new turn after restore"));
    manager.notify_message_added();

    assert!(
        manager.should_compact_with(&messages),
        "compaction should resume once a genuinely new message is added"
    );
}

#[test]
fn test_token_estimate() {
    let manager = CompactionManager::new();
    // 100 chars = ~25 tokens (plus 18k overhead for full budget)
    let messages = vec![make_text_message(Role::User, &"x".repeat(100))];
    let estimate = manager.token_estimate_with(&messages);
    // With DEFAULT_TOKEN_BUDGET and 18k overhead: 25 + 18000 = 18025
    assert!((18_000..19_000).contains(&estimate));
}

#[test]
fn test_should_compact() {
    let mut manager = CompactionManager::new().with_budget(100); // Very small budget

    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(
            Role::User,
            &format!("Message {} with some content", i),
        ));
        manager.notify_message_added();
    }

    assert!(manager.should_compact_with(&messages));
}

#[test]
fn test_context_usage_prefers_observed_tokens() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let messages = vec![make_text_message(Role::User, "short message")];
    manager.notify_message_added();
    manager.update_observed_input_tokens(900);

    assert!(manager.context_usage_with(&messages) >= 0.90);
    assert!(manager.effective_token_count_with(&messages) >= 900);
}

#[test]
fn test_should_compact_uses_observed_tokens() {
    let mut manager = CompactionManager::new().with_budget(1_000);

    let mut messages = Vec::new();
    for _ in 0..12 {
        messages.push(make_text_message(Role::User, "x"));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(850);

    assert!(manager.should_compact_with(&messages));
}

#[test]
fn test_messages_for_api_no_summary() {
    let mut manager = CompactionManager::new();
    let messages = vec![
        make_text_message(Role::User, "Hello"),
        make_text_message(Role::Assistant, "Hi!"),
    ];
    manager.notify_message_added();
    manager.notify_message_added();

    let msgs = manager.messages_for_api_with(&messages);
    assert_eq!(msgs.len(), 2);
}

#[tokio::test]
async fn test_force_compact_applies_summary() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..30 {
        messages.push(make_text_message(
            Role::User,
            &format!("Turn {} {}", i, "x".repeat(120)),
        ));
        manager.notify_message_added();
    }

    let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
    manager
        .force_compact_with(&messages, provider)
        .expect("manual compaction should start");

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        manager.check_and_apply_compaction();
        if manager.stats().has_summary {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        manager.stats().has_summary,
        "summary should be applied after compaction task completes"
    );

    // After compaction, compacted_count should be > 0
    assert!(manager.compacted_count > 0);

    let msgs = manager.messages_for_api_with(&messages);
    assert!(msgs.len() < 30);
    let first = msgs.first().expect("summary message missing");
    assert_eq!(first.role, Role::User);
    match &first.content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("Previous Conversation Summary"));
        }
        _ => panic!("expected text summary block"),
    }
}

// ── ensure_context_fits tests ──────────────────────────────

#[tokio::test]
async fn test_guard_below_80_does_nothing() {
    let mut manager = CompactionManager::new().with_budget(10_000);
    let mut messages = Vec::new();
    for i in 0..15 {
        messages.push(make_text_message(Role::User, &format!("msg {}", i)));
        manager.notify_message_added();
    }
    // Char estimate is tiny, observed tokens well below 80%
    manager.update_observed_input_tokens(5_000);

    let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
    let action = manager.ensure_context_fits(&messages, provider);
    assert_eq!(
        action,
        CompactionAction::None,
        "should do nothing below 80%"
    );
    assert!(
        !manager.is_compacting(),
        "should NOT start background compaction below 80%"
    );
    assert_eq!(manager.compacted_count, 0);
}

#[tokio::test]
async fn test_guard_between_80_and_95_starts_background_only() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(Role::User, &format!("msg {}", i)));
        manager.notify_message_added();
    }
    // 85% usage — above 80% threshold but below 95% critical
    manager.update_observed_input_tokens(850);

    let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
    let action = manager.ensure_context_fits(&messages, provider);
    assert_eq!(
        action,
        CompactionAction::BackgroundStarted {
            trigger: "reactive".to_string()
        },
        "should start background compaction at 85%"
    );
    assert!(
        manager.is_compacting(),
        "SHOULD start background compaction at 85%"
    );
    assert_eq!(
        manager.compacted_count, 0,
        "compacted_count should stay 0 (no hard compact)"
    );
}

#[tokio::test]
async fn test_guard_at_95_triggers_hard_compact() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(
            Role::User,
            &format!("message {} with padding {}", i, "x".repeat(50)),
        ));
        manager.notify_message_added();
    }
    // 96% usage — above critical threshold
    manager.update_observed_input_tokens(960);

    let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
    let action = manager.ensure_context_fits(&messages, provider);
    assert!(
        matches!(action, CompactionAction::HardCompacted(_)),
        "SHOULD hard-compact at 96%"
    );
    assert!(
        manager.compacted_count > 0,
        "compacted_count should increase after hard compact"
    );
    assert!(
        manager.active_summary.is_some(),
        "should have an emergency summary"
    );
}

#[tokio::test]
async fn test_guard_at_100_percent_drops_messages() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..30 {
        messages.push(make_text_message(
            Role::User,
            &format!("turn {} content {}", i, "y".repeat(80)),
        ));
        manager.notify_message_added();
    }
    // Over 100% — simulates the exact bug scenario
    manager.update_observed_input_tokens(1_050);

    let provider: Arc<dyn Provider> = Arc::new(MockSummaryProvider);
    let action = manager.ensure_context_fits(&messages, provider);
    assert!(
        matches!(action, CompactionAction::HardCompacted(_)),
        "MUST hard-compact when over 100%"
    );

    let api_messages = manager.messages_for_api_with(&messages);
    assert!(
        api_messages.len() < messages.len(),
        "API messages should be fewer after hard compact"
    );
    // First message should be the emergency summary
    match &api_messages[0].content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("Previous Conversation Summary"));
            assert!(text.contains("Emergency compaction"));
        }
        _ => panic!("expected text summary block"),
    }
}

// ── hard_compact_with edge cases ────────────────────────────────

#[test]
fn test_hard_compact_too_few_messages() {
    let mut manager = CompactionManager::new().with_budget(100);
    let messages = vec![
        make_text_message(Role::User, "hello"),
        make_text_message(Role::Assistant, "hi"),
    ];
    manager.notify_message_added();
    manager.notify_message_added();

    let result = manager.hard_compact_with(&messages);
    assert!(
        result.is_err(),
        "should fail with only 2 messages (MIN_TURNS_TO_KEEP)"
    );
}

#[test]
fn test_hard_compact_preserves_recent_turns() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..25 {
        messages.push(make_text_message(Role::User, &format!("turn {}", i)));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(950);

    let dropped = manager
        .hard_compact_with(&messages)
        .expect("should compact");
    assert!(dropped > 0, "should drop some messages");
    assert!(dropped < 25, "should not drop ALL messages");

    let api_messages = manager.messages_for_api_with(&messages);
    // Should have summary + recent turns
    assert!(
        api_messages.len() >= 2,
        "should keep at least MIN_TURNS_TO_KEEP + summary"
    );
    assert!(
        api_messages.len() <= 15,
        "should have dropped a significant number"
    );
}

// ── safe_compaction_cutoff: tool call/result pair integrity ─────────

#[test]
fn test_safe_cutoff_preserves_tool_pairs() {
    // Messages: [user, assistant(tool_use), user(tool_result), assistant, user]
    // If cutoff tries to split between tool_use and tool_result, it should back up
    let messages = vec![
        make_text_message(Role::User, "do something"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "ls"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "file1.txt\nfile2.txt".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        make_text_message(Role::Assistant, "I see the files"),
        make_text_message(Role::User, "thanks"),
    ];

    // Try to cut between tool_use (index 1) and tool_result (index 2)
    let cutoff = safe_compaction_cutoff(&messages, 2);
    // Should move back to include the tool_use at index 1
    assert!(
        cutoff <= 1,
        "cutoff should back up to include tool_use (got {})",
        cutoff
    );
}

#[test]
fn test_safe_cutoff_no_tool_pairs() {
    let messages = vec![
        make_text_message(Role::User, "hello"),
        make_text_message(Role::Assistant, "hi"),
        make_text_message(Role::User, "how are you"),
        make_text_message(Role::Assistant, "fine"),
    ];

    let cutoff = safe_compaction_cutoff(&messages, 2);
    assert_eq!(cutoff, 2, "no tool pairs, cutoff should stay unchanged");
}

#[test]
fn test_safe_cutoff_handles_chained_tool_dependencies_without_rescan() {
    let messages = vec![
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tool_a".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file": "a.txt"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        make_text_message(Role::User, "intermediate"),
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "tool_a".to_string(),
                    content: "a contents".to_string(),
                    is_error: Some(false),
                },
                ContentBlock::ToolUse {
                    id: "tool_b".to_string(),
                    name: "grep".to_string(),
                    input: serde_json::json!({"pattern": "foo"}),
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_b".to_string(),
                content: "foo".to_string(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        make_text_message(Role::Assistant, "done"),
    ];

    let cutoff = safe_compaction_cutoff(&messages, 3);
    assert_eq!(
        cutoff, 0,
        "cutoff should walk back through nested tool dependencies until the kept suffix is self-contained"
    );
}

// ── emergency_truncate_with ─────────────────────────────────────

#[test]
fn test_emergency_truncate_large_tool_results() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let big_result = "x".repeat(10_000); // Way over EMERGENCY_TOOL_RESULT_MAX_CHARS (4000)
    let mut messages = vec![
        make_text_message(Role::User, "run something"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cat bigfile"}),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: big_result.clone(),
                is_error: Some(false),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        make_text_message(Role::Assistant, "that's a big file"),
    ];
    for _ in &messages {
        manager.notify_message_added();
    }

    let truncated = manager.emergency_truncate_with(&mut messages);
    assert_eq!(truncated, 1, "should truncate exactly 1 tool result");

    // Check the truncated content
    if let ContentBlock::ToolResult { content, .. } = &messages[2].content[0] {
        assert!(
            content.len() < big_result.len(),
            "content should be shorter"
        );
        assert!(
            content.contains("truncated for context recovery"),
            "should have truncation marker"
        );
    } else {
        panic!("expected tool result");
    }
}

#[test]
fn test_emergency_truncate_skips_small_results() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool_1".to_string(),
            content: "small output".to_string(),
            is_error: Some(false),
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];
    manager.notify_message_added();

    let truncated = manager.emergency_truncate_with(&mut messages);
    assert_eq!(truncated, 0, "should not truncate small results");
}

// ── Double compaction ───────────────────────────────────────────

#[test]
fn test_hard_compact_twice() {
    let mut manager = CompactionManager::new().with_budget(500);
    let mut messages = Vec::new();
    for i in 0..30 {
        messages.push(make_text_message(
            Role::User,
            &format!("turn {} {}", i, "z".repeat(40)),
        ));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(480);

    // First hard compact
    let dropped1 = manager
        .hard_compact_with(&messages)
        .expect("first compact should work");
    assert!(dropped1 > 0);
    let count_after_first = manager.compacted_count;

    // Simulate more messages arriving after first compact
    for i in 30..45 {
        messages.push(make_text_message(
            Role::User,
            &format!("turn {} {}", i, "z".repeat(40)),
        ));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(490);

    // Second hard compact
    let dropped2 = manager
        .hard_compact_with(&messages)
        .expect("second compact should work");
    assert!(dropped2 > 0);
    assert!(
        manager.compacted_count > count_after_first,
        "compacted_count should increase"
    );

    // Summary should mention both compactions
    let api_messages = manager.messages_for_api_with(&messages);
    assert!(api_messages.len() < messages.len());
    match &api_messages[0].content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("Emergency compaction"));
        }
        _ => panic!("expected summary"),
    }
}

// ── messages_for_api_with after compaction ──────────────────────

#[test]
fn test_messages_for_api_with_summary_prepended() {
    let mut manager = CompactionManager::new().with_budget(500);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(Role::User, &format!("turn {}", i)));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(490);

    manager
        .hard_compact_with(&messages)
        .expect("should compact");

    let api_msgs = manager.messages_for_api_with(&messages);
    // First message should be the summary
    assert_eq!(api_msgs[0].role, Role::User);
    match &api_msgs[0].content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.starts_with("## Previous Conversation Summary"));
        }
        _ => panic!("expected text"),
    }
    // Remaining should be recent turns from original messages
    assert!(api_msgs.len() < messages.len());
}

#[test]
fn test_persisted_state_round_trip_preserves_compacted_view() {
    let mut manager = CompactionManager::new().with_budget(500);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(
            Role::User,
            &format!("turn {} {}", i, "x".repeat(40)),
        ));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(490);
    manager
        .hard_compact_with(&messages)
        .expect("should compact before persisting");

    let persisted = manager
        .persisted_state()
        .expect("compaction state should be exportable");
    let expected = manager.messages_for_api_with(&messages);

    let mut restored = CompactionManager::new().with_budget(500);
    restored.restore_persisted_state(&persisted, messages.len());
    let restored_msgs = restored.messages_for_api_with(&messages);

    assert_eq!(restored.compacted_count, persisted.compacted_count);
    assert_eq!(restored_msgs.len(), expected.len());
    match &restored_msgs[0].content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("Previous Conversation Summary"));
            assert!(text.contains("Emergency compaction"));
        }
        _ => panic!("expected restored summary block"),
    }
}

// ── context_usage accuracy ──────────────────────────────────────

#[test]
fn test_context_usage_with_both_estimate_and_observed() {
    let mut manager = CompactionManager::new().with_budget(200_000);
    // Build messages totalling ~50k chars = ~12.5k token estimate
    let mut messages = Vec::new();
    for i in 0..50 {
        messages.push(make_text_message(
            Role::User,
            &format!("{} {}", i, "a".repeat(1000)),
        ));
        manager.notify_message_added();
    }

    // Without observed tokens, usage should be based on char estimate
    let usage_no_observed = manager.context_usage_with(&messages);
    assert!(
        usage_no_observed < 0.2,
        "char estimate should be low: {}",
        usage_no_observed
    );

    // With observed tokens at 160k, should use observed (higher) value
    manager.update_observed_input_tokens(160_000);
    let usage_with_observed = manager.context_usage_with(&messages);
    assert!(
        usage_with_observed >= 0.79,
        "should use observed tokens: {}",
        usage_with_observed
    );
}

#[test]
fn test_context_usage_after_compaction_resets_observed() {
    let mut manager = CompactionManager::new().with_budget(1_000);
    let mut messages = Vec::new();
    for i in 0..20 {
        messages.push(make_text_message(
            Role::User,
            &format!("msg {} pad {}", i, "x".repeat(50)),
        ));
        manager.notify_message_added();
    }
    manager.update_observed_input_tokens(960);

    // Hard compact should reset observed_input_tokens
    manager
        .hard_compact_with(&messages)
        .expect("should compact");
    assert!(
        manager.observed_input_tokens.is_none(),
        "observed_input_tokens should be cleared after hard compact"
    );

    // After compaction, usage should be based on char estimate of remaining messages only
    let post_usage = manager.context_usage_with(&messages);
    // The remaining messages are small, so usage should be well below the critical threshold
    assert!(
        post_usage < CRITICAL_THRESHOLD,
        "post-compaction usage should be below critical: {}",
        post_usage
    );
}
