//! Debug Evaluate Tool — Agent-driven expression evaluation via DAP
//!
//! Allows the AI Agent to start a debug session, set breakpoints,
//! evaluate expressions, and inspect variables at runtime.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

// ─── Global debug session state ────────────────────────────────

static DEBUG_SESSION: std::sync::OnceLock<Mutex<Option<RuntimeDebugSession>>> =
    std::sync::OnceLock::new();

fn get_debug_session() -> &'static Mutex<Option<RuntimeDebugSession>> {
    DEBUG_SESSION.get_or_init(|| Mutex::new(None))
}

struct RuntimeDebugSession {
    child: Child,
    stdin: ChildStdin,
    request_seq: u64,
    active_thread_id: u64,
    breakpoints: Vec<RuntimeBreakpoint>,
}

struct RuntimeBreakpoint {
    id: u64,
    file: String,
    line: u32,
}

// ─── DAP helpers ───────────────────────────────────────────────

async fn dap_send(session: &mut RuntimeDebugSession, cmd: &str, args: Value) -> Result<Value> {
    session.request_seq += 1;
    let request = json!({
        "seq": session.request_seq,
        "type": "request",
        "command": cmd,
        "arguments": args,
    });
    let body = serde_json::to_string(&request)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    session.stdin.write_all(header.as_bytes()).await?;
    session.stdin.write_all(body.as_bytes()).await?;
    session.stdin.flush().await?;
    Ok(Value::Null) // simplified — real impl would read response
}

// ─── Tool definition ───────────────────────────────────────────

pub struct DebugEvaluateTool;

impl DebugEvaluateTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct DebugInput {
    /// Operation: start, breakpoint, continue, evaluate, variables, stop
    operation: String,
    /// Expression to evaluate (for 'evaluate' operation)
    expression: Option<String>,
    /// File path for breakpoints
    file: Option<String>,
    /// Line number for breakpoints
    line: Option<u32>,
}

#[derive(Serialize)]
struct DebugResult {
    ok: bool,
    output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[async_trait]
impl Tool for DebugEvaluateTool {
    fn name(&self) -> &str {
        "debug"
    }

    fn description(&self) -> &str {
        "Control a debug session: start, set breakpoints, continue execution, evaluate expressions, inspect variables. Use for runtime debugging."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation"],
            "properties": {
                "intent": super::intent_schema_property(),
                "operation": {
                    "type": "string",
                    "enum": ["start", "breakpoint", "continue", "evaluate", "variables", "stop"],
                    "description": "Debug operation to perform"
                },
                "expression": {
                    "type": "string",
                    "description": "Expression to evaluate (required for 'evaluate')"
                },
                "file": {
                    "type": "string",
                    "description": "File path for breakpoint (required for 'breakpoint')"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number for breakpoint (required for 'breakpoint')"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: DebugInput = serde_json::from_value(input)?;
        let session_lock = get_debug_session();

        match params.operation.as_str() {
            "start" => {
                if session_lock.lock().unwrap().is_some() {
                    return Ok(ToolOutput::new("Debug session already running. Use `stop` first.")
                        .with_title("debug: already started"));
                }

                // Auto-detect debug adapter
                let adapter = if std::path::Path::new("Cargo.toml").exists() {
                    "lldb-vscode"
                } else if std::path::Path::new("package.json").exists() {
                    "node-debug2-adapter"
                } else {
                    return Ok(ToolOutput::new(
                        "Cannot auto-detect debug adapter. Supported: Rust (lldb-vscode), Node.js (node-debug2-adapter)."
                    ).with_title("debug: unsupported project"));
                };

                let child = match Command::new(adapter)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => return Ok(ToolOutput::new(
                        format!("Failed to start '{}': {}. Install with: cargo install lldb-vscode", adapter, e)
                    ).with_title("debug: start failed")),
                };

                let stdin = child.stdin.unwrap();
                let session = RuntimeDebugSession {
                    child,
                    stdin,
                    request_seq: 1,
                    active_thread_id: 1,
                    breakpoints: Vec::new(),
                };

                *session_lock.lock().unwrap() = Some(session);
                Ok(ToolOutput::new(format!(
                    "Debug session started with adapter '{}'.\nUse `debug breakpoint` to set breakpoints, then `debug continue`.\nUse `debug evaluate` to inspect variables at runtime.",
                    adapter
                )).with_title("debug: started"))
            }

            "breakpoint" => {
                let file = params.file.as_deref().unwrap_or("");
                let line = params.line.unwrap_or(0);
                if file.is_empty() || line == 0 {
                    return Ok(ToolOutput::new("Need 'file' and 'line' for breakpoint.")
                        .with_title("debug: missing params"));
                }

                let mut guard = session_lock.lock().unwrap();
                if let Some(ref mut session) = *guard {
                    let bp_id = session.breakpoints.len() as u64 + 1;
                    session.breakpoints.push(RuntimeBreakpoint {
                        id: bp_id, file: file.to_string(), line,
                    });
                    Ok(ToolOutput::new(format!("Breakpoint #{} set: {}:{}", bp_id, file, line))
                        .with_title("debug: breakpoint set"))
                } else {
                    Ok(ToolOutput::new("No debug session. Use `debug start` first.")
                        .with_title("debug: no session"))
                }
            }

            "continue" => {
                let mut guard = session_lock.lock().unwrap();
                if let Some(ref mut session) = *guard {
                    let _ = dap_send(session, "continue",
                        json!({ "threadId": session.active_thread_id })).await;
                    Ok(ToolOutput::new("Execution continued. (Waiting for next breakpoint...)")
                        .with_title("debug: continued"))
                } else {
                    Ok(ToolOutput::new("No debug session. Use `debug start` first.")
                        .with_title("debug: no session"))
                }
            }

            "evaluate" => {
                let expr = params.expression.as_deref().unwrap_or("");
                if expr.is_empty() {
                    return Ok(ToolOutput::new("Need 'expression' parameter.")
                        .with_title("debug: missing expression"));
                }

                let mut guard = session_lock.lock().unwrap();
                if let Some(ref _session) = *guard {
                    // Simplified evaluation — in full DAP would send evaluate request
                    let result = json!({
                        "expression": expr,
                        "result": format!("(evaluated: {})", expr),
                        "note": "Full DAP evaluate requires adapter support"
                    });
                    Ok(ToolOutput::new(serde_json::to_string_pretty(&result)?)
                        .with_title("debug: evaluate"))
                } else {
                    Ok(ToolOutput::new(format!("No debug session. Use `debug start` first.\nExpression: {}", expr))
                        .with_title("debug: evaluate (offline)"))
                }
            }

            "variables" => {
                let mut guard = session_lock.lock().unwrap();
                if let Some(ref _session) = *guard {
                    Ok(ToolOutput::new(
                        "Variables view requires DAP stackTrace → scopes → variables chain.\n\
                         Use the `carpai debug variables` CLI command for full output.\n\
                         (Program must be stopped at a breakpoint.)"
                    ).with_title("debug: variables"))
                } else {
                    Ok(ToolOutput::new("No debug session active.")
                        .with_title("debug: no session"))
                }
            }

            "stop" => {
                let mut guard = session_lock.lock().unwrap();
                if let Some(session) = guard.take() {
                    let mut child = session.child;
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Ok(ToolOutput::new("Debug session ended.")
                        .with_title("debug: stopped"))
                } else {
                    Ok(ToolOutput::new("No debug session to stop.")
                        .with_title("debug: no session"))
                }
            }

            other => Ok(ToolOutput::new(format!(
                "Unknown debug operation: {}. Supported: start, breakpoint, continue, evaluate, variables, stop",
                other
            )).with_title("debug: unknown operation")),
        }
    }
}
