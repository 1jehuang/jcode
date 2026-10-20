use super::*;
use crate::session::StoredTokenUsage;

fn assistant_message(
    id: &str,
    input_tokens: u64,
    output_tokens: u64,
    prompt_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
) -> crate::session::StoredMessage {
    crate::session::StoredMessage {
        id: id.to_string(),
        role: Role::Assistant,
        content: Vec::new(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: Some(StoredTokenUsage {
            prompt_tokens,
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
            cost: None,
            currency: None,
        }),
    }
}

#[test]
fn sums_assistant_usage_only_from_the_turn_start() {
    let messages = vec![
        assistant_message("before", 100, 20, Some(100), Some(30), Some(4)),
        crate::session::StoredMessage {
            id: "user".to_string(),
            role: Role::User,
            content: Vec::new(),
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        },
        assistant_message("first", 10, 2, Some(12), Some(3), None),
        assistant_message("second", 20, 4, Some(25), Some(5), Some(7)),
    ];

    let usage = turn_token_usage_after(&messages, 1).expect("turn usage");
    assert_eq!(usage.prompt_tokens, Some(37));
    assert_eq!(usage.input_tokens, 30);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.cache_read_input_tokens, Some(8));
    assert_eq!(usage.cache_creation_input_tokens, Some(7));
}

#[test]
fn returns_none_without_assistant_usage() {
    let messages = vec![crate::session::StoredMessage {
        id: "user".to_string(),
        role: Role::User,
        content: Vec::new(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }];

    assert_eq!(turn_token_usage_after(&messages, 0), None);
}
