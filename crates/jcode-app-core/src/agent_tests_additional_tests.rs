use super::*;

#[tokio::test]
async fn working_directory_moves_preserve_first_active_session_origin() {
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    agent.session.messages.clear();
    agent.session.working_dir = Some("/daemon".into());
    agent.set_working_dir("/projects/start");
    assert_eq!(
        agent.session.original_working_dir, None,
        "initial subscription is not a move"
    );
    agent.session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        }],
    );
    agent.set_working_dir("/projects/second");
    agent.set_working_dir_for_pending_context(Some("/projects/third".into()));
    assert_eq!(
        agent.session.original_working_dir.as_deref(),
        Some("/projects/start")
    );
    assert_eq!(agent.working_dir(), Some("/projects/third"));
    agent.set_working_dir("/projects/start");
    assert_eq!(
        agent.session.original_working_dir.as_deref(),
        Some("/projects/start")
    );
}

#[tokio::test]
async fn user_response_capability_updates_locked_tool_surface() {
    let mut agent = Agent::new(Arc::new(BoundSavedAccountProvider), Registry::empty());
    assert!(
        !agent
            .tool_definitions()
            .await
            .iter()
            .any(|d| d.name == "user_response")
    );
    assert!(agent.locked_tools.is_some());
    assert!(!agent.tool_names().await.iter().any(|n| n == "applet"));
    agent.set_desktop_response_capability(true).await;
    assert!(agent.locked_tools.is_none());
    assert!(agent.tool_names().await.iter().any(|n| n == "applet"));
    assert!(
        agent
            .tool_definitions()
            .await
            .iter()
            .any(|d| d.name == "user_response")
    );
    agent.set_desktop_response_capability(true).await;
    assert!(
        agent.locked_tools.is_some(),
        "unchanged capability preserves the cache"
    );
    agent.set_desktop_response_capability(false).await;
    assert!(agent.locked_tools.is_none());
    assert!(!agent.tool_names().await.iter().any(|n| n == "applet"));
    assert!(
        !agent
            .tool_definitions()
            .await
            .iter()
            .any(|d| d.name == "user_response")
    );
}

#[tokio::test]
async fn saved_account_observed_binding_is_persisted() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();
    let provider: Arc<dyn Provider> = Arc::new(BoundSavedAccountProvider);
    let mut agent = Agent::new(provider, Registry::empty());
    agent.session.route_api_method = None;
    agent.session.save_prepared().unwrap();
    agent.sync_bound_account_route();
    assert_eq!(
        agent.session.route_api_method.as_deref(),
        Some("openai-oauth@work")
    );
    let saved = crate::session::Session::load(&agent.session.id).unwrap();
    assert_eq!(saved.route_api_method.as_deref(), Some("openai-oauth@work"));
    assert_eq!(saved.provider_key.as_deref(), Some("openai-oauth"));
    agent.sync_bound_account_route();
    assert_eq!(agent.session.route_api_method, saved.route_api_method);
    crate::provider::clear_openai_provider_unavailability_for_account_label(Some("work"));
    crate::provider::clear_openai_provider_unavailability_for_account_label(Some("personal"));
    agent.record_failed_account_route("429 rate limit reached");
    assert!(
        crate::provider::provider_unavailability_detail_for_account_label("openai", Some("work"))
            .is_some()
    );
    assert!(
        crate::provider::provider_unavailability_detail_for_account_label(
            "openai",
            Some("personal")
        )
        .is_none()
    );
    crate::provider::clear_openai_provider_unavailability_for_account_label(Some("work"));
    agent.session.route_api_method = Some("openai-oauth@personal".to_string());
    agent.sync_bound_account_route();
    assert_eq!(
        agent.session.route_api_method.as_deref(),
        Some("openai-oauth@personal")
    );
    assert!(
        agent
            .ensure_session_lease()
            .unwrap_err()
            .to_string()
            .contains("personal")
    );
}
