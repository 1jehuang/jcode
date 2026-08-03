//! Schema snapshot tests: fail if the wire shape changes accidentally.

use crate::*;

#[test]
fn client_frame_wire_shape() {
    let frame = ClientFrame::new(
        7,
        ApiRequest::SendMessage {
            session_id: "s1".into(),
            content: "hi".into(),
            images: vec![],
        },
    );
    let json = serde_json::to_string(&frame).unwrap();
    assert_eq!(
        json,
        r#"{"v":1,"id":7,"req":"send_message","session_id":"s1","content":"hi"}"#
    );
}

#[test]
fn server_frame_wire_shape() {
    let frame = ServerFrame::reply(
        3,
        ApiEvent::HelloOk {
            version: 1,
            server: "jcode/0.55.1".into(),
            capabilities: vec![],
        },
    );
    let json = serde_json::to_string(&frame).unwrap();
    assert_eq!(
        json,
        r#"{"v":1,"reply_to":3,"ev":"hello_ok","version":1,"server":"jcode/0.55.1"}"#
    );
}

#[test]
fn unknown_event_kind_is_skippable() {
    let json = r#"{"v":1,"ev":"some_future_event","payload":123}"#;
    let frame: ServerFrame = serde_json::from_str(json).unwrap();
    assert_eq!(frame.event, ApiEvent::Unknown);
}

#[test]
fn unknown_fields_are_ignored() {
    let json = r#"{"v":1,"ev":"turn_done","session_id":"s1","future_field":true}"#;
    let frame: ServerFrame = serde_json::from_str(json).unwrap();
    assert_eq!(
        frame.event,
        ApiEvent::TurnDone {
            session_id: "s1".into()
        }
    );
}

#[test]
fn request_roundtrip() {
    let reqs = [
        ApiRequest::Hello {
            min_version: 1,
            max_version: 1,
            client: "test/0".into(),
        },
        ApiRequest::ListSessions,
        ApiRequest::CreateSession { working_dir: None },
        ApiRequest::AttachSession {
            session_id: "s1".into(),
        },
        ApiRequest::Cancel {
            session_id: "s1".into(),
        },
        ApiRequest::PermissionResponse {
            session_id: "s1".into(),
            request_id: "p1".into(),
            decision: PermissionDecision::Allow,
        },
        ApiRequest::Ping,
    ];
    for req in reqs {
        let frame = ClientFrame::new(1, req);
        let json = serde_json::to_string(&frame).unwrap();
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(frame, back);
    }
}

#[test]
fn client_handshake_over_in_memory_pipe() {
    // Server side scripted: one hello_ok line.
    let reply = serde_json::to_string(&ServerFrame::reply(
        1,
        ApiEvent::HelloOk {
            version: 1,
            server: "jcode/test".into(),
            capabilities: vec!["sessions".into()],
        },
    ))
    .unwrap()
        + "\n";
    let mut out: Vec<u8> = Vec::new();
    let mut client = HarnessClient::new(std::io::BufReader::new(reply.as_bytes()), &mut out);
    let frame = client.hello("test-client/0.1").unwrap();
    match frame.event {
        ApiEvent::HelloOk { version, .. } => assert_eq!(version, 1),
        other => panic!("unexpected event: {other:?}"),
    }
    let sent = String::from_utf8(out).unwrap();
    assert!(sent.contains(r#""req":"hello""#), "sent: {sent}");
}

/// The TypeScript SDK mirrors these enums by hand, so a variant added here
/// without a matching entry in `sdk/typescript/src/protocol.ts` silently
/// leaves every JS client unable to name the new frame. Checking from the
/// Rust side means the guard runs in the normal `cargo test` suite, where
/// the change is actually being made, rather than only in the SDK's own
/// Node tests which a Rust-only contributor never runs.
#[test]
fn typescript_sdk_lists_every_variant() {
    let Some(sdk) = sdk_protocol_source() else {
        // Absent in vendored/packaged builds: nothing to check.
        return;
    };
    for (file, enum_name) in [("requests.rs", "ApiRequest"), ("events.rs", "ApiEvent")] {
        for variant in enum_variants(file, enum_name) {
            assert!(
                sdk.contains(&format!("\"{variant}\"")),
                "{enum_name}::{variant} is missing from sdk/typescript/src/protocol.ts; \
                 add it to the union and to KNOWN_*_KINDS"
            );
        }
    }
}

fn sdk_protocol_source() -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/typescript/src/protocol.ts");
    std::fs::read_to_string(path).ok()
}

/// Snake-cased variant names of `enum_name`, excluding the `Unknown` catch-all.
fn enum_variants(file: &str, enum_name: &str) -> Vec<String> {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(file),
    )
    .expect("read API source");
    let start = source
        .find(&format!("pub enum {enum_name} {{"))
        .expect("enum present");
    let body = &source[start..];
    let end = body.find("\n}").unwrap_or(body.len());
    body[..end]
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("    ")?;
            if rest.starts_with(' ') {
                return None;
            }
            let name: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect();
            let mut chars = name.chars();
            let first = chars.next()?;
            if !first.is_ascii_uppercase() || name == "Unknown" {
                return None;
            }
            Some(snake_case(&name))
        })
        .collect()
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
