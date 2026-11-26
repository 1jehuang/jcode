use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct InputBudgetProvider(Arc<AtomicUsize>);

#[async_trait]
impl Provider for InputBudgetProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        panic!("budget bookkeeping must not call a provider")
    }

    fn name(&self) -> &str {
        "input-budget-test"
    }

    fn model(&self) -> String {
        format!("budget-{}", self.input_token_limit())
    }

    fn context_window(&self) -> usize {
        200_000
    }

    fn input_token_limit(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    fn set_reasoning_effort(&self, _: &str) -> Result<()> {
        self.0.store(48_000, Ordering::SeqCst);
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test]
async fn input_budget_survives_caps_route_changes_and_reload() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let limit = Arc::new(AtomicUsize::new(32_000));
    let provider = Arc::new(InputBudgetProvider(limit.clone()));
    let mut agent = Agent::new(provider.clone(), Registry::empty());
    let manager = agent.registry.compaction();
    assert_eq!(manager.read().await.token_budget(), 32_000);
    assert_eq!(agent.provider_context_window(), 200_000);
    assert_eq!(agent.effective_context_budget().await, 32_000);
    assert_eq!(agent.set_context_cap(Some(64_000)).await.unwrap(), 32_000);
    assert_eq!(agent.set_context_cap(Some(16_000)).await.unwrap(), 16_000);
    assert_eq!(agent.set_context_cap(None).await.unwrap(), 32_000);

    // Some routes change model when effort changes; refresh that input budget.
    agent.set_reasoning_effort("high").unwrap();
    assert_eq!(manager.read().await.token_budget(), 48_000);
    assert_eq!(agent.effective_context_budget().await, 48_000);
    let restored = Session::load(&agent.session.id).unwrap();
    let mut agent = Agent::new_with_session(provider, Registry::empty(), restored, None);
    assert_eq!(
        agent.registry.compaction().read().await.token_budget(),
        48_000
    );

    limit.store(24_000, Ordering::SeqCst);
    agent.messages_for_provider();
    assert_eq!(
        agent.registry.compaction().read().await.token_budget(),
        24_000
    );
    assert_eq!(agent.effective_context_budget().await, 24_000);
}

#[tokio::test]
async fn input_budget_survives_image_and_native_compaction_resets() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut agent = Agent::new(
        Arc::new(InputBudgetProvider(Arc::new(AtomicUsize::new(32_000)))),
        Registry::empty(),
    );
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "Keep the original transcript".into(),
            cache_control: None,
        }],
    );
    let manager = agent.registry.compaction();
    manager.write().await.set_budget(200_000);
    agent.reset_after_transcript_image_rewrite();
    assert_eq!(manager.read().await.token_budget(), 32_000);
    manager.write().await.set_budget(200_000);
    agent
        .apply_openai_native_compaction("encrypted-fixture".into(), 1)
        .unwrap();
    assert_eq!(manager.read().await.token_budget(), 32_000);
}

#[tokio::test]
async fn input_budget_context_recovery_preserves_observed_tokens() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut agent = Agent::new(
        Arc::new(InputBudgetProvider(Arc::new(AtomicUsize::new(32_000)))),
        Registry::empty(),
    );
    for index in 0..16 {
        agent.add_message(
            if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            vec![ContentBlock::Text {
                text: format!("Retain caller history {index}"),
                cache_control: None,
            }],
        );
    }
    let original = serde_json::to_value(&agent.session.messages).unwrap();
    let manager = agent.registry.compaction();
    assert_eq!(manager.read().await.token_budget(), 32_000);
    manager.write().await.update_observed_input_tokens(47_123);
    assert!(agent.try_auto_compact_after_context_limit("maximum context length exceeded"));
    assert_eq!(agent.last_recovery_compaction.unwrap().0, 47_123);
    assert_eq!(manager.read().await.token_budget(), 32_000);
    assert_eq!(
        serde_json::to_value(&agent.session.messages).unwrap(),
        original
    );
}
