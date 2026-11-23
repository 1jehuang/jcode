use super::*;

struct TestProvider;

#[async_trait::async_trait]
impl Provider for TestProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        anyhow::bail!("catalog test must not call complete")
    }

    fn name(&self) -> &str {
        "catalog-retry-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[tokio::test]
#[expect(clippy::await_holding_lock, reason = "isolated test home")]
async fn queued_catalog_retry_recovers_after_agent_unlocks() {
    let _env = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }
    let _restore = RestoreHome(previous_home);
    crate::env::set_var("JCODE_HOME", home.path());
    let agent = Arc::new(Mutex::new(Agent::new(
        Arc::new(TestProvider),
        Registry::empty(),
    )));
    let busy = agent.lock().await;
    let mut retry = ModelCatalogRetry::default();
    retry.queue();
    assert!(
        try_available_models_catalog_offloaded(&agent)
            .await
            .is_none()
    );
    retry.busy();
    let deadline = retry.deadline.unwrap();
    retry.queue();
    assert_eq!(retry.deadline, Some(deadline));
    drop(busy);
    tokio::time::sleep_until(deadline).await;
    let catalog = try_available_models_catalog_offloaded(&agent)
        .await
        .unwrap();
    assert!(catalog.live_event().is_some());
    retry.complete();
    assert!(retry.deadline.is_none());
}

#[tokio::test]
async fn busy_model_catalog_updates_coalesce_and_retry_until_idle() {
    let mut retry = ModelCatalogRetry::default();
    assert!(retry.deadline.is_none());
    retry.queue();
    for _ in 0..32 {
        retry.busy();
        let deadline = retry.deadline.unwrap();
        for _ in 0..100 {
            retry.queue();
        }
        assert_eq!(retry.deadline, Some(deadline));
        let delay = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(delay <= Duration::from_millis(1600));
        assert!(delay > Duration::ZERO);
    }
    // No new bus event is needed when the busy turn ends.
    assert!(retry.deadline.is_some());
    retry.complete();
    assert!(retry.deadline.is_none());
    assert_eq!(retry.attempts, 0);
    retry.queue();
    retry.busy();
    assert_eq!(retry.attempts, 1);
}
