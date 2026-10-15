//! Exercises the real split handler and persisted child selection.
use super::super::set_global_agent_model;
use super::*;

fn server_oauth_fixture() {
    std::fs::write(crate::storage::jcode_dir().unwrap().join("openai-auth.json"), serde_json::json!({
        "openai_accounts": [{"label": "feedback-test", "access_token": "test", "refresh_token": "test-refresh", "account_id": "test"}],
        "active_openai_account": "feedback-test"
    }).to_string()).unwrap();
}

#[tokio::test]
async fn feedback_split_one_shot_uses_server_oauth_preference_when_unconfigured() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    server_oauth_fixture();
    let agent = new_split_test_agent().await;
    let parent = agent.lock().await.session_for_split().clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_split(46, &parent.id, &agent, &tx, Some("review")).await;
    let child = split_response(&mut rx, 46);
    assert_eq!(
        (child.model.as_deref(), child.provider_key.as_deref()),
        (Some("gpt-5.5"), Some("openai"))
    );
}

#[tokio::test]
async fn feedback_split_automatic_worker_does_not_use_one_shot_preference() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    server_oauth_fixture();
    let agent = new_split_test_agent().await;
    let mut parent = agent.lock().await.session_for_split().clone();
    parent.model = Some("parent-model".to_string());
    // An empty parent has no visible conversation yet, so ordinary save() is
    // deliberately a no-op. Persist the explicitly prepared model/role state.
    parent.save_prepared().unwrap();
    assert_eq!(
        crate::session::Session::load(&parent.id).unwrap().model,
        parent.model,
        "the handler must start from the prepared persisted fixture"
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_split(47, &parent.id, &agent, &tx, Some("autojudge")).await;
    assert_eq!(
        split_response(&mut rx, 47).model.as_deref(),
        Some("parent-model")
    );
}

#[tokio::test]
async fn feedback_split_uses_acknowledged_server_defaults_for_each_role() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let server_home = std::env::var_os("JCODE_HOME").unwrap();
    let client_home = tempfile::tempdir().unwrap();
    std::fs::write(
        client_home.path().join("config.toml"),
        "[autoreview]\nmodel = 'client-review'\n[autojudge]\nmodel = 'client-judge'\n",
    )
    .unwrap();
    crate::env::set_var("JCODE_HOME", client_home.path());
    let client_config = crate::config::Config::load_strict().unwrap();
    crate::env::set_var("JCODE_HOME", &server_home);
    let agent = new_split_test_agent().await;
    let parent = agent.lock().await.session_for_split().clone();
    // Prime the process cache before saving, so the handler must use a fresh read.
    let _ = crate::config::config();
    for (role, target, expected) in [
        ("review", "review", "server-review"),
        ("autoreview", "review", "server-review"),
        ("judge", "judge", "server-judge"),
        ("autojudge", "judge", "server-judge"),
    ] {
        let acknowledged =
            set_global_agent_model(&agent, &parent.id, target, Some(expected.to_string()))
                .await
                .unwrap();
        assert!(
            acknowledged.is_empty(),
            "a global save does not become a session pin"
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_split(41, &parent.id, &agent, &tx, Some(role)).await;
        assert_eq!(split_response(&mut rx, 41).model.as_deref(), Some(expected));
    }
    assert_eq!(
        client_config.autoreview.model.as_deref(),
        Some("client-review")
    );
    assert_eq!(
        client_config.autojudge.model.as_deref(),
        Some("client-judge")
    );
}

#[tokio::test]
async fn feedback_split_session_pin_wins_over_server_default_while_parent_busy() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let agent = new_split_test_agent().await;
    let mut parent = agent.lock().await.session_for_split().clone();
    parent
        .set_agent_model_override("judge", Some("session-judge".to_string()))
        .unwrap();
    // An empty parent has no visible conversation yet, so ordinary save() is
    // deliberately a no-op. Persist the explicitly prepared model/role state.
    parent.save_prepared().unwrap();
    assert_eq!(
        crate::session::Session::load(&parent.id).unwrap().model,
        parent.model,
        "the handler must start from the prepared persisted fixture"
    );
    crate::config::Config::set_agent_model_default("judge", Some("server-judge")).unwrap();
    let _busy = agent.lock().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    timeout(
        Duration::from_secs(1),
        handle_split(42, &parent.id, &agent, &tx, Some("judge")),
    )
    .await
    .unwrap();
    assert_eq!(
        split_response(&mut rx, 42).model.as_deref(),
        Some("session-judge")
    );
}

#[tokio::test]
async fn feedback_split_explicit_inherit_keeps_parent_model() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    server_oauth_fixture();
    let agent = new_split_test_agent().await;
    let mut parent = agent.lock().await.session_for_split().clone();
    parent.model = Some("parent-model".to_string());
    parent
        .set_agent_model_override("review", Some("inherit".to_string()))
        .unwrap();
    // An empty parent has no visible conversation yet, so ordinary save() is
    // deliberately a no-op. Persist the explicitly prepared model/role state.
    parent.save_prepared().unwrap();
    assert_eq!(
        crate::session::Session::load(&parent.id).unwrap().model,
        parent.model,
        "the handler must start from the prepared persisted fixture"
    );
    crate::config::Config::set_agent_model_default("review", Some("server-review")).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_split(43, &parent.id, &agent, &tx, Some("review")).await;
    assert_eq!(
        split_response(&mut rx, 43).model.as_deref(),
        Some("parent-model")
    );
}

#[tokio::test]
async fn feedback_split_global_inherit_keeps_parent_model() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    server_oauth_fixture();
    let agent = new_split_test_agent().await;
    let mut parent = agent.lock().await.session_for_split().clone();
    parent.model = Some("parent-model".to_string());
    // An empty parent has no visible conversation yet, so ordinary save() is
    // deliberately a no-op. Persist the explicitly prepared model/role state.
    parent.save_prepared().unwrap();
    assert_eq!(
        crate::session::Session::load(&parent.id).unwrap().model,
        parent.model,
        "the handler must start from the prepared persisted fixture"
    );
    crate::config::Config::set_agent_model_default("judge", Some("inherit")).unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_split(44, &parent.id, &agent, &tx, Some("judge")).await;
    assert_eq!(
        split_response(&mut rx, 44).model.as_deref(),
        Some("parent-model")
    );
}

#[tokio::test]
async fn feedback_split_rejects_unknown_role_without_publishing_child() {
    let _guard = crate::storage::lock_test_env();
    let _home = SplitTestHome::new();
    let agent = new_split_test_agent().await;
    let parent = agent.lock().await.session_for_split().clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_split(45, &parent.id, &agent, &tx, Some("memory")).await;
    assert!(matches!(
        rx.try_recv().unwrap(),
        ServerEvent::Error { id: 45, .. }
    ));
}
