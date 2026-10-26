use super::client_lifecycle::handle_client;
use super::debug::{ClientConnectionInfo, ClientDebugState, handle_debug_client};
use super::debug_jobs::DebugJob;
use super::util::get_shared_mcp_pool;
use super::{
    AwaitMembersRuntime, FileTouchService, ServerIdentity, SessionInterruptQueues, SharedContext,
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
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

/// Owns every connection task spawned by a server runtime.
///
/// Dropping a `JoinHandle` detaches its task, so accepting a connection must not
/// discard the handle. This scope gives the accept loops and their children one
/// cancellation boundary and lets server shutdown wait until all children have
/// observed cancellation and released their resources.
#[derive(Default)]
struct RuntimeTaskScope {
    cancellation: CancellationToken,
    tasks: Mutex<JoinSet<()>>,
}

impl RuntimeTaskScope {
    async fn spawn<F, Fut>(&self, task: F) -> bool
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.cancellation.is_cancelled() {
            return false;
        }

        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            log_task_completion(result);
        }
        if self.cancellation.is_cancelled() {
            return false;
        }

        tasks.spawn(task(self.cancellation.child_token()));
        true
    }

    async fn shutdown(&self) {
        self.cancellation.cancel();
        // Drain the set before awaiting children. An accept task may already be
        // waiting to register a just-accepted connection; leaving the mutex
        // held while joining would deadlock that task. Once cancelled, any
        // late registration observes cancellation and is rejected.
        let mut tasks = {
            let mut owned_tasks = self.tasks.lock().await;
            std::mem::take(&mut *owned_tasks)
        };
        while let Some(result) = tasks.join_next().await {
            log_task_completion(result);
        }
    }

    #[cfg(test)]
    async fn task_count(&self) -> usize {
        self.tasks.lock().await.len()
    }
}

fn log_task_completion(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result
        && !error.is_cancelled()
    {
        crate::logging::error(&format!("Server connection task failed: {error}"));
    }
}

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
    file_touch: FileTouchService,
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
    tasks: Arc<RuntimeTaskScope>,
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
            file_touch: server.file_touch.clone(),
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
            tasks: Arc::new(RuntimeTaskScope::default()),
        }
    }

    pub(super) fn spawn_main_accept_loop(&self, listener: Listener) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        let cancellation = self.tasks.cancellation.child_token();
        tokio::spawn(async move {
            #[cfg(windows)]
            let mut listener = listener;

            loop {
                let accepted = tokio::select! {
                    _ = cancellation.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                match accepted {
                    Ok((stream, _)) => {
                        crate::logging::info("[TIMING] main accept: connection accepted");
                        runtime.increment_client_count().await;
                        if !runtime
                            .spawn_client_task(stream, "Client error", true)
                            .await
                        {
                            runtime.decrement_client_count().await;
                            break;
                        }
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
        let cancellation = self.tasks.cancellation.child_token();
        tokio::spawn(async move {
            #[cfg(windows)]
            let mut listener = listener;

            loop {
                let accepted = tokio::select! {
                    _ = cancellation.cancelled() => break,
                    accepted = listener.accept() => accepted,
                };
                match accepted {
                    Ok((stream, _)) => {
                        // Debug clients do not participate in idle-timeout accounting.
                        if !runtime
                            .spawn_debug_client_task(stream, server_start_time)
                            .await
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Debug accept error: {}", e));
                    }
                }
            }
        })
    }

    pub(super) async fn spawn_gateway_accept_loop(
        &self,
        mut client_rx: mpsc::UnboundedReceiver<GatewayClient>,
    ) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                loop {
                    let gw_client = tokio::select! {
                        _ = cancellation.cancelled() => break,
                        client = client_rx.recv() => match client {
                            Some(client) => client,
                            None => break,
                        },
                    };
                    runtime.increment_client_count().await;
                    crate::logging::info(&format!(
                        "Gateway client connected: {} ({})",
                        gw_client.device_name, gw_client.device_id
                    ));
                    // Preserve prior behavior: gateway sessions do not nudge the
                    // ambient runner on disconnect.
                    if !runtime.spawn_gateway_client_task(gw_client).await {
                        runtime.decrement_client_count().await;
                        break;
                    }
                }
            })
            .await
    }

    pub(super) async fn spawn_background_task<Fut>(&self, task: Fut) -> bool
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.tasks
            .spawn(move |cancellation| async move {
                tokio::select! {
                    _ = cancellation.cancelled() => {}
                    _ = task => {}
                }
            })
            .await
    }

    async fn spawn_client_task(
        &self,
        stream: Stream,
        error_prefix: &'static str,
        nudge_ambient: bool,
    ) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                runtime
                    .run_client_stream(stream, error_prefix, nudge_ambient, cancellation)
                    .await;
            })
            .await
    }

    async fn spawn_gateway_client_task(&self, gw_client: GatewayClient) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                runtime
                    .run_client_stream(
                        gw_client.stream,
                        "Gateway client error",
                        false,
                        cancellation,
                    )
                    .await;
            })
            .await
    }

    async fn spawn_debug_client_task(&self, stream: Stream, server_start_time: Instant) -> bool {
        let runtime = self.clone();
        self.tasks
            .spawn(move |cancellation| async move {
                runtime
                    .run_debug_stream(stream, server_start_time, cancellation)
                    .await;
            })
            .await
    }

    pub(super) async fn shutdown(&self) {
        self.tasks.shutdown().await;
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
        cancellation: CancellationToken,
    ) {
        let result = {
            let client = async {
                let mcp_pool = get_shared_mcp_pool(&self.mcp_pool).await;
                handle_client(
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
                    self.file_touch.clone(),
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
                .await
            };
            tokio::pin!(client);
            tokio::select! {
                result = &mut client => Some(result),
                _ = cancellation.cancelled() => None,
            }
        };

        self.decrement_client_count().await;

        if nudge_ambient && let Some(ref runner) = self.ambient_runner {
            runner.nudge();
        }

        if let Some(Err(e)) = result {
            crate::logging::error(&format!("{}: {}", error_prefix, e));
        }
    }

    async fn run_debug_stream(
        self,
        stream: Stream,
        server_start_time: Instant,
        cancellation: CancellationToken,
    ) {
        let client = async {
            let mcp_pool = Some(get_shared_mcp_pool(&self.mcp_pool).await);
            handle_debug_client(
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
                self.file_touch.clone(),
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
        };
        tokio::pin!(client);
        if let Some(Err(e)) = tokio::select! {
            result = &mut client => Some(result),
            _ = cancellation.cancelled() => None,
        } {
            crate::logging::error(&format!("Debug client error: {}", e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeTaskScope;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn runtime_task_scope_cancels_and_joins_owned_tasks() {
        let scope = RuntimeTaskScope::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);

        assert!(
            scope
                .spawn(move |cancellation| async move {
                    let _drop_flag = DropFlag(task_dropped);
                    cancellation.cancelled().await;
                })
                .await
        );
        assert_eq!(scope.task_count().await, 1);

        tokio::time::timeout(Duration::from_secs(1), scope.shutdown())
            .await
            .expect("runtime task scope should join cancelled tasks");

        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(scope.task_count().await, 0);
        assert!(
            !scope
                .spawn(|_| async { panic!("task spawned after shutdown") })
                .await
        );
    }
}
