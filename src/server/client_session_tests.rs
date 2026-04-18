use super::{
    handle_clear_session, handle_reload, handle_resume_session, mark_remote_reload_started,
    rename_shutdown_signal, restored_session_was_interrupted, session_was_interrupted_by_reload,
};
use crate::agent::Agent;
use crate::message::ContentBlock;
use crate::message::{Message, ToolDefinition};
use crate::protocol::ServerEvent;
use crate::provider::{EventStream, Provider};
use crate::server::{
    ClientConnectionInfo, ClientDebugState, FileAccess, SessionInterruptQueues, SwarmEvent,
    SwarmMember, VersionedPlan,
};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

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
        Err(anyhow::anyhow!(
            "mock provider complete should not be called in client_session tests"
        ))
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

fn build_test_agent_with_id(
    provider: Arc<dyn Provider>,
    registry: Registry,
    session_id: &str,
    messages: Vec<crate::session::StoredMessage>,
) -> Agent {
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("mock".to_string());
    session.replace_messages(messages);
    Agent::new_with_session(provider, registry, session, None)
}

async fn collect_events_until_done(
    client_event_rx: &mut mpsc::UnboundedReceiver<ServerEvent>,
    done_id: u64,
) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), client_event_rx.recv())
            .await
            .expect("timed out waiting for server event")
            .expect("expected server event");
        let is_done = matches!(event, ServerEvent::Done { id } if id == done_id);
        events.push(event);
        if is_done {
            break;
        }
    }
    events
}

#[path = "client_session_tests/clear.rs"]
mod clear_tests;
#[path = "client_session_tests/reload.rs"]
mod reload_tests;
#[path = "client_session_tests/resume.rs"]
mod resume_tests;
