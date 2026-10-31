//! Server-side account router for clients that cannot run the TUI's
//! client-side countdown (Jcode Desktop and SDK clients via the harness API).
//!
//! The TUI catches a usage/rate-limit failure, picks the next pool-approved
//! route with the shared ranking, and resends after a cancellable countdown.
//! API clients only ever saw the raw error card. When a client opts in with
//! `supports_server_failover`, the server performs the same switch itself and
//! re-runs the failed turn on the new route, without duplicating the user
//! message. The same pool rules apply: the source must be in the auto-switch
//! pool, the target must be pool-approved, and `cross_provider_failover` must
//! be in countdown (automatic) mode.

use crate::agent::Agent;
use crate::protocol::ServerEvent;
use crate::provider::RouteSelection;
use jcode_provider_core::FailoverDecision;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// Distinct accounts to try after the first failure before giving up.
const MAX_SERVER_FAILOVERS: usize = 3;

/// Whether `error` is the kind of failure the account router should handle.
fn error_wants_failover(error: &str) -> bool {
    if crate::provider::parse_failover_prompt_message(error).is_some() {
        return true;
    }
    matches!(
        jcode_provider_core::classify_failover_error_message(error),
        FailoverDecision::RetryAndMarkUnavailable
    )
}

/// One planned switch: how to apply it and how to describe it.
pub(super) struct FailoverTarget {
    pub selection: Option<RouteSelection>,
    pub spec: String,
    pub label: String,
}

fn describe_route(route: &crate::provider::ModelRoute) -> String {
    let method = crate::provider::ModelRouteApiMethod::parse(&route.api_method).display_label();
    format!("{} via {} ({})", route.model, method, route.provider)
}

/// Pick the next pool-approved route for `agent` after `error`, or `None`
/// when automatic switching is disabled or nothing eligible remains.
pub(super) fn pick_failover_target(agent: &Agent, error: &str) -> Option<FailoverTarget> {
    if crate::config::Config::load()
        .provider
        .cross_provider_failover
        != crate::config::CrossProviderFailoverMode::Countdown
    {
        return None;
    }
    if !error_wants_failover(error) {
        return None;
    }
    if !super::provider_control::automatic_failover_source_is_pool_member(agent) {
        return None;
    }
    let routes = agent.model_routes();
    let route_for_spec = |spec: &str| {
        routes
            .iter()
            .filter(|route| route.available)
            .find(|route| RouteSelection::from_model_route(route).routed_model_spec() == spec)
    };

    // The provider router already chose an exact target. Honor it.
    if let Some(prompt) = crate::provider::parse_failover_prompt_message(error)
        && let Some(spec) = prompt.target_route.as_deref()
        && crate::provider::MultiProvider::is_failover_route_pool_approved(spec)
    {
        let route = route_for_spec(spec);
        return Some(FailoverTarget {
            selection: route.map(RouteSelection::from_model_route),
            spec: spec.to_string(),
            label: route
                .map(describe_route)
                .unwrap_or_else(|| prompt.to_label.clone()),
        });
    }

    // Otherwise use the same shared ranking the TUI uses.
    let current_api_method = agent.session_route_api_method().unwrap_or_default();
    let source_key = jcode_provider_core::route_pool_key(&current_api_method);
    let pool = crate::auth::account_pool::AccountPool::load();
    let ranked = jcode_provider_core::rank_fallback_routes(
        &routes,
        &agent.provider_model(),
        &agent.provider_name(),
        &current_api_method,
        crate::provider::FallbackPickOptions {
            credential_failure: true,
        },
        &pool.members,
    );
    ranked
        .into_iter()
        .map(|index| &routes[index])
        .find(|route| {
            let spec = RouteSelection::from_model_route(route).routed_model_spec();
            crate::provider::MultiProvider::is_failover_route_pool_approved(&spec)
                && jcode_provider_core::route_pool_key(&route.api_method) != source_key
        })
        .map(|route| {
            let selection = RouteSelection::from_model_route(route);
            FailoverTarget {
                spec: selection.routed_model_spec(),
                selection: Some(selection),
                label: describe_route(route),
            }
        })
}

/// Apply `target` to the agent and tell every attached client.
pub(super) fn apply_failover_target(
    agent: &mut Agent,
    target: &FailoverTarget,
    event_tx: &mpsc::UnboundedSender<ServerEvent>,
) -> anyhow::Result<()> {
    match target.selection.as_ref() {
        Some(selection) => agent.set_route_selection(selection)?,
        None => agent.set_model(&target.spec)?,
    }
    agent.reset_provider_session();
    crate::telemetry::record_model_switch();
    crate::logging::event_info(
        "server_account_failover",
        vec![
            ("target", target.spec.clone()),
            ("model", agent.provider_model()),
            ("provider", agent.provider_name()),
        ],
    );
    let _ = event_tx.send(ServerEvent::ModelChanged {
        id: 0,
        model: agent.provider_model(),
        provider_name: Some(agent.provider_name()),
        context_window: Some(agent.provider_context_window() as u64),
        error: None,
        resolved_credential: agent.active_resolved_credential(),
        provider_oauth_email: super::provider_oauth_email(agent.provider_handle().as_ref()),
        provider_oauth_account_label: agent.provider_handle().serving_oauth_account_label(),
        reasoning_effort: agent.provider_reasoning_effort(),
    });
    let _ = event_tx.send(ServerEvent::ConnectionPhase {
        phase: format!("retrying on {}", target.label),
    });
    Ok(())
}

/// Run a message turn, and when enabled, transparently fail over to the next
/// pool-approved account and retry the same turn on a usage/rate-limit error.
pub(super) async fn process_message_with_server_failover(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
    enabled: bool,
) -> anyhow::Result<()> {
    let reminder = system_reminder.clone();
    let result = super::client_lifecycle::process_message_streaming_mpsc(
        Arc::clone(&agent),
        content,
        images,
        system_reminder,
        event_tx.clone(),
    )
    .await;
    if !enabled && result.is_err() {
        return result;
    }
    let mut guard = agent.lock().await;
    retry_turn_with_server_failover(&mut guard, result, reminder, event_tx.clone()).await?;
    super::runtime_ssh_loops::run_followups(&mut guard, event_tx).await
}

/// Server-initiated turn (swarm spawn/assign, wake, headless reload recovery)
/// on an already locked agent. No client runs a countdown for these turns, so
/// the server always applies the account router itself.
pub(super) async fn process_locked_message_with_server_failover(
    agent: &mut Agent,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) -> anyhow::Result<()> {
    let reminder = system_reminder.clone();
    let result = super::client_lifecycle::process_locked_message_streaming_mpsc(
        agent,
        content,
        images,
        system_reminder,
        event_tx.clone(),
    )
    .await;
    retry_turn_with_server_failover(agent, result, reminder, event_tx.clone()).await?;
    super::runtime_ssh_loops::run_followups(agent, event_tx).await
}

/// [`process_locked_message_with_server_failover`] for callers holding an
/// `Arc<Mutex<Agent>>`.
pub(super) async fn process_server_initiated_message(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    images: Vec<(String, String)>,
    system_reminder: Option<String>,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) -> anyhow::Result<()> {
    let mut guard = agent.lock().await;
    process_locked_message_with_server_failover(
        &mut guard,
        content,
        images,
        system_reminder,
        event_tx,
    )
    .await
}

/// Given the result of a turn attempt, switch to the next pool-approved
/// account and re-run the same turn while it keeps failing with a
/// usage/rate-limit error.
pub(super) async fn retry_turn_with_server_failover(
    agent: &mut Agent,
    mut result: anyhow::Result<()>,
    reminder: Option<String>,
    event_tx: mpsc::UnboundedSender<ServerEvent>,
) -> anyhow::Result<()> {
    let mut attempts = 0;
    while let Err(error) = &result {
        if attempts >= MAX_SERVER_FAILOVERS {
            break;
        }
        let message = crate::util::format_error_chain(error);
        // Only retry a turn whose last persisted message is still the
        // request (user prompt or tool results). Never re-ask after the
        // model already produced a partial reply.
        if agent.last_message_role() != Some(crate::message::Role::User) {
            break;
        }
        let Some(target) = pick_failover_target(agent, &message) else {
            break;
        };
        if let Err(switch_error) = apply_failover_target(agent, &target, &event_tx) {
            crate::logging::warn(&format!(
                "Server account failover to {} failed: {switch_error}",
                target.spec
            ));
            break;
        }
        attempts += 1;
        result = agent
            .continue_turn_after_failover(reminder.clone(), event_tx.clone())
            .await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_limits_and_router_prompts_want_failover() {
        assert!(error_wants_failover(
            "The usage limit has been reached (usage_limit_reached)"
        ));
        assert!(error_wants_failover("HTTP 429 Too Many Requests"));
        let prompt = jcode_provider_core::ProviderFailoverPrompt {
            from_provider: "openai".into(),
            from_label: "OpenAI (a)".into(),
            to_provider: "openai".into(),
            to_label: "OpenAI (b)".into(),
            target_route: Some("openai-oauth@b:gpt-6.1-sol".into()),
            reason: "usage limit".into(),
            estimated_input_chars: 0,
            estimated_input_tokens: 0,
        };
        assert!(error_wants_failover(&prompt.to_error_message()));
    }

    use crate::message::{Message, StreamEvent, ToolDefinition};
    use crate::provider::{EventStream, ModelRoute, Provider};
    use async_trait::async_trait;
    use std::sync::Mutex as StdMutex;

    /// Fails with a usage limit while on the Anthropic API key, succeeds once
    /// the route switches to the OpenAI API key.
    #[derive(Clone)]
    struct TwoAccountProvider {
        on_openai: Arc<StdMutex<bool>>,
        calls: Arc<StdMutex<Vec<(bool, usize)>>>,
    }

    #[async_trait]
    impl Provider for TwoAccountProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            let on_openai = *self.on_openai.lock().unwrap();
            self.calls.lock().unwrap().push((on_openai, messages.len()));
            if !on_openai {
                anyhow::bail!("HTTP 429: usage_limit_reached for this account");
            }
            let events = vec![
                Ok(StreamEvent::TextDelta("answered on backup".into())),
                Ok(StreamEvent::MessageEnd { stop_reason: None }),
            ];
            Ok(Box::pin(futures::stream::iter(events)))
        }
        fn name(&self) -> &str {
            if *self.on_openai.lock().unwrap() {
                "OpenAI"
            } else {
                "Claude"
            }
        }
        fn model(&self) -> String {
            if *self.on_openai.lock().unwrap() {
                "gpt-6.1-sol".into()
            } else {
                "claude-opus-5-5".into()
            }
        }
        fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
            Some(jcode_provider_core::ResolvedCredential::ApiKey)
        }
        fn set_model(&self, model: &str) -> anyhow::Result<()> {
            *self.on_openai.lock().unwrap() = model.starts_with("openai-api:");
            Ok(())
        }
        fn model_routes(&self) -> Vec<ModelRoute> {
            vec![ModelRoute {
                model: "gpt-6.1-sol".into(),
                provider: "OpenAI".into(),
                api_method: "openai-api".into(),
                available: true,
                detail: String::new(),
                cheapness: None,
                usage: None,
            }]
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(self.clone())
        }
    }

    #[test]
    fn usage_limit_switches_pool_account_and_retries_same_turn() {
        let _guard = crate::storage::lock_test_env();
        // Serialize process-wide environment outside the asynchronous runtime.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async {
                let home = tempfile::tempdir().unwrap();
                let prev_home = std::env::var_os("JCODE_HOME");
                crate::env::set_var("JCODE_HOME", home.path());
                crate::config::Config::invalidate_cache();
                let pool = crate::auth::account_pool::AccountPool {
                    members: vec!["anthropic-api".into(), "openai-api".into()],
                    ..Default::default()
                };
                pool.save().unwrap();

                let provider = TwoAccountProvider {
                    on_openai: Arc::new(StdMutex::new(false)),
                    calls: Arc::new(StdMutex::new(Vec::new())),
                };
                let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
                let registry = crate::tool::Registry::new(Arc::clone(&provider_dyn)).await;
                let session = crate::session::Session::create_with_id(
                    "session_server_failover".into(),
                    None,
                    None,
                );
                let agent = Arc::new(Mutex::new(Agent::new_with_session(
                    provider_dyn,
                    registry,
                    session,
                    None,
                )));
                let (tx, mut rx) = mpsc::unbounded_channel();

                let result = process_message_with_server_failover(
                    Arc::clone(&agent),
                    "hello",
                    Vec::new(),
                    None,
                    tx,
                    true,
                )
                .await;

                let mut events = Vec::new();
                while let Ok(event) = rx.try_recv() {
                    events.push(event);
                }
                let calls = provider.calls.lock().unwrap().clone();
                let user_messages = agent
                    .lock()
                    .await
                    .messages()
                    .iter()
                    .filter(|message| {
                        message.role == crate::message::Role::User
                            && message.content.iter().any(|block| {
                                matches!(block, crate::message::ContentBlock::Text { text, .. }
                                if text.contains("hello"))
                            })
                    })
                    .count();
                match prev_home {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
                crate::config::Config::invalidate_cache();

                assert!(
                    result.is_ok(),
                    "turn should succeed after failover: {result:?}"
                );
                assert_eq!(
                    calls.iter().map(|(openai, _)| *openai).collect::<Vec<_>>(),
                    vec![false, true],
                    "one failed request on the source, one retry on the target"
                );
                assert_eq!(calls[0].1, calls[1].1, "retry must resend the same history");
                assert_eq!(user_messages, 1, "the user message must not be duplicated");
                assert!(events.iter().any(|event| matches!(
                    event,
                    ServerEvent::ModelChanged { model, error: None, .. } if model == "gpt-6.1-sol"
                )));
                assert!(events.iter().any(|event| matches!(
                    event,
                    ServerEvent::ConnectionPhase { phase } if phase.starts_with("retrying on")
                )));
            });
    }

    #[test]
    fn server_initiated_turns_always_use_account_router() {
        let _guard = crate::storage::lock_test_env();
        // Serialize process-wide environment outside the asynchronous runtime.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async {
                let home = tempfile::tempdir().unwrap();
                let prev_home = std::env::var_os("JCODE_HOME");
                crate::env::set_var("JCODE_HOME", home.path());
                crate::config::Config::invalidate_cache();
                let pool = crate::auth::account_pool::AccountPool {
                    members: vec!["anthropic-api".into(), "openai-api".into()],
                    ..Default::default()
                };
                pool.save().unwrap();

                let provider = TwoAccountProvider {
                    on_openai: Arc::new(StdMutex::new(false)),
                    calls: Arc::new(StdMutex::new(Vec::new())),
                };
                let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
                let registry = crate::tool::Registry::new(Arc::clone(&provider_dyn)).await;
                let session = crate::session::Session::create_with_id(
                    "session_swarm_failover".into(),
                    None,
                    None,
                );
                let agent = Arc::new(Mutex::new(Agent::new_with_session(
                    provider_dyn,
                    registry,
                    session,
                    None,
                )));
                let (tx, _rx) = mpsc::unbounded_channel();
                let result = process_server_initiated_message(
                    Arc::clone(&agent),
                    "swarm task",
                    Vec::new(),
                    None,
                    tx,
                )
                .await;
                let calls = provider.calls.lock().unwrap().clone();
                match prev_home {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
                crate::config::Config::invalidate_cache();

                assert!(result.is_ok(), "worker turn should fail over: {result:?}");
                assert_eq!(
                    calls.iter().map(|(openai, _)| *openai).collect::<Vec<_>>(),
                    vec![false, true]
                );
            });
    }

    #[test]
    fn disabled_clients_keep_the_original_error() {
        let _guard = crate::storage::lock_test_env();
        // Serialize process-wide environment outside the asynchronous runtime.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async {
                let provider = TwoAccountProvider {
                    on_openai: Arc::new(StdMutex::new(false)),
                    calls: Arc::new(StdMutex::new(Vec::new())),
                };
                let provider_dyn: Arc<dyn Provider> = Arc::new(provider.clone());
                let registry = crate::tool::Registry::new(Arc::clone(&provider_dyn)).await;
                let agent = Arc::new(Mutex::new(Agent::new(provider_dyn, registry)));
                let (tx, _rx) = mpsc::unbounded_channel();
                let result = process_message_with_server_failover(
                    agent,
                    "hello",
                    Vec::new(),
                    None,
                    tx,
                    false,
                )
                .await;
                assert!(result.is_err());
                assert_eq!(provider.calls.lock().unwrap().len(), 1);
            });
    }

    #[test]
    fn context_and_generic_errors_do_not_switch_accounts() {
        // Context overflows are handled by compaction, not account switching.
        assert!(!error_wants_failover("context length exceeded"));
        assert!(!error_wants_failover("stream closed unexpectedly"));
    }
}
