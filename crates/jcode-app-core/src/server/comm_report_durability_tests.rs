//! CommReport must checkpoint explicit recovery reports before its success reply.
//! Exercise both production request paths over a native socket pair, offline.
#![allow(clippy::await_holding_lock)]

use crate::message::{Message, ToolDefinition};
use crate::protocol::{Request, ServerEvent};
use crate::provider::{EventStream, Provider};
use crate::server::{Server, SwarmMember, VersionedPlan};
use crate::session::{Session, SessionStatus};
use crate::transport::{ReadHalf, Stream, WriteHalf};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

const SWARM: &str = "comm-report-durability";
const WORKER: &str = "session_report_recovered_worker";
const OWNER: &str = "session_report_original_owner";
const REPORT: &str = "Recovered original worker completed its file operation.";

struct IsolatedEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
    root: tempfile::TempDir,
}

impl IsolatedEnv {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let runtime = root.path().join("runtime");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        let values = [
            ("JCODE_HOME", home.into_os_string()),
            ("JCODE_RUNTIME_DIR", runtime.clone().into_os_string()),
            ("JCODE_SOCKET", runtime.join("jcode.sock").into_os_string()),
            ("JCODE_SWARM_ID", OsString::from(SWARM)),
            ("JCODE_MEMORY_ENABLED", OsString::from("0")),
            ("JCODE_MEMORY_SIDECAR_ENABLED", OsString::from("0")),
            ("JCODE_TEST_SESSION", OsString::from("1")),
        ];
        let previous = values
            .into_iter()
            .map(|(key, value)| {
                let old = std::env::var_os(key);
                crate::env::set_var(key, value);
                (key, old)
            })
            .collect();
        crate::config::invalidate_config_cache();
        Self { previous, root }
    }
}

impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        for (key, previous) in self.previous.drain(..).rev() {
            match previous {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        crate::config::invalidate_config_cache();
    }
}

#[derive(Clone, Default)]
struct NoInference(Arc<AtomicUsize>);

#[async_trait]
impl Provider for NoInference {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("report durability tests must not invoke inference")
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct Connection {
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Connection {
    fn new(server: &Server) -> Self {
        let (server_stream, client_stream) = Stream::pair().unwrap();
        let task = tokio::spawn(crate::server::client_lifecycle::handle_client(
            server_stream,
            server.sessions.clone(),
            server.event_tx.clone(),
            server.provider.clone(),
            server.is_processing.clone(),
            server.session_id.clone(),
            server.client_count.clone(),
            server.client_connections.clone(),
            server.swarm_state.members.clone(),
            server.swarm_state.swarms_by_id.clone(),
            server.shared_context.clone(),
            server.swarm_state.plans.clone(),
            server.swarm_state.coordinators.clone(),
            server.file_touch.clone(),
            server.channel_subscriptions.clone(),
            server.channel_subscriptions_by_session.clone(),
            server.client_debug_state.clone(),
            server.client_debug_response_tx.clone(),
            server.event_history.clone(),
            server.event_counter.clone(),
            server.swarm_event_tx.clone(),
            "report-durability-test".into(),
            "test".into(),
            Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
            server.shutdown_signals.clone(),
            server.soft_interrupt_queues.clone(),
            server.await_members_runtime.clone(),
            server.swarm_mutation_runtime.clone(),
        ));
        let (reader, writer) = client_stream.into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
            task,
        }
    }

    async fn send(&mut self, request: Request) {
        let payload = serde_json::to_string(&request).unwrap() + "\n";
        self.writer.write_all(payload.as_bytes()).await.unwrap();
    }

    async fn until(&mut self, predicate: impl Fn(&ServerEvent) -> bool) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut line = String::new();
                assert_ne!(
                    self.reader.read_line(&mut line).await.unwrap(),
                    0,
                    "unexpected EOF"
                );
                let event: ServerEvent = serde_json::from_str(&line).unwrap();
                assert!(!matches!(event, ServerEvent::Error { .. }), "{event:?}");
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("native request response deadline")
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn seed_failed_worker(server: &Server) {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    server.swarm_state.members.write().await.insert(
        WORKER.into(),
        SwarmMember {
            session_id: WORKER.into(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: Some(SWARM.into()),
            swarm_enabled: true,
            status: "failed".into(),
            detail: Some("old failed snapshot".into()),
            task_label: Some("original recovery task".into()),
            friendly_name: Some("recovered-worker".into()),
            report_back_to_session_id: Some(OWNER.into()),
            latest_completion_report: Some("old failure report".into()),
            role: "agent".into(),
            joined_at: now,
            last_status_change: now,
            is_headless: true,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: Default::default(),
        },
    );
    let swarm = &server.swarm_state;
    swarm
        .swarms_by_id
        .write()
        .await
        .insert(SWARM.into(), HashSet::from([WORKER.into(), OWNER.into()]));
    swarm
        .coordinators
        .write()
        .await
        .insert(SWARM.into(), OWNER.into());
    let mut plan = VersionedPlan::new();
    plan.version = 7;
    plan.items.push(crate::plan::PlanItem {
        id: "original-node".into(),
        content: "preserve this assigned plan".into(),
        status: "failed".into(),
        priority: "high".into(),
        assigned_to: Some(WORKER.into()),
        subsystem: None,
        file_scope: vec![],
        blocked_by: vec![],
    });
    swarm.plans.write().await.insert(SWARM.into(), plan);
    crate::server::persist_swarm_state_for(SWARM, &server.swarm_state).await;
}

async fn report_roundtrip(subscribed: bool) {
    let _lock = crate::storage::lock_test_env();
    let env = IsolatedEnv::new();
    let provider = Arc::new(NoInference::default());
    let server = Server::new(provider.clone());
    let mut session = Session::create_with_id(WORKER.into(), None, None);
    session.working_dir = Some(env.root.path().to_string_lossy().into_owned());
    session.status = SessionStatus::Closed;
    session.save_prepared().unwrap();
    seed_failed_worker(&server).await;
    let mut connection = Connection::new(&server);
    if subscribed {
        connection
            .send(Request::Subscribe {
                id: 1,
                system_prompt: None,
                supports_pdf_panels: false,
                supports_user_response: false,
                working_dir: Some(env.root.path().to_string_lossy().into_owned()),
                target_session_id: Some(WORKER.into()),
                selfdev: None,
                client_instance_id: None,
                client_has_local_history: false,
                allow_session_takeover: false,
                crash_on_disconnect: false,
                continue_on_disconnect: false,
                terminal_env: vec![],
                account_pins: vec![],
                supports_session_accounts: false,
            })
            .await;
        connection
            .until(|event| matches!(event, ServerEvent::Done { id: 1 }))
            .await;
        // Subscribe legitimately refreshes member state. Start the report from
        // the same durable failure as the standalone no-Subscribe case.
        seed_failed_worker(&server).await;
        assert!(server.sessions.read().await.contains_key(WORKER));
        let connections = server.client_connections.read().await;
        assert!(
            connections
                .values()
                .any(|client| client.session_id == WORKER)
        );
    } else {
        assert!(server.sessions.read().await.is_empty());
    }

    let operation = crate::server::swarm_persistence::swarm_operation_lock(SWARM);
    let gate = operation.lock().await;
    connection
        .send(Request::CommReport {
            id: 2,
            session_id: WORKER.into(),
            status: Some("completed".into()),
            message: REPORT.into(),
            validation: Some("offline validation passed".into()),
            follow_up: Some("no remaining work".into()),
            tldr: Some("original worker recovered".into()),
        })
        .await;
    connection
        .until(|event| matches!(event, ServerEvent::Ack { id: 2 }))
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server.swarm_state.members.read().await[WORKER].status == "completed" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("report must update runtime state before persistence");
    assert_eq!(
        crate::server::swarm_persistence::load_runtime_state().members[WORKER].status,
        "failed"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(40),
            connection
                .until(|event| matches!(event, ServerEvent::CommReportResponse { id: 2, .. }))
        )
        .await
        .is_err(),
        "a successful report response must wait for the durable checkpoint",
    );
    drop(gate);
    let response = connection
        .until(|event| matches!(event, ServerEvent::CommReportResponse { id: 2, .. }))
        .await;
    assert!(
        matches!(response, ServerEvent::CommReportResponse { status, .. } if status == "completed")
    );

    // Read immediately after the success response, before disconnect or any
    // turn-end hook could hide a missing write, then construct a fresh daemon.
    let loaded = crate::server::swarm_persistence::load_runtime_state();
    assert_eq!(loaded.members[WORKER].status, "completed");
    let restarted = Server::new(provider.clone());
    let members = restarted.swarm_state.members.read().await;
    let worker = &members[WORKER];
    assert_eq!(worker.status, "completed");
    assert_eq!(worker.report_back_to_session_id.as_deref(), Some(OWNER));
    assert_eq!(worker.swarm_id.as_deref(), Some(SWARM));
    let report = worker.latest_completion_report.as_deref().unwrap();
    assert!(report.contains(REPORT));
    assert!(report.contains("offline validation passed"));
    assert!(report.contains("no remaining work"));
    assert!(!report.contains("old failure report"));
    drop(members);
    let plans = restarted.swarm_state.plans.read().await;
    assert_eq!(plans[SWARM].version, 7);
    assert_eq!(plans[SWARM].items[0].assigned_to.as_deref(), Some(WORKER));
    assert_eq!(plans[SWARM].items[0].status, "failed");
    assert_eq!(plans[SWARM].items[0].content, "preserve this assigned plan");
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn lightweight_report_checkpoints_recovered_worker_before_success_response() {
    report_roundtrip(false).await;
}

#[tokio::test]
async fn subscribed_report_checkpoints_recovered_worker_before_success_response() {
    report_roundtrip(true).await;
}
