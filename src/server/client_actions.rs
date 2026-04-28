#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::client_lifecycle::process_message_streaming_mpsc;
use super::{
    ClientConnectionInfo, SessionInterruptQueues, SwarmEvent, SwarmMember, SwarmState,
    VersionedPlan, broadcast_swarm_status, fanout_session_event, persist_swarm_state_for,
    queue_soft_interrupt_for_session, remove_session_channel_subscriptions,
    remove_session_from_swarm, session_event_fanout_sender, swarm_id_for_dir, truncate_detail,
    update_member_status,
};
use crate::agent::Agent;
use crate::protocol::{FeatureToggle, NotificationType, ServerEvent};
use crate::session::Session;
use crate::util::truncate_str;
use jcode_agent_runtime::{SoftInterruptSource, StreamError};
use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

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
        crate::platform::suppress_child_console_async(&mut cmd);
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

async fn run_scheduled_task_in_live_session_if_idle(
    session_id: &str,
    message: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    };
    let Some(agent) = agent else {
        return false;
    };

    let has_live_attachments = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(|member| !member.event_txs.is_empty() || !member.event_tx.is_closed())
            .unwrap_or(false)
    };
    if !has_live_attachments {
        return false;
    }

    let is_idle = match agent.try_lock() {
        Ok(guard) => {
            drop(guard);
            true
        }
        Err(_) => false,
    };

    if !is_idle {
        return false;
    }

    let session_id = session_id.to_string();
    let message = message.to_string();
    let event_tx = session_event_fanout_sender(session_id.clone(), Arc::clone(swarm_members));
    tokio::spawn(async move {
        if let Err(err) =
            process_message_streaming_mpsc(agent, &message, vec![], None, event_tx).await
        {
            crate::logging::error(&format!(
                "Failed to run scheduled task immediately for live session {}: {}",
                session_id, err
            ));
        }
    });

    true
}

pub(super) struct NotifySessionContext<'a> {
    pub sessions: &'a SessionAgents,
    pub soft_interrupt_queues: &'a SessionInterruptQueues,
    pub client_connections: &'a Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    pub swarm_members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub client_event_tx: &'a mpsc::UnboundedSender<ServerEvent>,
}

pub(super) async fn handle_notify_session(
    id: u64,
    session_id: String,
    message: String,
    ctx: NotifySessionContext<'_>,
) {
    let target_has_client = {
        let connections = ctx.client_connections.read().await;
        connections
            .values()
            .any(|connection| connection.session_id == session_id)
    };

    let ran_immediately = if target_has_client {
        run_scheduled_task_in_live_session_if_idle(
            &session_id,
            &message,
            ctx.sessions,
            ctx.swarm_members,
        )
        .await
    } else {
        false
    };

    let notified = if ran_immediately {
        false
    } else {
        let members = ctx.swarm_members.read().await;
        if members.contains_key(&session_id) {
            drop(members);
            fanout_session_event(
                ctx.swarm_members,
                &session_id,
                ServerEvent::Notification {
                    from_session: "schedule".to_string(),
                    from_name: Some("scheduled task".to_string()),
                    notification_type: NotificationType::Message {
                        scope: Some("scheduled".to_string()),
                        channel: None,
                    },
                    message: message.clone(),
                },
            )
            .await
                > 0
        } else {
            false
        }
    };

    let queued_interrupt = if ran_immediately {
        false
    } else {
        queue_soft_interrupt_for_session(
            &session_id,
            message.clone(),
            false,
            SoftInterruptSource::System,
            ctx.soft_interrupt_queues,
            ctx.sessions,
        )
        .await
    };

    if ran_immediately || notified || queued_interrupt {
        let _ = ctx.client_event_tx.send(ServerEvent::Done { id });
    } else {
        let _ = ctx.client_event_tx.send(ServerEvent::Error {
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

#[expect(
    clippy::too_many_arguments,
    reason = "set feature mutates agent state, persistence, swarm/session metadata, and client notifications together"
)]
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
    channel_subscriptions: &ChannelSubscriptions,
    channel_subscriptions_by_session: &ChannelSubscriptions,
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
            crate::runtime_memory_log::emit_event(
                crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                    "memory_feature_toggled",
                    if enabled {
                        "memory_feature_enabled"
                    } else {
                        "memory_feature_disabled"
                    },
                )
                .with_session_id(client_session_id.to_string())
                .force_attribution(),
            );
            let _ = client_event_tx.send(ServerEvent::Done { id });
        }
        FeatureToggle::Autoreview => {
            let mut agent_guard = agent.lock().await;
            match agent_guard.set_autoreview_enabled(enabled) {
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
        FeatureToggle::Autojudge => {
            let mut agent_guard = agent.lock().await;
            match agent_guard.set_autojudge_enabled(enabled) {
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
                remove_session_channel_subscriptions(
                    client_session_id,
                    channel_subscriptions,
                    channel_subscriptions_by_session,
                )
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
                    let swarm_state = SwarmState {
                        members: Arc::clone(swarm_members),
                        swarms_by_id: Arc::clone(swarms_by_id),
                        plans: Arc::clone(swarm_plans),
                        coordinators: Arc::clone(swarm_coordinators),
                    };
                    persist_swarm_state_for(id, &swarm_state).await;
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

fn transfer_active_messages(session: &Session) -> Vec<crate::message::Message> {
    let start = session
        .compaction
        .as_ref()
        .map(|state| state.compacted_count.min(session.messages.len()))
        .unwrap_or(0);
    session.messages[start..]
        .iter()
        .map(crate::session::StoredMessage::to_message)
        .collect()
}

fn create_transfer_child_session(
    parent_session_id: &str,
    parent: &Session,
    compaction: Option<crate::session::StoredCompactionState>,
) -> anyhow::Result<(String, String)> {
    let todos = crate::todo::load_todos(parent_session_id).unwrap_or_default();
    let mut child = Session::create(Some(parent_session_id.to_string()), None);
    child.messages.clear();
    child.compaction = compaction;
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    child.provider_key = parent.provider_key.clone();
    child.subagent_model = parent.subagent_model.clone();
    child.improve_mode = parent.improve_mode;
    child.autoreview_enabled = parent.autoreview_enabled;
    child.autojudge_enabled = parent.autojudge_enabled;
    child.is_canary = parent.is_canary;
    child.testing_build = parent.testing_build.clone();
    child.provider_session_id = None;
    child.status = crate::session::SessionStatus::Closed;
    child.save()?;
    crate::todo::save_todos(&child.id, &todos)?;
    Ok((child.id.clone(), child.display_name().to_string()))
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

pub(super) async fn handle_transfer(
    id: u64,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let parent = match Session::load(client_session_id) {
        Ok(session) => session,
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to load session for transfer: {error}"),
                retry_after_secs: None,
            });
            return;
        }
    };

    let provider = {
        let agent_guard = agent.lock().await;
        agent_guard.provider_fork()
    };

    let transfer_compaction = match crate::compaction::build_transfer_compaction_state(
        provider,
        transfer_active_messages(&parent),
        parent.compaction.clone(),
    )
    .await
    {
        Ok(compaction) => compaction,
        Err(error) => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Failed to compact session for transfer: {error}"),
                retry_after_secs: None,
            });
            return;
        }
    };

    let (new_session_id, new_session_name) =
        match create_transfer_child_session(client_session_id, &parent, transfer_compaction) {
            Ok(result) => result,
            Err(error) => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!("Failed to create transfer session: {error}"),
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
#[path = "client_actions_tests.rs"]
mod tests;

pub(super) fn handle_compact(
    id: u64,
    agent: &Arc<Mutex<Agent>>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let agent = Arc::clone(agent);
    let tx = client_event_tx.clone();
    tokio::spawn(async move {
        let mut agent_guard = agent.lock().await;
        let session_id = agent_guard.session_id().to_string();
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
                    Ok(()) => {
                        crate::runtime_memory_log::emit_event(
                            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                                "manual_compaction_requested",
                                "manual_compaction_started",
                            )
                            .with_session_id(session_id.clone())
                            .force_attribution(),
                        );
                        ServerEvent::CompactResult {
                            id,
                            message: format!(
                                "{}\n\n📦 **Compacting context** (manual) — summarizing older messages in the background to stay within the context window.\n\
                            The summary will be applied automatically when ready.",
                                status_msg
                            ),
                            success: true,
                        }
                    }
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

pub(super) struct AgentTaskContext<'a> {
    pub(super) client_event_tx: &'a mpsc::UnboundedSender<ServerEvent>,
    pub(super) swarm_members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub(super) swarms_by_id: &'a Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub(super) event_history: &'a Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    pub(super) event_counter: &'a Arc<std::sync::atomic::AtomicU64>,
    pub(super) swarm_event_tx: &'a broadcast::Sender<SwarmEvent>,
}

pub(super) async fn handle_agent_task(
    id: u64,
    task: String,
    client_session_id: &str,
    agent: &Arc<Mutex<Agent>>,
    ctx: &AgentTaskContext<'_>,
) {
    update_member_status(
        client_session_id,
        "running",
        Some(truncate_detail(&task, 120)),
        ctx.swarm_members,
        ctx.swarms_by_id,
        Some(ctx.event_history),
        Some(ctx.event_counter),
        Some(ctx.swarm_event_tx),
    )
    .await;

    let result = process_message_streaming_mpsc(
        Arc::clone(agent),
        &task,
        vec![],
        None,
        ctx.client_event_tx.clone(),
    )
    .await;
    match result {
        Ok(()) => {
            update_member_status(
                client_session_id,
                "completed",
                None,
                ctx.swarm_members,
                ctx.swarms_by_id,
                Some(ctx.event_history),
                Some(ctx.event_counter),
                Some(ctx.swarm_event_tx),
            )
            .await;
            let _ = ctx.client_event_tx.send(ServerEvent::Done { id });
        }
        Err(e) => {
            update_member_status(
                client_session_id,
                "failed",
                Some(truncate_detail(&e.to_string(), 120)),
                ctx.swarm_members,
                ctx.swarms_by_id,
                Some(ctx.event_history),
                Some(ctx.event_counter),
                Some(ctx.swarm_event_tx),
            )
            .await;
            let retry_after_secs = e
                .downcast_ref::<StreamError>()
                .and_then(|stream_error| stream_error.retry_after_secs);
            let _ = ctx.client_event_tx.send(ServerEvent::Error {
                id,
                message: crate::util::format_error_chain(&e),
                retry_after_secs,
            });
        }
    }
}
