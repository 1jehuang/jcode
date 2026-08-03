//! Client behavior against a fake harness.
//!
//! The correlation logic is the part of an SDK that fails silently: a reply
//! delivered to the wrong waiter, or a stream event swallowed by a request
//! that happened to be in flight, produces a hang rather than an error. None
//! of that is visible from a passing `cargo build`, so it is driven here
//! against a scripted server on a real socket pair.

use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame, SessionInfo, read_frame,
    write_frame,
};
use jcode_sdk::{ConnectOptions, JcodeClient, Transport};
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
