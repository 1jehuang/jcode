use super::*;
use crate::protocol::{ServerEvent, SessionToolConfig, SessionToolDefinition};
use serde_json::json;
use tokio::sync::mpsc;

fn custom(name: &str) -> SessionToolDefinition {
    SessionToolDefinition {
        name: name.into(),
        description: "SDK callback".into(),
        parameters: json!({"type":"object"}),
    }
}
fn ctx(session: &str) -> ToolContext {
    ToolContext {
        session_id: session.into(),
        message_id: "sdk-test".into(),
        tool_call_id: "sdk-parent".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}
struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        sdk::remove_session(&self.0);
        clear_session_tool_policy(&self.0);
    }
}

#[tokio::test]
async fn sdk_selection_replacement_validation_and_isolation() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::new(home.path());
    let session = "sdk-selection";
    let _cleanup = Cleanup(session.into());
    let (tx, _rx) = mpsc::unbounded_channel();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = crate::agent::Agent::new(provider, registry.clone());
    let own_session = agent.session_id().to_owned();
    let _own_cleanup = Cleanup(own_session.clone());
    let config = SessionToolConfig {
        enabled: Some(vec![]),
        disabled: vec![],
        custom: vec![custom("read"), custom("local")],
    };
    sdk::configure(&own_session, "selection-owner", config.clone(), tx.clone()).unwrap();
    let definitions = agent.tool_definitions_for_debug().await;
    assert_eq!(
        definitions
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["local", "read"]
    );
    assert_eq!(definitions[1].description, "SDK callback");
    sdk::configure(session, "owner", config, tx.clone()).unwrap();
    assert!(
        registry
            .execute("ls", json!({}), ctx(session))
            .await
            .unwrap_err()
            .to_string()
            .contains("not allowed")
    );
    assert!(sdk::config("unrelated-session").is_none());
    assert!(
        sdk::configure(
            session,
            "stranger",
            SessionToolConfig::default(),
            tx.clone()
        )
        .is_err()
    );
    let bad = SessionToolConfig {
        custom: vec![SessionToolDefinition {
            parameters: json!(true),
            ..custom("bad")
        }],
        ..Default::default()
    };
    assert!(sdk::configure(session, "owner", bad, tx.clone()).is_err());
    assert_eq!(sdk::config(session).unwrap().custom.len(), 2);
    sdk::configure(session, "owner", SessionToolConfig::default(), tx).unwrap();
    assert!(sdk::config(session).unwrap().enabled.is_none());
    assert!(sdk::config(session).unwrap().custom.is_empty());
}

#[tokio::test]
async fn sdk_callbacks_owner_scoped_override_errors_and_disconnect() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::new(home.path());
    let session = "sdk-callback";
    let _cleanup = Cleanup(session.into());
    let (tx, mut rx) = mpsc::unbounded_channel();
    sdk::configure(
        session,
        "callback-owner",
        SessionToolConfig {
            enabled: Some(vec![]),
            custom: vec![custom("read")],
            ..Default::default()
        },
        tx,
    )
    .unwrap();
    let registry = Registry::new(Arc::new(MockProvider)).await;
    for error in [None, Some("callback failed".to_string())] {
        let run_registry = registry.clone();
        let task = tokio::spawn(async move {
            run_registry
                .execute("read", json!({"file_path":"never read this"}), ctx(session))
                .await
        });
        let ServerEvent::ToolCall {
            call_id,
            name,
            input,
            session_id,
        } = rx.recv().await.unwrap()
        else {
            panic!("expected callback")
        };
        assert_eq!(session_id, session);
        assert_eq!(name, "read");
        assert_eq!(input["file_path"], "never read this");
        assert!(sdk::complete("other", session, &call_id, "forged".into(), None).is_err());
        assert!(
            sdk::complete(
                "callback-owner",
                "other-session",
                &call_id,
                "forged".into(),
                None
            )
            .is_err()
        );
        sdk::complete(
            "callback-owner",
            session,
            &call_id,
            "callback output".into(),
            error.clone(),
        )
        .unwrap();
        let result = task.await.unwrap();
        if error.is_some() {
            assert!(result.unwrap_err().to_string().contains("callback failed"));
        } else {
            assert_eq!(result.unwrap().output, "callback output");
        }
        assert!(
            sdk::complete(
                "callback-owner",
                session,
                &call_id,
                "duplicate".into(),
                None
            )
            .is_err()
        );
    }
    let run_registry = registry.clone();
    let task =
        tokio::spawn(async move { run_registry.execute("read", json!({}), ctx(session)).await });
    rx.recv().await.unwrap();
    drop(sdk::ConnectionGuard("callback-owner".into()));
    assert!(
        task.await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("disconnected")
    );
    assert!(
        registry
            .execute("read", json!({}), ctx(session))
            .await
            .unwrap_err()
            .to_string()
            .contains("disconnected")
    );
}

#[tokio::test]
async fn sdk_disabled_batch_subcalls_and_cancel_cleanup() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::new(home.path());
    let session = "sdk-batch";
    let _cleanup = Cleanup(session.into());
    let (tx, mut rx) = mpsc::unbounded_channel();
    sdk::configure(
        session,
        "batch-owner",
        SessionToolConfig {
            enabled: Some(vec!["batch".into()]),
            disabled: vec!["read".into()],
            custom: vec![custom("read"), custom("callback")],
        },
        tx.clone(),
    )
    .unwrap();
    let registry = Registry::new(Arc::new(MockProvider)).await;
    let output = registry
        .execute(
            "batch",
            json!({"tool_calls":[{"tool":"read","intent":"denied","file_path":"no"}]}),
            ctx(session),
        )
        .await
        .unwrap();
    assert!(output.output.contains("disabled"), "{}", output.output);
    assert!(rx.try_recv().is_err());
    let task =
        tokio::spawn(async move { registry.execute("callback", json!({}), ctx(session)).await });
    let ServerEvent::ToolCall { call_id, .. } = rx.recv().await.unwrap() else {
        panic!("expected callback")
    };
    assert!(
        sdk::configure(
            session,
            "batch-owner",
            SessionToolConfig::default(),
            tx.clone()
        )
        .is_err()
    );
    task.abort();
    let _ = task.await;
    assert!(sdk::complete("batch-owner", session, &call_id, "late".into(), None).is_err());
    sdk::configure(session, "batch-owner", SessionToolConfig::default(), tx).unwrap();
}

#[test]
fn sdk_inheritance_and_deferred_mcp_policy() {
    let session = "sdk-mcp-policy";
    let _cleanup = Cleanup(session.into());
    let (tx, _rx) = mpsc::unbounded_channel();
    set_session_tool_policy(
        session,
        Some(HashSet::from(["read".into()])),
        HashSet::from(["read".into()]),
    );
    sdk::configure(
        session,
        "mcp-owner",
        SessionToolConfig::default(),
        tx.clone(),
    )
    .unwrap();
    assert_eq!(
        session_tool_policy_allows_tool_for_test(session, "read"),
        Some(false)
    );
    assert_eq!(
        session_tool_policy_allows_tool_for_test(session, "ls"),
        Some(false)
    );
    sdk::configure(
        session,
        "mcp-owner",
        SessionToolConfig {
            enabled: Some(vec!["mcp_call".into()]),
            disabled: vec!["mcp__test__blocked".into()],
            custom: vec![],
        },
        tx.clone(),
    )
    .unwrap();
    assert!(!session_mcp_dispatch_is_allowed(
        session,
        "mcp__test__blocked",
        "mcp_call"
    ));
    assert!(session_mcp_dispatch_is_allowed(
        session,
        "mcp__test__allowed",
        "mcp_call"
    ));
    sdk::configure(
        session,
        "mcp-owner",
        SessionToolConfig {
            enabled: Some(vec![]),
            ..Default::default()
        },
        tx,
    )
    .unwrap();
    assert!(!session_mcp_dispatch_is_allowed(
        session,
        "mcp__test__allowed",
        "mcp_call"
    ));
}

#[tokio::test]
async fn sdk_nested_alias_and_deferred_mcp_use_callbacks() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::new(home.path());
    let session = "sdk-nested-overrides";
    let _cleanup = Cleanup(session.into());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let registry = Registry::new(Arc::new(MockProvider)).await;
    let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    registry
        .register(
            "mcp_call".into(),
            Arc::new(mcp::McpCallTool::new(manager).with_registry(registry.clone())),
        )
        .await;
    let mut config = SessionToolConfig {
        enabled: Some(vec!["batch".into(), "mcp_call".into()]),
        disabled: vec![],
        custom: vec![custom("shell_exec"), custom("mcp__sdk__echo")],
    };
    sdk::configure(session, "nested-owner", config.clone(), tx.clone()).unwrap();
    for (surface, input, expected) in [
        (
            "batch",
            json!({"tool_calls":[{"tool":"shell_exec","intent":"callback", "command":"printf wrong"}]}),
            "shell_exec",
        ),
        (
            "batch",
            json!({"tool_calls":[{"tool":"functions.shell_exec","intent":"callback", "command":"printf wrong"}]}),
            "shell_exec",
        ),
        (
            "mcp_call",
            json!({"server":"sdk","tool":"echo","arguments":{"value":42}}),
            "mcp__sdk__echo",
        ),
    ] {
        let run_registry = registry.clone();
        let task =
            tokio::spawn(async move { run_registry.execute(surface, input, ctx(session)).await });
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("nested path must dispatch callback")
            .unwrap();
        let ServerEvent::ToolCall {
            call_id,
            name,
            session_id,
            ..
        } = event
        else {
            panic!("expected callback")
        };
        assert_eq!(session_id, session);
        assert_eq!(name, expected);
        sdk::complete(
            "nested-owner",
            session,
            &call_id,
            "nested SDK result".into(),
            None,
        )
        .unwrap();
        assert!(
            task.await
                .unwrap()
                .unwrap()
                .output
                .contains("nested SDK result")
        );
    }
    config.disabled.push("mcp__sdk__echo".into());
    sdk::configure(session, "nested-owner", config, tx).unwrap();
    let error = registry
        .execute(
            "mcp_call",
            json!({"server":"sdk","tool":"echo","arguments":{}}),
            ctx(session),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not allowed"));
    assert!(rx.try_recv().is_err());
}
