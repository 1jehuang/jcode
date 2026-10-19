#![allow(clippy::await_holding_lock)]
use super::*;
use crate::message::{Message, ToolDefinition};
use crate::provider::EventStream;
use async_trait::async_trait;
use std::sync::Mutex as StdMutex;
use tokio::time::{Duration, timeout};

struct IsolatedRuntimeDir {
    _prev_runtime: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
}

impl IsolatedRuntimeDir {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().expect("runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
        Self {
            _prev_runtime: prev_runtime,
            _temp: temp,
        }
    }
}

impl Drop for IsolatedRuntimeDir {
    fn drop(&mut self) {
        if let Some(prev_runtime) = self._prev_runtime.take() {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }
}

#[derive(Default)]
struct TestEffortProvider {
    model: StdMutex<Option<String>>,
    effort: StdMutex<Option<String>>,
    service_tier: StdMutex<Option<String>>,
    transport: StdMutex<Option<String>>,
}

#[async_trait]
impl Provider for TestEffortProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        panic!("complete should not run in provider control test")
    }

    fn name(&self) -> &str {
        "test-effort"
    }

    fn model(&self) -> String {
        self.model
            .lock()
            .expect("model lock")
            .clone()
            .unwrap_or_else(|| "test-model-a".to_string())
    }

    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        *self.model.lock().expect("model lock") = Some(model.to_string());
        Ok(())
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        vec!["test-model-a".to_string(), "test-model-b".to_string()]
    }

    fn context_window(&self) -> usize {
        if self.model() == "test-model-b" {
            32_000
        } else {
            16_000
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.effort.lock().expect("effort lock").clone()
    }

    fn set_reasoning_effort(&self, effort: &str) -> anyhow::Result<()> {
        *self.effort.lock().expect("effort lock") = Some(effort.to_string());
        Ok(())
    }

    fn service_tier(&self) -> Option<String> {
        self.service_tier.lock().expect("service lock").clone()
    }

    fn set_service_tier(&self, service_tier: &str) -> anyhow::Result<()> {
        *self.service_tier.lock().expect("service lock") = Some(service_tier.to_string());
        Ok(())
    }

    fn transport(&self) -> Option<String> {
        self.transport.lock().expect("transport lock").clone()
    }

    fn set_transport(&self, transport: &str) -> anyhow::Result<()> {
        *self.transport.lock().expect("transport lock") = Some(transport.to_string());
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            model: StdMutex::new(Some(self.model())),
            effort: StdMutex::new(self.reasoning_effort()),
            service_tier: StdMutex::new(self.service_tier()),
            transport: StdMutex::new(self.transport()),
        })
    }
}

async fn test_agent(
    session_id: &str,
) -> (
    Arc<TestEffortProvider>,
    Arc<Mutex<Agent>>,
    mpsc::UnboundedSender<ServerEvent>,
    mpsc::UnboundedReceiver<ServerEvent>,
) {
    let provider = Arc::new(TestEffortProvider::default());
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let registry = crate::tool::Registry::new(Arc::clone(&provider_dyn)).await;
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some(provider.model());
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        Arc::clone(&provider_dyn),
        registry,
        session,
        None,
    )));
    let (client_event_tx, client_event_rx) = mpsc::unbounded_channel();
    (provider, agent, client_event_tx, client_event_rx)
}

#[tokio::test]
async fn set_reasoning_effort_does_not_wait_for_busy_agent_lock() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();

    let (provider, agent, client_event_tx, mut client_event_rx) =
        test_agent("session_busy_reasoning_effort").await;
    let busy_agent_lock = agent.lock().await;

    timeout(
        Duration::from_millis(100),
        handle_set_reasoning_effort(7, "low".to_string(), &agent, &client_event_tx),
    )
    .await
    .expect("reasoning effort changes must not wait for a busy agent mutex");

    assert!(client_event_rx.try_recv().is_err());

    drop(busy_agent_lock);

    let event = timeout(Duration::from_secs(1), client_event_rx.recv())
        .await
        .expect("deferred reasoning effort change should finish after agent is idle");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("low"));
    assert!(matches!(
        event,
        Some(ServerEvent::ReasoningEffortChanged {
            id: 7,
            effort: Some(effort),
            error: None,
        }) if effort == "low"
    ));
}

#[tokio::test]
async fn automatic_failover_revalidates_after_waiting_for_agent_lock() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
    let (provider, agent, tx, mut rx) = test_agent("session_rejected_auto_route").await;
    let selection = crate::provider::RouteSelection {
        model: "gpt-6.1-sol".to_string(),
        runtime_key: jcode_provider_core::RuntimeKey::OpenAIOAuth,
        api_method: "openai-oauth@nonexistent-failover-test-account".to_string(),
        provider_label: "OpenAI".to_string(),
        detail: String::new(),
    };
    let lock = agent.lock().await;
    timeout(
        Duration::from_millis(100),
        handle_set_route(29, selection, true, &agent, &tx),
    )
    .await
    .expect("route request must not block on the agent lock");
    assert!(rx.try_recv().is_err());
    drop(lock);
    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, ServerEvent::ModelChanged { id: 29, error: Some(error), .. }
        if error.contains("Auto-switch canceled"))
    );
    assert_eq!(provider.model(), "test-model-a");
}

#[tokio::test]
async fn automatic_failover_source_respects_current_api_pool_membership() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
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
    struct ApiSource;
    #[async_trait]
    impl Provider for ApiSource {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            panic!("must not send")
        }
        fn name(&self) -> &str {
            "OpenAI"
        }
        fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
            Some(jcode_provider_core::ResolvedCredential::ApiKey)
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }
    let agent = Agent::new(Arc::new(ApiSource), crate::tool::Registry::empty());
    assert!(
        !automatic_failover_source_is_pool_member(&agent),
        "metered keys stay manual by default"
    );
    let mut pool = crate::auth::account_pool::AccountPool::default();
    pool.members.push("openai-api".to_string());
    pool.save().unwrap();
    assert!(automatic_failover_source_is_pool_member(&agent));
    pool.members.clear();
    pool.excluded.push("openai-api".to_string());
    pool.save().unwrap();
    assert!(
        !automatic_failover_source_is_pool_member(&agent),
        "a source moved below the boundary must stay manual"
    );
}

#[tokio::test]
async fn set_model_does_not_wait_for_busy_agent_lock() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();

    let (provider, agent, client_event_tx, mut client_event_rx) =
        test_agent("session_busy_set_model").await;
    let busy_agent_lock = agent.lock().await;

    timeout(
        Duration::from_millis(100),
        handle_set_model(8, "test-model-b".to_string(), &agent, &client_event_tx),
    )
    .await
    .expect("model changes must not wait for a busy agent mutex");

    assert!(client_event_rx.try_recv().is_err());

    drop(busy_agent_lock);

    let event = timeout(Duration::from_secs(1), client_event_rx.recv())
        .await
        .expect("deferred model change should finish after agent is idle");
    assert_eq!(provider.model(), "test-model-b");
    assert_eq!(agent.lock().await.compaction_token_budget().await, 32_000);
    assert!(matches!(
        event,
        Some(ServerEvent::ModelChanged {
            id: 8,
            model,
            provider_name: Some(provider_name),
            error: None,
            ..
        }) if model == "test-model-b" && provider_name == "test-effort"
    ));
}

#[tokio::test]
async fn set_service_tier_does_not_wait_for_busy_agent_lock() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();

    let (provider, agent, client_event_tx, mut client_event_rx) =
        test_agent("session_busy_set_service_tier").await;
    let busy_agent_lock = agent.lock().await;

    timeout(
        Duration::from_millis(100),
        handle_set_service_tier(9, "priority".to_string(), &agent, None, &client_event_tx),
    )
    .await
    .expect("service tier changes must not wait for a busy agent mutex");

    assert!(matches!(
        client_event_rx.try_recv(),
        Ok(ServerEvent::ServiceTierChanged {
            id: 9,
            pending: true,
            service_tier: None,
            error: None,
        })
    ));
    assert_eq!(provider.service_tier(), None);

    drop(busy_agent_lock);

    let event = timeout(Duration::from_secs(1), client_event_rx.recv())
        .await
        .expect("deferred service tier change should finish after agent is idle");
    assert_eq!(provider.service_tier().as_deref(), Some("priority"));
    assert!(matches!(
        event,
        Some(ServerEvent::ServiceTierChanged {
            id: 9,
            pending: false,
            service_tier: Some(service_tier),
            error: None,
        }) if service_tier == "priority"
    ));
}

#[tokio::test]
async fn set_service_tier_configures_busy_session_immediately_with_one_confirmation() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
    let (provider, agent, tx, mut rx) = test_agent("session_busy_ultrafast_control").await;
    let locked = agent.lock().await;
    let control = super::super::state::SessionControlHandle::new(
        "session_busy_ultrafast_control",
        locked.soft_interrupt_queue(),
        locked.background_tool_signal(),
        locked.graceful_shutdown_signal(),
    )
    .with_provider(locked.provider_handle());
    // Busy reattachment uses the same session's registered provider, never the
    // creator connection's provisional provider or a different session's tier.
    let reattached = super::super::state::SessionControlHandle::cancel_only(
        "session_busy_ultrafast_control",
        locked.soft_interrupt_queue(),
        locked.graceful_shutdown_signal(),
    );
    assert!(control.provider_handle().is_some());
    for (id, tier) in [(10, "priority"), (11, "ultrafast"), (12, "off")] {
        timeout(
            Duration::from_millis(100),
            handle_set_service_tier(
                id,
                tier.to_string(),
                &agent,
                reattached.provider_handle(),
                &tx,
            ),
        )
        .await
        .expect("configuration must not wait for the turn to finish");
        assert_eq!(provider.service_tier().as_deref(), Some(tier));
        assert!(matches!(rx.try_recv(), Ok(ServerEvent::ServiceTierChanged {
            id: actual_id, pending: false, service_tier: Some(actual), error: None,
        }) if actual_id == id && actual == tier));
        assert!(
            rx.try_recv().is_err(),
            "instant changes emit one response only"
        );
    }
    drop(locked);
    super::super::state::remove_background_tool_signal("session_busy_ultrafast_control");
}

#[tokio::test]
async fn service_tier_control_provider_registry_tracks_rebind_and_session_lifecycle() {
    let _guard = crate::storage::lock_test_env();
    let _runtime = IsolatedRuntimeDir::new();
    let (original, agent, _, _) = test_agent("tier_registry_original").await;
    let locked = agent.lock().await;
    let control = super::super::state::SessionControlHandle::new(
        "tier_registry_original",
        locked.soft_interrupt_queue(),
        locked.background_tool_signal(),
        locked.graceful_shutdown_signal(),
    )
    .with_provider(locked.provider_handle());
    let other = super::super::state::SessionControlHandle::cancel_only(
        "tier_registry_other",
        locked.soft_interrupt_queue(),
        locked.graceful_shutdown_signal(),
    );
    assert!(
        other.provider_handle().is_none(),
        "never borrow another session's provider"
    );
    let replacement: Arc<dyn Provider> = Arc::new(TestEffortProvider::default());
    let control = control.with_provider(replacement.clone());
    assert!(Arc::ptr_eq(
        &control.provider_handle().unwrap(),
        &replacement
    ));
    assert_eq!(original.service_tier(), None);
    super::super::state::rename_background_tool_signal(
        "tier_registry_original",
        "tier_registry_other",
    );
    assert!(control.provider_handle().is_none());
    assert!(Arc::ptr_eq(&other.provider_handle().unwrap(), &replacement));
    drop(replacement);
    assert!(
        other.provider_handle().is_none(),
        "registry must not retain providers"
    );
    super::super::state::remove_background_tool_signal("tier_registry_other");
}
