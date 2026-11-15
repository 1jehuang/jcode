use super::*;

#[test]
fn test_debug_memory_profile_retained_capacity_deduplicates_cache() {
    let mut session = Session::create_with_id("capacity_profile_test".into(), None, None);
    let mut text = String::with_capacity(64 * 1024);
    text.push_str("small payload");
    let expected_capacity = text.capacity();
    let expected_len = text.len();
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    );
    let before = session.debug_memory_profile();
    let _ = session.provider_messages();
    let profile = session.debug_memory_profile();
    let capacity = &profile["retained_transcript_capacity"];
    assert_eq!(capacity["string_len_bytes"], expected_len);
    assert_eq!(capacity["string_capacity_bytes"], expected_capacity);
    assert_eq!(capacity["shared_backings"], 1);
    assert_eq!(capacity["shared_references"], 2);
    assert_eq!(capacity["is_lower_bound"], true);
    let vector_bytes = session.messages.capacity() * std::mem::size_of::<StoredMessage>()
        + session.provider_messages_cache.capacity() * std::mem::size_of::<Message>()
        + session.messages[0].content.capacity() * std::mem::size_of::<ContentBlock>();
    assert_eq!(capacity["vec_capacity_bytes"], vector_bytes);
    assert_eq!(
        capacity["known_heap_bytes"],
        vector_bytes + expected_capacity
    );
    assert_eq!(
        before["retained_transcript_capacity"]["string_capacity_bytes"],
        capacity["string_capacity_bytes"]
    );
    // Diagnostics must not detach backing storage or mutate the provider cache.
    assert!(
        session.messages[0]
            .content
            .shares_storage_with(&session.provider_messages_cache[0].content)
    );
}

#[test]
fn test_debug_memory_profile_retained_capacity_counts_detached_cache() {
    let mut session = Session::create_with_id("capacity_detached_profile_test".into(), None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Reasoning {
            text: "original".into(),
        }],
    );
    let _ = session.provider_messages();
    if let ContentBlock::Reasoning { text } = &mut session.provider_messages_cache[0].content[0] {
        text.reserve(8192);
    }
    let expected_capacity = [
        &session.messages[0].content,
        &session.provider_messages_cache[0].content,
    ]
    .iter()
    .map(|content| match &content[0] {
        ContentBlock::Reasoning { text } => text.capacity(),
        _ => unreachable!(),
    })
    .sum::<usize>();
    let profile = session.debug_memory_profile();
    assert_eq!(
        profile["retained_transcript_capacity"]["shared_backings"],
        2
    );
    assert_eq!(
        profile["retained_transcript_capacity"]["string_capacity_bytes"],
        expected_capacity
    );
}

#[test]
fn test_debug_memory_profile_reports_messages_and_provider_cache() {
    let mut session = Session::create_with_id(
        "session_memory_profile_test".to_string(),
        None,
        Some("Memory profile".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello world".to_string(),
            cache_control: None,
        }],
    );
    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "echo hi"}),
                thought_signature: None,
            },
            ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "hi".to_string(),
                is_error: None,
            },
        ],
    );

    session.compaction = Some(
        StoredCompactionState {
            summary_text: "summary".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 7,
            original_turn_count: 9,
            compacted_count: 7,
        }
        .into(),
    );

    let _ = session.provider_messages();
    let profile = session.debug_memory_profile();

    assert_eq!(profile["messages"]["count"], 2);
    assert_eq!(profile["messages"]["memory"]["text_blocks"], 1);
    assert_eq!(profile["messages"]["memory"]["tool_use_blocks"], 1);
    assert_eq!(profile["messages"]["memory"]["tool_result_blocks"], 1);
    assert!(profile["messages"]["json_bytes"].as_u64().unwrap_or(0) > 0);
    assert_eq!(profile["provider_messages_cache"]["count"], 2);
    assert_eq!(profile["compaction"]["present"], true);
    assert_eq!(profile["compaction"]["covers_up_to_turn"], 7);
    assert_eq!(profile["compaction"]["original_turn_count"], 9);
    assert_eq!(profile["compaction"]["compacted_count"], 7);
    assert!(
        profile["provider_messages_cache"]["json_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
}
