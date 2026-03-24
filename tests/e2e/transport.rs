use super::*;

#[tokio::test]
async fn test_websocket_transport_matches_unix_socket_for_subscribe_history_message_and_resume()
-> Result<()> {
    let _env = setup_test_env()?;
    let unix = run_unix_transport_scenario().await?;
    let websocket = run_websocket_transport_scenario().await?;

    assert!(
        unix.subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Ack { id } if *id == 1))
    );
    assert!(
        unix.subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 1))
    );
    assert!(
        websocket
            .subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Ack { id } if *id == 1))
    );
    assert!(
        websocket
            .subscribe_events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 1))
    );

    let unix_history = unix
        .history_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing unix history event"))?;
    let websocket_history = websocket
        .history_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing websocket history event"))?;
    assert_eq!(
        unix_history, websocket_history,
        "history payload should match across transports"
    );

    assert_eq!(
        summarize_message_invariant(&unix.message_events),
        summarize_message_invariant(&websocket.message_events),
        "message streaming events should match across transports after removing broadcast noise"
    );

    let unix_resume = unix
        .resume_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing unix resume history event"))?;
    let websocket_resume = websocket
        .resume_events
        .iter()
        .find_map(summarize_history_invariant)
        .ok_or_else(|| anyhow::anyhow!("missing websocket resume history event"))?;
    assert_eq!(
        unix_resume, websocket_resume,
        "resume history payload should match across transports"
    );

    Ok(())
}
