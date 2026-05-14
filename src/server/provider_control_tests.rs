use super::*;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, ModelRoute, Provider};
use crate::tool::Registry;
use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::RwLock as StdRwLock;

#[derive(Default)]
struct AuthChangeMockState {
    logged_in: StdRwLock<bool>,
}

struct AuthChangeMockProvider {
    state: Arc<AuthChangeMockState>,
}

impl AuthChangeMockProvider {
    fn new() -> Self {
        Self {
            state: Arc::new(AuthChangeMockState::default()),
        }
    }
}

#[async_trait]
impl Provider for AuthChangeMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        let stream = futures::stream::empty::<anyhow::Result<StreamEvent>>();
        Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = _> + Send>>)
    }

    fn name(&self) -> &str {
        "mock-auth"
    }

    fn model(&self) -> String {
        if *self.state.logged_in.read().unwrap_or_else(|e| e.into_inner()) {
            "logged-in-model".to_string()
        } else {
            "logged-out-model".to_string()
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        if *self.state.logged_in.read().unwrap_or_else(|e| e.into_inner()) {
            vec!["logged-in-model".to_string(), "second-model".to_string()]
        } else {
            vec!["logged-out-model".to_string()]
        }
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: "MockAuth".to_string(),
                api_method: "mock-auth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect()
    }

    fn on_auth_changed(&self) {
        *self.state.logged_in.write().unwrap_or_else(|e| e.into_inner()) = true;
        crate::bus::Bus::global().publish_models_updated();
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            state: Arc::clone(&self.state),
        })
    }
}

#[tokio::test]
async fn notify_auth_changed_emits_available_models_updated_after_provider_update() {
    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "test-session".to_string(),
        Arc::clone(&agent),
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_notify_auth_changed(
        42,
        &provider,
        &provider,
        &sessions,
        &agent,
        &client_event_tx,
    )
    .await;

    let mut saw_done = false;
    let mut saw_models = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, client_event_rx.recv())
            .await
            .expect("receive server event before timeout");
        match event.expect("channel open") {
            ServerEvent::Done { id } => {
                assert_eq!(id, 42);
                saw_done = true;
            }
            ServerEvent::AvailableModelsUpdated {
                provider_name,
                provider_model,
                available_models,
                available_model_routes,
            } => {
                saw_models = Some((
                    provider_name,
                    provider_model,
                    available_models,
                    available_model_routes,
                ));
                break;
            }
            _ => {}
        }
    }

    assert!(saw_done, "expected immediate Done ack");
    let (provider_name, provider_model, available_models, available_model_routes) =
        saw_models.expect("expected AvailableModelsUpdated event");
    assert_eq!(provider_name.as_deref(), Some("mock-auth"));
    assert_eq!(provider_model.as_deref(), Some("logged-in-model"));
    assert_eq!(
        available_models,
        vec!["logged-in-model".to_string(), "second-model".to_string()]
    );
    assert!(available_model_routes.iter().any(|route| {
        route.model == "logged-in-model"
            && route.provider == "MockAuth"
            && route.api_method == "mock-auth"
    }));
}

#[tokio::test]
async fn notify_auth_changed_defers_busy_session_refresh_until_idle() {
    crate::bus::reset_models_updated_publish_state_for_tests();
    let current_provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
    let busy_provider = Arc::new(AuthChangeMockProvider::new());
    let busy_state = Arc::clone(&busy_provider.state);
    let busy_provider: Arc<dyn Provider> = busy_provider;
    let registry = Registry::empty();
    let current_agent = Arc::new(Mutex::new(Agent::new(
        Arc::clone(&current_provider),
        registry.clone(),
    )));
    let busy_agent = Arc::new(Mutex::new(Agent::new(busy_provider, registry)));
    let busy_guard = busy_agent.lock().await;
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "busy-session".to_string(),
        Arc::clone(&busy_agent),
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_notify_auth_changed(
        43,
        &current_provider,
        &current_provider,
        &sessions,
        &current_agent,
        &client_event_tx,
    )
    .await;

    assert!(
        matches!(
            client_event_rx.recv().await,
            Some(ServerEvent::Done { id: 43 })
        ),
        "expected immediate Done ack before waiting for the busy session"
    );
    assert!(
        !*busy_state.logged_in.read().unwrap_or_else(|e| e.into_inner()),
        "busy session provider should not refresh until its agent lock is released"
    );

    drop(busy_guard);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if *busy_state.logged_in.read().unwrap_or_else(|e| e.into_inner()) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("busy session provider was not refreshed after it became idle");
}

#[tokio::test]
async fn refresh_models_emits_available_models_updated_after_prefetch() {
    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_refresh_models(7, &provider, &agent, &client_event_tx).await;

    let mut saw_done = false;
    let mut saw_models = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let event = tokio::time::timeout(remaining, client_event_rx.recv())
            .await
            .expect("receive server event before timeout");
        match event.expect("channel open") {
            ServerEvent::Done { id } => {
                assert_eq!(id, 7);
                saw_done = true;
            }
            ServerEvent::AvailableModelsUpdated {
                provider_name,
                provider_model,
                available_models,
                available_model_routes,
            } => {
                saw_models = Some((
                    provider_name,
                    provider_model,
                    available_models,
                    available_model_routes,
                ));
                break;
            }
            _ => {}
        }
    }

    assert!(saw_done, "expected immediate Done ack");
    let (provider_name, provider_model, available_models, available_model_routes) =
        saw_models.expect("expected AvailableModelsUpdated event");
    assert_eq!(provider_name.as_deref(), Some("mock-auth"));
    assert_eq!(provider_model.as_deref(), Some("logged-out-model"));
    assert_eq!(available_models, vec!["logged-out-model".to_string()]);
    assert!(available_model_routes.iter().any(|route| {
        route.model == "logged-out-model"
            && route.provider == "MockAuth"
            && route.api_method == "mock-auth"
    }));
}
