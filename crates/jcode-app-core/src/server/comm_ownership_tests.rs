//! Socket-level regression tests for P0.1 session ownership.
//!
//! `comm_auth_tests.rs` proves the capability arithmetic in isolation. These
//! tests prove the property that actually matters at the boundary: a client
//! that is attached to session A cannot reach session B's data.
//!
//! Two rejection paths are covered, because `Comm*` requests arrive two ways:
//!
//! 1. **Subscribed connection.** The connection owns `client_session_id`. A
//!    `Comm*` naming any other session must be refused before dispatch, so no
//!    handler ever runs against the other session.
//! 2. **One-shot control connection.** The connection has no session at all,
//!    so authority comes from the in-process capability. A request without a
//!    valid capability must be refused, and one carrying session A's
//!    capability must not thereby gain session B.
//!
//! The assertion is deliberately about *absence of effect*, not about the
//! error text: session B's shared context is compared before and after. A
//! rejection that still mutated B would pass a message-only test.

#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::comm_auth::{CAPABILITY_FIELD, mint};
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::protocol::{Request, ServerEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{RwLock, broadcast};

/// A provider that must never be called: these tests only exercise
/// authorization, which has to reject before any agent work begins.
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
        anyhow::bail!("session ownership tests must not invoke a provider")
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[derive(Clone)]
struct Fixture {
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, crate::server::SharedContext>>>>,
    swarm_members: Arc<RwLock<HashMap<String, crate::server::SwarmMember>>>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            swarm_members: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Snapshot the whole shared-context store as a comparable string.
    ///
    /// `handle_comm_share` keys entries by *swarm id*, resolved from the session
    /// named in the request, so a per-session snapshot would miss a write that
    /// landed under the wrong session's swarm. Snapshotting everything makes the
    /// assertion about absence of effect rather than about key naming.
    async fn snapshot(&self) -> String {
        let ctx = self.shared_context.read().await;
        let mut outer: Vec<_> = ctx
            .iter()
            .map(|(swarm, entries)| {
                let mut inner: Vec<String> = entries
                    .iter()
                    .map(|(key, entry)| format!("{}={}", key, entry.value))
                    .collect();
                inner.sort();
                (swarm.clone(), inner.join("\n"))
            })
            .collect();
        outer.sort();
        outer
            .into_iter()
            .map(|(swarm, inner)| format!("[{swarm}]\n{inner}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Start a daemon-side connection loop wired to a fresh fixture.
fn spawn_server(
    stream: crate::transport::Stream,
    fixture: Fixture,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    use crate::server::{
        AwaitMembersRuntime, ClientDebugState, FileTouchService, SessionAgents,
        SwarmMutationRuntime,
    };

    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::new()));
    let (_global_event_tx, _) = broadcast::channel(8);
    let provider_template: Arc<dyn Provider> = Arc::new(NoRequests);
    let client_count = Arc::new(RwLock::new(0usize));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::clone(&fixture.swarm_members);
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
    let (_debug_response_tx, _) = broadcast::channel(8);
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let global_is_processing = Arc::new(RwLock::new(false));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    tokio::spawn(super::client_lifecycle::handle_client(
        stream,
        sessions,
        _global_event_tx,
        provider_template,
        global_is_processing,
        Arc::new(RwLock::new(String::new())),
        client_count,
        client_connections,
        swarm_members,
        swarms_by_id,
        Arc::clone(&fixture.shared_context),
        swarm_plans,
        swarm_coordinators,
        file_touch,
        channel_subscriptions,
        channel_subscriptions_by_session,
        client_debug_state,
        _debug_response_tx,
        event_history,
        event_counter,
        swarm_event_tx,
        "jcode-test".to_string(),
        "\u{1f9ea}".to_string(),
        mcp_pool,
        shutdown_signals,
        soft_interrupt_queues,
        AwaitMembersRuntime::default(),
        SwarmMutationRuntime::default(),
    ))
}

/// Serialize a `Comm*` request, optionally attaching a capability.
fn encode(request: &Request, capability: Option<&str>) -> String {
    let mut value = serde_json::to_value(request).expect("serialize request");
    if let Some(capability) = capability {
        value
            .as_object_mut()
            .expect("request object")
            .insert(
                CAPABILITY_FIELD.to_string(),
                serde_json::Value::String(capability.to_string()),
            );
    }
    serde_json::to_string(&value).expect("serialize request") + "\n"
}

#[tokio::test]
async fn one_shot_comm_without_capability_is_rejected_and_leaves_target_untouched() {
    let fixture = Fixture::new();
    let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");

    let server_task = spawn_server(server_stream, fixture.clone());

    let (reader, mut writer) = client_stream.into_split();
    let mut reader = BufReader::new(reader);

    // The attacker names session B. No capability is presented.
    let request = Request::CommShare {
        id: 1,
        session_id: "session-b".to_string(),
        key: "stolen".to_string(),
        value: "attacker payload".to_string(),
        append: false,
    };
    writer
        .write_all(encode(&request, None).as_bytes())
        .await
        .expect("write request");

    let response = read_event(&mut reader).await;
    match response {
        ServerEvent::Error { id, message, .. } => {
            assert_eq!(id, 1);
            assert!(
                message.contains("capability"),
                "rejection should explain a capability is required, got: {message}"
            );
        }
        other => panic!("expected an authorization error, got {other:?}"),
    }

    assert_eq!(
        fixture.snapshot().await,
        "",
        "a rejected CommShare must not create shared context for session B"
    );

    drop(writer);
    let _ = server_task.await;
}

#[tokio::test]
async fn one_shot_comm_cannot_replay_another_sessions_capability() {
    let fixture = Fixture::new();
    let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");

    let server_task = spawn_server(server_stream, fixture.clone());

    let (reader, mut writer) = client_stream.into_split();
    let mut reader = BufReader::new(reader);

    // A valid capability for session A must not authorize writing as session
    // B. The capability is verified against the claimed id, so replaying it
    // across sessions fails.
    let request = Request::CommShare {
        id: 2,
        session_id: "session-b".to_string(),
        key: "stolen".to_string(),
        value: "replayed".to_string(),
        append: false,
    };
    let payload = encode(&request, Some(&mint("session-a")));
    writer
        .write_all(payload.as_bytes())
        .await
        .expect("write request");

    let response = read_event(&mut reader).await;
    match response {
        ServerEvent::Error { id, .. } => assert_eq!(id, 2),
        other => panic!("expected an authorization error, got {other:?}"),
    }

    assert_eq!(
        fixture.snapshot().await,
        "",
        "replaying session A's capability must not let the caller write as session B"
    );

    drop(writer);
    let _ = server_task.await;
}

#[tokio::test]
async fn subscribed_comm_naming_another_session_is_rejected() {
    let fixture = Fixture::new();
    let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");

    let server_task = spawn_server(server_stream, fixture.clone());

    let (reader, mut writer) = client_stream.into_split();
    let mut reader = BufReader::new(reader);

    // Attach as a real session first, so the connection owns a session and the
    // structural path applies rather than the capability path. `Subscribe` binds
    // whatever session id the daemon resolves, so read it back from `SessionId`
    // instead of assuming the requested one sticks.
    let session_a = subscribe(&mut reader, &mut writer).await;

    // Now, on that same connection, name a different session.
    let request = Request::CommShare {
        id: 2,
        session_id: "session-b".to_string(),
        key: "stolen".to_string(),
        value: "cross-session".to_string(),
        append: false,
    };
    writer
        .write_all(encode(&request, None).as_bytes())
        .await
        .expect("write request");

    // The daemon must not Ack a request it is going to refuse.
    let response = read_event(&mut reader).await;
    match response {
        ServerEvent::Error { id, message, .. } => {
            assert_eq!(id, 2);
            assert!(
                message.contains("ownership mismatch"),
                "rejection should name the ownership mismatch, got: {message}"
            );
        }
        other => panic!("expected an ownership mismatch error, got {other:?}"),
    }

    assert_eq!(
        fixture.snapshot().await,
        "",
        "a connection attached to session A must not write session B's shared context"
    );
    assert_ne!(
        session_a, "session-b",
        "the guard would be untested if this connection owned session B"
    );

    drop(writer);
    drop(reader);
    let _ = server_task.await;
}

#[tokio::test]
async fn subscribed_comm_for_own_session_still_works() {
    let fixture = Fixture::new();
    let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");

    let server_task = spawn_server(server_stream, fixture.clone());

    let (reader, mut writer) = client_stream.into_split();
    let mut reader = BufReader::new(reader);

    let session_a = subscribe(&mut reader, &mut writer).await;

    // Same shape as the rejected request above, but naming the connection's
    // own session. This must still be allowed, or the fix is just a denial
    // of service rather than a boundary.
    let request = Request::CommShare {
        id: 3,
        session_id: session_a.clone(),
        key: "mine".to_string(),
        value: "self-authored".to_string(),
        append: false,
    };
    writer
        .write_all(encode(&request, None).as_bytes())
        .await
        .expect("write request");

    // Ack arrives rather than an error.
    let response = read_event(&mut reader).await;
    match response {
        ServerEvent::Ack { id } => assert_eq!(id, 3),
        other => panic!("expected an ack for a legitimate request, got {other:?}"),
    }

    // Drop *both* halves. Dropping only the writer leaves the read half open,
    // so the daemon's `read_line` never sees EOF and this test would hang
    // forever at the join below instead of failing with a useful message.
    drop(writer);
    drop(reader);
    let _ = server_task.await;
}

/// Bind this connection to a session and return the id the daemon assigned.
///
/// The request omits `target_session_id`, so the daemon creates a fresh session
/// whose id it only reveals through `ServerEvent::SessionId`. Reading it back
/// matters: the ownership guard compares against the *bound* id, so a test that
/// assumed a chosen id would be asserting against the wrong value.
async fn subscribe<R, W>(reader: &mut BufReader<R>, writer: &mut W) -> String
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let working_dir = std::env::temp_dir().join("jcode-comm-ownership-project");
    let request = Request::Subscribe {
        system_prompt: None,
        supports_pdf_panels: false,
        id: 1,
        working_dir: Some(working_dir.to_string_lossy().to_string()),
        selfdev: None,
        target_session_id: None,
        client_instance_id: None,
        client_has_local_history: false,
        allow_session_takeover: false,
        crash_on_disconnect: false,
        continue_on_disconnect: false,
        terminal_env: Vec::new(),
    };
    writer
        .write_all(
            &(serde_json::to_string(&request).expect("serialize subscribe") + "\n").as_bytes(),
        )
        .await
        .expect("write subscribe");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut bound: Option<String> = None;
    // The handshake emits `SessionId` and then `Done`, with other events (state
    // snapshot, plans) in between. Drain to `Done` so the next read starts on
    // the event the test cares about instead of a leftover handshake event.
    //
    // Each read carries its own timeout: a deadline checked only *between* reads
    // cannot fire while a read is blocked on a peer that will never answer, which
    // is exactly the hang this guards against.
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "Subscribe never completed; session id seen so far: {bound:?}"
        );
        let event = tokio::time::timeout(remaining, read_event(reader))
            .await
            .unwrap_or_else(|_| {
                panic!("Subscribe stalled with no event; session id seen so far: {bound:?}")
            });
        match event {
            ServerEvent::SessionId {
                session_id: assigned,
            } => bound = Some(assigned),
            ServerEvent::Done { .. } => {
                return bound.expect("Subscribe completed without reporting a session id");
            }
            ServerEvent::Error { message, .. } => panic!("subscribe failed: {message}"),
            // The handshake interleaves state, swarm, and MCP snapshots before
            // `SessionId`. They are irrelevant here; only the id and the
            // terminating `Done` matter.
            _ => continue,
        }
    }
}

/// Read one newline-delimited event, failing the test if the peer closes.
async fn read_event<R>(reader: &mut BufReader<R>) -> ServerEvent
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("timed out waiting for an event")
        .expect("read event bytes");
    assert_ne!(read, 0, "connection closed before sending an event");
    serde_json::from_str(&line).unwrap_or_else(|error| panic!("decode event: {error} in {line:?}"))
}