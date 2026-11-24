//! Apply a complete native picker choice without touching the client's composer.

use crate::agent::Agent;
use crate::provider::RouteSelection;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Deserialize)]
struct Choice {
    selection: RouteSelection,
    reasoning_effort: Option<String>,
    client_id: Option<String>,
}

pub(super) async fn handle(
    raw: &str,
    session: Option<&str>,
    sessions: &super::super::SessionAgents,
    connections: &std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, super::ClientConnectionInfo>>,
    >,
    members: &std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, super::SwarmMember>>,
    >,
) -> Result<String> {
    let choice: Choice =
        serde_json::from_str(raw).context("Expected a native model picker choice")?;
    let session = session
        .filter(|session| !session.is_empty())
        .context("Select an explicit conversation for this model choice")?;
    let client = choice
        .client_id
        .as_deref()
        .context("Load client:model-options before applying a native choice")?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let result = {
        // Attachment transitions use this same connection -> session lock order.
        let connections = connections.read().await;
        let attached: Vec<_> = connections
            .values()
            .filter(|connection| {
                connection.session_id == session && connection.debug_client_id.is_some()
            })
            .collect();
        if attached.len() != 1 || attached[0].debug_client_id.as_deref() != Some(client) {
            bail!(
                "The selected jcode reconnected, changed conversation, or has another attached view. Reload its native model choices."
            );
        }
        let sessions = sessions.read().await;
        let current = sessions
            .get(session)
            .context("This conversation is no longer running")?;
        let mut agent = current.try_lock().map_err(|_| {
            anyhow::anyhow!("This jcode is busy. Retry the model change after its current turn.")
        })?;
        if agent.session_id() != session {
            bail!("This conversation changed. Reload the native model choices.");
        }
        apply(&mut agent, raw, &tx)
    };
    while let Ok(event) = rx.try_recv() {
        super::super::fanout_session_event(members, session, event).await;
    }
    result
}

pub(super) fn apply(
    agent: &mut Agent,
    raw: &str,
    tx: &tokio::sync::mpsc::UnboundedSender<crate::protocol::ServerEvent>,
) -> Result<String> {
    let choice: Choice =
        serde_json::from_str(raw).context("Expected a native model picker choice")?;
    if choice.reasoning_effort.as_deref().is_some_and(|effort| {
        effort.is_empty() || effort.len() > 64 || effort.chars().any(char::is_control)
    }) {
        bail!("Invalid reasoning level");
    }
    let offered = agent
        .model_routes()
        .into_iter()
        .filter(|route| route.available)
        .find(|route| {
            let route = RouteSelection::from_model_route(route);
            route.model == choice.selection.model
                && route.runtime_key == choice.selection.runtime_key
                && route.api_method == choice.selection.api_method
                && route.provider_label == choice.selection.provider_label
        })
        .context(
            "That model/provider route is no longer available. Reopen /models and choose again.",
        )?;
    let levels = crate::provider::model_route_reasoning_efforts(
        &offered.model,
        &offered.api_method,
        offered.catalog_metadata.as_ref(),
        crate::config::config(),
    );
    if choice
        .reasoning_effort
        .as_deref()
        .map_or(!levels.is_empty(), |effort| !levels.contains(&effort))
    {
        bail!("Choose a reasoning level offered for this model/provider route");
    }
    let offered = RouteSelection::from_model_route(&offered);
    agent.set_route_selection(&offered)?;
    agent.reset_provider_session();
    let effort_result = choice
        .reasoning_effort
        .as_deref()
        .map(|effort| agent.set_reasoning_effort(effort))
        .transpose();
    let output = serde_json::json!({
        "id": offered.picker_choice_id(),
        "model": agent.provider_model(),
        "provider": agent.provider_name(),
        "selection": offered,
        "reasoning_effort": agent.provider_reasoning_effort(),
    })
    .to_string();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    super::super::provider_control::send_model_changed_result(
        0,
        Ok((
            agent.provider_model(),
            agent.provider_name(),
            agent.active_resolved_credential(),
            agent.provider_reasoning_effort(),
        )),
        agent.provider_model(),
        agent,
        &event_tx,
    );
    while let Ok(mut event) = event_rx.try_recv() {
        if let crate::protocol::ServerEvent::ModelChanged {
            route_selection, ..
        } = &mut event
        {
            *route_selection = Some(offered.clone());
        }
        tx.send(event)
            .context("Model/provider changed, but publishing its updated state failed")?;
    }
    effort_result.with_context(|| format!("Model/provider changed to {} via {}; actual reasoning is {}. Applying or saving the reasoning setting failed", agent.provider_model(), offered.api_method, agent.provider_reasoning_effort().as_deref().unwrap_or("none")))?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{EventStream, ModelRoute, Provider};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct TestProvider(Mutex<(Option<RouteSelection>, Option<String>)>);

    fn route(method: &str) -> ModelRoute {
        ModelRoute {
            model: "gpt-future-catalog-2099".into(),
            provider: "test".into(),
            api_method: method.into(),
            available: true,
            detail: String::new(),
            catalog_metadata: None,
            cheapness: None,
            usage: None,
        }
    }

    #[async_trait]
    impl Provider for TestProvider {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<EventStream> {
            bail!("model control must never run inference")
        }
        fn name(&self) -> &str {
            "test"
        }
        fn model(&self) -> String {
            self.0
                .lock()
                .unwrap()
                .0
                .as_ref()
                .map_or("before", |route| &route.model)
                .to_string()
        }
        fn model_routes(&self) -> Vec<ModelRoute> {
            vec![route("openai-oauth"), route("openai-api-key")]
        }
        fn set_route_selection(&self, selection: &RouteSelection) -> Result<()> {
            self.0.lock().unwrap().0 = Some(selection.clone());
            Ok(())
        }
        fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
            if !["low", "high"].contains(&effort) {
                bail!("unsupported level");
            }
            self.0.lock().unwrap().1 = Some(effort.into());
            if effort == "low" {
                bail!("simulated persistence failure");
            }
            Ok(())
        }
        fn reasoning_effort(&self) -> Option<String> {
            self.0.lock().unwrap().1.clone()
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self::default())
        }
    }

    #[tokio::test]
    async fn native_model_choice_keeps_route_and_reasoning_together_and_rejects_missing_routes() {
        let _env = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                if let Some(home) = &self.0 {
                    crate::env::set_var("JCODE_HOME", home);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
                crate::config::Config::invalidate_cache();
            }
        }
        let _home = RestoreHome(std::env::var_os("JCODE_HOME"));
        crate::env::set_var("JCODE_HOME", home.path());
        crate::config::Config::invalidate_cache();
        let provider = Arc::new(TestProvider::default());
        let registry = crate::tool::Registry::new(provider.clone()).await;
        let mut agent =
            Agent::new_with_initial_working_dir(provider.clone(), registry, home.path().to_str());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let selection = RouteSelection::from_model_route(&route("openai-api-key"));
        let mut choice = serde_json::json!({ "selection": selection, "reasoning_effort": "high" });
        let output: serde_json::Value =
            serde_json::from_str(&apply(&mut agent, &choice.to_string(), &tx).unwrap()).unwrap();
        assert_eq!(output["selection"], choice["selection"]);
        assert_eq!(output["reasoning_effort"], "high");
        assert_eq!(output["id"], selection.picker_choice_id());
        assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(event, crate::protocol::ServerEvent::ModelChanged { reasoning_effort: Some(effort), .. } if effort == "high")));
        assert_eq!(
            provider.0.lock().unwrap().0.as_ref().unwrap().api_method,
            "openai-api-key"
        );
        choice["selection"]["model"] = "not-offered".into();
        assert!(
            apply(&mut agent, &choice.to_string(), &tx)
                .unwrap_err()
                .to_string()
                .contains("no longer available")
        );
        assert_eq!(agent.provider_model(), "gpt-future-catalog-2099");
        choice["selection"] =
            serde_json::to_value(RouteSelection::from_model_route(&route("openai-oauth"))).unwrap();
        for level in ["unsupported", "swarm"] {
            choice["reasoning_effort"] = level.into();
            assert!(
                apply(&mut agent, &choice.to_string(), &tx)
                    .unwrap_err()
                    .to_string()
                    .contains("reasoning level offered")
            );
            assert_eq!(
                provider.0.lock().unwrap().0.as_ref().unwrap().api_method,
                "openai-api-key",
                "invalid levels cannot change the route"
            );
        }
        choice["reasoning_effort"] = "low".into();
        assert!(
            apply(&mut agent, &choice.to_string(), &tx)
                .unwrap_err()
                .to_string()
                .contains("actual reasoning is low")
        );
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(
                |event| matches!(event, crate::protocol::ServerEvent::ModelChanged {
            route_selection: Some(route), reasoning_effort: Some(effort), ..
        } if route.api_method == "openai-oauth" && effort == "low")
            ),
            "a partial failure publishes the actual changed route and level"
        );

        let session = agent.session_id().to_owned();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));
        let sessions = Arc::new(super::super::super::session_manager::SessionManager::new());
        sessions
            .write_for_test()
            .await
            .insert(session.clone(), agent.clone());
        let connections = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
            (
                "connection".into(),
                super::super::ClientConnectionInfo {
                    client_id: "connection".into(),
                    session_id: session.clone(),
                    client_instance_id: None,
                    debug_client_id: Some("native-token".into()),
                    connected_at: std::time::Instant::now(),
                    last_seen: std::time::Instant::now(),
                    is_processing: false,
                    current_tool_name: None,
                    terminal_env: Vec::new(),
                    supports_user_response: false,
                    ask_user_tx: None,
                    disconnect_tx: tokio::sync::mpsc::unbounded_channel().0,
                },
            ),
        ])));
        let members = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        choice["selection"] = serde_json::to_value(selection).unwrap();
        choice["reasoning_effort"] = "high".into();
        assert!(
            handle(
                &choice.to_string(),
                Some(&session),
                &sessions,
                &connections,
                &members
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("Load client:model-options")
        );
        choice["client_id"] = "native-token".into();
        assert!(
            handle(&choice.to_string(), None, &sessions, &connections, &members)
                .await
                .is_err()
        );
        let busy = agent.lock().await;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                handle(
                    &choice.to_string(),
                    Some(&session),
                    &sessions,
                    &connections,
                    &members
                )
            )
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("busy")
        );
        drop(busy);
        tokio::task::yield_now().await;
        assert_eq!(
            provider.0.lock().unwrap().0.as_ref().unwrap().api_method,
            "openai-oauth",
            "a busy refusal must never queue a later mutation"
        );
        connections
            .write()
            .await
            .get_mut("connection")
            .unwrap()
            .debug_client_id = Some("reconnected-token".into());
        assert!(
            handle(
                &choice.to_string(),
                Some(&session),
                &sessions,
                &connections,
                &members
            )
            .await
            .is_err()
        );
        choice["client_id"] = "reconnected-token".into();
        connections
            .write()
            .await
            .get_mut("connection")
            .unwrap()
            .session_id = "another-conversation".into();
        assert!(
            handle(
                &choice.to_string(),
                Some(&session),
                &sessions,
                &connections,
                &members
            )
            .await
            .is_err()
        );
        connections
            .write()
            .await
            .get_mut("connection")
            .unwrap()
            .session_id = session.clone();
        handle(
            &choice.to_string(),
            Some(&session),
            &sessions,
            &connections,
            &members,
        )
        .await
        .unwrap();
        assert_eq!(
            provider.0.lock().unwrap().0.as_ref().unwrap().api_method,
            "openai-api-key"
        );
    }
}
