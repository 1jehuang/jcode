//! Session ownership proof for `Comm*` requests.
//!
//! One `jcode` daemon serves sessions for many projects at once, so the
//! session id on the wire is a client-supplied string with no ambient
//! project scoping behind it. Before this module, every `Comm*` handler
//! resolved authority from that string alone, which meant any client could
//! name session B and mutate B's shared context, plan, channels, or task
//! graph.
//!
//! There are two ways a `Comm*` request reaches a handler, and each needs a
//! different proof:
//!
//! 1. **Subscribed connections.** The main request loop owns a
//!    `client_session_id`. Authority is structural: the daemon created this
//!    connection for exactly this session, so the connection *is* the proof
//!    and the check is an equality test against its own id.
//! 2. **One-shot control connections.** `tool/communicate.rs` opens a fresh
//!    connection per request, before any `Subscribe`, so there is no
//!    `client_session_id` to compare against. Those requests carry a
//!    capability minted in-process: a digest over a per-daemon secret and
//!    the session id. `jcode-transport` exposes no portable peer-credential
//!    API for either Unix sockets or Windows named pipes, so process
//!    identity cannot be checked directly and a local bearer capability is
//!    the strongest available proof.
//!
//! The two proofs are deliberately not interchangeable. A subscribed
//! connection's authority comes from the daemon having created the session;
//! a one-shot connection's comes from a secret the socket never carries.
//! Accepting either for the other path would mean accepting a self-asserted
//! id on the one-shot path, which is the bug being closed.

use crate::protocol::{Request, ServerEvent};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

/// The per-daemon secret that makes minted capabilities unforgeable.
///
/// Process-wide by design, and deliberately **not** keyed by project. A
/// capability is only meaningful against the daemon that minted it, and
/// there is exactly one daemon per machine serving every project over one
/// shared socket, so per-project scoping would add no isolation.
fn daemon_secret() -> &'static [u8; 32] {
    static SECRET: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    SECRET.get_or_init(|| {
        let mut bytes = [0u8; 32];
        // A capability is a bearer token living as long as the daemon, and
        // `rand`'s OS-backed entropy is what the rest of the process already
        // draws ids from, so this needs no new dependency. No `hmac` crate is
        // available in-tree, so this is a plain digest over
        // length-prefixed secret||message rather than a keyed MAC. It is
        // never used as a general-purpose MAC, and the secret is never
        // transmitted or logged.
        use rand::RngCore;
        rand::rngs::ThreadRng::default().fill_bytes(&mut bytes);
        bytes
    })
}

/// Mint the capability authorizing one-shot `Comm*` requests for `session_id`.
///
/// Only callable from inside the daemon process. That is the security
/// property: a socket client can present a capability but cannot mint one,
/// because minting requires the process-local secret.
pub(crate) fn mint(session_id: &str) -> String {
    let mut hasher = Sha256::new();
    // Length-prefix both inputs so the digest cannot be reinterpreted as a
    // digest over a different (secret, message) split.
    hasher.update((daemon_secret().len() as u64).to_le_bytes());
    hasher.update(daemon_secret());
    hasher.update((session_id.len() as u64).to_le_bytes());
    hasher.update(session_id.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Verify a presented capability against `session_id`.
///
/// Returns `false` for a missing, malformed, or wrong token rather than
/// panicking, so a hostile client cannot probe the verifier by crashing the
/// daemon.
fn verify(session_id: &str, presented: Option<&str>) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    // Cheap shape rejection first; the constant-time compare below is over
    // raw bytes so no timing signal leaks the digest's contents.
    let Some(expected) = hex_decode(&mint(session_id)) else {
        return false;
    };
    let Some(actual) = hex_decode(presented) else {
        return false;
    };
    constant_time_eq(&actual, &expected)
}

/// The capability field carried alongside a `Comm*` request on the wire.
///
/// Kept out of the `Request` enum on purpose: `Request` is
/// internally tagged with `#[serde(tag = "type")]` and does not set
/// `deny_unknown_fields`, so an envelope field is ignored by every variant
/// without adding a 29th field to 29 variants. Authentication material
/// belongs beside the message, not inside its typed shape.
pub(crate) const CAPABILITY_FIELD: &str = "capability";

/// Read the capability out of a raw request line.
///
/// Returns `None` for a missing or non-string capability. A malformed line
/// is not treated as an error here: the request itself is decoded and
/// validated separately, and a bad capability simply fails to verify.
pub(crate) fn capability_from_line(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()?
        .get(CAPABILITY_FIELD)?
        .as_str()
        .map(str::to_string)
}

/// Authorize a `Comm*` request that arrived on a one-shot control connection.
///
/// Pre-`Subscribe` connections have no session attached, so structural
/// ownership does not exist yet and the in-process capability is the only
/// available proof. `request_line` is the raw JSON because the capability
/// rides in the envelope where `Request` decoding cannot carry it.
///
/// Called before the `Ack`, so a rejected request is never reported as
/// acknowledged, and it writes the refusal to the socket before returning so
/// the caller learns why instead of seeing the connection close silently. A
/// silent close would be indistinguishable from a daemon crash to a client
/// that is trying to report an authorization failure.
///
/// Returns `true` when the request was rejected.
pub(crate) async fn authorize_lightweight_comm(
    request: &Request,
    request_line: &str,
    writer: &Arc<Mutex<crate::transport::WriteHalf>>,
) -> bool {
    let Some(claim) = comm_claimed_session(request) else {
        // Not a `Comm*` request; nothing to authorize here.
        return false;
    };
    let capability = capability_from_line(request_line);
    let auth = claim.authorize_one_shot(capability.as_deref());
    if auth.is_owned() {
        return false;
    }
    let request_id = request.id();
    let request_type = super::client_lifecycle_logging::request_type_from_line(request_line);
    let message = auth.rejection_message(&request_type);
    let owned = auth.owned_session_id().unwrap_or("<none>");
    crate::logging::warn(&format!(
        "Rejected one-shot {request_type} naming session {} without a valid capability (owns {owned})",
        claim.claimed()
    ));
    let _ = super::client_writer::write_direct_event(
        writer,
        &ServerEvent::Error {
            id: request_id,
            message,
            retry_after_secs: None,
        },
    )
    .await;
    true
}

/// The caller-identity fields a `Comm*` request carries.
///
/// Every `Comm*` variant names its own caller in exactly one field:
/// `session_id` for all but `comm_message`, which uses `from_session`.
/// Targets always live in a separate field (`to_session`, `target_session`,
/// `proposer_session`), which is why this one field is the whole ownership
/// surface.
///
/// Returns `None` for non-`Comm*` requests.
pub(crate) fn comm_claimed_session(request: &Request) -> Option<CommClaim<'_>> {
    let claimed = match request {
        Request::CommMessage { from_session, .. } => from_session.as_str(),
        Request::CommShare { session_id, .. }
        | Request::CommRead { session_id, .. }
        | Request::CommSetSwarmLabel { session_id, .. }
        | Request::CommChannelMembers { session_id, .. }
        | Request::CommProposePlan { session_id, .. }
        | Request::CommApprovePlan { session_id, .. }
        | Request::CommRejectPlan { session_id, .. }
        | Request::CommSeedGraph { session_id, .. }
        | Request::CommExpandNode { session_id, .. }
        | Request::CommCompleteNode { session_id, .. }
        | Request::CommInjectGap { session_id, .. }
        | Request::CommSpawn { session_id, .. }
        | Request::CommList { session_id, .. }
        | Request::CommListSwarms { session_id, .. }
        | Request::CommListChannels { session_id, .. }
        | Request::CommListModels { session_id, .. }
        | Request::CommStop { session_id, .. }
        | Request::CommAssignRole { session_id, .. }
        | Request::CommSummary { session_id, .. }
        | Request::CommStatus { session_id, .. }
        | Request::CommReport { session_id, .. }
        | Request::CommReadContext { session_id, .. }
        | Request::CommPlanStatus { session_id, .. }
        | Request::CommResyncPlan { session_id, .. }
        | Request::CommAssignTask { session_id, .. }
        | Request::CommAssignNext { session_id, .. }
        | Request::CommTaskControl { session_id, .. }
        | Request::CommSubscribeChannel { session_id, .. }
        | Request::CommUnsubscribeChannel { session_id, .. }
        | Request::CommAwaitMembers { session_id, .. } => session_id.as_str(),
        _ => return None,
    };
    Some(CommClaim { claimed })
}

/// The caller id a `Comm*` request names, as an owned `String`.
///
/// Used by the in-process producer to mint the matching capability without
/// re-matching every variant.
pub(crate) fn claimed_session_id(request: &Request) -> Option<String> {
    comm_claimed_session(request).map(|claim| claim.claimed().to_string())
}

/// The caller identity named by a `Comm*` request.
pub(crate) struct CommClaim<'a> {
    claimed: &'a str,
}

impl<'a> CommClaim<'a> {
    /// The session id the request claims to act as.
    pub(crate) fn claimed(&self) -> &'a str {
        self.claimed
    }

    /// Authorize a request arriving on a connection subscribed to
    /// `client_session_id`.
    pub(crate) fn authorize_subscribed(&self, client_session_id: &str) -> CommSessionAuth {
        authorize_subscribed(self.claimed, client_session_id)
    }

    /// Authorize a request arriving on a one-shot control connection.
    ///
    /// The capability is verified against the id the request *claims*, not
    /// against any session the connection is known to own. That is what
    /// stops a valid token for session A from being replayed to authorize
    /// session B.
    pub(crate) fn authorize_one_shot(&self, capability: Option<&str>) -> CommSessionAuth {
        authorize_one_shot(self.claimed, capability)
    }
}

/// Authorize a `Comm*` request arriving on a subscribed connection.
pub(crate) fn authorize_subscribed(claimed: &str, client_session_id: &str) -> CommSessionAuth {
    if claimed == client_session_id {
        CommSessionAuth::Owned
    } else {
        CommSessionAuth::Mismatch {
            claimed: claimed.to_string(),
            owned: client_session_id.to_string(),
        }
    }
}

/// Authorize a `Comm*` request arriving on a one-shot control connection.
pub(crate) fn authorize_one_shot(claimed: &str, capability: Option<&str>) -> CommSessionAuth {
    if verify(claimed, capability) {
        CommSessionAuth::Owned
    } else {
        CommSessionAuth::Unauthenticated
    }
}

/// Reject a `Comm*` request whose claim failed authorization.
///
/// Sends a `ServerEvent::Error` on the request's own id so the caller can
/// correlate the failure, and logs at warn level: a cross-session `Comm*` is
/// not a client bug to be swallowed, it is a rejected authorization attempt.
///
/// Returns `true` when the request was rejected, so call sites can write
/// `if reject_unauthorized_comm(..) { continue; }`.
pub(crate) fn reject_unauthorized_comm(
    claim: &CommClaim<'_>,
    auth: CommSessionAuth,
    request_type: &str,
    request_id: u64,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) -> bool {
    if auth.is_owned() {
        return false;
    }
    let owned = auth.owned_session_id().unwrap_or("<none>");
    let _ = client_event_tx.send(ServerEvent::Error {
        id: request_id,
        message: auth.rejection_message(request_type),
        retry_after_secs: None,
    });
    crate::logging::warn(&format!(
        "Rejected {request_type} naming session {} from a connection owning {owned}",
        claim.claimed()
    ));
    true
}

/// The outcome of checking a `Comm*` caller's claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommSessionAuth {
    /// The caller proved ownership of the session it named, so the request
    /// may proceed with that id unchanged.
    Owned,
    /// A subscribed connection named a session it does not own.
    Mismatch { claimed: String, owned: String },
    /// A one-shot connection presented no usable capability.
    Unauthenticated,
}

impl CommSessionAuth {
    /// Whether the request may proceed.
    pub(crate) fn is_owned(&self) -> bool {
        matches!(self, CommSessionAuth::Owned)
    }

    /// The session this connection legitimately owns, when that is known.
    pub(crate) fn owned_session_id(&self) -> Option<&str> {
        match self {
            CommSessionAuth::Owned => None,
            CommSessionAuth::Mismatch { owned, .. } => Some(owned),
            CommSessionAuth::Unauthenticated => None,
        }
    }

    /// A client-facing error message for a rejected claim.
    ///
    /// Never includes capability material, so a rejection cannot be used as
    /// an oracle for a valid token.
    pub(crate) fn rejection_message(&self, request_type: &str) -> String {
        match self {
            CommSessionAuth::Mismatch { claimed, owned } => format!(
                "{request_type}: session ownership mismatch (this connection owns session \
                 {owned}, request named {claimed})"
            ),
            CommSessionAuth::Unauthenticated => format!(
                "{request_type}: this connection is not attached to a session, so a session \
                 capability is required"
            ),
            CommSessionAuth::Owned => {
                unreachable!("rejection_message called on an authorized claim")
            }
        }
    }
}

/// Compare two byte strings without an early return.
///
/// `subtle`'s `ConstantTimeEq` is not a dependency here, so this does the
/// same job with a plain accumulator. Length is compared up front, which is
/// safe: the expected length is a fixed property of the digest and reveals
/// nothing about the secret.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing into a `String` is infallible.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        out.push((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?);
    }
    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[path = "comm_auth_tests.rs"]
mod tests;
