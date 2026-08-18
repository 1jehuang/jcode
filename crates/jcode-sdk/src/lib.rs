//! Rust SDK for the jcode harness.
//!
//! The API crate (`jcode-harness-api`) defines the wire protocol. This crate
//! is what you actually build a client with: connect, drive sessions, stream
//! events, and get told why the connection died in a sentence a user can act
//! on.
//!
//! It is the Rust counterpart of `sdk/typescript`, and deliberately mirrors it
//! capability for capability. Desktop2 is built on this crate rather than on
//! the raw protocol, so the SDK's design is exercised by a real, shipping
//! client every day rather than only by its own examples.
//!
//! ```no_run
//! use jcode_sdk::{ConnectOptions, JcodeClient, RunOptions};
//!
//! let client = JcodeClient::connect(ConnectOptions::default())?;
//! let session = client.create_session(None)?;
//! let turn = client.run(&session.session_id, "what is 2 + 2?", RunOptions::default())?;
//! println!("{}", turn.text);
//! # Ok::<(), jcode_sdk::Error>(())
//! ```

mod client;
mod diagnostics;
mod errors;
mod launch;
mod structured;

#[cfg(test)]
#[path = "sdk_tests/parity.rs"]
mod parity_tests;

pub use client::{
    ConnectOptions, EventStream, FileContent, FileStatus, GlobalEventStream, GlobalEventsOptions,
    JcodeClient, RunOptions, RuntimeInfo, SearchTextOptions, ToolCall, Transport, TurnResult,
    UnixTransport, Usage,
};
pub use diagnostics::{SocketState, Stage, describe_disconnect, explain, human_duration};
pub use errors::{Error, ErrorKind, Result};
pub use launch::{
    LaunchOptions, LaunchedInstance, ensure_runtime, inherit_credentials, launch_instance,
    socket_accepts, user_app_config_dir, user_jcode_home, wait_for_socket,
};
pub use structured::{
    RunStructuredError, RunStructuredOptions, StructuredEventCallback, StructuredOutputAttempt,
    StructuredOutputError, StructuredOutputSchema, StructuredSchemaError, StructuredTurnResult,
    StructuredValidationIssue,
};

/// The protocol types, re-exported so a client needs one dependency, not two.
pub use jcode_harness_api as api;
pub use jcode_harness_api::{
    ApiEvent, ApiRequest, HistoryMessage, ModelRouteInfo, PermissionDecision, RenderedImage,
    RenderedImageAnchor, RenderedImageSource, SessionInfo, TextMatch, api_socket_path,
};
