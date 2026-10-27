use super::*;

async fn collect_with_hint(
    provider: &ClaudeCodeProvider,
    messages: &[Message],
    resume_hint: Option<&str>,
) -> Vec<StreamEvent> {
    let mut stream = provider
        .complete(messages, &tools(), SYSTEM, resume_hint)
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let item = tokio::time::timeout(Duration::from_secs(20), stream.next())
            .await
            .expect("fake CLI turn timed out")
            .expect("fake CLI ended without MessageEnd")
            .unwrap();
        let done = matches!(item, StreamEvent::MessageEnd { .. });
        events.push(item);
        if done {
            return events;
        }
    }
}

fn session_id(events: &[StreamEvent]) -> &str {
    events
        .iter()
        .find_map(|event| match event {
            StreamEvent::SessionId(id) => Some(id.as_str()),
            _ => None,
        })
        .expect("provider must report its child session")
}

#[tokio::test]
async fn rewind_reset_starts_fresh_and_seeds_only_retained_history() {
    let fake = Fake::new();
    let provider = fake.provider();
    let old_history = [
        Message::user("retained question"),
        Message::assistant_text("retained answer"),
        Message::user("REMOVED_FUTURE_SECRET"),
        Message::assistant_text("REMOVED_FUTURE_RESPONSE"),
        Message::user("echo-prompt"),
    ];
    let first = collect_with_hint(&provider, &old_history, None).await;
    assert!(text(&first).contains("REMOVED_FUTURE_SECRET"));
    let old_session = session_id(&first).to_string();
    let model = provider.model();
    let pin = provider.account_pin(AccountProviderKind::ClaudeCode);
    provider.reset_conversation_state().await.unwrap();
    provider.reset_conversation_state().await.unwrap();

    let retained = [
        Message::user("retained question"),
        Message::assistant_text("retained answer"),
        Message::user("echo-prompt"),
    ];
    // An accidentally retained Agent resume hint must not resurrect the old
    // on-disk Claude session after the runtime has been invalidated.
    let next = collect_with_hint(&provider, &retained, Some(&old_session)).await;
    let sent = text(&next);
    assert!(sent.contains("previous_conversation"), "{sent}");
    assert!(sent.contains("retained question"), "{sent}");
    assert!(sent.contains("retained answer"), "{sent}");
    assert!(!sent.contains("REMOVED_FUTURE"), "{sent}");
    assert_ne!(session_id(&next), old_session);
    let launches = fake.launches();
    assert_eq!(launches.len(), 2, "reset must retire the live child");
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
    assert_eq!(provider.model(), model);
    assert_eq!(provider.account_pin(AccountProviderKind::ClaudeCode), pin);

    let followup = run_turn(&provider, &[Message::user("echo-prompt")], None).await;
    assert!(!text(&followup).contains("previous_conversation"));
    assert_eq!(fake.launches().len(), 2, "new history must be seeded once");
    provider.shutdown().await;
}

#[tokio::test]
async fn rewind_reset_also_forgets_a_stopped_childs_resume_cursor() {
    let fake = Fake::new();
    let provider = fake.provider();
    let first = run_turn(&provider, &[Message::user("session")], None).await;
    let old_session = session_id(&first).to_string();
    provider.shutdown().await;
    provider.reset_conversation_state().await.unwrap();
    let next = collect_with_hint(&provider, &[Message::user("session")], Some(&old_session)).await;
    assert!(text(&next).starts_with("session:new:"), "{next:?}");
    assert_ne!(session_id(&next), old_session);
    assert_eq!(fake.launches().len(), 2);
    provider.shutdown().await;
}
