use super::*;

#[test]
fn permanent_telemetry_statuses_trip_the_process_breaker() {
    assert!(telemetry_status_is_permanent(400));
    assert!(telemetry_status_is_permanent(401));
    assert!(telemetry_status_is_permanent(404));
    assert!(!telemetry_status_is_permanent(408));
    assert!(!telemetry_status_is_permanent(425));
    assert!(!telemetry_status_is_permanent(429));
    assert!(!telemetry_status_is_permanent(500));
}

#[test]
fn background_delivery_queue_is_bounded() {
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let sender = spawn_background_worker(1, move |_| {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
    })
    .expect("start test telemetry worker");

    sender
        .send(serde_json::json!({"event": "first"}))
        .expect("enqueue first payload");
    started_rx.recv().expect("worker started first payload");
    sender
        .try_send(serde_json::json!({"event": "second"}))
        .expect("bounded queue accepts one waiting payload");
    assert!(matches!(
        sender.try_send(serde_json::json!({"event": "third"})),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));

    release_tx.send(()).expect("release telemetry worker");
}

#[test]
fn telemetry_endpoint_uses_production_custom_domain() {
    assert_eq!(TELEMETRY_ENDPOINT, "https://telemetry.jcode.sh/v1/event");
    assert_eq!(
        TRANSCRIPT_ENDPOINT,
        "https://telemetry.jcode.sh/v1/transcript"
    );
}

#[test]
fn transcript_upload_requires_separate_content_consent() {
    let _guard = lock_test_env();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();

    assert!(!record_transcript(
        "provider",
        "model",
        SessionEndReason::NormalExit,
        serde_json::json!([{"role": "user", "content": "secret prompt"}]),
    ));
    assert!(TEST_EMITTED_PAYLOADS.lock().unwrap().is_empty());
}

#[test]
fn opted_in_transcript_payload_contains_full_structured_messages() {
    let _guard = lock_test_env();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();
    assert!(set_content_sharing_enabled(true));
    let messages = serde_json::json!([
        {"role": "user", "content": [{"type": "text", "text": "secret prompt"}]},
        {"role": "assistant", "content": [{"type": "reasoning", "text": "private reasoning"}]}
    ]);

    assert!(record_transcript(
        "provider",
        "model",
        SessionEndReason::NormalExit,
        messages.clone(),
    ));
    let payloads = TEST_EMITTED_PAYLOADS.lock().unwrap();
    let payload = payloads.last().expect("transcript payload");
    assert_eq!(payload["event"], "transcript");
    assert_eq!(payload["consent_version"], 1);
    assert_eq!(payload["message_count"], 2);
    assert_eq!(payload["messages"], messages);
    assert!(uuid::Uuid::parse_str(payload["upload_id"].as_str().unwrap()).is_ok());
}

#[test]
fn test_opt_out_env_var() {
    let _guard = lock_test_env();
    jcode_core::env::set_var("JCODE_NO_TELEMETRY", "1");
    assert!(!is_enabled());
    jcode_core::env::remove_var("JCODE_NO_TELEMETRY");
}

#[test]
fn test_do_not_track() {
    let _guard = lock_test_env();
    jcode_core::env::set_var("DO_NOT_TRACK", "1");
    assert!(!is_enabled());
    jcode_core::env::remove_var("DO_NOT_TRACK");
}

#[test]
fn false_opt_out_values_keep_telemetry_enabled() {
    let _guard = lock_test_env();
    for value in ["", "0", "false", "no", "off"] {
        jcode_core::env::set_var("JCODE_NO_TELEMETRY", value);
        jcode_core::env::set_var("DO_NOT_TRACK", value);
        assert!(is_enabled(), "{value:?} should not opt out");
    }
}

#[test]
fn in_app_opt_out_emits_once_then_stops_telemetry() {
    let _guard = lock_test_env();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();

    assert!(set_usage_telemetry_enabled(false));
    assert!(!is_enabled());
    assert!(set_usage_telemetry_enabled(false));

    let payloads = TEST_EMITTED_PAYLOADS.lock().unwrap();
    let opt_outs = payloads
        .iter()
        .filter(|payload| payload["event"] == "telemetry_opt_out")
        .collect::<Vec<_>>();
    assert_eq!(opt_outs.len(), 1);
    assert_eq!(opt_outs[0]["step"], "telemetry_settings");
}
