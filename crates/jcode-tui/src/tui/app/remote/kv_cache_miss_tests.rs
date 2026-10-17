use super::*;
use crate::tui::app::tests::create_test_app;
use crate::tui::backend::RemoteConnection;

fn miss(reason: &str) -> ServerEvent {
    ServerEvent::KvCacheMiss {
        reason: reason.into(),
        harness_caused: false,
        missed_tokens: 8_000,
        expected_tokens: 10_000,
        read_tokens: 2_000,
        documented_cause: None,
        message: "daemon KV cache miss".into(),
    }
}

#[test]
fn remote_kv_cache_miss_without_baseline_is_visible_in_order() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = RemoteConnection::dummy();
    app.streaming.streaming_text = "preceding assistant text".into();
    assert!(app.kv_cache.kv_cache_baseline.is_none());
    assert!(handle_server_event(&mut app, miss("expired"), &mut remote));
    let last = app.display_messages.len() - 1;
    assert_eq!(app.display_messages[last].role, "system");
    assert_eq!(app.display_messages[last].content, "daemon KV cache miss");
    assert_eq!(
        app.display_messages[last - 1].content,
        "preceding assistant text"
    );
    assert!(app.streaming.streaming_text.is_empty());
}

#[test]
fn remote_kv_cache_miss_maps_sample_and_deduplicates_current_call_only() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = RemoteConnection::dummy();
    app.kv_cache.kv_cache_turn_number = Some(3);
    app.kv_cache.kv_cache_turn_call_index = 2;
    app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
        turn_number: 3,
        call_index: 1,
        missed_tokens: 1_500,
        reason: KvCacheMissReason::ZeroRead,
    });
    assert!(handle_server_event(
        &mut app,
        miss("tools_changed"),
        &mut remote
    ));
    let sample = app.kv_cache.kv_cache_miss_samples.last().unwrap();
    assert_eq!(sample.reason, KvCacheMissReason::HarnessToolsChanged);
    assert_eq!(
        (sample.turn_number, sample.call_index, sample.missed_tokens),
        (3, 2, 8_000)
    );
    let messages = app.display_messages.len();
    assert!(!handle_server_event(
        &mut app,
        miss("tools_changed"),
        &mut remote
    ));
    assert_eq!(app.display_messages.len(), messages);
    assert_eq!(app.kv_cache.kv_cache_miss_samples.len(), 2);
}

#[test]
fn remote_kv_cache_miss_deduplicates_token_usage_local_classification() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = RemoteConnection::dummy();
    let messages = vec![crate::message::Message::user("prompt")];
    app.begin_kv_cache_request(&messages, &[], "system", "");
    handle_server_event(
        &mut app,
        ServerEvent::TokenUsage {
            input: 10_000,
            output: 10,
            cache_read_input: Some(0),
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.kv_cache.current_api_usage_recorded = false;
    app.begin_kv_cache_request(&messages, &[], "system", "");
    handle_server_event(
        &mut app,
        ServerEvent::TokenUsage {
            input: 10_000,
            output: 10,
            cache_read_input: Some(0),
            cache_creation_input: None,
        },
        &mut remote,
    );
    assert!(!app.kv_cache.kv_cache_miss_samples.is_empty());
    let count = app.display_messages.len();
    assert!(!handle_server_event(
        &mut app,
        miss("zero_read"),
        &mut remote
    ));
    assert_eq!(app.display_messages.len(), count);
}

#[test]
fn remote_kv_cache_miss_disabled_suppresses_notice_but_retains_widget_sample() {
    let mut app = create_test_app();
    app.kv_cache.kv_cache_turn_number = Some(1);
    app.kv_cache.kv_cache_turn_call_index = 1;
    let count = app.display_messages.len();
    assert!(!handle_kv_cache_miss(
        &mut app,
        "future_reason",
        8_000,
        "hidden".into(),
        false
    ));
    assert_eq!(app.display_messages.len(), count);
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason,
        KvCacheMissReason::Unknown
    );
}
