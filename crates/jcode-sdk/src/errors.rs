//! Errors the SDK reports.
//!
//! Mirrors the TypeScript SDK's `HarnessError`: one error type with a machine
//! readable code, so a caller can branch on the cause instead of matching on
//! prose that is free to change.

use std::fmt;

/// What went wrong, in a form a caller can branch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The socket could not be dialed. Usually: the harness is not running.
    ConnectFailed,
    /// The version handshake was refused or answered with the wrong frame.
    HandshakeFailed,
    /// No reply arrived within the request timeout.
    Timeout,
    /// The connection is closed; nothing further can be sent on it.
    Disconnected,
    /// The reply was a different event kind than the request implies.
    UnexpectedReply,
    /// The harness answered with an error frame. Carries the wire code.
    Harness(jcode_harness_api::ErrorCode),
    /// I/O or JSON trouble on the transport.
    Transport,
    /// Starting a runtime (daemon or bridge) failed.
    LaunchFailed,
    /// The configured jcode executable could not be started.
    JcodeNotFound,
    /// The private bridge exited before its socket became ready.
    StartupFailed,
    /// The private bridge did not become ready before its deadline.
    StartupTimeout,
    /// An instance home was unsafe or aliased the user's own home.
    InvalidInstanceHome,
    /// An option was outside its documented range.
    InvalidOption,
    /// An operation requires a cloneable native socket transport.
    UnsupportedTransport,
    /// A global event consumer fell behind its bounded queue.
    EventBufferOverflow,
}

impl ErrorKind {
    /// Stable, lowercase identifier. Matches the TS SDK's code strings so the
    /// two SDKs describe the same failure by the same name.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConnectFailed => "connect_failed",
            Self::HandshakeFailed => "handshake_failed",
            Self::Timeout => "timeout",
            Self::Disconnected => "disconnected",
            Self::UnexpectedReply => "unexpected_reply",
            Self::Transport => "transport",
            Self::LaunchFailed => "launch_failed",
            Self::JcodeNotFound => "jcode_not_found",
            Self::StartupFailed => "startup_failed",
            Self::StartupTimeout => "startup_timeout",
            Self::InvalidInstanceHome => "invalid_instance_home",
            Self::InvalidOption => "invalid_option",
            Self::UnsupportedTransport => "unsupported_transport",
            Self::EventBufferOverflow => "event_buffer_overflow",
            Self::Harness(code) => match code {
                jcode_harness_api::ErrorCode::UnsupportedVersion => "unsupported_version",
                jcode_harness_api::ErrorCode::UnknownRequest => "unknown_request",
                jcode_harness_api::ErrorCode::UnknownSession => "unknown_session",
                jcode_harness_api::ErrorCode::InvalidRequest => "invalid_request",
                jcode_harness_api::ErrorCode::Internal => "internal",
            },
        }
    }
}

/// An SDK failure: a code plus a sentence that says what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// The stable code for this failure.
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.code(), self.message)
    }
}

impl std::error::Error for Error {}

impl From<jcode_harness_api::FrameError> for Error {
    fn from(error: jcode_harness_api::FrameError) -> Self {
        let kind = match error {
            jcode_harness_api::FrameError::Eof => ErrorKind::Disconnected,
            _ => ErrorKind::Transport,
        };
        Self::new(kind, error.to_string())
    }
}

/// Result alias used throughout the SDK.
pub type Result<T> = std::result::Result<T, Error>;
