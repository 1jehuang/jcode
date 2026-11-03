use super::*;

#[tokio::test]
async fn invalidate_openai_usage_acknowledges_after_clearing_pinned_daemon_cooldown() {
    let _guard = crate::storage::lock_test_env();
    let target = "daemon-reset-target";
    let other = "daemon-reset-other";
    crate::auth::codex::set_active_account_override(Some(target.to_string()));
    crate::provider::record_provider_unavailable_for_account("openai", "target quota exhausted");
    crate::auth::codex::set_active_account_override(Some(other.to_string()));
    crate::provider::record_provider_unavailable_for_account("openai", "other quota exhausted");

    let (tx, mut rx) = mpsc::unbounded_channel();
    for id in [51, 52] {
        handle_invalidate_openai_usage(id, Some(target.to_string()), &tx).await;
        assert!(matches!(rx.try_recv(), Ok(ServerEvent::Done { id: ack }) if ack == id));
        assert!(rx.try_recv().is_err());
        assert_eq!(
            crate::auth::codex::active_account_label().as_deref(),
            Some(other)
        );
        assert!(crate::provider::provider_unavailability_detail_for_account("openai").is_some());
        crate::auth::codex::set_active_account_override(Some(target.to_string()));
        assert!(crate::provider::provider_unavailability_detail_for_account("openai").is_none());
        crate::auth::codex::set_active_account_override(Some(other.to_string()));
    }
    crate::provider::clear_openai_provider_unavailability_for_account_label(Some(other));
    crate::auth::codex::set_active_account_override(None);
}

#[tokio::test]
async fn invalidate_anthropic_usage_acknowledges_after_clearing_only_pinned_claude_cooldown() {
    let _guard = crate::storage::lock_test_env();
    let target = "daemon-claude-reset-target";
    let other = "daemon-claude-reset-other";
    crate::auth::claude::set_active_account_override(Some(target.to_string()));
    crate::provider::record_provider_unavailable_for_account("claude", "target session limit");
    crate::auth::claude::set_active_account_override(Some(other.to_string()));
    crate::provider::record_provider_unavailable_for_account("claude", "other session limit");

    let (tx, mut rx) = mpsc::unbounded_channel();
    // Repeating the request is harmless: it only touches local state.
    for id in [61, 62] {
        handle_invalidate_anthropic_usage(id, Some(target.to_string()), &tx).await;
        assert!(matches!(rx.try_recv(), Ok(ServerEvent::Done { id: ack }) if ack == id));
        assert!(rx.try_recv().is_err());
        // The other login keeps its cooldown and stays the active account.
        assert!(crate::provider::provider_unavailability_detail_for_account("claude").is_some());
        crate::auth::claude::set_active_account_override(Some(target.to_string()));
        assert!(crate::provider::provider_unavailability_detail_for_account("claude").is_none());
        crate::auth::claude::set_active_account_override(Some(other.to_string()));
    }
    crate::provider::clear_claude_provider_unavailability_for_account_label(Some(other));
    crate::auth::claude::set_active_account_override(None);
}

#[tokio::test]
async fn account_switch_clears_only_selected_login_failure_even_when_agent_is_busy() {
    let _guard = EnvGuard::save(&[]);
    let previous_claude = crate::auth::claude::get_active_account_override();
    let previous_openai = crate::auth::codex::get_active_account_override();
    let target = "switch-failure-target";
    let other = "switch-failure-other";
    for (kind, provider_id) in [("anthropic", "claude"), ("openai", "openai")] {
        crate::auth::claude::set_active_account_override(Some(target.to_string()));
        crate::auth::codex::set_active_account_override(Some(target.to_string()));
        for id in ["claude", "openai"] {
            crate::provider::record_provider_unavailable_for_account(id, "old credential rejected");
        }
        crate::auth::claude::set_active_account_override(Some(other.to_string()));
        crate::auth::codex::set_active_account_override(Some(other.to_string()));
        crate::provider::record_provider_unavailable_for_account(
            provider_id,
            "other login rejected",
        );

        let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
        let agent = Arc::new(Mutex::new(Agent::new(provider, Registry::empty())));
        let busy = agent.lock().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_account_switch_refresh(71, kind, target.to_string(), Arc::clone(&agent), tx);
        assert!(crate::provider::provider_unavailability_detail_for_account(provider_id).is_some());
        crate::auth::claude::set_active_account_override(Some(target.to_string()));
        crate::auth::codex::set_active_account_override(Some(target.to_string()));
        assert!(crate::provider::provider_unavailability_detail_for_account(provider_id).is_none());
        let unrelated = if provider_id == "claude" {
            "openai"
        } else {
            "claude"
        };
        assert!(crate::provider::provider_unavailability_detail_for_account(unrelated).is_some());
        drop(busy);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(event) = rx.recv().await {
                if matches!(event, ServerEvent::Done { id: 71 }) {
                    return;
                }
            }
            panic!("account switch must acknowledge completion");
        })
        .await
        .expect("account switch must finish after the agent becomes idle");
        for id in ["claude", "openai"] {
            crate::provider::clear_provider_unavailable_for_account(id);
        }
        crate::auth::claude::set_active_account_override(Some(other.to_string()));
        crate::auth::codex::set_active_account_override(Some(other.to_string()));
        crate::provider::clear_provider_unavailable_for_account(provider_id);
    }
    crate::auth::claude::set_active_account_override(previous_claude);
    crate::auth::codex::set_active_account_override(previous_openai);
}

#[tokio::test]
async fn notify_auth_changed_clears_renewed_claude_login_failure() {
    let _guard = EnvGuard::save(&[
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_INITIAL_PROVIDER_EXPLICIT",
    ]);
    let previous = crate::auth::claude::get_active_account_override();
    crate::auth::claude::set_active_account_override(Some("renewed-login".to_string()));
    crate::provider::record_provider_unavailable_for_account(
        "claude",
        "old refresh token rejected",
    );
    let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), Registry::empty())));
    let session_id = agent.lock().await.session_id().to_string();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let (tx, mut rx) = mpsc::unbounded_channel();
    handle_notify_auth_changed(
        72,
        Some("claude".to_string()),
        None,
        false,
        &provider,
        &provider,
        &sessions,
        &session_id,
        &agent,
        &tx,
    )
    .await;
    recv_final_catalog_notification(&mut rx).await;
    assert!(crate::provider::provider_unavailability_detail_for_account("claude").is_none());
    crate::auth::claude::set_active_account_override(previous);
}
