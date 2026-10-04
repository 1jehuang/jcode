//! One persistent `claude` child per provider instance and the turn driver.
//!
//! The child speaks the Agent SDK stream-json protocol: host -> child lines on
//! stdin (`user`, `control_request`, `control_response`), child -> host lines
//! on stdout. A reader task owns stdout and routes every line to the active
//! turn's channel. Control requests that need no turn state are answered by
//! the reader itself so a child never stalls between turns.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use jcode_message_types::ToolDefinition;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::control;
use crate::settings::{self, ClaudeCodeInstance, SpawnSpec};

/// How long the `initialize` control round trip may take.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// Grace period after a control `interrupt` before the child is killed.
pub const INTERRUPT_GRACE: Duration = Duration::from_secs(3);

/// Data the CLI reports in its `initialize` response.
#[derive(Debug, Clone, Default)]
pub struct InitInfo {
    pub email: Option<String>,
    pub subscription: Option<String>,
    pub organization: Option<String>,
    /// `(alias value, resolved model id)` pairs, e.g. `("opus", "claude-opus-5-5")`.
    pub models: Vec<(String, String)>,
}

impl InitInfo {
    pub fn from_response(response: &Value) -> Self {
        let account = response.get("account");
        let field = |key: &str| {
            account
                .and_then(|a| a.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let models = response
            .get("models")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|m| {
                        let value = m.get("value").and_then(Value::as_str)?.to_string();
                        let resolved = m
                            .get("resolvedModel")
                            .and_then(Value::as_str)
                            .unwrap_or(&value)
                            .to_string();
                        Some((value, resolved))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            email: field("email"),
            subscription: field("subscriptionType"),
            organization: field("organization"),
            models,
        }
    }

    /// Concrete model ids the CLI offers (aliases resolved, deduplicated).
    pub fn model_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, resolved) in &self.models {
            let id = resolved.trim();
            if id.starts_with("claude-") && !out.iter().any(|m| m == id) {
                out.push(id.to_string());
            }
        }
        out
    }
}

/// Everything needed to (re)spawn a child.
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub binary: String,
    pub instance: ClaudeCodeInstance,
    pub spec: SpawnSpec,
    pub cwd: Option<PathBuf>,
    pub append_system_prompt: String,
}

/// A line routed to the active turn.
#[derive(Debug)]
pub enum ChildEvent {
    Line(Value),
    /// stdout closed (the child exited). Carries recent stderr.
    Exited(String),
}

/// What a turn sees of the child's stdin plus its routing table.
pub struct ChildHandle {
    pub stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    /// Sender for the active turn, if any.
    turn_tx: StdMutex<Option<mpsc::UnboundedSender<ChildEvent>>>,
    /// Pending host -> child control requests awaiting a response.
    pending: StdMutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    stderr_tail: Arc<StdMutex<String>>,
    pub launch: LaunchConfig,
    pub init: StdMutex<InitInfo>,
    exited: Arc<std::sync::atomic::AtomicBool>,
    /// jcode tools offered over the SDK MCP server (latest turn's list).
    pub tools: StdMutex<Vec<ToolDefinition>>,
    /// Model the child currently runs (changes with control `set_model`).
    model: StdMutex<String>,
}

impl ChildHandle {
    pub fn is_alive(&self) -> bool {
        !self.exited.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_model(&self, model: &str) {
        *self.model.lock().unwrap_or_else(|p| p.into_inner()) = model.to_string();
    }

    pub fn set_tools(&self, tools: &[ToolDefinition]) {
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = tools.to_vec();
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn session_id(&self) -> &str {
        self.launch.spec.session.session_id()
    }

    /// Write one JSON line to the child's stdin.
    pub async fn send(&self, value: &Value) -> Result<()> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| anyhow!("Claude Code stdin is closed"))?;
        let mut line = serde_json::to_string(value)?;
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .await
            .context("write to Claude Code stdin")?;
        stdin.flush().await.context("flush Claude Code stdin")?;
        Ok(())
    }

    /// Send a host control request and wait for its response.
    pub async fn control(&self, request: Value, timeout: Duration) -> Result<Value> {
        let request_id = format!("jcode-{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(request_id.clone(), tx);
        if let Err(err) = self
            .send(&control::control_request(&request_id, request))
            .await
        {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&request_id);
            return Err(err);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(response))) => Ok(response),
            Ok(Ok(Err(error))) => Err(anyhow!("Claude Code control request failed: {error}")),
            Ok(Err(_)) => Err(anyhow!(
                "Claude Code exited before answering a control request{}",
                self.stderr_suffix()
            )),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&request_id);
                Err(anyhow!(
                    "Claude Code did not answer a control request in time"
                ))
            }
        }
    }

    /// Route stdout lines to `tx` until it is replaced or cleared.
    pub fn attach_turn(&self, tx: mpsc::UnboundedSender<ChildEvent>) {
        *self.turn_tx.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
    }

    pub fn detach_turn(&self) {
        *self.turn_tx.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .trim()
            .to_string()
    }

    fn stderr_suffix(&self) -> String {
        let tail = self.stderr_tail();
        if tail.is_empty() {
            String::new()
        } else {
            format!(": {tail}")
        }
    }

    /// Stop the child: close stdin, SIGTERM, then SIGKILL.
    pub async fn shutdown(&self) {
        self.exited.store(true, std::sync::atomic::Ordering::SeqCst);
        self.stdin.lock().await.take();
        let mut guard = self.child.lock().await;
        let Some(child) = guard.as_mut() else {
            return;
        };
        if tokio::time::timeout(Duration::from_millis(500), child.wait())
            .await
            .is_ok()
        {
            guard.take();
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            // SAFETY: plain signal to our own child pid.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
            if tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_ok()
            {
                guard.take();
                return;
            }
        }
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        guard.take();
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Spawn a child, start its reader tasks and run the `initialize` handshake.
pub async fn spawn(launch: LaunchConfig, tools: &[ToolDefinition]) -> Result<Arc<ChildHandle>> {
    let argv = settings::build_argv(&launch.spec);
    let mut command = Command::new(&launch.binary);
    command
        .args(&argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in settings::instance_env(&launch.instance) {
        command.env(key, value);
    }
    if let Some(cwd) = launch.cwd.as_ref().filter(|dir| dir.is_dir()) {
        command.current_dir(cwd);
    }
    jcode_base::logging::info(&format!(
        "Claude Code: spawning `{}` (instance {}, model {}, session {}, cwd {})",
        launch.binary,
        launch.instance.id,
        launch.spec.model,
        launch.spec.session.session_id(),
        launch
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<inherited>".into())
    ));
    let mut child = command.spawn().with_context(|| {
        format!(
            "Failed to start the Claude Code CLI `{}`. Install Claude Code or set [provider.claude_code].binary.",
            launch.binary
        )
    })?;
    let stdin = child.stdin.take().context("Claude Code stdin")?;
    let stdout = child.stdout.take().context("Claude Code stdout")?;
    let stderr = child.stderr.take().context("Claude Code stderr")?;

    let stderr_tail = Arc::new(StdMutex::new(String::new()));
    let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = launch.spec.model.clone();
    let handle = Arc::new(ChildHandle {
        stdin: Mutex::new(Some(stdin)),
        child: Mutex::new(Some(child)),
        turn_tx: StdMutex::new(None),
        pending: StdMutex::new(HashMap::new()),
        stderr_tail: stderr_tail.clone(),
        launch,
        init: StdMutex::new(InitInfo::default()),
        exited: exited.clone(),
        tools: StdMutex::new(tools.to_vec()),
        model: StdMutex::new(model),
    });

    // stderr: keep a short tail for error messages, log everything at debug.
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            jcode_base::logging::debug(&format!("Claude Code stderr: {line}"));
            let mut tail = stderr_tail.lock().unwrap_or_else(|p| p.into_inner());
            tail.push_str(&line);
            tail.push('\n');
            if tail.len() > 4000 {
                let cut = tail.len() - 4000;
                let cut = (cut..tail.len())
                    .find(|i| tail.is_char_boundary(*i))
                    .unwrap_or(tail.len());
                tail.drain(..cut);
            }
        }
    });

    // stdout: route lines.
    let weak = Arc::downgrade(&handle);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) | Err(_) => break,
            };
            let Some(handle) = weak.upgrade() else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                jcode_base::logging::debug(&format!(
                    "Claude Code: non-JSON stdout line: {}",
                    trimmed.chars().take(200).collect::<String>()
                ));
                continue;
            };
            route_line(&handle, value).await;
        }
        if let Some(handle) = weak.upgrade() {
            handle
                .exited
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let tail = handle.stderr_tail();
            for (_, tx) in handle
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .drain()
            {
                let _ = tx.send(Err(format!("Claude Code exited. {tail}")));
            }
            let tx = handle
                .turn_tx
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(tx) = tx {
                let _ = tx.send(ChildEvent::Exited(tail));
            }
        } else {
            exited.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });

    let init_request = control::initialize_request(
        handle.launch.spec.expose_mcp,
        &handle.launch.append_system_prompt,
    );
    let started = now_millis();
    let response = handle
        .control(init_request, INITIALIZE_TIMEOUT)
        .await
        .map_err(|err| {
            let tail = handle.stderr_tail();
            if tail.is_empty() {
                err
            } else {
                anyhow!("{err}\n{tail}")
            }
        })?;
    let info = InitInfo::from_response(&response);
    jcode_base::logging::info(&format!(
        "Claude Code: initialized in {}ms (account {}, plan {}, {} models)",
        now_millis().saturating_sub(started),
        info.email.as_deref().unwrap_or("?"),
        info.subscription.as_deref().unwrap_or("?"),
        info.models.len()
    ));
    *handle.init.lock().unwrap_or_else(|p| p.into_inner()) = info;
    Ok(handle)
}

/// Route one stdout line: control responses resolve pending requests, MCP
/// handshake messages and idle control requests are answered here, everything
/// else goes to the turn.
async fn route_line(handle: &Arc<ChildHandle>, value: Value) {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "control_request"
        && let Some(request) = value.get("request")
        && request.get("subtype").and_then(Value::as_str) == Some("mcp_message")
    {
        let message = request.get("message").cloned().unwrap_or(Value::Null);
        let tools = handle.tools();
        if let control::McpAction::Reply(reply) = control::handle_mcp_message(&message, &tools) {
            let request_id = value
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let _ = handle
                .send(&control::control_success(
                    request_id,
                    control::mcp_response_body(reply),
                ))
                .await;
            return;
        }
    }
    match kind {
        "control_response" => {
            let response = value.get("response").cloned().unwrap_or(Value::Null);
            let request_id = response
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let tx = handle
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&request_id);
            if let Some(tx) = tx {
                let outcome = if response.get("subtype").and_then(Value::as_str) == Some("error") {
                    Err(response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string())
                } else {
                    Ok(response.get("response").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(outcome);
            }
        }
        "keep_alive" | "control_cancel_request" => {}
        _ => {
            let tx = handle
                .turn_tx
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            match tx {
                Some(tx) => {
                    if let Err(mpsc::error::SendError(ChildEvent::Line(value))) =
                        tx.send(ChildEvent::Line(value))
                    {
                        answer_idle(handle, &value).await;
                    }
                }
                None => answer_idle(handle, &value).await,
            }
        }
    }
}

/// Answer control requests that arrive while no turn is listening, so the
/// child never waits forever.
async fn answer_idle(handle: &Arc<ChildHandle>, value: &Value) {
    if value.get("type").and_then(Value::as_str) != Some("control_request") {
        return;
    }
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let request = value.get("request").cloned().unwrap_or(Value::Null);
    let subtype = request.get("subtype").and_then(Value::as_str).unwrap_or("");
    let reply = match subtype {
        "can_use_tool" => control::control_success(
            request_id,
            json!({"behavior": "deny", "message": "The jcode turn that requested this tool has ended."}),
        ),
        "mcp_message" => {
            let message = request.get("message").cloned().unwrap_or(Value::Null);
            match control::handle_mcp_message(&message, &[]) {
                control::McpAction::Reply(reply) => {
                    control::control_success(request_id, control::mcp_response_body(reply))
                }
                control::McpAction::CallTool { jsonrpc_id, .. } => control::control_success(
                    request_id,
                    control::mcp_response_body(control::mcp_tool_result(
                        &jsonrpc_id,
                        "The jcode turn that requested this tool has ended.",
                        true,
                    )),
                ),
            }
        }
        other => match control::simple_control_answer(other) {
            Some(Ok(body)) => control::control_success(request_id, body),
            Some(Err(error)) => control::control_error(request_id, &error),
            None => return,
        },
    };
    let _ = handle.send(&reply).await;
}
