use super::*;

fn session_id(events: &[StreamEvent]) -> &str {
    events
        .iter()
        .find_map(|event| match event {
            StreamEvent::SessionId(id) => Some(id.as_str()),
            _ => None,
        })
        .expect("provider reports its native session ID")
}

#[tokio::test]
async fn profile_policy_reset_starts_new_native_session_with_changed_instructions_and_history() {
    let fake = Fake::new();
    let provider = fake.provider();
    let first = run_turn_with_system(
        &provider,
        "OLD_PROFILE_AND_FILES_58231",
        &[Message::user("system")],
        None,
    )
    .await;
    assert!(text(&first).contains("OLD_PROFILE_AND_FILES_58231"));
    let old_session = session_id(&first).to_owned();
    let model = provider.model();
    let pin = provider.account_pin(AccountProviderKind::ClaudeCode);
    // This is the same explicit hook the Agent's successful profile mutation
    // schedules before either normal or streaming provider request boundary.
    provider.reset_conversation_state().await.unwrap();
    let history = [
        Message::user("retained profile task"),
        Message::assistant_text("retained task response"),
        Message::user("echo-prompt"),
    ];
    // The shared helper's last argument answers tool calls, not resume hints.
    // Exercise the public Provider API directly so the stale hint remains real.
    let mut stream = provider
        .complete(
            &history,
            &tools(),
            "NEW_PROFILE_ONLY_58231",
            Some(&old_session),
        )
        .await
        .unwrap();
    let mut next = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while let Some(item) = tokio::time::timeout_at(deadline, stream.next())
        .await
        .expect("reset native turn finishes before its deadline")
    {
        let event = item.unwrap();
        let done = matches!(event, StreamEvent::MessageEnd { .. });
        next.push(event);
        if done {
            break;
        }
    }
    assert!(matches!(next.last(), Some(StreamEvent::MessageEnd { .. })));
    drop(stream);
    let sent = text(&next);
    assert!(sent.contains("previous_conversation"), "{sent}");
    assert!(sent.contains("retained profile task"), "{sent}");
    assert!(sent.contains("retained task response"), "{sent}");
    assert_ne!(session_id(&next), old_session);
    let launches = fake.launches();
    assert_eq!(launches.len(), 2);
    let argv = launches[1]["argv"].as_array().unwrap();
    assert!(
        argv.iter()
            .any(|arg| arg.as_str().unwrap().starts_with("--session-id="))
    );
    assert!(
        !argv
            .iter()
            .any(|arg| arg.as_str().unwrap().starts_with("--resume"))
    );
    let reply = run_turn_with_system(
        &provider,
        "NEW_PROFILE_ONLY_58231",
        &[Message::user("system")],
        None,
    )
    .await;
    assert!(text(&reply).contains("NEW_PROFILE_ONLY_58231"));
    assert!(!text(&reply).contains("OLD_PROFILE_AND_FILES_58231"));
    assert_eq!(
        fake.launches().len(),
        2,
        "same new profile reuses its new child"
    );
    assert_eq!(provider.model(), model);
    assert_eq!(provider.account_pin(AccountProviderKind::ClaudeCode), pin);
    provider.shutdown().await;
}
