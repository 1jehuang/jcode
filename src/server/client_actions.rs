use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    ClientConnectionInfo, SessionInterruptQueues, SwarmEvent, SwarmMember, VersionedPlan,
    broadcast_swarm_status, queue_soft_interrupt_for_session, remove_session_channel_subscriptions,
    remove_session_from_swarm, swarm_id_for_dir, truncate_detail, update_member_status,
};
use crate::agent::{Agent, SoftInterruptSource, StreamError};
use crate::protocol::{FeatureToggle, NotificationType, ServerEvent};
use crate::session::Session;
use crate::util::truncate_str;
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

const INPUT_SHELL_MAX_OUTPUT_LEN: usize = 30_000;

fn derive_subagent_description(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(4).collect();
    if words.is_empty() {
        "Manual subagent".to_string()
    } else {
        words.join(" ")
    }
}

fn build_input_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(command);
        cmd
    }
}

fn combine_input_shell_output(stdout: &[u8], stderr: &[u8]) -> (String, bool) {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let mut output = String::new();

    if !stdout.is_empty() {
        output.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("[stderr]\n");
        output.push_str(&stderr);
    }

    let truncated = if output.len() > INPUT_SHELL_MAX_OUTPUT_LEN {
        output = truncate_str(&output, INPUT_SHELL_MAX_OUTPUT_LEN).to_string();
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("… output truncated");
        true
    } else {
        false
    };

    (output, truncated)
}

pub(super) async fn handle_notify_session(
    id: u64,
    session_id: String,
    message: String,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let target_has_client = {
        let connections = client_connections.read().await;
        connections
            .values()
            .any(|connection| connection.session_id == session_id)
    };

    let notified = {
        let members = swarm_members.read().await;
        if let Some(member) = members.get(&session_id) {
            member
                .event_tx
                .send(ServerEvent::Notification {
                    from_session: "schedule".to_string(),
                    from_name: Some("scheduled task".to_string()),
                    notification_type: NotificationType::Message {
                        scope: Some("scheduled".to_string()),
                        channel: None,
                    },
                    message: message.clone(),
                })
                .is_ok()
        } else {
            false
        }
    };

    let queued_interrupt = if target_has_client {
        false
    } else {
        queue_soft_interrupt_for_session(
            &session_id,
            message.clone(),
            false,
            SoftInterruptSource::System,
            soft_interrupt_queues,
            sessions,
        )
        .await
    };

    if notified || queued_interrupt {
        let _ = client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Session '{}' is not currently live", session_id),
            retry_after_secs: None,
        });
    }
}

pub(super) fn handle_input_shell(
    id: u64,
    command: String,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let agent = Arc::clone(agent);
    let tx = client_event_tx.clone();

    tokio::spawn(async move {
        let cwd = {
            let agent_guard = agent.lock().await;
            agent_guard.working_dir().map(|dir| dir.to_string())
        };

        let started = Instant::now();
        let mut cmd = build_input_shell_command(&command);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd.as_ref() {
            cmd.current_dir(dir);
        }

        let result = match cmd.output().await {
            Ok(output) => {
                let (combined_output, truncated) =
                    combine_input_shell_output(&output.stdout, &output.stderr);
                crate::message::InputShellResult {
                    command,
                    cwd,
                    output: combined_output,
                    exit_code: output.status.code(),
                    duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    truncated,
                    failed_to_start: false,
                }
            }
            Err(error) => crate::message::InputShellResult {
                command,
                cwd,
                output: format!("Failed to run command: {}", error),
                exit_code: None,
                duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                truncated: false,
                failed_to_start: true,
            },
        };

        let _ = tx.send(ServerEvent::InputShellResult { result });
        let _ = tx.send(ServerEvent::Done { id });
    });
}

pub(super) async fn handle_set_subagent_model(
    id: u64,
    model: Option<String>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let mut agent_guard = agent.lock().await;
    match agent_guard.set_subagent_model(model) {
        Ok(()) => {
            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: crate::util::format_error_chain(&error),
                retry_after_secs: None,
            });
        }
    }
}

pub(super) fn handle_run_subagent(
    id: u64,
    prompt: String,
    subagent_type: String,
    model: Option<String>,
    session_id: Option<String>,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let agent = Arc::clone(agent);
    let tx = client_event_tx.clone();

    tokio::spawn(async move {
        let description = derive_subagent_description(&prompt);
        let tool_call_id = crate::id::new_id("call");
        let tool_name = "subagent".to_string();
        let tool_input = serde_json::json!({
            "description": description,
            "prompt": prompt,
            "subagent_type": subagent_type,
            "model": model,
            "session_id": session_id,
            "command": "/subagent",
        });

        let message_id = {
            let mut agent_guard = agent.lock().await;
            match agent_guard.add_manual_tool_use(
                tool_call_id.clone(),
                tool_name.clone(),
                tool_input.clone(),
            ) {
                Ok(message_id) => message_id,
                Err(error) => {
                    let _ = tx.send(ServerEvent::Error {
                        id,
                        message: crate::util::format_error_chain(&error),
                        retry_after_secs: None,
                    });
                    return;
                }
            }
        };

        let _ = tx.send(ServerEvent::ToolStart {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
        });
        let _ = tx.send(ServerEvent::ToolInput {
            delta: tool_input.to_string(),
        });
        let _ = tx.send(ServerEvent::ToolExec {
            id: tool_call_id.clone(),
            name: tool_name.clone(),
        });

        let (registry, session_id, working_dir) = {
            let agent_guard = agent.lock().await;
            (
                agent_guard.registry(),
                agent_guard.session_id().to_string(),
                agent_guard.working_dir().map(std::path::PathBuf::from),
            )
        };

        let ctx = crate::tool::ToolContext {
            session_id,
            message_id,
            tool_call_id: tool_call_id.clone(),
            working_dir,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };

        let started = Instant::now();
        let tool_name_for_exec = tool_name.clone();
        let result = match tokio::spawn(async move {
            registry.execute(&tool_name_for_exec, tool_input, ctx).await
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(anyhow::anyhow!("Tool task panicked: {}", error)),
        };
        let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

        match result {
            Ok(output) => {
                let output_text = output.output.clone();
                let _ = tx.send(ServerEvent::ToolDone {
                    id: tool_call_id.clone(),
                    name: tool_name,
                    output: output_text,
                    error: None,
                });
                let persist = {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.add_manual_tool_result(tool_call_id, output, duration_ms)
                };
                if let Err(error) = persist {
                    let _ = tx.send(ServerEvent::Error {
                        id,
                        message: crate::util::format_error_chain(&error),
                        retry_after_secs: None,
                    });
                    return;
                }
                let _ = tx.send(ServerEvent::Done { id });
            }
            Err(error) => {
                let error_msg = format!("Error: {}", error);
                let _ = tx.send(ServerEvent::ToolDone {
                    id: tool_call_id.clone(),
                    name: tool_name,
                    output: error_msg.clone(),
                    error: Some(error_msg.clone()),
                });
                let persist = {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.add_manual_tool_error(tool_call_id, error_msg, duration_ms)
                };
                if let Err(persist_error) = persist {
                    let _ = tx.send(ServerEvent::Error {
                        id,
                        message: crate::util::format_error_chain(&persist_error),
                        retry_after_secs: None,
                    });
                    return;
                }
                let _ = tx.send(ServerEvent::Done { id });
            }
        }
    });
}

pub(super) async fn handle_set_feature(
    id: u64,
    feature: FeatureToggle,
    enabled: bool,
    agent: &Arc<Mutex<Agent>>,
    client_session_id: &str,
    _friendly_name: &Option<String>,
    swarm_enabled: &mut bool,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    channel_subscriptions: &Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    match feature {
        FeatureToggle::Memory => {
            let mut agent_guard = agent.lock().await;
            agent_guard.set_memory_enabled(enabled);
            drop(agent_guard);
            if !enabled {
                crate::memory::clear_pending_memory(client_session_id);
            }
            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        FeatureToggle::Swarm => {
            if *swarm_enabled == enabled {
                let _ = client_event_tx.send(ServerEvent::Done { id });
                return;
            }
            *swarm_enabled = enabled;

            let (old_swarm_id, working_dir) = {
                let mut members = swarm_members.write().await;
                if let Some(member) = members.get_mut(client_session_id) {
                    let old = member.swarm_id.clone();
                    let wd = member.working_dir.clone();
                    member.swarm_enabled = enabled;
                    if !enabled {
                        member.swarm_id = None;
                        member.role = "agent".to_string();
                    }
                    (old, wd)
                } else {
                    (None, None)
                }
            };

            if let Some(ref old_id) = old_swarm_id {
                remove_session_from_swarm(
                    client_session_id,
                    old_id,
                    swarm_members,
                    swarms_by_id,
                    swarm_coordinators,
                    swarm_plans,
                )
                .await;
                remove_session_channel_subscriptions(client_session_id, channel_subscriptions)
                    .await;
            }

            if enabled {
                let new_swarm_id = swarm_id_for_dir(working_dir);
                if let Some(ref id) = new_swarm_id {
                    {
                        let mut swarms = swarms_by_id.write().await;
                        swarms
                            .entry(id.clone())
                            .or_insert_with(HashSet::new)
                            .insert(client_session_id.to_string());
                    }

                    {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.get_mut(client_session_id) {
                            member.swarm_id = Some(id.clone());
                            member.role = "agent".to_string();
                        }
                    }

                    broadcast_swarm_status(id, swarm_members, swarms_by_id).await;
                } else {
                    let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                        members: Vec::new(),
                    });
                }
            } else {
                let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                    members: Vec::new(),
                });
            }

            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
    }
}

pub(super) async fn handle_trigger_memory_extraction(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let extraction = {
        let agent_guard = agent.lock().await;
        if !agent_guard.memory_enabled() {
            None
        } else {
            let transcript = agent_guard.build_transcript_for_extraction();
            if transcript.len() < 200 {
                None
            } else {
                Some((
                    transcript,
                    agent_guard.session_id().to_string(),
                    agent_guard.working_dir().map(|dir| dir.to_string()),
                ))
            }
        }
    };

    if let Some((transcript, session_id, working_dir)) = extraction {
        crate::memory_agent::trigger_final_extraction_with_dir(transcript, session_id, working_dir);
    }

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

fn clone_split_session(parent_session_id: &str) -> anyhow::Result<(String, String)> {
    let parent = Session::load(parent_session_id)?;

    let mut child = Session::create(Some(parent_session_id.to_string()), None);
    child.replace_messages(parent.messages.clone());
    child.compaction = parent.compaction.clone();
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    child.status = crate::session::SessionStatus::Closed;
    child.save()?;

    let name = child.display_name().to_string();
    Ok((child.id.clone(), name))
}

pub(super) async fn handle_split(
    id: u64,
    client_session_id: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let (new_session_id, new_session_name) = match clone_split_session(client_session_id) {
        Ok(result) => result,
        Err(e) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to save split session: {e}"),
                retry_after_secs: None,
            });
            return;
        }
    };

    let _ = client_event_tx.send(ServerEvent::SplitResponse {
        id,
        new_session_id,
        new_session_name,
    });
}

#[cfg(test)]
mod tests {
    use super::{clone_split_session, handle_notify_session, handle_set_feature};
    use crate::agent::Agent;
    use crate::message::{ContentBlock, Role};
    use crate::protocol::{FeatureToggle, NotificationType, ServerEvent};
    use crate::provider::{EventStream, Provider};
    use crate::server::{ClientConnectionInfo, SwarmMember};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{Mutex, RwLock, mpsc};

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
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

    #[test]
    fn clone_split_session_uses_persisted_session_state() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let mut parent = crate::session::Session::create_with_id(
            "session_parent_split_test".to_string(),
            None,
            None,
        );
        parent.working_dir = Some("/tmp/jcode-split-test".to_string());
        parent.model = Some("gpt-test".to_string());
        parent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello from parent".to_string(),
                cache_control: None,
            }],
        );
        parent.compaction = Some(crate::session::StoredCompactionState {
            summary_text: "summary".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 1,
            original_turn_count: 1,
            compacted_count: 1,
        });
        parent.save().expect("save parent");

        let (child_id, _child_name) = clone_split_session(&parent.id).expect("clone split");
        let child = crate::session::Session::load(&child_id).expect("load child");

        assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(child.messages.len(), parent.messages.len());
        assert_eq!(
            child.messages[0].content_preview(),
            parent.messages[0].content_preview()
        );
        assert_eq!(child.compaction, parent.compaction);
        assert_eq!(child.working_dir, parent.working_dir);
        assert_eq!(child.model, parent.model);
        assert_eq!(child.status, crate::session::SessionStatus::Closed);
        assert_ne!(child.id, parent.id);

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[tokio::test]
    async fn enabling_swarm_does_not_auto_elect_coordinator() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider.clone()).await;
        let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
        let (member_event_tx, _member_event_rx) = mpsc::unbounded_channel();
        let now = Instant::now();
        let session_id = "session_test_swarm_toggle";
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            session_id.to_string(),
            crate::server::SwarmMember {
                session_id: session_id.to_string(),
                event_tx: member_event_tx,
                working_dir: Some(PathBuf::from("/tmp/jcode-passive-swarm")),
                swarm_id: None,
                swarm_enabled: false,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("duck".to_string()),
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
                is_headless: false,
            },
        )])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
        let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
        let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
            String,
            HashMap<String, HashSet<String>>,
        >::new()));
        let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
        let mut swarm_enabled = false;

        handle_set_feature(
            42,
            FeatureToggle::Swarm,
            true,
            &agent,
            session_id,
            &Some("duck".to_string()),
            &mut swarm_enabled,
            &swarm_members,
            &swarms_by_id,
            &swarm_coordinators,
            &channel_subscriptions,
            &swarm_plans,
            &client_event_tx,
        )
        .await;

        assert!(swarm_enabled);
        assert!(swarm_coordinators.read().await.is_empty());
        assert_eq!(
            swarm_members
                .read()
                .await
                .get(session_id)
                .and_then(|member| member.swarm_id.clone())
                .as_deref(),
            Some("/tmp/jcode-passive-swarm")
        );
        assert_eq!(
            swarm_members
                .read()
                .await
                .get(session_id)
                .map(|member| member.role.as_str()),
            Some("agent")
        );

        let events: Vec<_> = std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ServerEvent::Done { id: 42 }))
        );
        assert!(events.iter().all(|event| {
            !matches!(
                event,
                ServerEvent::Notification { message, .. }
                    if message == "You are the coordinator for this swarm."
            )
        }));
    }

    #[tokio::test]
    async fn notify_session_sends_scheduled_notification_to_live_member() {
        let sessions = Arc::new(RwLock::new(HashMap::<String, Arc<Mutex<Agent>>>::new()));
        let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "client-1".to_string(),
            ClientConnectionInfo {
                client_id: "client-1".to_string(),
                session_id: "session-live".to_string(),
                debug_client_id: Some("debug-1".to_string()),
                connected_at: Instant::now(),
                last_seen: Instant::now(),
            },
        )])));
        let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session-live".to_string(),
            SwarmMember {
                session_id: "session-live".to_string(),
                event_tx: member_event_tx,
                working_dir: None,
                swarm_id: None,
                swarm_enabled: false,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("otter".to_string()),
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
            },
        )])));
        let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

        handle_notify_session(
            77,
            "session-live".to_string(),
            "[Scheduled task]\nTask: Follow up".to_string(),
            &sessions,
            &soft_interrupt_queues,
            &client_connections,
            &swarm_members,
            &client_event_tx,
        )
        .await;

        let member_event = member_event_rx
            .recv()
            .await
            .expect("live member should receive notification");
        match member_event {
            ServerEvent::Notification {
                from_session,
                from_name,
                notification_type,
                message,
            } => {
                assert_eq!(from_session, "schedule");
                assert_eq!(from_name.as_deref(), Some("scheduled task"));
                assert!(matches!(
                    notification_type,
                    NotificationType::Message {
                        scope: Some(ref scope),
                        channel: None,
                    } if scope == "scheduled"
                ));
                assert!(message.contains("[Scheduled task]"));
                assert!(message.contains("Task: Follow up"));
            }
            other => panic!("expected notification event, got {other:?}"),
        }

        let client_events: Vec<_> =
            std::iter::from_fn(|| client_event_rx.try_recv().ok()).collect();
        assert!(
            client_events
                .iter()
                .any(|event| matches!(event, ServerEvent::Done { id } if *id == 77))
        );
    }
}

pub(super) fn handle_compact(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let agent = Arc::clone(agent);
    let tx = client_event_tx.clone();
    tokio::spawn(async move {
        let agent_guard = agent.lock().await;
        let provider = agent_guard.provider_fork();
        let compaction = agent_guard.registry().compaction();
        let messages = agent_guard.provider_messages();
        drop(agent_guard);

        if !provider.supports_compaction() {
            let _ = tx.send(ServerEvent::CompactResult {
                id,
                message: "Manual compaction is not available for this provider.".to_string(),
                success: false,
            });
            return;
        }

        let result = match compaction.try_write() {
            Ok(mut manager) => {
                let stats = manager.stats_with(&messages);
                let status_msg = format!(
                    "**Context Status:**\n\
                    • Messages: {} (active), {} (total history)\n\
                    • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                    • Has summary: {}\n\
                    • Compacting: {}",
                    stats.active_messages,
                    stats.total_turns,
                    stats.effective_tokens / 1000,
                    stats.token_estimate / 1000,
                    manager.token_budget() / 1000,
                    stats.context_usage * 100.0,
                    if stats.has_summary { "yes" } else { "no" },
                    if stats.is_compacting {
                        "in progress..."
                    } else {
                        "no"
                    }
                );

                match manager.force_compact_with(&messages, provider) {
                    Ok(()) => ServerEvent::CompactResult {
                        id,
                        message: format!(
                            "{}\n\n📦 **Compacting context** (manual) — summarizing older messages in the background to stay within the context window.\n\
                            The summary will be applied automatically when ready.",
                            status_msg
                        ),
                        success: true,
                    },
                    Err(reason) => ServerEvent::CompactResult {
                        id,
                        message: format!("{status_msg}\n\n⚠ **Cannot compact:** {reason}"),
                        success: false,
                    },
                }
            }
            Err(_) => ServerEvent::CompactResult {
                id,
                message: "⚠ Cannot access compaction manager (lock held)".to_string(),
                success: false,
            },
        };
        let _ = tx.send(result);
    });
}

pub(super) async fn handle_stdin_response(
    id: u64,
    request_id: String,
    input: String,
    stdin_responses: &Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    if let Some(tx) = stdin_responses.lock().await.remove(&request_id) {
        let _ = tx.send(input);
    }
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_agent_task(
    id: u64,
    task: String,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    update_member_status(
        client_session_id,
        "running",
        Some(truncate_detail(&task, 120)),
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;

    let result = process_message_streaming_mpsc(
        Arc::clone(agent),
        &task,
        vec![],
        None,
        client_event_tx.clone(),
    )
    .await;
    match result {
        Ok(()) => {
            update_member_status(
                client_session_id,
                "completed",
                None,
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        Err(e) => {
            update_member_status(
                client_session_id,
                "failed",
                Some(truncate_detail(&e.to_string(), 120)),
                swarm_members,
                swarms_by_id,
                Some(event_history),
                Some(event_counter),
                Some(swarm_event_tx),
            )
            .await;
            let retry_after_secs = e
                .downcast_ref::<StreamError>()
                .and_then(|stream_error| stream_error.retry_after_secs);
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: crate::util::format_error_chain(&e),
                retry_after_secs,
            });
        }
    }
}
