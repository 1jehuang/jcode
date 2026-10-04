//! Shared fixtures and mock providers for the `client_actions` tests.
//!
//! The tests themselves live in `client_actions/tests/*.rs`, one module per
//! handler (`split`, `resume_all`, `notify_session`, plus the session-state and
//! swarm-toggle regressions). They are split out because this file passed the
//! 1200-line test-size ratchet at 1234 LOC. Each child repeats the `use` block
//! below, because `use super::*` does not re-export a parent's private
//! imports; only the fixtures and mock providers here are shared.

#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{
    NotifySessionContext, clone_split_session, create_transfer_child_session,
    handle_notify_session, handle_rename_session, handle_resume_all_sessions, handle_set_feature,
    handle_split,
};

use crate::agent::Agent;

use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};

use crate::protocol::{FeatureToggle, ServerEvent};

use crate::provider::{EventStream, Provider};

use crate::server::{ClientConnectionInfo, SwarmMember};

use crate::tool::Registry;

use anyhow::Result;

use async_stream::stream;

use async_trait::async_trait;

use std::collections::{HashMap, HashSet, VecDeque};

use std::path::PathBuf;

use std::sync::{Arc, Mutex as StdMutex};

use std::time::Instant;

use tokio::sync::{Mutex, RwLock, mpsc};

use tokio::time::{Duration, timeout};

#[allow(clippy::type_complexity)]
fn empty_swarm_status_state() -> (
    Arc<RwLock<HashMap<String, std::collections::HashSet<String>>>>,
    Arc<RwLock<std::collections::VecDeque<crate::server::SwarmEvent>>>,
    Arc<std::sync::atomic::AtomicU64>,
    tokio::sync::broadcast::Sender<crate::server::SwarmEvent>,
) {
    let (swarm_event_tx, _) = tokio::sync::broadcast::channel(16);
    (
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(std::collections::VecDeque::new())),
        Arc::new(std::sync::atomic::AtomicU64::new(0)),
        swarm_event_tx,
    )
}

struct MockProvider;

#[derive(Clone, Default)]
struct StreamingMockProvider {
    responses: Arc<StdMutex<VecDeque<Vec<StreamEvent>>>>,
}

impl StreamingMockProvider {
    fn queue_response(&self, events: Vec<StreamEvent>) {
        self.responses.lock().unwrap().push_back(events);
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!(
            "mock provider complete should not be called in client_actions tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockProvider)
    }
}

#[async_trait]
impl Provider for StreamingMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let events = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        let stream = stream! {
            for event in events {
                yield Ok(event);
            }
        };
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct SplitTestHome {
    _directory: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
}

impl SplitTestHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("split test home");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", directory.path());
        Self {
            _directory: directory,
            previous_home,
        }
    }
}

impl Drop for SplitTestHome {
    fn drop(&mut self) {
        if let Some(home) = &self.previous_home {
            crate::env::set_var("JCODE_HOME", home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

async fn new_split_test_agent() -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    Arc::new(Mutex::new(Agent::new_with_initial_working_dir(
        provider,
        registry,
        Some("/project/empty-split"),
    )))
}

fn split_response(
    rx: &mut mpsc::UnboundedReceiver<ServerEvent>,
    request_id: u64,
) -> crate::session::Session {
    let event = rx.try_recv().expect("split must respond");
    let ServerEvent::SplitResponse {
        id,
        new_session_id,
        new_session_name,
    } = event
    else {
        panic!("expected SplitResponse, got {event:?}");
    };
    assert_eq!(id, request_id);
    assert!(!new_session_name.is_empty());
    assert!(rx.try_recv().is_err(), "exactly one split response");
    crate::session::Session::load(&new_session_id).expect("fork must be persisted for attachment")
}

/// Build a live SwarmMember with a real client attachment so the resume-all
/// sweep treats it as live. Returns the member and the receiver for events
/// fanned out to that attachment.
/// `working_dir` is the session's project directory. The sweep is now scoped to
/// the caller's project, so a test that wants its session resumed must give it a
/// directory the caller also has.
fn live_member(
    session_id: &str,
    working_dir: Option<&str>,
) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
    let (attach_tx, attach_rx) = mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: session_id.to_string(),
        event_tx: mpsc::unbounded_channel().0,
        event_txs: HashMap::from([("client-1".to_string(), attach_tx)]),
        working_dir: working_dir.map(std::path::PathBuf::from),
        swarm_id: None,
        swarm_enabled: false,
        status: "ready".to_string(),
        detail: None,
        task_label: None,
        friendly_name: Some("otter".to_string()),
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    };
    (member, attach_rx)
}

// Every child module below repeats this file's `use` block: a glob import
// reaches the parent's own items but not its private imports, so a child that
// needs `HashMap` has to import it. Grouped by concern rather than by file
// size, so the split says what each module is about.
#[path = "tests/resume_all.rs"]
mod resume_all;

#[path = "tests/notify_session.rs"]
mod notify_session;

#[path = "tests/split.rs"]
mod split;

#[path = "tests/session_state.rs"]
mod session_state;

#[path = "tests/swarm_toggle.rs"]
mod swarm_toggle;
