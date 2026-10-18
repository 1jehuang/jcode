//! Embedded QuickJS sandbox for Jcode's opt-in Codemode.
//!
//! Codemode lets the model write a short JavaScript program that orchestrates
//! harness tool calls (built-in tools and MCP tools alike) instead of issuing
//! them one model turn at a time. Intermediate results stay inside the
//! sandbox, so only what the script returns reaches the transcript.
//!
//! The sandbox runs where the harness runs, not where `bash` runs. It has no
//! filesystem, network, timer, or process APIs. The only way to affect the
//! outside world is through [`Host::call_tool`], which the harness routes
//! through its normal tool registry and session policy.
//!
//! QuickJS runtimes are `!Send`, so each run executes on a short-lived
//! dedicated thread with a current-thread Tokio runtime. Host calls are
//! spawned back onto the caller's runtime, so host futures only need to be
//! `Send`.

use futures::future::BoxFuture;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(target_os = "freebsd"))]
mod engine;

/// Whether this build includes the QuickJS engine. False on FreeBSD, where
/// rquickjs has no prebuilt bindings.
pub const AVAILABLE: bool = cfg!(not(target_os = "freebsd"));

/// A tool the script may call through `tools.<name>(args)`.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Harness side of the sandbox.
pub trait Host: Send + Sync + 'static {
    /// Execute a tool. `Ok` carries the tool result (already decoded to JSON
    /// when the output was JSON, otherwise a string). `Err` becomes a thrown
    /// `Error` inside the script.
    fn call_tool(&self, name: String, args: Value) -> BoxFuture<'static, Result<Value, String>>;
}

/// Resource limits for one run.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// QuickJS heap limit in bytes.
    pub memory_bytes: usize,
    /// Wall-clock limit for the whole script, including awaited tool calls.
    pub timeout: Duration,
    /// Maximum tool calls per run.
    pub max_tool_calls: usize,
    /// Maximum bytes of captured `console.log` output.
    pub max_log_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            timeout: Duration::from_secs(300),
            max_tool_calls: 2_000,
            max_log_bytes: 64 * 1024,
        }
    }
}

/// Result of one run.
#[derive(Debug, Clone, Default)]
pub struct RunOutput {
    /// JSON value the script returned (`None` for `undefined`).
    pub result: Option<Value>,
    /// Error message if the script threw, timed out, or was cancelled.
    pub error: Option<String>,
    /// Captured `console.*` lines.
    pub logs: Vec<String>,
    /// Whether log capture hit [`Limits::max_log_bytes`].
    pub logs_truncated: bool,
    /// Number of tool calls the script made.
    pub tool_calls: usize,
    /// Session store after the run (persisted by the caller between runs).
    pub store: Map<String, Value>,
    /// Wall-clock duration of the run.
    pub elapsed: Duration,
}

/// Run `code` as the body of an async function. `return` sets the result.
///
/// Must be called from within a Tokio runtime; host calls are spawned onto it.
/// Dropping the returned future cancels the script.
#[cfg(not(target_os = "freebsd"))]
pub async fn run(
    code: String,
    host: Arc<dyn Host>,
    tools: Vec<ToolInfo>,
    store: Map<String, Value>,
    limits: Limits,
) -> RunOutput {
    engine::run(code, host, tools, store, limits).await
}

/// FreeBSD builds have no JavaScript engine. Report that instead of failing
/// the whole build.
#[cfg(target_os = "freebsd")]
pub async fn run(
    _code: String,
    _host: Arc<dyn Host>,
    _tools: Vec<ToolInfo>,
    store: Map<String, Value>,
    _limits: Limits,
) -> RunOutput {
    RunOutput {
        error: Some("codemode is not available on FreeBSD builds".into()),
        store,
        ..RunOutput::default()
    }
}

/// Decode a tool's text output into the most useful script value: parsed JSON
/// when the output is a JSON object or array, otherwise the raw string.
pub fn decode_tool_output(text: &str) -> Value {
    let trimmed = text.trim();
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
    {
        return value;
    }
    Value::String(text.to_string())
}

#[cfg(all(test, not(target_os = "freebsd")))]
mod tests;
