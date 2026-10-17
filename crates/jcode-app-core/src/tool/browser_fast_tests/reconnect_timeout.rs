use super::*;

#[tokio::test(start_paused = true)]
async fn observation_timeout_retains_last_connection_error_without_dispatch() {
    for (timeout_ms, expected_ms) in [(100, 100), (60_000, 8_000)] {
        let browser = MockBrowser::new(vec![
            json!({"bridge_error":"message port closed: first connection"}),
            json!({"bridge_error":"Could not establish connection. Receiving end does not exist."}),
        ]);
        let transport = MockTransport::new(&[]);
        let mut input = input();
        input.timeout_ms = Some(timeout_ms);
        let start = tokio::time::Instant::now();
        let value = result(&browser, &transport, &input).await;
        assert_eq!(start.elapsed(), Duration::from_millis(expected_ms));
        assert_eq!(value["status"], "hand_back");
        assert!(value["action_trace"].as_array().unwrap().is_empty());
        let reason = value["reason"].as_str().unwrap();
        assert!(reason.contains("last connection error"), "{value}");
        assert!(
            reason.contains(if timeout_ms == 100 {
                "first connection"
            } else {
                "Receiving end does not exist"
            }),
            "{value}"
        );
        assert!(!reason.contains("in-flight"), "{value}");
        assert!(transport.requests.lock().unwrap().is_empty());
        assert!(
            browser
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| matches!(call.as_str(), "status" | "eval"))
        );
    }
}

#[tokio::test(start_paused = true)]
async fn pending_reconnect_retains_prior_connection_error() {
    let browser = MockBrowser::new(vec![
        json!({"bridge_error":"Receiving end does not exist"}),
        json!({"bridge_pending":true}),
    ]);
    let transport = MockTransport::new(&[]);
    let mut input = input();
    input.timeout_ms = Some(1_000);
    let start = tokio::time::Instant::now();
    let value = result(&browser, &transport, &input).await;
    assert_eq!(start.elapsed(), Duration::from_secs(1));
    let reason = value["reason"].as_str().unwrap();
    assert!(reason.contains("Receiving end does not exist"), "{value}");
    assert!(reason.contains("timed out"), "{value}");
    assert!(!reason.contains("in-flight"), "{value}");
    assert_eq!(value["status"], "hand_back");
    assert!(value["action_trace"].as_array().unwrap().is_empty());
    assert_eq!(
        *browser.calls.lock().unwrap(),
        vec!["status", "eval", "eval"]
    );
    assert!(transport.requests.lock().unwrap().is_empty());
}
