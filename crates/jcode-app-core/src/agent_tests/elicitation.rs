use super::*;
fn attach_form_client(
    session: &str,
    connection: &str,
) -> tokio::sync::mpsc::UnboundedReceiver<crate::protocol::ServerEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    crate::elicitation::global().attach(connection, session, true, false, false, tx);
    rx
}
#[tokio::test]
async fn ask_user_registers_for_a_form_capable_client_and_survives_detach() {
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let session = agent.session_id().to_string();
    assert!(agent.locked_tools.is_none());
    let _ = agent.tool_definitions().await;
    assert!(agent.locked_tools.is_some());

    // Headless: no form-capable attachment, so the tool stays unregistered.
    agent.register_ask_user().await;
    assert!(
        !agent
            .tool_names()
            .await
            .iter()
            .any(|name| name == "ask_user")
    );

    let _rx = attach_form_client(&session, "ask-user-capability-connection");
    agent.register_ask_user().await;
    assert!(
        agent.locked_tools.is_none(),
        "first registration must unlock the frozen surface"
    );
    assert!(
        agent
            .tool_names()
            .await
            .iter()
            .any(|name| name == "ask_user")
    );

    // Already registered: a later capability change must not unlock again.
    let _ = agent.tool_definitions().await;
    assert!(agent.locked_tools.is_some());
    agent.register_ask_user().await;
    assert!(agent.locked_tools.is_some());

    // Detach keeps the registry entry: no unregister, no cache churn.
    crate::elicitation::global().detach("ask-user-capability-connection");
    agent.register_ask_user().await;
    assert!(
        agent
            .tool_names()
            .await
            .iter()
            .any(|name| name == "ask_user")
    );
    agent.register_ask_user().await;
    assert!(
        agent
            .tool_names()
            .await
            .iter()
            .any(|name| name == "ask_user")
    );
}

#[tokio::test]
async fn ask_user_is_offered_on_the_next_request_after_a_late_attach() {
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let session = agent.session_id().to_string();
    let first = agent.tool_definitions().await;
    assert!(!first.iter().any(|tool| tool.name == "ask_user"));
    assert!(agent.locked_tools.is_some());

    let _rx = attach_form_client(&session, "ask-user-late-connection");
    agent.register_ask_user().await;
    let next = agent.tool_definitions().await;
    assert!(next.iter().any(|tool| tool.name == "ask_user"));
}

#[tokio::test]
async fn ask_user_is_not_forced_into_an_explicit_allowlist() {
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    agent.allowed_tools = Some(["bash".to_string()].into_iter().collect());
    let session = agent.session_id().to_string();
    let _rx = attach_form_client(&session, "ask-user-allowlist-connection");
    agent.register_ask_user().await;
    assert!(
        agent
            .registry
            .tool_names()
            .await
            .iter()
            .any(|name| name == "ask_user"),
        "the tool is registered"
    );
    assert!(
        !agent
            .tool_definitions()
            .await
            .iter()
            .any(|tool| tool.name == "ask_user"),
        "an explicit allowlist is respected"
    );
}

#[tokio::test]
async fn hibernation_wake_keeps_registry_and_offered_tools_in_agreement() {
    let _env = crate::storage::lock_test_env();
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    let session = agent.session_id().to_string();
    let _rx = attach_form_client(&session, "ask-user-hibernation-connection");
    agent.register_ask_user().await;
    let frozen = agent.tool_definitions().await;
    assert!(frozen.iter().any(|tool| tool.name == "ask_user"));

    crate::elicitation::global().detach("ask-user-hibernation-connection");
    let _ = agent.hibernate_idle_caches();
    let registered = agent
        .registry
        .tool_names()
        .await
        .iter()
        .any(|name| name == "ask_user");
    let offered = agent
        .tool_definitions()
        .await
        .iter()
        .any(|tool| tool.name == "ask_user");
    assert_eq!(
        offered, registered,
        "the frozen surface and the registry must agree after an idle wake"
    );
    assert!(offered);
}
