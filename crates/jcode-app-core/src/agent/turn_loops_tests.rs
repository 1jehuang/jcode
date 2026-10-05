use super::*;

fn user_text(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn tool_result(id: &str, content: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: None,
        }],
        timestamp: None,
        tool_duration_ms: Some(1),
    }
}

#[test]
fn messages_end_with_tool_result_detects_tool_continuation_context() {
    let messages = vec![
        user_text("tell me about the desktop application"),
        tool_result("functions.read:0", "desktop architecture docs"),
        tool_result("functions.agentgrep:4", "desktop source summary"),
    ];

    assert!(Agent::messages_end_with_tool_result(&messages));
}

#[test]
fn messages_end_with_tool_result_allows_memory_after_tool_results() {
    let messages = vec![
        user_text("tell me about the desktop application"),
        tool_result("functions.read:0", "desktop architecture docs"),
        user_text("<system-reminder>Relevant memory</system-reminder>"),
    ];

    assert!(Agent::messages_end_with_tool_result(&messages));
}

#[test]
fn recovery_reminder_alone_is_not_a_tool_result() {
    // The recovery continuation is itself a User-role `<system-reminder>`. If the
    // predicate counted it, the injected reminder would keep
    // `prompt_has_recent_tool_result` true on the following turn even with no
    // tool result anywhere near, so every whitespace-only provider response
    // would inject another recovery reminder, up to the attempt cap, spending
    // an API call each time.
    let messages = vec![user_text(
        "<system-reminder>The previous provider response was empty after tool results. Provide the final answer to the user's last request using the tool results above. Do not call more tools unless absolutely necessary.</system-reminder>",
    )];

    assert!(
        !Agent::messages_end_with_tool_result(&messages),
        "a recovery reminder must not count as evidence of tool results"
    );
}

#[test]
fn messages_end_with_tool_result_ignores_plain_user_prompt() {
    let messages = vec![user_text("hello")];

    assert!(!Agent::messages_end_with_tool_result(&messages));
}

#[test]
fn sequential_tool_rounds_trigger_after_three_single_calls() {
    let mut rounds = 0;
    for _ in 0..3 {
        rounds = Agent::update_sequential_tool_rounds(rounds, 1, false);
    }

    assert_eq!(rounds, Agent::SEQUENTIAL_TOOL_ROUNDS_BEFORE_BATCH_NUDGE);
}

#[test]
fn parallel_or_batch_calls_reset_sequential_tool_rounds() {
    assert_eq!(Agent::update_sequential_tool_rounds(2, 2, false), 0);
    assert_eq!(Agent::update_sequential_tool_rounds(2, 1, true), 0);
    assert_eq!(Agent::update_sequential_tool_rounds(2, 0, false), 0);
}

#[test]
fn pending_nudge_is_injected_only_when_batch_is_available() {
    assert!(Agent::should_inject_batch_nudge(true, true));
    assert!(!Agent::should_inject_batch_nudge(false, true));
    assert!(!Agent::should_inject_batch_nudge(true, false));
    assert!(Agent::BATCH_NUDGE.contains("use the batch tool"));
    assert!(Agent::BATCH_NUDGE.contains("result is required"));
}

#[test]
fn plan_limit_reminder_relays_upgrade_link_without_purchasing() {
    let reminder = Agent::plan_limit_reminder(&crate::subscription_notice::QuotaExceeded {
        feature: "memory".into(),
        tier: Some("plus".into()),
        upgrade_tier: Some("pro".into()),
        upgrade_url: Some("https://jcode.sh/pricing".into()),
        resets_at: None,
    });
    assert!(reminder.starts_with("<system-reminder>"));
    assert!(reminder.contains("Daily memory recall limit reached on your Plus plan"));
    assert!(reminder.contains("Upgrade to Pro"));
    assert!(reminder.contains("https://jcode.sh/pricing"));
    assert!(reminder.contains("Do not open checkout"));
}
