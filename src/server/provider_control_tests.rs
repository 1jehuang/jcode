use super::*;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, ModelRoute, Provider};
use crate::tool::Registry;
use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, MutexGuard as StdMutexGuard, OnceLock};

#[derive(Default)]
struct AuthChangeMockState {
    logged_in: StdRwLock<bool>,
    selected_model: StdRwLock<Option<String>>,
    route_provider: StdRwLock<String>,
    route_api_method: StdRwLock<String>,
    expose_selected_model_in_routes: StdRwLock<bool>,
    complete_calls: AtomicUsize,
    complete_models: StdMutex<Vec<String>>,
}

struct AuthChangeMockProvider {
    state: Arc<AuthChangeMockState>,
}

impl AuthChangeMockProvider {
    fn new() -> Self {
        let state = AuthChangeMockState {
            route_provider: StdRwLock::new("MockAuth".to_string()),
            route_api_method: StdRwLock::new("mock-auth".to_string()),
            expose_selected_model_in_routes: StdRwLock::new(true),
            ..AuthChangeMockState::default()
        };
        Self {
            state: Arc::new(state),
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
        self.state.complete_calls.fetch_add(1, Ordering::SeqCst);
        self.state
            .complete_models
            .lock()
            .unwrap()
            .push(self.model());
        let stream = futures::stream::iter([
            Ok(StreamEvent::TextDelta("ok".to_string())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ]);
        Ok(Box::pin(stream) as Pin<Box<dyn futures::Stream<Item = _> + Send>>)
    }

    fn name(&self) -> &str {
        "mock-auth"
    }

    fn model(&self) -> String {
        if let Some(model) = self.state.selected_model.read().unwrap().clone() {
            return model;
        }

        if *self.state.logged_in.read().unwrap() {
            "logged-in-model".to_string()
        } else {
            "logged-out-model".to_string()
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        let mut models = if *self.state.logged_in.read().unwrap() {
            vec!["logged-in-model".to_string(), "second-model".to_string()]
        } else {
            vec!["logged-out-model".to_string()]
        };

        if *self.state.expose_selected_model_in_routes.read().unwrap()
            && let Some(model) = self.state.selected_model.read().unwrap().clone()
            && !models.iter().any(|candidate| candidate == &model)
        {
            models.insert(0, model);
        }

        models
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn set_model(&self, model: &str) -> anyhow::Result<()> {
        let model = model.trim();
        let model = model
            .split_once(':')
            .map(|(_, model)| model)
            .unwrap_or(model)
            .trim();
        if model.is_empty() {
            anyhow::bail!("model cannot be empty");
        }

        *self.state.selected_model.write().unwrap() = Some(model.to_string());
        Ok(())
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        let provider = self.state.route_provider.read().unwrap().clone();
        let api_method = self.state.route_api_method.read().unwrap().clone();
        self.available_models_display()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: provider.clone(),
                api_method: api_method.clone(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect()
    }

    fn on_auth_changed(&self) {
        *self.state.logged_in.write().unwrap() = true;
        crate::bus::Bus::global().publish_models_updated();
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            state: Arc::clone(&self.state),
        })
    }
}

fn lock_env() -> StdMutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(())).lock().unwrap()
}

struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
    _lock: StdMutexGuard<'static, ()>,
}

impl EnvGuard {
    fn save(keys: &[&'static str]) -> Self {
        let lock = lock_env();
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var(key).ok()))
            .collect();
        for key in keys {
            crate::env::remove_var(key);
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

#[tokio::test]
async fn notify_auth_changed_emits_available_models_updated_after_provider_update() {
    let _guard = EnvGuard::save(&[]);
    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider: Arc<dyn Provider> = Arc::new(AuthChangeMockProvider::new());
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let session_id = { agent.lock().await.session_id().to_string() };
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "test-session".to_string(),
        Arc::clone(&agent),
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    handle_notify_auth_changed(
        42,
        None,
        None,
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

    let final_activity = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match bus_rx.recv().await.expect("bus should stay open") {
                crate::bus::BusEvent::UiActivity(activity)
                    if activity.kind == crate::bus::UiActivityKind::Catalog
                        && activity.session_id.as_deref() == Some(session_id.as_str())
                        && activity.message.contains("Auth Model Catalog Updated") =>
                {
                    break activity;
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("expected final auth catalog activity");
    assert!(final_activity.message.contains("Added models:"));
    assert!(final_activity.message.contains("`logged-in-model`"));
    assert!(final_activity.message.contains("`second-model`"));
    assert!(
        final_activity
            .message
            .contains("Selected model: `logged-in-model`")
    );
    assert!(final_activity.message.contains("Use `/model`"));
}

#[tokio::test]
async fn notify_auth_changed_defers_busy_session_refresh_until_idle() {
    let _guard = EnvGuard::save(&[]);
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
        None,
        None,
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
        !*busy_state.logged_in.read().unwrap(),
        "busy session provider should not refresh until its agent lock is released"
    );

    drop(busy_guard);

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if *busy_state.logged_in.read().unwrap() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!("busy session provider was not refreshed after it became idle");
}

#[tokio::test]
async fn notify_auth_changed_with_azure_hint_applies_runtime_model_without_completion() {
    let _guard = EnvGuard::save(&[
        "AZURE_OPENAI_ENDPOINT",
        "AZURE_OPENAI_MODEL",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_USE_ENTRA",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);
    crate::env::set_var("AZURE_OPENAI_ENDPOINT", "https://example.openai.azure.com");
    crate::env::set_var("AZURE_OPENAI_MODEL", "azure-deployment");
    crate::env::set_var("AZURE_OPENAI_API_KEY", "test-key");
    crate::env::set_var("AZURE_OPENAI_USE_ENTRA", "0");

    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider = Arc::new(AuthChangeMockProvider::new());
    let state = Arc::clone(&provider.state);
    let provider: Arc<dyn Provider> = provider;
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "test-session".to_string(),
        Arc::clone(&agent),
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_notify_auth_changed(
        44,
        Some("Azure OpenAI".to_string()),
        None,
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
                assert_eq!(id, 44);
                saw_done = true;
            }
            ServerEvent::AvailableModelsUpdated {
                provider_model,
                available_models,
                ..
            } => {
                saw_models = Some((provider_model, available_models));
                break;
            }
            _ => {}
        }
    }

    assert!(saw_done, "expected immediate Done ack");
    let (provider_model, available_models) = saw_models.expect("expected model refresh event");
    assert_eq!(provider_model.as_deref(), Some("azure-deployment"));
    assert!(
        available_models
            .iter()
            .any(|model| model == "azure-deployment")
    );
    assert_eq!(
        std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
        Ok("azure-openai")
    );
    assert_eq!(
        std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
        Ok("openrouter")
    );
    assert_eq!(
        state.complete_calls.load(Ordering::SeqCst),
        0,
        "auth refresh must not issue a completion with the old prompt/model"
    );
}

#[test]
fn cerebras_auth_hint_applies_openai_compatible_runtime_profile() {
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);

    let request =
        crate::auth::lifecycle::AuthActivationRequest::new(Some("Cerebras".to_string()), None);
    assert_eq!(request.provider_id().as_deref(), Some("cerebras"));

    let activation = crate::auth::lifecycle::activate_auth_change(&request);
    let default_model = activation.activated_model.as_deref();
    assert_eq!(default_model, Some("qwen-3-235b-a22b-instruct-2507"));
    assert_eq!(
        std::env::var("JCODE_RUNTIME_PROVIDER").as_deref(),
        Ok("openai-compatible")
    );
    assert_eq!(
        std::env::var("JCODE_ACTIVE_PROVIDER").as_deref(),
        Ok("openrouter")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_BASE").as_deref(),
        Ok("https://api.cerebras.ai/v1")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_API_KEY_NAME").as_deref(),
        Ok("CEREBRAS_API_KEY")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_ENV_FILE").as_deref(),
        Ok("cerebras.env")
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE").as_deref(),
        Ok("cerebras")
    );
    assert_eq!(
        activation.model_switch_request("mock-auth", "llama3.1-8b"),
        "cerebras:llama3.1-8b"
    );
}

#[tokio::test]
async fn notify_auth_changed_typed_cerebras_event_controls_user_visible_catalog_identity() {
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);

    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider = Arc::new(AuthChangeMockProvider::new());
    let provider: Arc<dyn Provider> = provider;
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let session_id = { agent.lock().await.session_id().to_string() };
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "test-session".to_string(),
        Arc::clone(&agent),
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    let mut auth = crate::protocol::AuthChanged::new("cerebras");
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
    auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
        "openai-compatible",
    ));
    auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new("cerebras"));

    handle_notify_auth_changed(
        45,
        Some("openai".to_string()),
        Some(auth),
        &provider,
        &provider,
        &sessions,
        &agent,
        &client_event_tx,
    )
    .await;

    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Done { id: 45 })
    ));

    let final_activity = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match bus_rx.recv().await.expect("bus should stay open") {
                crate::bus::BusEvent::UiActivity(activity)
                    if activity.kind == crate::bus::UiActivityKind::Catalog
                        && activity.session_id.as_deref() == Some(session_id.as_str())
                        && activity.message.contains("Auth Model Catalog Updated") =>
                {
                    break activity;
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("expected final auth catalog activity");

    assert!(
        final_activity
            .message
            .contains("Cerebras credentials are active"),
        "typed auth event should control user-visible provider label, got: {}",
        final_activity.message
    );
    assert!(
        !final_activity
            .message
            .contains("OpenAI credentials are active"),
        "stale legacy provider identity leaked into user-visible auth message: {}",
        final_activity.message
    );
    assert!(
        final_activity
            .message
            .contains("Auth Model Catalog Warning"),
        "typed auth event should warn when matching provider routes are missing: {}",
        final_activity.message
    );
    assert!(
        final_activity
            .message
            .contains("Expected selectable Cerebras model routes"),
        "warning should identify the expected provider: {}",
        final_activity.message
    );
    assert_eq!(
        std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE").as_deref(),
        Ok("cerebras")
    );
}

#[tokio::test]
async fn notify_auth_changed_switches_from_stale_model_to_matching_provider_route() {
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);

    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider = Arc::new(AuthChangeMockProvider::new());
    *provider.state.selected_model.write().unwrap() = Some("gpt-5.5".to_string());
    *provider.state.route_provider.write().unwrap() = "Cerebras".to_string();
    *provider.state.route_api_method.write().unwrap() = "openai-compatible:cerebras".to_string();
    let provider: Arc<dyn Provider> = provider;
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let session_id = { agent.lock().await.session_id().to_string() };
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "test-session".to_string(),
        Arc::clone(&agent),
    )])));
    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel();
    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    let mut auth = crate::protocol::AuthChanged::new("cerebras");
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
    auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
        "openai-compatible",
    ));
    auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new("cerebras"));

    handle_notify_auth_changed(
        46,
        Some("openai".to_string()),
        Some(auth),
        &provider,
        &provider,
        &sessions,
        &agent,
        &client_event_tx,
    )
    .await;

    let final_activity = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match bus_rx.recv().await.expect("bus should stay open") {
                crate::bus::BusEvent::UiActivity(activity)
                    if activity.kind == crate::bus::UiActivityKind::Catalog
                        && activity.session_id.as_deref() == Some(session_id.as_str())
                        && activity.message.contains("Auth Model Catalog Updated") =>
                {
                    break activity;
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("expected final auth catalog activity");

    assert!(
        final_activity
            .message
            .contains("Cerebras credentials are active"),
        "{}",
        final_activity.message
    );
    assert!(
        final_activity
            .message
            .contains("Selected model: `qwen-3-235b-a22b-instruct-2507`"),
        "final auth catalog update should switch away from stale OpenAI model: {}",
        final_activity.message
    );
    assert!(
        !final_activity.message.contains("Selected model: `gpt-5.5`"),
        "stale selected model leaked into final auth update: {}",
        final_activity.message
    );
    assert!(
        !final_activity
            .message
            .contains("Auth Model Catalog Warning"),
        "successful recovery should not warn: {}",
        final_activity.message
    );
}

#[tokio::test]
async fn notify_auth_changed_does_not_override_manual_model_selected_during_refresh() {
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);

    crate::bus::reset_models_updated_publish_state_for_tests();
    let provider = Arc::new(AuthChangeMockProvider::new());
    *provider.state.selected_model.write().unwrap() = Some("stale-model".to_string());
    *provider.state.route_provider.write().unwrap() = "Cerebras".to_string();
    *provider.state.route_api_method.write().unwrap() = "openai-compatible:cerebras".to_string();
    *provider
        .state
        .expose_selected_model_in_routes
        .write()
        .unwrap() = false;
    let provider: Arc<dyn Provider> = provider;
    let registry = Registry::empty();
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
    let session_id = { agent.lock().await.session_id().to_string() };
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
        "test-session".to_string(),
        Arc::clone(&agent),
    )])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let mut bus_rx = crate::bus::Bus::global().subscribe();
    while bus_rx.try_recv().is_ok() {}

    let mut auth = crate::protocol::AuthChanged::new("cerebras");
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
    auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
        "openai-compatible",
    ));
    auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new("cerebras"));

    handle_notify_auth_changed(
        48,
        None,
        Some(auth),
        &provider,
        &provider,
        &sessions,
        &agent,
        &client_event_tx,
    )
    .await;

    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Done { id: 48 })
    ));

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                client_event_rx.recv().await,
                Some(ServerEvent::AvailableModelsUpdated { .. })
            ) {
                break;
            }
        }
    })
    .await
    .expect("expected immediate auth model snapshot");

    {
        let mut agent_guard = agent.lock().await;
        agent_guard
            .set_model("user-picked-model")
            .expect("manual model switch should succeed");
    }

    let final_activity = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match bus_rx.recv().await.expect("bus should stay open") {
                crate::bus::BusEvent::UiActivity(activity)
                    if activity.kind == crate::bus::UiActivityKind::Catalog
                        && activity.session_id.as_deref() == Some(session_id.as_str())
                        && activity.message.contains("Auth Model Catalog Updated") =>
                {
                    break activity;
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("expected final auth catalog activity");

    assert!(
        final_activity
            .message
            .contains("Selected model: `user-picked-model`"),
        "late auth reconciliation must not override manual model selection: {}",
        final_activity.message
    );
    assert!(
        !final_activity
            .message
            .contains("Selected model: `logged-in-model`"),
        "late auth auto-selection overrode manual choice: {}",
        final_activity.message
    );
}

#[derive(Clone, Copy)]
struct AuthModelE2eScenario {
    name: &'static str,
    manual_pick_after_first_snapshot: Option<&'static str>,
    prompt_immediately_after_model_pick: bool,
    expected_first_prompt_model: &'static str,
}

#[tokio::test]
async fn auth_model_first_prompt_e2e_state_space_is_bounded_by_selection_source() {
    let scenarios = [
        AuthModelE2eScenario {
            name: "auth auto-selects matching route when user does not intervene",
            manual_pick_after_first_snapshot: None,
            prompt_immediately_after_model_pick: false,
            expected_first_prompt_model: "logged-in-model",
        },
        AuthModelE2eScenario {
            name: "manual picker selection during auth refresh wins first prompt",
            manual_pick_after_first_snapshot: Some("user-picked-model"),
            prompt_immediately_after_model_pick: true,
            expected_first_prompt_model: "user-picked-model",
        },
    ];

    for scenario in scenarios {
        let _guard = EnvGuard::save(&[
            "JCODE_OPENROUTER_API_BASE",
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENROUTER_CACHE_NAMESPACE",
            "JCODE_OPENROUTER_PROVIDER_FEATURES",
            "JCODE_OPENROUTER_MODEL_CATALOG",
            "JCODE_OPENROUTER_AUTH_HEADER",
            "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
            "JCODE_OPENROUTER_MODEL",
            "JCODE_RUNTIME_PROVIDER",
            "JCODE_ACTIVE_PROVIDER",
            "JCODE_FORCE_PROVIDER",
        ]);

        crate::bus::reset_models_updated_publish_state_for_tests();
        let provider_concrete = Arc::new(AuthChangeMockProvider::new());
        *provider_concrete.state.selected_model.write().unwrap() = Some("stale-model".to_string());
        *provider_concrete.state.route_provider.write().unwrap() = "Cerebras".to_string();
        *provider_concrete.state.route_api_method.write().unwrap() =
            "openai-compatible:cerebras".to_string();
        *provider_concrete
            .state
            .expose_selected_model_in_routes
            .write()
            .unwrap() = false;
        let provider: Arc<dyn Provider> = provider_concrete.clone();
        let registry = Registry::empty();
        let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry)));
        let session_id = { agent.lock().await.session_id().to_string() };
        let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([(
            format!("test-session-{}", scenario.name),
            Arc::clone(&agent),
        )])));
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
        let mut bus_rx = crate::bus::Bus::global().subscribe();
        while bus_rx.try_recv().is_ok() {}

        let mut auth = crate::protocol::AuthChanged::new("cerebras");
        auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
        auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
        auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
            "openai-compatible",
        ));
        auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new("cerebras"));

        handle_notify_auth_changed(
            148,
            None,
            Some(auth),
            &provider,
            &provider,
            &sessions,
            &agent,
            &client_event_tx,
        )
        .await;

        assert!(
            matches!(
                client_event_rx.recv().await,
                Some(ServerEvent::Done { id: 148 })
            ),
            "{}: expected auth Done",
            scenario.name
        );

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if matches!(
                    client_event_rx.recv().await,
                    Some(ServerEvent::AvailableModelsUpdated { .. })
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{}: expected immediate auth model snapshot", scenario.name));

        let mut first_prompt_output = None;
        if let Some(model) = scenario.manual_pick_after_first_snapshot {
            handle_set_model(248, model.to_string(), &agent, &client_event_tx).await;
            loop {
                match client_event_rx.recv().await {
                    Some(ServerEvent::ModelChanged {
                        id: 248,
                        model: changed,
                        error,
                        ..
                    }) => {
                        assert_eq!(error, None, "{}: manual model switch failed", scenario.name);
                        assert_eq!(
                            changed, model,
                            "{}: manual model switch mismatch",
                            scenario.name
                        );
                        break;
                    }
                    Some(_) => continue,
                    None => panic!("{}: model switch channel closed", scenario.name),
                }
            }

            if scenario.prompt_immediately_after_model_pick {
                let agent_for_prompt = Arc::clone(&agent);
                let scenario_name = scenario.name;
                first_prompt_output = Some(
                    tokio::time::timeout(std::time::Duration::from_secs(2), async move {
                        let mut agent_guard = agent_for_prompt.lock().await;
                        agent_guard
                            .run_once_capture("first prompt immediately after model selection")
                            .await
                    })
                    .await
                    .unwrap_or_else(|_| panic!("{}: first prompt stalled", scenario_name))
                    .unwrap_or_else(|error| {
                        panic!("{}: first prompt failed: {error:?}", scenario_name)
                    }),
                );
            }
        }

        let final_activity = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match bus_rx.recv().await.expect("bus should stay open") {
                    crate::bus::BusEvent::UiActivity(activity)
                        if activity.kind == crate::bus::UiActivityKind::Catalog
                            && activity.session_id.as_deref() == Some(session_id.as_str())
                            && activity.message.contains("Auth Model Catalog Updated") =>
                    {
                        break activity;
                    }
                    _ => continue,
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{}: expected final auth catalog activity", scenario.name));
        assert!(
            final_activity.message.contains(&format!(
                "Selected model: `{}`",
                scenario.expected_first_prompt_model
            )),
            "{}: final activity selected wrong model: {}",
            scenario.name,
            final_activity.message
        );

        let first_prompt_output = if let Some(output) = first_prompt_output {
            output
        } else {
            let mut agent_guard = agent.lock().await;
            agent_guard
                .run_once_capture("first prompt after auth/model selection")
                .await
                .unwrap_or_else(|error| panic!("{}: first prompt failed: {error:?}", scenario.name))
        };
        assert!(
            first_prompt_output.contains("ok"),
            "{}: fake provider response not observed: {}",
            scenario.name,
            first_prompt_output
        );
        let completed_models = provider_concrete
            .state
            .complete_models
            .lock()
            .unwrap()
            .clone();
        assert_eq!(
            completed_models.last().map(String::as_str),
            Some(scenario.expected_first_prompt_model),
            "{}: first provider request used wrong model; all completions: {:?}",
            scenario.name,
            completed_models
        );
    }
}

#[tokio::test]
async fn notify_auth_changed_switches_only_current_session_model() {
    let _guard = EnvGuard::save(&[
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_ACTIVE_PROVIDER",
        "JCODE_FORCE_PROVIDER",
    ]);

    crate::bus::reset_models_updated_publish_state_for_tests();
    let current_provider = Arc::new(AuthChangeMockProvider::new());
    let current_state = Arc::clone(&current_provider.state);
    *current_state.selected_model.write().unwrap() = Some("gpt-5.5".to_string());
    *current_state.route_provider.write().unwrap() = "Groq".to_string();
    *current_state.route_api_method.write().unwrap() = "openai-compatible:groq".to_string();
    let peer_provider = Arc::new(AuthChangeMockProvider::new());
    let peer_state = Arc::clone(&peer_provider.state);
    *peer_state.selected_model.write().unwrap() = Some("gpt-5.5".to_string());
    *peer_state.route_provider.write().unwrap() = "Groq".to_string();
    *peer_state.route_api_method.write().unwrap() = "openai-compatible:groq".to_string();

    let current_provider: Arc<dyn Provider> = current_provider;
    let peer_provider: Arc<dyn Provider> = peer_provider;
    let registry = Registry::empty();
    let current_agent = Arc::new(Mutex::new(Agent::new(
        Arc::clone(&current_provider),
        registry.clone(),
    )));
    let peer_agent = Arc::new(Mutex::new(Agent::new(peer_provider, registry)));
    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::from([
        ("current-session".to_string(), Arc::clone(&current_agent)),
        ("peer-session".to_string(), Arc::clone(&peer_agent)),
    ])));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let mut auth = crate::protocol::AuthChanged::new("groq");
    auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
    auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
        "openai-compatible",
    ));
    auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new("groq"));

    handle_notify_auth_changed(
        47,
        Some("openai".to_string()),
        Some(auth),
        &current_provider,
        &current_provider,
        &sessions,
        &current_agent,
        &client_event_tx,
    )
    .await;

    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Done { id: 47 })
    ));

    let expected = "llama-3.1-8b-instant";
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let current = current_state.selected_model.read().unwrap().clone();
        let peer = peer_state.selected_model.read().unwrap().clone();
        let peer_refreshed = *peer_state.logged_in.read().unwrap();
        if current.as_deref() == Some(expected)
            && peer.as_deref() == Some("gpt-5.5")
            && peer_refreshed
        {
            let peer_snapshot = available_models_updated_event(&peer_agent).await;
            let ServerEvent::AvailableModelsUpdated {
                provider_name,
                provider_model,
                available_model_routes,
                ..
            } = peer_snapshot
            else {
                panic!("expected available models snapshot for peer session");
            };
            assert_eq!(provider_name.as_deref(), Some("mock-auth"));
            assert_eq!(provider_model.as_deref(), Some("gpt-5.5"));
            assert!(available_model_routes.iter().any(|route| {
                route.model == "gpt-5.5"
                    && route.provider == "Groq"
                    && route.api_method == "openai-compatible:groq"
            }));
            assert!(
                available_model_routes
                    .iter()
                    .all(|route| route.model != expected),
                "auth-triggered Groq model leaked into peer session routes: {:?}",
                available_model_routes
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    panic!(
        "auth change did not keep model switch session-local: current={:?}, peer={:?}, peer_refreshed={}",
        current_state.selected_model.read().unwrap().clone(),
        peer_state.selected_model.read().unwrap().clone(),
        *peer_state.logged_in.read().unwrap()
    );
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
