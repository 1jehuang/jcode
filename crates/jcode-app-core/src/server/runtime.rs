use super::client_lifecycle::handle_client;
use super::debug::{ClientConnectionInfo, ClientDebugState, handle_debug_client};
use super::debug_jobs::DebugJob;
use super::util::get_shared_mcp_pool;
use super::{
    AwaitMembersRuntime, FileAccess, ServerIdentity, SessionInterruptQueues, SharedContext,
    SwarmEvent, SwarmMutationRuntime, SwarmState,
};
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::gateway::GatewayClient;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use crate::transport::{Listener, Stream};
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast, mpsc};

type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

#[derive(Clone)]
pub(super) struct ServerRuntime {
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    event_tx: broadcast::Sender<ServerEvent>,
    provider: Arc<dyn Provider>,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
    client_count: Arc<RwLock<usize>>,
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_state: SwarmState,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: ChannelSubscriptions,
    channel_subscriptions_by_session: ChannelSubscriptions,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: Arc<AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    server_name: String,
    server_icon: String,
    server_identity: ServerIdentity,
    ambient_runner: Option<AmbientRunnerHandle>,
    mcp_pool: Arc<OnceCell<Arc<crate::mcp::SharedMcpPool>>>,
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: SessionInterruptQueues,
    await_members_runtime: AwaitMembersRuntime,
    swarm_mutation_runtime: SwarmMutationRuntime,
}

impl ServerRuntime {
    pub(super) fn from_server(server: &super::Server) -> Self {
        Self {
            sessions: Arc::clone(&server.sessions),
            event_tx: server.event_tx.clone(),
            provider: Arc::clone(&server.provider),
            is_processing: Arc::clone(&server.is_processing),
            session_id: Arc::clone(&server.session_id),
            client_count: Arc::clone(&server.client_count),
            client_connections: Arc::clone(&server.client_connections),
            swarm_state: server.swarm_state.clone(),
            shared_context: Arc::clone(&server.shared_context),
            file_touches: Arc::clone(&server.file_touches),
            files_touched_by_session: Arc::clone(&server.files_touched_by_session),
            channel_subscriptions: Arc::clone(&server.channel_subscriptions),
            channel_subscriptions_by_session: Arc::clone(&server.channel_subscriptions_by_session),
            client_debug_state: Arc::clone(&server.client_debug_state),
            client_debug_response_tx: server.client_debug_response_tx.clone(),
            debug_jobs: Arc::clone(&server.debug_jobs),
            event_history: Arc::clone(&server.event_history),
            event_counter: Arc::clone(&server.event_counter),
            swarm_event_tx: server.swarm_event_tx.clone(),
            server_name: server.identity.name.clone(),
            server_icon: server.identity.icon.clone(),
            server_identity: server.identity.clone(),
            ambient_runner: server.ambient_runner.clone(),
            mcp_pool: Arc::clone(&server.mcp_pool),
            shutdown_signals: Arc::clone(&server.shutdown_signals),
            soft_interrupt_queues: Arc::clone(&server.soft_interrupt_queues),
            await_members_runtime: server.await_members_runtime.clone(),
            swarm_mutation_runtime: server.swarm_mutation_runtime.clone(),
        }
    }

    pub(super) fn spawn_main_accept_loop(&self, listener: Listener) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        tokio::spawn(async move {
            #[cfg(windows)]
            let mut listener = listener;

            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        runtime.increment_client_count().await;
                        runtime.spawn_client_task(stream, "Client error", true);
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Main accept error: {}", e));
                    }
                }
            }
        })
    }

    pub(super) fn spawn_debug_accept_loop(
        &self,
        listener: Listener,
        server_start_time: Instant,
    ) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        tokio::spawn(async move {
            #[cfg(windows)]
            let mut listener = listener;

            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        // Debug clients do not participate in idle-timeout accounting.
                        runtime.spawn_debug_client_task(stream, server_start_time);
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Debug accept error: {}", e));
                    }
                }
            }
        })
    }

    pub(super) fn spawn_gateway_accept_loop(
        &self,
        mut client_rx: mpsc::UnboundedReceiver<GatewayClient>,
    ) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        tokio::spawn(async move {
            while let Some(gw_client) = client_rx.recv().await {
                runtime.increment_client_count().await;
                crate::logging::info(&format!(
                    "Gateway client connected: {} ({})",
                    gw_client.device_name, gw_client.device_id
                ));
                // Preserve prior behavior: gateway sessions do not nudge the
                // ambient runner on disconnect.
                runtime.spawn_gateway_client_task(gw_client);
            }
        })
    }

    fn spawn_client_task(&self, stream: Stream, error_prefix: &'static str, nudge_ambient: bool) {
        let runtime = self.clone();
        tokio::spawn(async move {
            runtime
                .run_client_stream(stream, error_prefix, nudge_ambient)
                .await;
        });
    }

    fn spawn_gateway_client_task(&self, gw_client: GatewayClient) {
        let runtime = self.clone();
        tokio::spawn(async move {
            runtime
                .run_client_stream(gw_client.stream, "Gateway client error", false)
                .await;
        });
    }

    fn spawn_debug_client_task(&self, stream: Stream, server_start_time: Instant) {
        let runtime = self.clone();
        tokio::spawn(async move {
            runtime.run_debug_stream(stream, server_start_time).await;
        });
    }

    async fn increment_client_count(&self) {
        *self.client_count.write().await += 1;
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "client_connected",
                "client_count_incremented",
            ),
        );
    }

    async fn decrement_client_count(&self) {
        *self.client_count.write().await -= 1;
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "client_disconnected",
                "client_count_decremented",
            ),
        );
    }

    async fn run_client_stream(
        self,
        stream: Stream,
        error_prefix: &'static str,
        nudge_ambient: bool,
    ) {
        let mcp_pool = get_shared_mcp_pool(&self.mcp_pool).await;
        let result = handle_client(
            stream,
            Arc::clone(&self.sessions),
            self.event_tx.clone(),
            Arc::clone(&self.provider),
            Arc::clone(&self.is_processing),
            Arc::clone(&self.session_id),
            Arc::clone(&self.client_count),
            Arc::clone(&self.client_connections),
            Arc::clone(&self.swarm_state.members),
            Arc::clone(&self.swarm_state.swarms_by_id),
            Arc::clone(&self.shared_context),
            Arc::clone(&self.swarm_state.plans),
            Arc::clone(&self.swarm_state.coordinators),
            Arc::clone(&self.file_touches),
            Arc::clone(&self.files_touched_by_session),
            Arc::clone(&self.channel_subscriptions),
            Arc::clone(&self.channel_subscriptions_by_session),
            Arc::clone(&self.client_debug_state),
            self.client_debug_response_tx.clone(),
            Arc::clone(&self.event_history),
            Arc::clone(&self.event_counter),
            self.swarm_event_tx.clone(),
            self.server_name.clone(),
            self.server_icon.clone(),
            mcp_pool,
            Arc::clone(&self.shutdown_signals),
            Arc::clone(&self.soft_interrupt_queues),
            self.await_members_runtime.clone(),
            self.swarm_mutation_runtime.clone(),
        )
        .await;

        self.decrement_client_count().await;

        if nudge_ambient && let Some(ref runner) = self.ambient_runner {
            runner.nudge();
        }

        if let Err(e) = result {
            crate::logging::error(&format!("{}: {}", error_prefix, e));
        }
    }

    async fn run_debug_stream(self, stream: Stream, server_start_time: Instant) {
        let mcp_pool = Some(get_shared_mcp_pool(&self.mcp_pool).await);

        if let Err(e) = handle_debug_client(
            stream,
            Arc::clone(&self.sessions),
            Arc::clone(&self.is_processing),
            Arc::clone(&self.session_id),
            Arc::clone(&self.provider),
            Arc::clone(&self.client_connections),
            Arc::clone(&self.swarm_state.members),
            Arc::clone(&self.swarm_state.swarms_by_id),
            Arc::clone(&self.shared_context),
            Arc::clone(&self.swarm_state.plans),
            Arc::clone(&self.swarm_state.coordinators),
            Arc::clone(&self.file_touches),
            Arc::clone(&self.files_touched_by_session),
            Arc::clone(&self.channel_subscriptions),
            Arc::clone(&self.channel_subscriptions_by_session),
            Arc::clone(&self.client_debug_state),
            self.client_debug_response_tx.clone(),
            Arc::clone(&self.debug_jobs),
            Arc::clone(&self.event_history),
            Arc::clone(&self.event_counter),
            self.swarm_event_tx.clone(),
            self.server_identity.clone(),
            server_start_time,
            self.ambient_runner.clone(),
            mcp_pool,
            Arc::clone(&self.shutdown_signals),
            Arc::clone(&self.soft_interrupt_queues),
        )
        .await
        {
            crate::logging::error(&format!("Debug client error: {}", e));
        }
    }
}
