//! Client behavior against a fake harness.
//!
//! The correlation logic is the part of an SDK that fails silently: a reply
//! delivered to the wrong waiter, or a stream event swallowed by a request
//! that happened to be in flight, produces a hang rather than an error. None
//! of that is visible from a passing `cargo build`, so it is driven here
//! against a scripted server on a real socket pair.

#![cfg(unix)]

use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ModelRouteInfo, ServerFrame, SessionInfo,
    TextMatch, read_frame, write_frame,
};
use jcode_sdk::{ConnectOptions, JcodeClient, SearchTextOptions, Transport};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::channel;
use std::time::Duration;

/// A socket-pair transport, so the test drives the client over the same code
/// path a real connection uses.
struct PairTransport(UnixStream);

impl Transport for PairTransport {
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self.0.try_clone().expect("socket pair must clone");
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

fn session(id: &str) -> SessionInfo {
    SessionInfo {
        session_id: id.to_string(),
        working_dir: None,
        title: None,
        status: "idle".to_string(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
    }
}

/// Start a fake harness on one end of a socket pair. `handle` is called for
/// every client frame with a writer for replies.
fn fake_harness(handle: impl Fn(&ClientFrame, &mut dyn Write) + Send + 'static) -> JcodeClient {
    let (ours, theirs) = UnixStream::pair().expect("socket pair");
    std::thread::spawn(move || {
        let mut reader = BufReader::new(theirs.try_clone().expect("clone"));
        let mut writer = theirs;
        loop {
            let frame: ClientFrame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(_) => break,
            };
            // The handshake is boilerplate every test would repeat.
            if let ApiRequest::Hello { .. } = frame.request {
                let reply = ServerFrame {
                    v: API_VERSION_MAJOR,
                    reply_to: Some(frame.id),
                    event: ApiEvent::HelloOk {
                        version: API_VERSION_MAJOR,
                        server: "fake-harness/1.0".to_string(),
                        capabilities: vec!["sessions".to_string()],
                    },
                };
                write_frame(&mut writer, &reply).expect("hello reply");
                continue;
            }
            handle(&frame, &mut writer);
        }
    });
    JcodeClient::connect_with(
        Box::new(PairTransport(ours)),
        ConnectOptions {
            request_timeout: Some(Duration::from_secs(5)),
            ensure_runtime: false,
            ..Default::default()
        },
    )
    .expect("handshake must succeed")
}

fn reply(frame: &ClientFrame, event: ApiEvent, writer: &mut dyn Write) {
    let out = ServerFrame {
        v: API_VERSION_MAJOR,
        reply_to: Some(frame.id),
        event,
    };
    write_frame(&mut { writer }, &out).expect("reply");
}

fn push(event: ApiEvent, writer: &mut dyn Write) {
    let out = ServerFrame {
        v: API_VERSION_MAJOR,
        reply_to: None,
        event,
    };
    write_frame(&mut { writer }, &out).expect("push");
}

/// The handshake populates the server identity and capabilities, which is how
/// a client decides what it may depend on.
#[test]
fn the_handshake_reports_the_server_and_its_capabilities() {
    let client = fake_harness(|_, _| {});
    assert_eq!(client.server, "fake-harness/1.0");
    assert!(client.supports("sessions"));
    assert!(
        !client.supports("permissions"),
        "a capability the server did not advertise must not be claimed"
    );
}

#[test]
fn soft_interrupt_with_images_preserves_attachments_and_legacy_helper() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&requests);
    let client = fake_harness(move |frame, writer| {
        seen.lock()
            .expect("request log")
            .push(frame.request.clone());
        reply(frame, ApiEvent::Ok, writer);
    });

    client
        .soft_interrupt_with_images(
            "s1",
            "look",
            vec![("image/png".into(), "aW1hZ2U=".into())],
            true,
        )
        .expect("image interrupt");
    client
        .soft_interrupt("s1", "text only", false)
        .expect("legacy helper");

    let requests = requests.lock().expect("request log");
    assert!(matches!(
        &requests[0],
        ApiRequest::SoftInterrupt {
            session_id,
            content,
            images,
            urgent: true,
        } if session_id == "s1"
            && content == "look"
            && images == &vec![("image/png".into(), "aW1hZ2U=".into())]
    ));
    assert!(matches!(
        &requests[1],
        ApiRequest::SoftInterrupt {
            images,
            urgent: false,
            ..
        } if images.is_empty()
    ));
}

/// GA session-management, runtime, credential, and file methods must preserve
/// the stable protocol shapes while returning ergonomic SDK-owned values.
#[test]
fn ga_runtime_and_file_methods_map_requests_and_typed_replies() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&requests);
    let routes = vec![
        ModelRouteInfo {
            model: "claude".to_string(),
            provider: "anthropic".to_string(),
            api_method: "messages".to_string(),
            available: true,
            detail: "ready".to_string(),
        },
        ModelRouteInfo {
            model: "gemini".to_string(),
            provider: "google".to_string(),
            api_method: "generate_content".to_string(),
            available: true,
            detail: "ready".to_string(),
        },
    ];
    let reply_routes = routes.clone();
    let client = fake_harness(move |frame, writer| {
        seen.lock()
            .expect("request log")
            .push(frame.request.clone());
        let event = match &frame.request {
            ApiRequest::ArchiveSession { .. }
            | ApiRequest::RestoreSession { .. }
            | ApiRequest::SetRetentionPolicy { .. } => ApiEvent::Ok,
            ApiRequest::Ping => ApiEvent::Pong,
            ApiRequest::GetRuntimeInfo { .. } => ApiEvent::RuntimeInfo {
                session_id: "s1".to_string(),
                provider: Some("anthropic".to_string()),
                model: Some("claude".to_string()),
                reasoning_effort: Some("high".to_string()),
                routes: reply_routes.clone(),
            },
            ApiRequest::SetApiKey { provider, .. } => ApiEvent::CredentialUpdated {
                provider: provider.clone(),
                configured: true,
            },
            ApiRequest::ClearApiKey { provider } => ApiEvent::CredentialUpdated {
                provider: provider.clone(),
                configured: false,
            },
            ApiRequest::ReadFile { .. } => ApiEvent::FileContent {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
                content: "hello".to_string(),
                size: 8,
                truncated: true,
            },
            ApiRequest::FindFiles { .. } => ApiEvent::Files {
                session_id: "s1".to_string(),
                paths: vec!["src/a.rs".to_string()],
            },
            ApiRequest::SearchText { .. } => ApiEvent::TextMatches {
                session_id: "s1".to_string(),
                matches: vec![TextMatch {
                    path: "src/a.rs".to_string(),
                    line: 2,
                    column: 3,
                    preview: "  hello".to_string(),
                }],
            },
            ApiRequest::FileStatus { .. } => ApiEvent::FileStatus {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
                exists: true,
                kind: "file".to_string(),
                size: Some(8),
                modified_ms: Some(123),
            },
            other => panic!("unexpected request: {other:?}"),
        };
        reply(frame, event, writer);
    });

    client.archive_session("s1").expect("archive");
    client.restore_session("s1").expect("restore");
    client
        .set_retention_policy(Some(30))
        .expect("retention policy");

    let runtime = client.get_runtime_info("s1").expect("runtime info");
    assert_eq!(runtime.server, "fake-harness/1.0");
    assert_eq!(runtime.protocol_version, API_VERSION_MAJOR);
    assert_eq!(runtime.capabilities, ["sessions"]);
    assert!(runtime.healthy);
    assert_eq!(runtime.session_id, "s1");
    assert_eq!(runtime.provider.as_deref(), Some("anthropic"));
    assert_eq!(runtime.model.as_deref(), Some("claude"));
    assert_eq!(runtime.providers, ["anthropic", "google"]);
    assert_eq!(runtime.routes, routes);

    client.set_api_key("gemini-api", "secret").expect("set key");
    client.clear_api_key("jcode").expect("clear key");

    let content = client
        .read_file("s1", "src/a.rs", Some(5))
        .expect("read file");
    assert_eq!(content.path, "src/a.rs");
    assert_eq!(content.content, "hello");
    assert_eq!(content.size, 8);
    assert!(content.truncated);
    assert_eq!(
        client
            .find_files("s1", "a.rs", Some(4))
            .expect("find files"),
        ["src/a.rs"]
    );
    assert_eq!(
        client
            .search_text(
                "s1",
                "hello",
                SearchTextOptions {
                    path: Some("src".to_string()),
                    limit: Some(2),
                },
            )
            .expect("search text"),
        [TextMatch {
            path: "src/a.rs".to_string(),
            line: 2,
            column: 3,
            preview: "  hello".to_string(),
        }]
    );
    let status = client.file_status("s1", "src/a.rs").expect("file status");
    assert_eq!(status.path, "src/a.rs");
    assert!(status.exists);
    assert_eq!(status.kind, "file");
    assert_eq!(status.size, Some(8));
    assert_eq!(status.modified_ms, Some(123));

    assert_eq!(
        *requests.lock().expect("request log"),
        vec![
            ApiRequest::ArchiveSession {
                session_id: "s1".to_string(),
            },
            ApiRequest::RestoreSession {
                session_id: "s1".to_string(),
            },
            ApiRequest::SetRetentionPolicy {
                archive_after_days: Some(30),
            },
            ApiRequest::Ping,
            ApiRequest::GetRuntimeInfo {
                session_id: "s1".to_string(),
            },
            ApiRequest::SetApiKey {
                provider: "gemini-api".to_string(),
                api_key: "secret".to_string(),
            },
            ApiRequest::ClearApiKey {
                provider: "jcode".to_string(),
            },
            ApiRequest::ReadFile {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
                max_bytes: Some(5),
            },
            ApiRequest::FindFiles {
                session_id: "s1".to_string(),
                query: "a.rs".to_string(),
                limit: Some(4),
            },
            ApiRequest::SearchText {
                session_id: "s1".to_string(),
                query: "hello".to_string(),
                path: Some("src".to_string()),
                limit: Some(2),
            },
            ApiRequest::FileStatus {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
            },
        ]
    );
}

/// Replies must go to the request that asked for them, not to whichever one
/// happens to be waiting. Answered out of order on purpose: in-order delivery
/// would pass even with the correlation removed entirely.
#[test]
fn out_of_order_replies_reach_the_right_waiter() {
    let client = fake_harness(|frame, writer| match &frame.request {
        // Delay the first request's reply so the second overtakes it.
        ApiRequest::AttachSession { session_id } => {
            let id = session_id.clone();
            std::thread::sleep(Duration::from_millis(120));
            reply(
                frame,
                ApiEvent::Attached {
                    session: session(&id),
                },
                writer,
            );
        }
        ApiRequest::Ping => reply(frame, ApiEvent::Pong, writer),
        _ => {}
    });

    let slow = {
        let client = client.clone();
        std::thread::spawn(move || client.attach_session("slow-session"))
    };
    std::thread::sleep(Duration::from_millis(20));
    client
        .ping()
        .expect("the fast request must not wait on the slow one");
    let attached = slow.join().expect("thread").expect("attach must succeed");
    assert_eq!(attached.session_id, "slow-session");
}

/// An error frame becomes a typed failure with the wire code, so a caller can
/// branch on the cause rather than matching on prose.
#[test]
fn an_error_frame_becomes_a_typed_failure() {
    let client = fake_harness(|frame, writer| {
        reply(
            frame,
            ApiEvent::Error {
                code: jcode_harness_api::ErrorCode::UnknownSession,
                message: "no such session".to_string(),
            },
            writer,
        );
    });
    let error = client.attach_session("gone").expect_err("must fail");
    assert_eq!(error.code(), "unknown_session");
    assert!(error.message.contains("no such session"));
}

/// A reply of the wrong kind is a distinct failure from a harness error: it
/// means the client and server disagree about the protocol, not that the
/// request was refused.
#[test]
fn a_reply_of_the_wrong_kind_is_reported_as_such() {
    let client = fake_harness(|frame, writer| reply(frame, ApiEvent::Pong, writer));
    let error = client.list_sessions().expect_err("must fail");
    assert_eq!(error.code(), "unexpected_reply");
}

/// Two subscriptions both see every event: one loop rendering the session must
/// not starve another waiting for a single acknowledgement.
#[test]
fn every_subscriber_sees_every_event() {
    let (ready_tx, ready_rx) = channel();
    let client = fake_harness(move |frame, writer| {
        if let ApiRequest::Ping = frame.request {
            reply(frame, ApiEvent::Pong, writer);
            for i in 0..3 {
                push(
                    ApiEvent::TextDelta {
                        session_id: "s1".to_string(),
                        text: format!("chunk{i}"),
                    },
                    writer,
                );
            }
            push(
                ApiEvent::TurnDone {
                    session_id: "s1".to_string(),
                },
                writer,
            );
            let _ = ready_tx.send(());
        }
    });

    let first = client.events(Some("s1"));
    let second = client.events(None);
    client.ping().expect("ping");
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the fake harness must have pushed its events");

    for stream in [&first, &second] {
        let mut text = String::new();
        loop {
            match stream.next_timeout(Duration::from_secs(5)) {
                Some(ApiEvent::TextDelta { text: chunk, .. }) => text.push_str(&chunk),
                Some(ApiEvent::TurnDone { .. }) | None => break,
                Some(_) => continue,
            }
        }
        assert_eq!(text, "chunk0chunk1chunk2");
    }
}

/// A session filter keeps another session's stream out. A dashboard attached
/// to one session must not render another's deltas into it.
#[test]
fn a_filtered_subscription_only_sees_its_own_session() {
    let client = fake_harness(|frame, writer| {
        if let ApiRequest::Ping = frame.request {
            reply(frame, ApiEvent::Pong, writer);
            push(
                ApiEvent::TextDelta {
                    session_id: "other".to_string(),
                    text: "not mine".to_string(),
                },
                writer,
            );
            push(
                ApiEvent::TextDelta {
                    session_id: "mine".to_string(),
                    text: "mine".to_string(),
                },
                writer,
            );
        }
    });
    let stream = client.events(Some("mine"));
    client.ping().expect("ping");
    match stream.next_timeout(Duration::from_secs(5)) {
        Some(ApiEvent::TextDelta { text, session_id }) => {
            assert_eq!(session_id, "mine");
            assert_eq!(text, "mine", "the other session's delta leaked through");
        }
        other => panic!("expected a text delta for `mine`, got {other:?}"),
    }
}

/// `run` collects one turn: text, reasoning, tool calls, usage. The turn ends
/// on `turn_done` rather than on the stream closing.
#[test]
fn run_collects_one_turn() {
    let client = fake_harness(|frame, writer| {
        if let ApiRequest::SendMessage { session_id, .. } = &frame.request {
            let s = session_id.clone();
            push(
                ApiEvent::MessageAccepted {
                    session_id: s.clone(),
                },
                writer,
            );
            push(
                ApiEvent::ReasoningDelta {
                    session_id: s.clone(),
                    text: "thinking".to_string(),
                },
                writer,
            );
            push(
                ApiEvent::TextDelta {
                    session_id: s.clone(),
                    text: "hello ".to_string(),
                },
                writer,
            );
            push(
                ApiEvent::ToolDone {
                    session_id: s.clone(),
                    call_id: "c1".to_string(),
                    name: "bash".to_string(),
                    output: "ok".to_string(),
                    error: None,
                },
                writer,
            );
            push(
                ApiEvent::TextDelta {
                    session_id: s.clone(),
                    text: "world".to_string(),
                },
                writer,
            );
            push(
                ApiEvent::TokenUsage {
                    session_id: s.clone(),
                    input: 10,
                    output: 5,
                    cache_read_input: Some(2),
                },
                writer,
            );
            push(ApiEvent::TurnDone { session_id: s }, writer);
        }
    });

    let turn = client
        .run("s1", "hi", Default::default())
        .expect("the turn must complete");
    assert_eq!(turn.text, "hello world");
    assert_eq!(turn.reasoning, "thinking");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].name, "bash");
    assert_eq!(turn.usage.expect("usage").input, 10);
}

/// An error mid-turn fails `run` rather than hanging: the harness sends `error`
/// *instead of* `done`, so a client waiting only for `turn_done` waits forever.
#[test]
fn an_error_mid_turn_fails_the_run_instead_of_hanging() {
    let client = fake_harness(|frame, writer| {
        if let ApiRequest::SendMessage { session_id, .. } = &frame.request {
            push(
                ApiEvent::MessageAccepted {
                    session_id: session_id.clone(),
                },
                writer,
            );
            push(
                ApiEvent::Error {
                    code: jcode_harness_api::ErrorCode::Internal,
                    message: "provider exploded".to_string(),
                },
                writer,
            );
        }
    });
    let error = client
        .run("s1", "hi", Default::default())
        .expect_err("must fail");
    assert_eq!(error.code(), "internal");
    assert!(error.message.contains("provider exploded"));
}

/// When the harness goes away, in-flight requests fail instead of blocking on
/// a reply that can never arrive.
#[test]
fn a_lost_connection_fails_requests_in_flight() {
    // A server that reads the request and then hangs up without replying.
    let (ours, theirs) = UnixStream::pair().expect("socket pair");
    std::thread::spawn(move || {
        let mut reader = BufReader::new(theirs.try_clone().expect("clone"));
        let mut writer = theirs;
        let hello: ClientFrame = read_frame(&mut reader).expect("hello");
        write_frame(
            &mut writer,
            &ServerFrame {
                v: API_VERSION_MAJOR,
                reply_to: Some(hello.id),
                event: ApiEvent::HelloOk {
                    version: API_VERSION_MAJOR,
                    server: "fake/1.0".to_string(),
                    capabilities: vec![],
                },
            },
        )
        .expect("hello reply");
        let _: ClientFrame = read_frame(&mut reader).expect("request");
        // Drop both ends: the client's pending request must be failed.
    });

    let client = JcodeClient::connect_with(
        Box::new(PairTransport(ours)),
        ConnectOptions {
            request_timeout: Some(Duration::from_secs(5)),
            ensure_runtime: false,
            ..Default::default()
        },
    )
    .expect("handshake");
    let error = client
        .ping()
        .expect_err("a dropped harness must fail the request");
    assert_eq!(error.code(), "disconnected");
}

#[test]
fn model_switch_preserves_identity_and_catalog_events_around_the_reply() {
    let client = fake_harness(|frame, writer| {
        let ApiRequest::SetModel { session_id, model } = &frame.request else {
            panic!("unexpected request: {:?}", frame.request);
        };
        assert_eq!(session_id, "s1");
        assert_eq!(model, "openai-api:new-model");
        push(
            ApiEvent::ConnectionPhase {
                session_id: session_id.clone(),
                phase: "connecting".into(),
            },
            writer,
        );
        // The stream can interleave with a synchronous command in either order.
        push(
            ApiEvent::ModelInfo {
                session_id: session_id.clone(),
                provider: Some("openai-api".into()),
                model: Some("new-model".into()),
                reasoning_effort: None,
            },
            writer,
        );
        reply(frame, ApiEvent::Ok, writer);
        push(
            ApiEvent::RuntimeInfo {
                session_id: session_id.clone(),
                provider: Some("openai-api".into()),
                model: Some("new-model".into()),
                reasoning_effort: None,
                routes: vec![ModelRouteInfo {
                    model: "new-model".into(),
                    provider: "openai-api".into(),
                    api_method: "responses".into(),
                    available: true,
                    detail: "ready".into(),
                }],
            },
            writer,
        );
    });
    let events = client.events(Some("s1"));
    let unrelated = client.events(Some("s2"));
    client
        .set_model("s1", "openai-api:new-model")
        .expect("model switch");
    assert!(matches!(
        events.next_timeout(Duration::from_secs(1)),
        Some(ApiEvent::ConnectionPhase { .. })
    ));
    assert!(matches!(events.next_timeout(Duration::from_secs(1)),
        Some(ApiEvent::ModelInfo { model, .. }) if model.as_deref() == Some("new-model")));
    assert!(matches!(events.next_timeout(Duration::from_secs(1)),
        Some(ApiEvent::RuntimeInfo { routes, .. }) if routes.len() == 1 && routes[0].available));
    assert!(unrelated.next_timeout(Duration::from_millis(20)).is_none());
}

#[test]
fn model_switch_refusal_is_a_typed_error_not_a_success() {
    let client = fake_harness(|frame, writer| {
        assert!(matches!(frame.request, ApiRequest::SetModel { .. }));
        reply(
            frame,
            ApiEvent::Error {
                code: jcode_harness_api::ErrorCode::InvalidRequest,
                message: "provider credential is missing".into(),
            },
            writer,
        );
    });
    let error = client
        .set_model("s1", "unavailable-model")
        .expect_err("switch must fail");
    assert_eq!(
        error.kind,
        jcode_sdk::ErrorKind::Harness(jcode_harness_api::ErrorCode::InvalidRequest)
    );
    assert!(error.message.contains("credential"));
}
