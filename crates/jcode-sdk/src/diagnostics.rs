//! Explaining a lost connection in the user's terms.
//!
//! Pure functions so the wording is testable: the whole point of this module
//! is the text, and text that is only exercised by unplugging a socket by hand
//! rots. Lives in the SDK because every long-lived client hits the same
//! failures, and "disconnected: harness API stream closed" is a useless
//! sentence no matter who prints it.

use std::path::Path;
use std::time::Duration;

/// How far a connection attempt got before it died.
///
/// A bridge that exited, a bridge that was replaced, and a session that could
/// not be attached all look identical at the stream level. Recording the stage
/// as the attempt progresses is what lets the report name what was happening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Starting the daemon/bridge, or waiting for their sockets.
    Starting,
    /// Connecting to the API socket.
    Connecting,
    /// Version handshake with the bridge.
    Handshake,
    /// Attach/create sent, waiting for the session.
    Attaching,
    /// Attached; streaming events.
    Streaming,
}

impl Stage {
    /// What the client was doing at this stage, as a participle phrase.
    pub fn doing(self) -> &'static str {
        match self {
            Self::Starting => "starting the jcode runtime",
            Self::Connecting => "connecting to the harness API socket",
            Self::Handshake => "negotiating the harness API version",
            Self::Attaching => "attaching a session",
            Self::Streaming => "streaming the conversation",
        }
    }
}

/// Whether the API socket still answers, used to tell "the bridge died" apart
/// from "the bridge is alive and dropped us".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketState {
    Listening,
    Gone,
}

impl SocketState {
    /// Probe a socket path.
    pub fn probe(path: &Path) -> Self {
        match crate::launch::socket_accepts(path) {
            true => Self::Listening,
            false => Self::Gone,
        }
    }
}

/// Turn a lost connection into a sentence that names the cause.
pub fn describe_disconnect(
    stage: Stage,
    error: &str,
    connected_for: Option<Duration>,
    socket: &Path,
    socket_state: SocketState,
) -> String {
    let lower = error.to_ascii_lowercase();
    let daemon_closed = lower.contains("daemon connection closed");
    let stream_closed = lower.contains("harness api stream closed")
        || lower.contains("harness connection closed")
        || lower.contains("the harness closed the connection")
        || lower.contains("broken pipe")
        || lower.contains("connection reset");
    let cause = if daemon_closed {
        format!(
            "the jcode runtime connection closed while {} (the bridge is still available and will reconnect)",
            stage.doing()
        )
    } else if stream_closed {
        match socket_state {
            // The socket answers but our stream is gone: the bridge we were
            // talking to is not the one on the pathname any more, which is
            // what a second bridge taking the socket looks like from here.
            SocketState::Listening => format!(
                "the harness API bridge dropped our connection while {} \
                 (its socket {} still accepts, so a replacement bridge most \
                 likely took over)",
                stage.doing(),
                socket.display()
            ),
            SocketState::Gone => format!(
                "the harness API bridge exited while {} (its socket {} no \
                 longer accepts connections)",
                stage.doing(),
                socket.display()
            ),
        }
    } else {
        format!("{} failed while {}", explain(error), stage.doing())
    };
    match connected_for {
        Some(uptime) => format!("disconnected after {}: {cause}", human_duration(uptime)),
        None => format!("disconnected: {cause}"),
    }
}

/// Human wording for a failure, when the cause is one the user can act on.
///
/// Provider errors arrive as whatever the HTTP stack said, and "error sending
/// request for url (...): dns error: failed to lookup address information"
/// does not tell a user their wifi is off. Everything unrecognised is passed
/// through unchanged: a wrong guess would be worse than the raw text.
pub fn explain(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    const OFFLINE: [&str; 6] = [
        "dns error",
        "failed to lookup address information",
        "temporary failure in name resolution",
        "network is unreachable",
        "no route to host",
        "name or service not known",
    ];
    if OFFLINE.iter().any(|needle| lower.contains(needle)) {
        return format!("no network connection: {message}");
    }
    message.to_string()
}

/// A duration in the coarsest unit that still reads precisely.
pub fn human_duration(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0 => format!("{}ms", d.as_millis()),
        1..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure a user is most likely to hit, and the one that motivated
    /// this: the machine is offline, so the provider's DNS lookup fails. The
    /// raw text names a URL and a resolver; the user needs to be told their
    /// network is down.
    #[test]
    fn an_offline_failure_is_explained_in_the_users_terms() {
        let raw = "error sending request for url (https://api.example.com/v1/messages): \
                   dns error: failed to lookup address information: Name or service not known";
        let explained = explain(raw);
        assert!(
            explained.starts_with("no network connection"),
            "offline was not named: {explained}"
        );
        assert!(
            explained.contains("dns error"),
            "the underlying cause must survive: {explained}"
        );
    }

    /// Anything unrecognised is passed through untouched. A wrong guess about
    /// a cause is worse than the provider's own words.
    #[test]
    fn an_unrecognised_failure_is_passed_through() {
        assert_eq!(
            explain("overloaded_error: try again"),
            "overloaded_error: try again"
        );
    }

    /// A live socket after our stream died means a replacement bridge, and
    /// saying so is the difference between "restart it" and "stop restarting
    /// it".
    #[test]
    fn a_live_socket_after_a_drop_points_at_a_replacement_bridge() {
        let text = describe_disconnect(
            Stage::Streaming,
            "harness connection closed",
            Some(Duration::from_secs(90)),
            Path::new("/run/user/1000/jcode-api.sock"),
            SocketState::Listening,
        );
        assert!(text.contains("replacement bridge"), "{text}");
        assert!(text.contains("1m30s"), "uptime must be reported: {text}");
    }

    /// A dead socket means the bridge exited, which is the other half of the
    /// same question.
    #[test]
    fn a_dead_socket_after_a_drop_points_at_an_exited_bridge() {
        let text = describe_disconnect(
            Stage::Streaming,
            "harness connection closed",
            None,
            Path::new("/run/user/1000/jcode-api.sock"),
            SocketState::Gone,
        );
        assert!(text.contains("exited"), "{text}");
    }
}
