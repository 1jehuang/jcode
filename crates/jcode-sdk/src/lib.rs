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
//! println!("{}", turn.final_text);
//! # Ok::<(), jcode_sdk::Error>(())
//! ```
//!
//! `TurnResult::text` retains the whole turn, including tool narration.
//! `TurnResult::final_text` selects the last completed assistant message and
//! `TurnResult::messages` exposes all completed messages. On older bridges
//! without framing, `final_text` falls back to `text` and `messages` is empty.
//! Reasoning interleaved between text chunks does not split a message.
//! Streaming consumers should correlate `TextDelta`, `TextDone`, and
//! `TextReplace` by their optional message id. An empty replacement retracts
//! text, including messages completed before a provider retry. Wait for
//! `TurnDone` before publishing an irreversible final answer. These ids are
//! connection-local stream correlators, not persisted transcript ids.
//!
//! # Full system prompt override
//!
//! ```no_run
//! use jcode_sdk::{ConnectOptions, CreateSessionOptions, JcodeClient};
//!
//! let client = JcodeClient::connect(ConnectOptions::default())?;
//! let session = client.create_session_with_options(CreateSessionOptions {
//!     working_dir: Some("/path/to/project".into()),
//!     system_prompt: Some("You are a concise code reviewer.".into()),
//! })?;
//! # Ok::<(), jcode_sdk::Error>(())
//! ```
//!
//! `system_prompt` replaces the **entire assembled system prompt**, not just the
//! base prompt. Default instructions and assembled instruction/context additions
//! are not appended. The override is immutable after session creation and is
//! persisted by the runtime for resume. `None` preserves normal prompt assembly,
//! while `Some(String::new())` explicitly selects an empty system prompt.
//! Existing `create_session(working_dir)` calls retain their normal behavior.
//!
//! # Session tool control
//!
//! Tool configuration is memory-only. Reconfigure after reconnecting, daemon
//! restart, session resume, or fork, before starting the next turn.
//! Custom tools run in your application,
//! not in the SDK: subscribe before sending the message, validate each call's
//! input, and return a result using the call id. Do not use blocking `run()` on
//! the same thread unless its `RunOptions::on_event` callback handles tool calls
//! (a cloned client can submit results from that callback).
//!
//! ```no_run
//! use jcode_sdk::{ApiEvent, ConnectOptions, JcodeClient, SessionToolDefinition,
//!                 ToolConfiguration};
//! use serde_json::json;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = JcodeClient::connect(ConnectOptions::default())?;
//! if !client.supports("session_tools") {
//!     return Err("This harness does not support session tools".into());
//! }
//! let session = client.create_session(None)?;
//! let id = &session.session_id;
//! client.configure_tools(id, ToolConfiguration {
//!     enabled: Some(vec!["greet".into()]),
//!     disabled: vec![],
//!     custom: vec![SessionToolDefinition {
//!         name: "greet".into(),
//!         description: "Greet a person by name".into(),
//!         parameters: serde_json::from_value(json!({
//!             "type": "object", "properties": {"name": {"type": "string"}},
//!             "required": ["name"], "additionalProperties": false
//!         }))?,
//!     }],
//! })?;
//! println!("Available tools: {:?}", client.list_tools(id)?);
//! let events = client.events(Some(id));
//! client.send_message(id, "Use greet to say hello to Ada", vec![], None)?;
//! let mut completed = false;
//! while let Some(event) = events.next() {
//!     match event {
//!         ApiEvent::ToolCall { session_id, call_id, name, input } => {
//!             let (output, error) = match (name.as_str(), input["name"].as_str()) {
//!                 ("greet", Some(person)) => (format!("Hello, {person}!"), None),
//!                 _ => (String::new(), Some("Unknown tool or invalid input".into())),
//!             };
//!             client.submit_tool_result(&session_id, &call_id, &output, error)?;
//!         }
//!         ApiEvent::TextDelta { text, .. } => print!("{text}"),
//!         ApiEvent::TurnDone { .. } => { completed = true; break; }
//!         ApiEvent::Error { code, message } => {
//!             return Err(format!("{code:?}: {message}").into());
//!         }
//!         _ => {}
//!     }
//! }
//! if !completed { return Err("Harness disconnected before the turn finished".into()); }
//! # Ok(())
//! # }
//! ```
//!
//! Connect to a remote user's persistent harness through system OpenSSH:
//! ```no_run
//! use jcode_sdk::{JcodeClient, SshConnectOptions};
//! let client = JcodeClient::connect_ssh(SshConnectOptions::new("my-ssh-alias"))?;
//! let sessions = client.list_sessions()?;
//! # Ok::<(), jcode_sdk::Error>(())
//! ```
//! This uses existing SSH config, keys, agent, and known_hosts. Verify new host
//! keys with SSH before connecting. The remote needs a release supporting
//! `jcode api --stdio`, which is never installed or updated by this SDK. A POSIX
//! remote shell is required. Dropping the final client clone kills and reaps its
//! SSH child, not the remote shared daemon. `connect_timeout` bounds startup and
//! hello independently of the ordinary request timeout.

mod auth;
mod client;
mod diagnostics;
mod errors;
mod launch;
mod ssh;
mod structured;
pub mod worktrees;

#[cfg(test)]
#[path = "sdk_tests/parity.rs"]
mod parity_tests;

pub use auth::{
    AuthClient, AuthFlow, AuthInputKind, AuthOptions, AuthPrompt, AuthResult, LoginMethod,
    LoginProvider,
};
pub use client::{
    AssistantTextMessage, ConnectOptions, CreateSessionOptions, EventStream, FileContent,
    FileStatus, GlobalEventStream, GlobalEventsOptions, JcodeClient, RunOptions, RuntimeInfo,
    SearchTextOptions, ToolCall, Transport, TurnResult, UnixTransport, Usage,
};
pub use jcode_applet_types as applet;
pub use diagnostics::{SocketState, Stage, describe_disconnect, explain, human_duration};
pub use errors::{Error, ErrorKind, Result};
pub use jcode_harness_api::{
    SessionEditStats, enrich_sessions_from_edit_stats, enrich_sessions_from_local_edit_stats,
    enrich_sessions_from_local_swarm_state, enrich_sessions_from_swarm_state,
};
pub use launch::{
    LaunchOptions, LaunchedInstance, WakeMode, ensure_runtime, inherit_credentials,
    launch_instance, socket_accepts, user_app_config_dir, user_jcode_home, wait_for_socket,
};
pub use ssh::SshConnectOptions;
#[cfg(unix)]
pub use ssh::{SharedSshTransport, WeakSharedSshTransport};
pub use structured::{
    RunStructuredError, RunStructuredOptions, StructuredEventCallback, StructuredOutputAttempt,
    StructuredOutputError, StructuredOutputSchema, StructuredSchemaError, StructuredTurnResult,
    StructuredValidationIssue,
};

/// The protocol types, re-exported so a client needs one dependency, not two.
pub use jcode_harness_api as api;
pub use jcode_harness_api::{
    ApiEvent, ApiRequest, HistoryMessage, ModelRouteInfo, PermissionDecision, RenderedImage,
    RenderedImageAnchor, RenderedImageSource, ResponseStats, SessionInfo, SessionToolDefinition,
    SidePanelPage, SidePanelPageFormat, SidePanelPageSource, SidePanelSnapshot, TextMatch,
    ToolConfiguration, TurnStopReason, api_socket_path,
};
pub use jcode_harness_api::{ModelUsage, compare_model_usage};
