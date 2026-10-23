//! Checkpoint protocol operations, kept separate from the connection lifecycle.
use super::*;
use crate::protocol::RewindMode;
use crate::transport::WriteHalf;

pub(super) fn send_agent_busy_error(
    request_id: u64,
    request_kind: &'static str,
    client_session_id: &str,
    client_is_processing: bool,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    crate::logging::event_warn(
        "SERVER_REQUEST_BUSY_AGENT_REJECTED",
        vec![
            ("request_id", request_id.to_string()),
            ("request_kind", request_kind.to_string()),
            ("session_id", client_session_id.to_string()),
            ("client_processing", client_is_processing.to_string()),
            ("reason", "agent_busy".to_string()),
        ],
    );
    let _ = client_event_tx.send(ServerEvent::Error {
        id: request_id,
        message: format!(
            "Cannot handle {request_kind} while the session is busy. Try again after the current turn finishes."
        ),
        retry_after_secs: Some(1),
        server_resumes: false,
    });
}

pub(super) fn list(
    id: u64,
    session_id: &str,
    processing: bool,
    agent: &Arc<Mutex<Agent>>,
    events: &mpsc::UnboundedSender<ServerEvent>,
) {
    // Never wait behind a turn on the connection that carries cancellation.
    let response = match agent.try_lock() {
        Ok(agent) => match agent.rewind_checkpoints() {
            Ok(checkpoints) => ServerEvent::RewindCheckpoints {
                id,
                session_id: session_id.to_owned(),
                checkpoints,
            },
            Err(message) => ServerEvent::Error {
                id,
                message,
                retry_after_secs: None,
                server_resumes: false,
            },
        },
        Err(_) => {
            send_agent_busy_error(
                id,
                "list rewind checkpoints",
                session_id,
                processing,
                events,
            );
            return;
        }
    };
    let _ = events.send(response);
}

pub(super) struct RestoreContext<'a> {
    pub session_id: &'a str,
    pub processing: bool,
    pub agent: &'a Arc<Mutex<Agent>>,
    pub provider: &'a Arc<dyn Provider>,
    pub sessions: &'a SessionAgents,
    pub connections: &'a Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    pub client_count: &'a Arc<RwLock<usize>>,
    pub writer: &'a Arc<Mutex<WriteHalf>>,
    pub server_name: &'a str,
    pub server_icon: &'a str,
    pub pdf_panels: bool,
    pub members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub plans: &'a Arc<RwLock<HashMap<String, VersionedPlan>>>,
    pub events: &'a mpsc::UnboundedSender<ServerEvent>,
}

impl RestoreContext<'_> {
    fn busy(&self, id: u64) {
        send_agent_busy_error(
            id,
            "rewind checkpoint",
            self.session_id,
            self.processing,
            self.events,
        );
    }

    async fn history(&self, id: u64) -> Result<()> {
        handle_get_history(
            id,
            self.session_id,
            self.processing,
            self.agent,
            self.provider,
            self.sessions,
            self.connections,
            self.client_count,
            self.writer,
            self.server_name,
            self.server_icon,
            None,
            self.pdf_panels,
        )
        .await
    }

    async fn session_busy(&self) -> bool {
        self.processing
            || crate::turn_cancel_registry::has_active_turn(self.session_id)
            || self.connections.read().await.values().any(|connection| {
                connection.session_id == self.session_id && connection.is_processing
            })
    }

    pub async fn restore_legacy(&self, id: u64, message_index: Option<usize>) -> Result<()> {
        if self.session_busy().await {
            self.busy(id);
            return Ok(());
        }
        let (result, changed) = match self.agent.try_lock() {
            Ok(mut agent) => {
                if crate::turn_cancel_registry::has_active_turn(self.session_id) {
                    self.busy(id);
                    return Ok(());
                }
                let previous_ids: Vec<_> = agent
                    .messages()
                    .iter()
                    .map(|message| message.id.clone())
                    .collect();
                let result = match message_index {
                    Some(index) => agent.rewind_to_message(index),
                    None => agent.undo_rewind(),
                };
                let changed = !agent
                    .messages()
                    .iter()
                    .map(|message| &message.id)
                    .eq(previous_ids.iter());
                if changed {
                    crate::server::usage_limit_resume::cancel_pending_resume(self.session_id);
                }
                (result, changed)
            }
            Err(_) => {
                self.busy(id);
                return Ok(());
            }
        };
        if result.is_ok() || changed {
            // A durable replacement may have committed before cleanup failed.
            // Always publish the installed transcript before a terminal error.
            self.history(id).await?;
            send_swarm_plan_to_session(self.session_id, self.members, self.plans).await;
        }
        if let Err(message) = result {
            let _ = self.events.send(ServerEvent::Error {
                id,
                message,
                retry_after_secs: None,
                server_resumes: false,
            });
        }
        Ok(())
    }

    pub async fn restore(&self, id: u64, checkpoint_id: &str, mode: RewindMode) -> Result<()> {
        if self.session_busy().await {
            self.busy(id);
            return Ok(());
        }
        let (result, conversation_changed) = match self.agent.try_lock() {
            Ok(mut agent) => {
                // A peer may have registered a turn since the first check.
                if crate::turn_cancel_registry::has_active_turn(self.session_id) {
                    self.busy(id);
                    return Ok(());
                }
                let previous_count = agent.message_count();
                let result = agent.rewind_checkpoint(checkpoint_id, mode).await;
                let changed =
                    mode.restores_conversation() && agent.message_count() != previous_count;
                if changed {
                    // Cancel held continuations before releasing the agent.
                    crate::server::usage_limit_resume::cancel_pending_resume(self.session_id);
                }
                (result, changed)
            }
            Err(_) => {
                self.busy(id);
                return Ok(());
            }
        };
        match result {
            Ok(result) => {
                if mode.restores_conversation() {
                    // Replace the transcript before the editor is refilled.
                    self.history(id).await?;
                    send_swarm_plan_to_session(self.session_id, self.members, self.plans).await;
                }
                let _ = self.events.send(ServerEvent::RewindComplete {
                    id,
                    session_id: self.session_id.to_owned(),
                    result,
                });
            }
            Err(message) => {
                // A durable commit can precede a failed sidecar cleanup. Show
                // the actual transcript while reporting that recovery error.
                if conversation_changed {
                    self.history(id).await?;
                }
                let _ = self.events.send(ServerEvent::Error {
                    id,
                    message,
                    retry_after_secs: None,
                    server_resumes: false,
                });
            }
        }
        Ok(())
    }
}
