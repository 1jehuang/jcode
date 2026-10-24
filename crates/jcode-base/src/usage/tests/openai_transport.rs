#[test]
fn test_openai_transport_error_retries_sooner_than_api_errors() {
    let at = Instant::now() - Duration::from_secs(60);
    let transport = OpenAIUsageData {
        fetched_at: Some(at),
        last_error: Some(format!("{TRANSPORT_ERROR_PREFIX}error sending request")),
        ..Default::default()
    };
    assert!(transport.is_transport_error());
    assert!(transport.is_stale());

    let api = OpenAIUsageData {
        fetched_at: Some(at),
        last_error: Some("API error (500): oops".into()),
        ..Default::default()
    };
    assert!(!api.is_transport_error());
    assert!(!api.is_stale());
}

#[test]
fn test_openai_transport_failure_keeps_recent_last_good_only() {
    let key = "label:transport-last-good-test".to_string();
    let window = OpenAIUsageWindow {
        name: "5-hour window".into(),
        usage_ratio: 0.2,
        resets_at: None,
    };
    store_openai_usage(
        key.clone(),
        OpenAIUsageData {
            five_hour: Some(window.clone()),
            fetched_at: Some(Instant::now()),
            ..Default::default()
        },
    );
    assert!(recent_good_openai_usage(&key).is_some());

    store_openai_usage(
        key.clone(),
        OpenAIUsageData {
            five_hour: Some(window),
            fetched_at: Some(Instant::now() - TRANSPORT_LAST_GOOD_MAX_AGE - Duration::from_secs(1)),
            ..Default::default()
        },
    );
    assert!(recent_good_openai_usage(&key).is_none());

    store_openai_usage(
        key.clone(),
        OpenAIUsageData {
            fetched_at: Some(Instant::now()),
            last_error: Some("API error (500)".into()),
            ..Default::default()
        },
    );
    assert!(recent_good_openai_usage(&key).is_none());
}
