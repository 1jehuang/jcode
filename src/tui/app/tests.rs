#![cfg_attr(test, allow(clippy::clone_on_copy))]

include!("tests/support_failover/part_01.rs");
include!("tests/support_failover/part_02.rs");
include!("tests/commands_accounts_01/part_01.rs");
include!("tests/commands_accounts_01/part_02.rs");
include!("tests/commands_accounts_02/part_01.rs");
include!("tests/commands_accounts_02/part_02.rs");
include!("tests/state_model_poke_01/part_01.rs");
include!("tests/state_model_poke_01/part_02.rs");
include!("tests/state_model_poke_02/part_01.rs");
include!("tests/state_model_poke_02/part_02.rs");
include!("tests/state_model_poke_03.rs");
include!("tests/remote_startup_input_01/part_01.rs");
include!("tests/remote_startup_input_01/part_02.rs");
include!("tests/remote_startup_input_02/part_01.rs");
include!("tests/remote_startup_input_02/part_02.rs");
include!("tests/remote_startup_input_03/part_01.rs");
include!("tests/remote_startup_input_03/part_02.rs");
include!("tests/remote_startup_input_04.rs");
include!("tests/remote_events_reload_01/part_01.rs");
include!("tests/remote_events_reload_01/part_02.rs");
include!("tests/remote_events_reload_02/part_01.rs");
include!("tests/remote_events_reload_02/part_02.rs");
include!("tests/remote_events_reload_03/part_01.rs");
include!("tests/remote_events_reload_03/part_02.rs");
include!("tests/remote_events_reload_04.rs");
include!("tests/scroll_copy_01/part_01.rs");
include!("tests/scroll_copy_01/part_02.rs");
include!("tests/scroll_copy_02/part_01.rs");
include!("tests/scroll_copy_02/part_02.rs");
include!("tests/scroll_copy_03.rs");

#[test]
fn kv_cache_signature_prefix_match_allows_appended_messages() {
    let baseline_messages = vec![
        crate::message::Message::user("first prompt"),
        crate::message::Message::assistant_text("first answer"),
    ];
    let mut current_messages = baseline_messages.clone();
    current_messages.push(crate::message::Message::user("follow up"));

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "memory a");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "memory b");

    assert!(App::kv_cache_signatures_prefix_match(&current, &baseline));
    assert_eq!(
        App::kv_cache_common_prefix_messages(&current, &baseline),
        baseline_messages.len()
    );
    assert_ne!(baseline.ephemeral_hash, current.ephemeral_hash);
}

#[test]
fn kv_cache_signature_prefix_match_detects_prefix_mutation() {
    let baseline_messages = vec![
        crate::message::Message::user("first prompt"),
        crate::message::Message::assistant_text("first answer"),
    ];
    let current_messages = vec![
        crate::message::Message::user("changed first prompt"),
        crate::message::Message::assistant_text("first answer"),
        crate::message::Message::user("follow up"),
    ];

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "");

    assert!(!App::kv_cache_signatures_prefix_match(&current, &baseline));
    assert_eq!(App::kv_cache_common_prefix_messages(&current, &baseline), 0);
}

#[test]
fn cold_cache_warning_is_persisted_when_starting_next_request() {
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::user("first"));
    app.kv_cache_baseline = Some(KvCacheBaseline {
        input_tokens: 911_873,
        completed_at: Instant::now() - Duration::from_secs(301),
        provider: "anthropic".to_string(),
        model: "claude-opus-4-6".to_string(),
        upstream_provider: None,
        signature: None,
    });

    app.display_messages.push(DisplayMessage::user("second"));
    app.begin_kv_cache_request(&[Message::user("second")], &[], "system", "");

    let warning = app
        .display_messages()
        .iter()
        .find(|message| {
            message.role == "system" && message.content.contains("Prompt cache is cold")
        })
        .expect("cold cache warning should be persisted in the transcript");
    assert!(warning.content.contains("911K"));
    assert!(warning.content.contains("300s TTL expired"));
}

#[test]
fn oversized_pasted_submit_is_rejected_and_preserves_input() {
    let mut app = create_test_app();
    let pasted = format!(
        "{}tail",
        "x\n".repeat(crate::tui::app::input::MAX_SUBMITTED_TEXT_BYTES / 2 + 1)
    );

    crate::tui::app::input::handle_text_paste(&mut app, pasted);
    let placeholder = app.input.clone();
    assert!(placeholder.starts_with("[pasted "));

    app.submit_input();

    assert!(
        !app.is_processing,
        "oversized input must not enter sending state"
    );
    assert_eq!(
        app.input, placeholder,
        "placeholder input should be preserved"
    );
    assert_eq!(
        app.pasted_contents.len(),
        1,
        "expanded paste should remain recoverable"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.role == "system"
                && message.content.contains("Message is too large to send"))
    );
}
