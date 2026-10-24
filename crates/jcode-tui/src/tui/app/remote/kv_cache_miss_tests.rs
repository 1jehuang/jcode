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
fn remote_prefix_tag_upgrades_provisional_notice_once_and_preserves_counters() {
    for reason in [
        KvCacheMissReason::LowRead,
        KvCacheMissReason::HarnessToolsChanged,
    ] {
        let mut app = create_test_app();
        app.kv_cache.kv_cache_turn_number = Some(2);
        app.kv_cache.kv_cache_turn_call_index = 3;
        app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
            turn_number: 2,
            call_index: 3,
            missed_tokens: 15_618,
            reason,
        });
        let content = format!(
            "ℹ️ KV cache miss [{}] turn 2 call 3: 4.35K cached / 20K expected tokens: ~15.6K tokens resent (provisional).",
            reason.label()
        );
        app.push_display_message(DisplayMessage::system(content.clone()));
        let count = app.display_messages.len();
        let daemon = "KV cache miss [low_read] [prefix unchanged]: ~15.6K tokens resent";
        let expected_upgrade = reason == KvCacheMissReason::LowRead;
        assert_eq!(
            handle_kv_cache_miss(&mut app, "low_read", 15_618, daemon.into(), true),
            expected_upgrade
        );
        assert_eq!(app.display_messages.len(), count);
        let notice = &app.display_messages[count - 1].content;
        assert_eq!(notice.contains("[prefix unchanged]"), expected_upgrade);
        assert!(notice.contains("turn 2 call 3: 4.35K cached / 20K expected tokens"));
        if !expected_upgrade {
            assert_eq!(notice, &content);
        }
        assert!(!handle_kv_cache_miss(
            &mut app,
            "low_read",
            15_618,
            daemon.into(),
            true
        ));
        assert_eq!(app.display_messages.len(), count);
    }
}

#[test]
fn remote_prefix_tag_is_revoked_by_authoritative_harness_change() {
    for (reason, expected) in [
        ("system_changed", KvCacheMissReason::HarnessSystemChanged),
        ("tools_changed", KvCacheMissReason::HarnessToolsChanged),
        ("prefix_changed", KvCacheMissReason::HarnessPrefixChanged),
    ] {
        let mut app = create_test_app();
        app.kv_cache.kv_cache_turn_number = Some(2);
        app.kv_cache.kv_cache_turn_call_index = 3;
        app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
            turn_number: 2,
            call_index: 3,
            missed_tokens: 15_618,
            reason: KvCacheMissReason::LowRead,
        });
        app.push_display_message(DisplayMessage::system(
            "ℹ️ KV cache miss [low read] [prefix unchanged] turn 2 call 3: provisional",
        ));
        let count = app.display_messages.len();
        let corrected = format!("KV cache miss [{reason}]: ~15.6K tokens resent");
        assert!(handle_kv_cache_miss(
            &mut app,
            reason,
            15_618,
            corrected.clone(),
            true
        ));
        assert_eq!(app.display_messages.len(), count);
        assert_eq!(app.display_messages[count - 1].content, corrected);
        assert_eq!(app.kv_cache.kv_cache_miss_samples[0].reason, expected);
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

#[test]
fn remote_compaction_refresh_corrects_provisional_miss_in_place() {
    let mut app = create_test_app();
    app.kv_cache.kv_cache_turn_number = Some(19);
    app.kv_cache.kv_cache_turn_call_index = 36;
    app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
        turn_number: 19,
        call_index: 36,
        missed_tokens: 216_517,
        reason: KvCacheMissReason::ZeroRead,
    });
    app.push_display_message(DisplayMessage::system(
        "ℹ️ KV cache miss [zero read] turn 19 call 36: provisional",
    ));
    app.push_display_message(DisplayMessage::system("later transcript event"));
    let count = app.display_messages.len();
    let corrected =
        "ℹ️ KV cache refresh [provider_auto_compaction] turn 19 call 36: expected rebuild";
    assert!(handle_kv_cache_miss(
        &mut app,
        "compaction",
        204_393,
        corrected.into(),
        true
    ));
    assert_eq!(app.display_messages.len(), count);
    assert_eq!(app.display_messages[count - 2].content, corrected);
    assert_eq!(
        app.display_messages[count - 1].content,
        "later transcript event"
    );
    assert_eq!(app.kv_cache.kv_cache_miss_samples.len(), 1);
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason,
        KvCacheMissReason::Compaction
    );
    assert_eq!(app.kv_cache.kv_cache_miss_samples[0].missed_tokens, 204_393);
    assert!(!handle_kv_cache_miss(
        &mut app,
        "compaction",
        204_393,
        corrected.into(),
        true
    ));
}

#[test]
fn remote_compaction_refresh_maps_without_local_baseline() {
    let mut app = create_test_app();
    app.kv_cache.kv_cache_turn_number = Some(2);
    app.kv_cache.kv_cache_turn_call_index = 1;
    assert!(handle_kv_cache_miss(
        &mut app,
        "compaction",
        12_000,
        "expected refresh".into(),
        true
    ));
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason,
        KvCacheMissReason::Compaction
    );
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason.label(),
        "compaction refresh"
    );
}

#[test]
fn remote_compaction_refresh_keeps_disabled_notices_hidden() {
    let mut app = create_test_app();
    app.kv_cache.kv_cache_turn_number = Some(2);
    app.kv_cache.kv_cache_turn_call_index = 1;
    assert!(!handle_kv_cache_miss(
        &mut app,
        "zero_read",
        12_000,
        "hidden miss".into(),
        false
    ));
    let count = app.display_messages.len();
    assert!(!handle_kv_cache_miss(
        &mut app,
        "compaction",
        11_000,
        "hidden refresh".into(),
        false
    ));
    assert_eq!(app.display_messages.len(), count);
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason,
        KvCacheMissReason::Compaction
    );
}

#[test]
fn remote_compaction_notice_follows_buffered_response() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = RemoteConnection::dummy();
    app.streaming.streaming_text = "response before compaction".into();
    handle_server_event(
        &mut app,
        ServerEvent::Compaction {
            trigger: "automatic".into(),
            pre_tokens: Some(216_517),
            post_tokens: None,
            tokens_saved: None,
            duration_ms: None,
            messages_dropped: None,
            messages_compacted: None,
            summary_chars: None,
            active_messages: None,
        },
        &mut remote,
    );
    let count = app.display_messages.len();
    assert_eq!(
        app.display_messages[count - 2].content,
        "response before compaction"
    );
    assert!(
        app.display_messages[count - 1]
            .content
            .starts_with("📦 Compacted (automatic): from ~216,517 tokens")
    );
    assert!(!app.display_messages[count - 1].content.contains("unknown"));
    assert!(
        !app.display_messages[count - 1]
            .content
            .contains("unavailable")
    );
    assert!(app.streaming.streaming_text.is_empty());
}

#[test]
fn remote_daily_rollover_corrects_provisional_low_read_in_place() {
    let mut app = create_test_app();
    app.kv_cache.kv_cache_turn_number = Some(7);
    app.kv_cache.kv_cache_turn_call_index = 1;
    app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
        turn_number: 7,
        call_index: 1,
        missed_tokens: 590_713,
        reason: KvCacheMissReason::LowRead,
    });
    app.push_display_message(DisplayMessage::system(
        "ℹ️ KV cache miss [low read] turn 7 call 1: provisional",
    ));
    let count = app.display_messages.len();
    let corrected = "KV cache refresh [provider_daily_rollover]: ~590K tokens resent";
    assert!(handle_kv_cache_miss(
        &mut app,
        "daily_rollover",
        590_713,
        corrected.into(),
        true
    ));
    assert_eq!(app.display_messages.len(), count);
    assert_eq!(app.display_messages[count - 1].content, corrected);
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason,
        KvCacheMissReason::DailyRollover
    );
}

#[test]
fn remote_account_switch_corrects_provisional_zero_read_in_place() {
    let mut app = create_test_app();
    app.kv_cache.kv_cache_turn_number = Some(2);
    app.kv_cache.kv_cache_turn_call_index = 2;
    app.kv_cache.kv_cache_miss_samples.push(KvCacheMissSample {
        turn_number: 2,
        call_index: 2,
        missed_tokens: 33_096,
        reason: KvCacheMissReason::ZeroRead,
    });
    app.push_display_message(DisplayMessage::system(
        "ℹ️ KV cache miss [zero read] turn 2 call 2: 0 cached / 33.1K expected tokens",
    ));
    let count = app.display_messages.len();
    let corrected = "KV cache miss [account_switch]: ~33.1K tokens resent (account switched claude-fox -> claude-otter (e.g. failover after a rate limit); prompt caches are per account).";
    assert!(handle_kv_cache_miss(
        &mut app,
        "account_switch",
        33_096,
        corrected.into(),
        true
    ));
    assert_eq!(app.display_messages.len(), count, "replaced, not appended");
    assert_eq!(app.display_messages[count - 1].content, corrected);
    assert_eq!(
        app.kv_cache.kv_cache_miss_samples[0].reason,
        KvCacheMissReason::AccountSwitch
    );
}
