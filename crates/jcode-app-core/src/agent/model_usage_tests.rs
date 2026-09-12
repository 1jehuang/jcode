use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct UsageProvider {
    calls: Arc<AtomicUsize>,
    model: Arc<std::sync::Mutex<String>>,
    fail: bool,
}

impl UsageProvider {
    fn routes() -> Vec<crate::provider::ModelRoute> {
        ["requested-model", "serving-model"]
            .into_iter()
            .map(|model| crate::provider::ModelRoute {
                model: model.into(),
                provider: "OpenAI".into(),
                api_method: "openai-api-key".into(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            })
            .collect()
    }
}

#[async_trait]
impl Provider for UsageProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        if self.fail {
            anyhow::bail!("synthetic request failed");
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        *self.model.lock().unwrap() = "serving-model".into();
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta("answer".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some(if call == 0 { "max_tokens" } else { "end_turn" }.into()),
            }),
        ])))
    }
    fn name(&self) -> &str {
        "openai"
    }
    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }
    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        Self::routes()
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(jcode_provider_core::ResolvedCredential::ApiKey)
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn usage_agent(fail: bool) -> Agent {
    let provider: Arc<dyn Provider> = Arc::new(UsageProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        model: Arc::new(std::sync::Mutex::new("requested-model".into())),
        fail,
    });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    agent.session.is_debug = false;
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "test".into(),
            cache_control: None,
        }],
    );
    agent
}

#[tokio::test]
async fn both_turn_paths_record_once_across_continuations_and_attribute_serving_model() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for (index, streaming) in [false, true].into_iter().enumerate() {
        let mut agent = usage_agent(false).await;
        let mut updates = Bus::global().subscribe();
        if streaming {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            agent.run_turn_streaming_mpsc(tx).await.unwrap();
        } else {
            agent.run_turn(false).await.unwrap();
        }
        let routes = agent.model_routes();
        let serving = routes
            .iter()
            .find(|route| route.model == "serving-model")
            .unwrap();
        assert_eq!(serving.usage.as_ref().unwrap().count, index as u64 + 1);
        assert!(
            serving
                .usage
                .as_ref()
                .unwrap()
                .last_used_unix_secs
                .is_some()
        );
        let requested = routes
            .iter()
            .find(|route| route.model == "requested-model")
            .unwrap();
        assert_eq!(requested.usage.as_ref().unwrap().count, 0);
        assert!(
            agent
                .session
                .messages
                .iter()
                .filter(|m| m.role == Role::Assistant)
                .count()
                >= 2
        );
        let mut pushed = false;
        while let Ok(event) = updates.try_recv() {
            if let BusEvent::ModelUsageUpdated(route) = event {
                assert_eq!(route.model, "serving-model");
                pushed = true;
            }
        }
        assert!(
            pushed,
            "metadata must refresh without taking the busy Agent lock"
        );
    }
}

#[tokio::test]
async fn failed_requests_and_debug_sessions_do_not_record_turns() {
    let home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut failed = usage_agent(true).await;
    assert!(failed.run_turn(false).await.is_err());
    let mut debug = usage_agent(false).await;
    debug.session.is_debug = true;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    debug.run_turn_streaming_mpsc(tx).await.unwrap();
    assert!(!home.root().join("model-usage-v1.sqlite3").exists());
    assert!(
        debug
            .model_routes()
            .iter()
            .all(|route| route.usage.is_none())
    );
}

#[tokio::test]
async fn resumed_session_keeps_turn_identity_until_new_input() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let mut original = usage_agent(false).await;
    original.run_turn(false).await.unwrap();
    let session_id = original.session.id.clone();
    let anchor = original.session.model_usage_turn_id.clone();
    let mut resumed = usage_agent(false).await;
    resumed.session = crate::session::Session::load(&session_id).unwrap();
    assert_eq!(resumed.session.model_usage_turn_id, anchor);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    resumed
        .run_once_streaming_mpsc("", vec![], Some("Continue after reload".into()), tx)
        .await
        .unwrap();
    let count = |agent: &Agent| {
        agent
            .model_routes()
            .into_iter()
            .find(|route| route.model == "serving-model")
            .unwrap()
            .usage
            .unwrap()
            .count
    };
    assert_eq!(
        count(&resumed),
        1,
        "reload continuation must not count twice"
    );
    assert_eq!(resumed.session.model_usage_turn_id, anchor);
    resumed.run_once_capture("new input").await.unwrap();
    assert_eq!(count(&resumed), 2);
    assert_ne!(resumed.session.model_usage_turn_id, anchor);
    assert_eq!(
        crate::session::Session::load(&session_id)
            .unwrap()
            .model_usage_turn_id,
        resumed.session.model_usage_turn_id,
        "journal must persist the changed anchor"
    );
}
