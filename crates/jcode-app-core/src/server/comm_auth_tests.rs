//! Tests for the `Comm*` session ownership proof.
//!
//! These cover the module in isolation. The dispatch-level guarantee (that a
//! rejected request leaves session B untouched) is covered in
//! `comm_ownership_tests.rs`.

use super::*;
use crate::protocol::Request;

#[test]
fn capability_round_trips_for_its_own_session() {
    assert!(verify("session-a", Some(&mint("session-a"))));
}

#[test]
fn capability_does_not_transfer_to_another_session() {
    // The core of P0.1 in miniature: a valid token for A must not authorize
    // B, otherwise one leaked tool context would unlock every session.
    let capability = mint("session-a");
    assert!(!verify("session-b", Some(&capability)));
}

#[test]
fn verify_rejects_missing_malformed_and_wrong_tokens() {
    assert!(!verify("session-a", None));
    assert!(!verify("session-a", Some("")));
    // Odd length is not valid hex.
    assert!(!verify("session-a", Some("abc")));
    // Valid hex, wrong length and wrong content.
    assert!(!verify("session-a", Some("00")));
    // Non-hex characters must not panic.
    assert!(!verify("session-a", Some("zzzz")));
    // Correct length, wrong content.
    let wrong = "f".repeat(64);
    assert!(!verify("session-a", Some(&wrong)));
}

#[test]
fn capabilities_differ_per_session_and_are_stable() {
    assert_ne!(mint("session-a"), mint("session-b"));
    // Deterministic per session under the daemon secret, so a retry of the
    // same request presents the same token rather than churning one.
    assert_eq!(mint("session-a"), mint("session-a"));
}

#[test]
fn capability_is_hex_of_digest_length() {
    let capability = mint("session-a");
    assert_eq!(capability.len(), 64);
    assert!(capability.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn capability_from_line_reads_the_envelope_field() {
    let line = r#"{"type":"comm_share","id":1,"capability":"abc123"}"#;
    assert_eq!(capability_from_line(line).as_deref(), Some("abc123"));
}

#[test]
fn capability_from_line_is_none_when_absent_or_malformed() {
    assert_eq!(capability_from_line(r#"{"type":"comm_share","id":1}"#), None);
    assert_eq!(capability_from_line("not json"), None);
    // A non-string capability is ignored rather than trusted.
    assert_eq!(
        capability_from_line(r#"{"type":"comm_share","capability":42}"#),
        None
    );
}

#[test]
fn capability_field_is_ignored_by_request_decoding() {
    // The envelope field must not break decoding of any Comm variant. This is
    // what keeps the wire change additive instead of a protocol break.
    let line = r#"{"type":"comm_share","id":7,"session_id":"s","key":"k","value":"v","capability":"deadbeef"}"#;
    let request = crate::protocol::decode_request(line).expect("decodes");
    assert!(matches!(request, Request::CommShare { id: 7, .. }));
    assert!(comm_claimed_session(&request).is_some());
}

#[test]
fn claimed_session_is_the_caller_not_the_target() {
    // Every Comm variant names its caller in one field and its target in a
    // separate one. Getting this backwards would authorize acting as the
    // recipient.
    let share = Request::CommShare {
        id: 1,
        session_id: "caller".into(),
        key: "k".into(),
        value: "v".into(),
        append: false,
    };
    assert_eq!(comm_claimed_session(&share).unwrap().claimed(), "caller");

    let message = Request::CommMessage {
        id: 2,
        from_session: "caller".into(),
        message: "hi".into(),
        to_session: Some("target".into()),
        channel: None,
        delivery: None,
        wake: None,
        tldr: None,
        to_swarm: None,
    };
    assert_eq!(comm_claimed_session(&message).unwrap().claimed(), "caller");

    let stop = Request::CommStop {
        id: 3,
        session_id: "caller".into(),
        target_session: "target".into(),
        force: None,
    };
    assert_eq!(comm_claimed_session(&stop).unwrap().claimed(), "caller");
}

#[test]
fn comm_claimed_session_covers_every_comm_variant() {
    // Guards against a new `Comm*` variant being added without a match arm in
    // `comm_claimed_session`, which would silently disable its ownership check.
    //
    // Enumerates the variant names by reading `wire.rs` rather than listing a
    // sample: a hand-written list only proves the variants someone remembered
    // are covered, which is exactly the case this test exists to catch.
    let wire = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../jcode-protocol/src/wire.rs");
    let source = std::fs::read_to_string(&wire).unwrap_or_else(|error| {
        panic!("cannot read {}: {error}", wire.display())
    });
    let start = source
        .find("pub enum Request {")
        .expect("Request enum not found in wire.rs");
    let body = &source[start..];
    let end = body.find("\n}").unwrap_or(body.len());

    let mut comm_variants: Vec<String> = body[..end]
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("    ")?;
            if rest.starts_with(' ') {
                // A field, not a variant.
                return None;
            }
            let name: String = rest
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            name.starts_with("Comm")
                .then_some(name)
        })
        .collect();
    assert!(
        comm_variants.len() >= 29,
        "expected the full Comm* surface, parsed only {}: {comm_variants:?}",
        comm_variants.len()
    );
    comm_variants.sort();
    comm_variants.dedup();
    // Pin the count so a rename or removal is noticed here rather than
    // silently reducing the checked surface.
    assert_eq!(comm_variants.len(), 31, "Comm variants: {comm_variants:?}");

    for name in &comm_variants {
        assert!(
            comm_auth_source_for(name).is_some(),
            "{name} is a Comm* variant but has no arm in comm_claimed_session; \
             add it or it will bypass the ownership check"
        );
    }
}

/// Find the match arm that claims the caller id for one `Comm*` variant.
///
/// Arms may be written bare or as members of an or-pattern (`| Request::X ..`),
/// so the leading `|` is stripped before matching.
fn comm_auth_source_for(variant: &str) -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/comm_auth.rs");
    let source = std::fs::read_to_string(path).ok()?;
    source
        .lines()
        .map(|line| line.trim_start().trim_start_matches('|').trim_start())
        .find(|line| line.starts_with(&format!("Request::{variant} ")))
        .map(str::to_string)
}

#[test]
fn comm_claimed_session_is_none_for_non_comm_requests() {
    assert!(comm_claimed_session(&Request::Ping { id: 1 }).is_none());
}

#[test]
fn subscribed_caller_owns_its_own_session() {
    let claim = CommClaim {
        claimed: "session-a",
    };
    assert_eq!(claim.authorize_subscribed("session-a"), CommSessionAuth::Owned);
}

#[test]
fn subscribed_caller_cannot_name_another_session() {
    let claim = CommClaim {
        claimed: "session-b",
    };
    assert_eq!(
        claim.authorize_subscribed("session-a"),
        CommSessionAuth::Mismatch {
            claimed: "session-b".into(),
            owned: "session-a".into(),
        }
    );
    assert!(!claim.authorize_subscribed("session-a").is_owned());
}

#[test]
fn one_shot_caller_with_valid_capability_is_owned() {
    let claim = CommClaim {
        claimed: "session-a",
    };
    assert_eq!(
        claim.authorize_one_shot(Some(&mint("session-a"))),
        CommSessionAuth::Owned
    );
}

#[test]
fn one_shot_caller_cannot_replay_capability_across_sessions() {
    // The one-shot path has no connection-owned id to compare against, so
    // the capability itself is the only thing bounding its authority. This is
    // why `authorize_one_shot` verifies against the *claimed* id.
    let claim = CommClaim {
        claimed: "session-b",
    };
    assert_eq!(
        claim.authorize_one_shot(Some(&mint("session-a"))),
        CommSessionAuth::Unauthenticated
    );
    assert!(!claim.authorize_one_shot(Some(&mint("session-a"))).is_owned());
}

#[test]
fn one_shot_caller_without_capability_is_rejected() {
    let claim = CommClaim { claimed: "session-a" };
    assert_eq!(
        claim.authorize_one_shot(None),
        CommSessionAuth::Unauthenticated
    );
}

#[test]
fn rejection_messages_do_not_leak_capability_material() {
    let message = CommSessionAuth::Unauthenticated.rejection_message("comm_share");
    assert!(message.contains("comm_share"));
    for session in ["session-a", "session-b"] {
        assert!(!message.contains(&mint(session)));
    }
}

#[test]
fn mismatch_message_names_both_sessions() {
    let message = CommSessionAuth::Mismatch {
        claimed: "session-b".into(),
        owned: "session-a".into(),
    }
    .rejection_message("comm_message");
    assert!(message.contains("session-b"));
    assert!(message.contains("session-a"));
    assert!(message.contains("comm_message"));
}

#[test]
fn hex_round_trips() {
    let bytes = [0u8, 1, 15, 16, 127, 128, 255];
    let encoded = hex_encode(&bytes);
    assert_eq!(encoded, "00010f107f80ff");
    assert_eq!(hex_decode(&encoded).unwrap(), bytes);
}

#[test]
fn hex_decode_accepts_uppercase() {
    assert_eq!(hex_decode("AbCdEf").unwrap(), vec![0xab, 0xcd, 0xef]);
}

#[test]
fn hex_decode_rejects_bad_input() {
    assert!(hex_decode("abc").is_none());
    assert!(hex_decode("zz").is_none());
}

#[test]
fn constant_time_eq_is_length_sensitive() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
}