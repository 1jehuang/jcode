#![allow(clippy::await_holding_lock)]
use super::*;
use crate::message::{Message, ToolDefinition};
use crate::provider::EventStream;
use async_trait::async_trait;

struct NoRequests;
#[async_trait]
impl Provider for NoRequests {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("target attachment must not invoke a provider")
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

struct Home {
    _temp: tempfile::TempDir,
    old: Option<std::ffi::OsString>,
}
impl Home {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let old = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        Self { _temp: temp, old }
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            crate::env::set_var("JCODE_HOME", old);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn subscribe(target: &str) -> Request {
    Request::Subscribe {
        id: 71,
        working_dir: None,
        target_session_id: Some(target.into()),
        selfdev: None,
        client_instance_id: None,
        client_has_local_history: false,
        allow_session_takeover: false,
        crash_on_disconnect: false,
        continue_on_disconnect: false,
        terminal_env: vec![],
    }
}

async fn live_agent(id: &str, root: &str) -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(NoRequests);
    let registry = Registry::new(provider.clone()).await;
    let mut session = crate::session::Session::create_with_id(id.into(), None, None);
    session.working_dir = Some(root.into());
    Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )))
}

#[tokio::test]
async fn target_subscribe_uses_live_unsaved_root_without_changing_it() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let id = "session_live_empty_attach";
    let agent = live_agent(id, "/workspace/live-original").await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(id.into(), agent.clone())])));
    let members = Arc::new(RwLock::new(HashMap::new()));
    let mut request = subscribe(id);
    resolve_target_subscribe_working_dir(&mut request, &sessions, &members)
        .await
        .unwrap();
    assert_eq!(
        initial_subscribe_working_dir(&request).unwrap(),
        "/workspace/live-original"
    );
    assert_eq!(
        agent.lock().await.working_dir(),
        Some("/workspace/live-original")
    );
    assert!(!crate::session::session_exists(id));
}

#[tokio::test]
async fn target_subscribe_uses_persisted_root_when_no_live_agent_exists() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let mut session = crate::session::Session::create(None, Some("persisted".into()));
    session.working_dir = Some("/workspace/persisted-original".into());
    session.save().unwrap();
    let mut request = subscribe(&session.id);
    resolve_target_subscribe_working_dir(
        &mut request,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await
    .unwrap();
    assert_eq!(
        initial_subscribe_working_dir(&request).unwrap(),
        "/workspace/persisted-original"
    );
}

#[tokio::test]
async fn target_subscribe_live_root_wins_over_stale_persisted_root() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let mut session = crate::session::Session::create(None, Some("persisted".into()));
    session.working_dir = Some("/workspace/stale".into());
    session.save().unwrap();
    let agent = live_agent(&session.id, "/workspace/live").await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(session.id.clone(), agent)])));
    let mut request = subscribe(&session.id);
    resolve_target_subscribe_working_dir(
        &mut request,
        &sessions,
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await
    .unwrap();
    assert_eq!(
        initial_subscribe_working_dir(&request).unwrap(),
        "/workspace/live"
    );
}

#[tokio::test]
async fn target_subscribe_busy_live_agent_uses_member_root_without_waiting() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let id = "session_busy_empty_attach";
    let agent = live_agent(id, "/workspace/busy-original").await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(id.into(), agent.clone())])));
    let (event_tx, _) = mpsc::unbounded_channel();
    let now = std::time::Instant::now();
    let members = Arc::new(RwLock::new(HashMap::from([(
        id.into(),
        SwarmMember {
            session_id: id.into(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: Some("/workspace/busy-original".into()),
            swarm_id: None,
            swarm_enabled: false,
            status: "running".into(),
            detail: None,
            task_label: None,
            friendly_name: None,
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "agent".into(),
            joined_at: now,
            last_status_change: now,
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: vec![],
            runtime: Default::default(),
        },
    )])));
    let _busy = agent.lock().await;
    let mut request = subscribe(id);
    tokio::time::timeout(
        Duration::from_millis(100),
        resolve_target_subscribe_working_dir(&mut request, &sessions, &members),
    )
    .await
    .expect("must not wait on busy Agent")
    .unwrap();
    assert_eq!(
        initial_subscribe_working_dir(&request).unwrap(),
        "/workspace/busy-original"
    );
}

#[tokio::test]
async fn target_subscribe_unknown_target_never_uses_process_working_dir() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let mut request = subscribe("session_missing");
    let error = resolve_target_subscribe_working_dir(
        &mut request,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await
    .unwrap_err();
    assert!(error.contains("Unknown session"));
    assert!(initial_subscribe_working_dir(&request).is_err());
}

#[tokio::test]
async fn target_subscribe_preserves_explicit_directory_and_its_validation() {
    let mut request = subscribe("session_explicit");
    if let Request::Subscribe { working_dir, .. } = &mut request {
        *working_dir = Some("/workspace/explicit".into());
    }
    resolve_target_subscribe_working_dir(
        &mut request,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await
    .unwrap();
    assert_eq!(
        initial_subscribe_working_dir(&request).unwrap(),
        "/workspace/explicit"
    );
    if let Request::Subscribe { working_dir, .. } = &mut request {
        *working_dir = Some("relative".into());
    }
    resolve_target_subscribe_working_dir(
        &mut request,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await
    .unwrap();
    assert!(initial_subscribe_working_dir(&request).is_err());
}
