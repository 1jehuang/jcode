use super::client_state::{send_history, spawn_model_prefetch_update};
use super::{
    ClientConnectionInfo, FileAccess, SessionInterruptQueues, SwarmEvent, SwarmMember,
    VersionedPlan, broadcast_swarm_status, register_session_interrupt_queue,
    remove_plan_participant, remove_session_channel_subscriptions, remove_session_file_touches,
    remove_session_interrupt_queue, rename_plan_participant, rename_session_interrupt_queue,
    swarm_id_for_dir, update_member_status,
};
use crate::agent::Agent;
use crate::message::ContentBlock;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::tool::Registry;
use crate::transport::WriteHalf;
use anyhow::Result;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

fn session_was_interrupted_by_reload(agent: &Agent) -> bool {
    let messages = agent.messages();
    let Some(last) = messages.last() else {
        return false;
    };

    last.content.iter().any(|block| match block {
        ContentBlock::Text { text, .. } => {
            text.ends_with("[generation interrupted - server reloading]")
        }
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            is_error.unwrap_or(false)
                && (content.contains("interrupted by server reload")
                    || content.contains("Skipped - server reloading"))
        }
        _ => false,
    })
}

fn mark_remote_reload_started(request_id: &str) {
    crate::server::write_reload_state(
        request_id,
        env!("JCODE_VERSION"),
        crate::server::ReloadPhase::Starting,
        None,
    );
}

async fn rename_shutdown_signal(
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    old_session_id: &str,
    new_session_id: &str,
) {
    if old_session_id == new_session_id {
        return;
    }

    let mut signals = shutdown_signals.write().await;
    if let Some(signal) = signals.remove(old_session_id) {
        signals.insert(new_session_id.to_string(), signal);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_clear_session(
    id: u64,
    client_selfdev: bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let preserve_debug = {
        let agent_guard = agent.lock().await;
        agent_guard.is_debug()
    };

    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let mut new_agent = Agent::new(Arc::clone(provider), registry.clone());
    let new_id = new_agent.session_id().to_string();

    if client_selfdev {
        new_agent.set_canary("self-dev");
    }
    if preserve_debug {
        new_agent.set_debug(true);
    }

    let mut agent_guard = agent.lock().await;
    *agent_guard = new_agent;
    drop(agent_guard);

    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.remove(client_session_id);
        sessions_guard.insert(new_id.clone(), Arc::clone(agent));
    }
    {
        let agent_guard = agent.lock().await;
        register_session_interrupt_queue(
            soft_interrupt_queues,
            &new_id,
            agent_guard.soft_interrupt_queue(),
        )
        .await;
    }
    remove_session_interrupt_queue(soft_interrupt_queues, client_session_id).await;

    let swarm_id_for_update = {
        let mut members = swarm_members.write().await;
        if let Some(mut member) = members.remove(client_session_id) {
            let swarm_id = member.swarm_id.clone();
            member.session_id = new_id.clone();
            member.status = "ready".to_string();
            member.detail = None;
            members.insert(new_id.clone(), member);
            swarm_id
        } else {
            None
        }
    };
    if let Some(ref swarm_id) = swarm_id_for_update {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(client_session_id);
            swarm.insert(new_id.clone());
        }
    }
    remove_session_file_touches(client_session_id, file_touches, files_touched_by_session).await;
    remove_session_channel_subscriptions(
        client_session_id,
        channel_subscriptions,
        channel_subscriptions_by_session,
    )
    .await;
    update_member_status(
        &new_id,
        "ready",
        None,
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;
    if let Some(swarm_id) = swarm_id_for_update {
        rename_plan_participant(&swarm_id, client_session_id, &new_id, swarm_plans).await;
    }

    *client_session_id = new_id.clone();
    {
        let mut connections = client_connections.write().await;
        if let Some(info) = connections.get_mut(client_connection_id) {
            info.session_id = new_id.clone();
            info.last_seen = Instant::now();
        }
    }
    let _ = client_event_tx.send(ServerEvent::SessionId { session_id: new_id });
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_subscribe(
    id: u64,
    subscribe_working_dir: Option<String>,
    selfdev: Option<bool>,
    client_selfdev: &mut bool,
    client_session_id: &str,
    _friendly_name: &Option<String>,
    agent: &Arc<Mutex<Agent>>,
    registry: &Registry,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
) {
    if let Some(ref dir) = subscribe_working_dir {
        let mut agent_guard = agent.lock().await;
        agent_guard.set_working_dir(dir);
        drop(agent_guard);

        let new_path = PathBuf::from(dir);
        let new_swarm_id = swarm_id_for_dir(Some(new_path.clone()));
        let mut old_swarm_id: Option<String> = None;
        let mut updated_swarm_id: Option<String> = None;
        {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                old_swarm_id = member.swarm_id.clone();
                member.working_dir = Some(new_path);
                member.swarm_id = if member.swarm_enabled {
                    new_swarm_id.clone()
                } else {
                    None
                };
                updated_swarm_id = member.swarm_id.clone();
            }
        }

        if let Some(ref old_id) = old_swarm_id {
            if updated_swarm_id.as_ref() != Some(old_id) {
                remove_session_channel_subscriptions(
                    client_session_id,
                    channel_subscriptions,
                    channel_subscriptions_by_session,
                )
                .await;
            }
            let mut swarms = swarms_by_id.write().await;
            if let Some(swarm) = swarms.get_mut(old_id) {
                swarm.remove(client_session_id);
                if swarm.is_empty() {
                    swarms.remove(old_id);
                }
            }
        }

        if let Some(ref new_id) = updated_swarm_id {
            let mut swarms = swarms_by_id.write().await;
            swarms
                .entry(new_id.clone())
                .or_insert_with(HashSet::new)
                .insert(client_session_id.to_string());
        }

        if updated_swarm_id != old_swarm_id {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.get_mut(client_session_id) {
                member.role = "agent".to_string();
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(&old_id)
                    .map(|session_id| session_id == client_session_id)
                    .unwrap_or(false)
            };
            if was_coordinator {
                let mut new_coordinator: Option<String> = None;
                {
                    let swarms = swarms_by_id.read().await;
                    if let Some(swarm) = swarms.get(&old_id) {
                        new_coordinator = swarm.iter().min().cloned();
                    }
                }
                {
                    let mut coordinators = swarm_coordinators.write().await;
                    coordinators.remove(&old_id);
                    if let Some(ref new_id) = new_coordinator {
                        coordinators.insert(old_id.clone(), new_id.clone());
                    }
                }
                if let Some(new_id) = new_coordinator.clone() {
                    let members = swarm_members.read().await;
                    if let Some(member) = members.get(&new_id) {
                        let _ = member.event_tx.send(ServerEvent::Notification {
                            from_session: new_id.clone(),
                            from_name: member.friendly_name.clone(),
                            notification_type: NotificationType::Message {
                                scope: Some("swarm".to_string()),
                                channel: None,
                            },
                            message: "You are now the coordinator for this swarm.".to_string(),
                        });
                    }
                }
            }
        }

        if let Some(old_id) = old_swarm_id.clone() {
            if updated_swarm_id.as_ref() != Some(&old_id) {
                remove_plan_participant(&old_id, client_session_id, swarm_plans).await;
            }
            broadcast_swarm_status(&old_id, swarm_members, swarms_by_id).await;
        }
        if let Some(new_id) = updated_swarm_id {
            if old_swarm_id.as_ref() != Some(&new_id) {
                broadcast_swarm_status(&new_id, swarm_members, swarms_by_id).await;
            }
        }
    }

    let should_selfdev = *client_selfdev || matches!(selfdev, Some(true));

    if should_selfdev {
        *client_selfdev = true;
        let mut agent_guard = agent.lock().await;
        if !agent_guard.is_canary() {
            agent_guard.set_canary("self-dev");
        }
        drop(agent_guard);
        registry.register_selfdev_tools().await;
    }

    registry
        .register_mcp_tools(
            Some(client_event_tx.clone()),
            Some(Arc::clone(mcp_pool)),
            Some(client_session_id.to_string()),
        )
        .await;

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

pub(super) async fn handle_reload(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let request_id = crate::id::new_id("reload");
    mark_remote_reload_started(&request_id);
    let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });

    let (triggering_session, prefer_selfdev_binary) = {
        let agent_guard = agent.lock().await;
        (
            Some(agent_guard.session_id().to_string()),
            agent_guard.is_canary(),
        )
    };
    let hash = env!("JCODE_GIT_HASH").to_string();
    let signal_request_id =
        crate::server::send_reload_signal(hash, triggering_session.clone(), prefer_selfdev_binary);

    crate::logging::info(&format!(
        "handle_reload: queued reload signal {} from remote client request {} (triggering_session={:?}, prefer_selfdev_binary={})",
        signal_request_id, request_id, triggering_session, prefer_selfdev_binary
    ));

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_resume_session(
    id: u64,
    session_id: String,
    client_selfdev: &mut bool,
    client_session_id: &mut String,
    client_connection_id: &str,
    agent: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    shutdown_signals: &Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    file_touches: &Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: &Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    channel_subscriptions_by_session: &Arc<
        RwLock<HashMap<String, HashMap<String, HashSet<String>>>>,
    >,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    client_count: &Arc<RwLock<usize>>,
    writer: &Arc<Mutex<WriteHalf>>,
    server_name: &str,
    server_icon: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) -> Result<()> {
    {
        let mut agent_guard = agent.lock().await;
        agent_guard.mark_closed();
    }

    let (result, is_canary) = {
        let mut agent_guard = agent.lock().await;
        let result = agent_guard.restore_session(&session_id);
        if *client_selfdev {
            agent_guard.set_canary("self-dev");
        }
        let is_canary = agent_guard.is_canary();
        (result, is_canary)
    };

    let was_interrupted = match &result {
        Ok(status) => match status {
            crate::session::SessionStatus::Crashed { .. } => true,
            crate::session::SessionStatus::Active => {
                let agent_guard = agent.lock().await;
                let last_role = agent_guard.last_message_role();
                let last_is_user = last_role
                    .as_ref()
                    .map(|role| *role == crate::message::Role::User)
                    .unwrap_or(false);
                let last_is_reload_interrupted = session_was_interrupted_by_reload(&agent_guard);
                if last_is_user {
                    crate::logging::info(&format!(
                        "Session {} was Active with pending user message - treating as interrupted",
                        session_id
                    ));
                }
                if last_is_reload_interrupted {
                    crate::logging::info(&format!(
                        "Session {} was interrupted by reload - will auto-resume",
                        session_id
                    ));
                }
                last_is_user || last_is_reload_interrupted
            }
            _ => false,
        },
        Err(_) => false,
    };

    if result.is_ok() && is_canary {
        *client_selfdev = true;
        registry.register_selfdev_tools().await;
    }

    if result.is_ok() {
        registry
            .register_mcp_tools(
                Some(client_event_tx.clone()),
                Some(Arc::clone(mcp_pool)),
                Some(client_session_id.clone()),
            )
            .await;
    }

    match result {
        Ok(_prev_status) => {
            let old_session_id = client_session_id.clone();
            *client_session_id = session_id.clone();

            {
                let mut sessions_guard = sessions.write().await;
                sessions_guard.remove(&old_session_id);
                sessions_guard.insert(session_id.clone(), Arc::clone(agent));
            }
            rename_shutdown_signal(shutdown_signals, &old_session_id, &session_id).await;
            rename_session_interrupt_queue(soft_interrupt_queues, &old_session_id, &session_id)
                .await;
            {
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(client_connection_id) {
                    info.session_id = session_id.clone();
                    info.last_seen = Instant::now();
                }
            }

            {
                let mut members = swarm_members.write().await;
                if let Some(mut member) = members.remove(&old_session_id) {
                    if let Some(ref swarm_id) = member.swarm_id {
                        let mut swarms = swarms_by_id.write().await;
                        if let Some(swarm) = swarms.get_mut(swarm_id) {
                            swarm.remove(&old_session_id);
                            swarm.insert(session_id.clone());
                        }
                    }
                    member.session_id = session_id.clone();
                    member.status = "ready".to_string();
                    member.detail = None;
                    members.insert(session_id.clone(), member);
                }
            }
            remove_session_channel_subscriptions(
                &old_session_id,
                channel_subscriptions,
                channel_subscriptions_by_session,
            )
            .await;
            remove_session_file_touches(&old_session_id, file_touches, files_touched_by_session)
                .await;
            {
                let mut coordinators = swarm_coordinators.write().await;
                for coordinator in coordinators.values_mut() {
                    if *coordinator == old_session_id {
                        *coordinator = session_id.clone();
                    }
                }
            }
            update_member_status(
                &session_id,
                "ready",
                None,
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
            if let Some(swarm_id) = {
                let members = swarm_members.read().await;
                members
                    .get(&session_id)
                    .and_then(|member| member.swarm_id.clone())
            } {
                rename_plan_participant(&swarm_id, &old_session_id, &session_id, swarm_plans).await;
            }

            send_history(
                id,
                &session_id,
                agent,
                sessions,
                client_count,
                writer,
                server_name,
                server_icon,
                if was_interrupted { Some(true) } else { None },
            )
            .await?;
            spawn_model_prefetch_update(
                Arc::clone(provider),
                Arc::clone(agent),
                Arc::clone(writer),
            );
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "Failed to restore session: {}",
                    crate::util::format_error_chain(&error)
                ),
                retry_after_secs: None,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        handle_reload, mark_remote_reload_started, rename_shutdown_signal,
        session_was_interrupted_by_reload,
    };
    use crate::agent::Agent;
    use crate::message::ContentBlock;
    use crate::message::{Message, ToolDefinition};
    use crate::protocol::ServerEvent;
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use jcode_agent_runtime::InterruptSignal;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock, mpsc};

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    fn test_agent(messages: Vec<crate::session::StoredMessage>) -> Agent {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let registry = rt.block_on(Registry::new(provider.clone()));
        build_test_agent(provider, registry, messages)
    }

    fn build_test_agent(
        provider: Arc<dyn Provider>,
        registry: Registry,
        messages: Vec<crate::session::StoredMessage>,
    ) -> Agent {
        let mut session =
            crate::session::Session::create_with_id("session_test_reload".to_string(), None, None);
        session.model = Some("mock".to_string());
        session.replace_messages(messages);
        Agent::new_with_session(provider, registry, session, None)
    }

    #[test]
    fn detects_reload_interrupted_generation_text() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_1".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "partial\n\n[generation interrupted - server reloading]".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn detects_reload_interrupted_tool_result() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_2".to_string(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
                is_error: Some(true),
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn detects_reload_skipped_tool_result() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_3".to_string(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_2".to_string(),
                content: "[Skipped - server reloading]".to_string(),
                is_error: Some(true),
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn ignores_normal_tool_errors() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_4".to_string(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_3".to_string(),
                content: "Error: file not found".to_string(),
                is_error: Some(true),
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(!session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn mark_remote_reload_started_writes_starting_marker() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

        mark_remote_reload_started("reload-test");

        let state = crate::server::recent_reload_state(std::time::Duration::from_secs(5))
            .expect("reload state should exist");
        assert_eq!(state.request_id, "reload-test");
        assert_eq!(state.phase, crate::server::ReloadPhase::Starting);

        crate::server::clear_reload_marker();
        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[test]
    fn handle_reload_queues_signal_for_canary_session() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let mut rx = crate::server::subscribe_reload_signal_for_tests();
            let provider: Arc<dyn Provider> = Arc::new(MockProvider);
            let registry = Registry::new(provider.clone()).await;
            let mut agent = build_test_agent(provider, registry, Vec::new());
            agent.set_canary("self-dev");
            let agent = Arc::new(Mutex::new(agent));
            let (tx, mut events) = mpsc::unbounded_channel::<ServerEvent>();

            handle_reload(7, &agent, &tx).await;

            let reloading = events.recv().await.expect("reloading event");
            assert!(matches!(reloading, ServerEvent::Reloading { .. }));
            let done = events.recv().await.expect("done event");
            assert!(matches!(done, ServerEvent::Done { id: 7 }));

            tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
                .await
                .expect("reload signal timeout")
                .expect("reload signal should be delivered");
            let signal = rx
                .borrow_and_update()
                .clone()
                .expect("reload signal payload should exist");
            assert_eq!(
                signal.triggering_session.as_deref(),
                Some("session_test_reload")
            );
            assert!(signal.prefer_selfdev_binary);
            assert_eq!(signal.hash, env!("JCODE_GIT_HASH"));

            let state = crate::server::recent_reload_state(std::time::Duration::from_secs(5))
                .expect("reload state should exist");
            assert_eq!(state.phase, crate::server::ReloadPhase::Starting);
        });

        crate::server::clear_reload_marker();
        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn rename_shutdown_signal_moves_registration_to_restored_session() {
        let signal = InterruptSignal::new();
        let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
            "session_old".to_string(),
            signal.clone(),
        )])));

        rename_shutdown_signal(&shutdown_signals, "session_old", "session_restored").await;

        let signals = shutdown_signals.read().await;
        assert!(!signals.contains_key("session_old"));
        let renamed = signals
            .get("session_restored")
            .expect("restored session should retain shutdown signal");
        renamed.fire();
        assert!(signal.is_set());
    }
}
