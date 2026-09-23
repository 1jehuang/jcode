//! Antigravity CLI (`agy`) provider over agy's NDJSON stdio transport.
//!
//! This runtime deliberately has **no** Antigravity HTTP or API-key path.
//! Authentication and agent execution are delegated to the installed `agy`
//! binary: Jcode spawns `agy -p <prompt> --output-format stream-json` and
//! translates agy's newline-delimited JSON events into Jcode [`StreamEvent`]s.
//! `agy` owns its own login, its own tool loop, and its own conversation state.
//!
//! Because that spawn is **headless**, agy cannot answer its own permission
//! prompt: it initialises with `permission_mode: "request-review"` and an
//! empty `permissions.allow`, so every gated tool — file reads included — is
//! auto-denied and the agent answers that it "was denied by the system's
//! permission checks". We therefore pass `--dangerously-skip-permissions` by
//! default; set `JCODE_AGY_SKIP_PERMISSIONS=0` to defer back to agy's own gate.
//!
//! Wire format observed from `agy -p '<p>' --output-format stream-json`:
//!
//! ```json
//! {"event":"init","conversation_id":"<uuid>","init":{"cwd":"...","tools":[...],"permission_mode":"request-review"}}
//! {"event":"step_update","step_update":{"conversation_id":"<uuid>","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"..."}}
//! {"event":"step_update","step_update":{"conversation_id":"<uuid>","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"\n","usage":{...}}}
//! {"event":"result","result":{"conversation_id":"<uuid>","status":"SUCCESS","response":"...","usage":{...}}}
//! ```

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures::Stream;
use jcode_message_types::{
    ContentBlock as JcodeContentBlock, Message, Role, StreamEvent, ToolDefinition,
};
use jcode_provider_core::{EventStream, ModelRoute, Provider};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

/// Stable machine-facing provider key, also the `--provider` value and the
/// prefix used by the model picker (`agy:<model>`).
pub const PROVIDER_KEY: &str = "agy";

/// Fallback model used before `agy models` has been discovered.
const DEFAULT_MODEL: &str = "gemini-3.6-flash-high";

/// Env override for the `agy` binary path.
const CLI_PATH_ENV: &str = "JCODE_AGY_CLI_PATH";

/// Upper bound on a single agy turn.
const AGY_TURN_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// Max stderr bytes retained for error reporting.
const STDERR_LIMIT: usize = 64 * 1024;

/// How to launch the Antigravity CLI.
#[derive(Clone, Debug)]
pub struct AgyProcess {
    pub command: PathBuf,
    pub env: BTreeMap<String, String>,
    /// Pass `--dangerously-skip-permissions` to agy.
    ///
    /// A headless (`-p`) turn has no TTY, so agy cannot answer its own
    /// permission prompt. With the default `toolPermission=request-review`
    /// and an empty `permissions.allow`, every gated tool call — including
    /// file reads — is *auto-denied*: agy prints "no output produced — a tool
    /// required the \"mcp\" permission that headless mode cannot prompt for,
    /// so it was auto-denied", and the agent tells the user it has no file
    /// access. Auto-approve by default so headless turns have real tools;
    /// set `JCODE_AGY_SKIP_PERMISSIONS=0` to restore agy's own review gate.
    pub skip_permissions: bool,
}

impl AgyProcess {
    pub fn from_env() -> Self {
        let command = std::env::var_os(CLI_PATH_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("agy"));
        Self {
            command,
            env: BTreeMap::new(),
            skip_permissions: skip_permissions_from_env(),
        }
    }
}

/// Read `JCODE_AGY_SKIP_PERMISSIONS` (default `true`).
fn skip_permissions_from_env() -> bool {
    match std::env::var("JCODE_AGY_SKIP_PERMISSIONS") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Jcode provider that drives the Antigravity CLI as an agent backend.
#[derive(Clone)]
pub struct AgyProvider {
    process: AgyProcess,
    model: Arc<RwLock<String>>,
    models: Arc<RwLock<Vec<String>>>,
    model_selected: Arc<AtomicBool>,
}

impl AgyProvider {
    pub fn new() -> Self {
        Self::with_process(AgyProcess::from_env())
    }

    pub fn with_process(process: AgyProcess) -> Self {
        Self {
            process,
            model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
            // Seed the roster so `--provider agy`, the model picker and swarm
            // routing can offer agy models before `prefetch_models()` refreshes
            // the list from `agy models`.
            models: Arc::new(RwLock::new(
                [
                    "gemini-3.6-flash-high",
                    "gemini-3.6-flash-medium",
                    "gemini-3.6-flash-low",
                    "gemini-3.1-pro-high",
                    "claude-sonnet-4-6",
                    "claude-opus-4-6-thinking",
                ]
                .iter()
                .map(|model| (*model).to_string())
                .collect(),
            )),
            model_selected: Arc::new(AtomicBool::new(false)),
        }
    }

    fn known_models(&self) -> Vec<String> {
        self.models
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn update_models(&self, discovered: Vec<String>) {
        if discovered.is_empty() {
            return;
        }
        *self
            .models
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = discovered;
    }
}

impl Default for AgyProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AgyProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_prompt(messages, system, resume_session_id.is_some())?;
        let process = self.process.clone();
        // Only force `--model` once the user (or the picker) actually chose one;
        // otherwise let agy keep its own configured default.
        let selected_model = self
            .model_selected
            .load(Ordering::Acquire)
            .then(|| self.model());
        let resume_session_id = resume_session_id.map(ToOwned::to_owned);
        let (tx, rx) = mpsc::channel(128);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let thread = std::thread::Builder::new()
            .name("jcode-agy-ndjson".to_string())
            .spawn(move || {
                if let Err(error) = run_turn_thread(
                    process,
                    selected_model,
                    resume_session_id,
                    prompt,
                    tx.clone(),
                    cancel_rx,
                ) {
                    let _ = tx.blocking_send(Err(error));
                }
            })
            .context("Failed to start agy runtime thread")?;

        Ok(Box::pin(AgyEventStream {
            inner: ReceiverStream::new(rx),
            cancel: Some(cancel_tx),
            thread: Some(thread),
        }))
    }

    fn name(&self) -> &str {
        PROVIDER_KEY
    }

    fn display_name(&self) -> String {
        "Antigravity (agy)".to_string()
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = model
            .strip_prefix("agy:")
            .unwrap_or(model)
            .trim()
            .to_string();
        if model.is_empty() {
            bail!("Antigravity (agy) model cannot be empty");
        }
        let available = self.known_models();
        if !available.is_empty() && !available.iter().any(|candidate| *candidate == model) {
            bail!(
                "Model '{model}' is not advertised by agy. Available models: {}",
                available.join(", ")
            );
        }
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = model;
        self.model_selected.store(true, Ordering::Release);
        Ok(())
    }

    fn available_models_display(&self) -> Vec<String> {
        self.known_models()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.known_models()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        let provider = self.display_name();
        self.known_models()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: provider.clone(),
                api_method: "agy-cli".to_string(),
                available: true,
                detail: "Antigravity CLI over NDJSON stdio".to_string(),
                cheapness: None,
                usage: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let process = self.process.clone();
        let discovered = tokio::task::spawn_blocking(move || discover_models(&process))
            .await
            .context("agy model discovery task failed")?
            .with_context(|| {
                "Failed to discover Antigravity models; run `agy models` and check that the \
                 Antigravity CLI is installed and signed in"
            })?;
        self.update_models(discovered);
        Ok(())
    }

    fn active_auth_method_label(&self) -> Option<&'static str> {
        Some("Antigravity CLI login")
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn transport(&self) -> Option<String> {
        Some("agy CLI (NDJSON stdio)".to_string())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let fork = Self::with_process(self.process.clone());
        *fork
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.model();
        *fork
            .models
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.known_models();
        fork.model_selected.store(
            self.model_selected.load(Ordering::Acquire),
            Ordering::Release,
        );
        Arc::new(fork)
    }
}

/// Stream wrapper that cancels the agy turn when dropped.
struct AgyEventStream {
    inner: ReceiverStream<Result<StreamEvent>>,
    cancel: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Stream for AgyEventStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Drop for AgyEventStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        // Joining can block while the child shuts down. Detach; the child has
        // kill_on_drop enabled inside the runtime thread.
        self.thread.take();
    }
}

fn run_turn_thread(
    process: AgyProcess,
    selected_model: Option<String>,
    resume_session_id: Option<String>,
    prompt: String,
    tx: mpsc::Sender<Result<StreamEvent>>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build agy Tokio runtime")?;
    runtime.block_on(run_turn(
        process,
        selected_model,
        resume_session_id,
        prompt,
        tx,
        cancel_rx,
    ))
}

async fn run_turn(
    process: AgyProcess,
    selected_model: Option<String>,
    resume_session_id: Option<String>,
    prompt: String,
    tx: mpsc::Sender<Result<StreamEvent>>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let mut command = Command::new(&process.command);
    command
        .arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--print-timeout")
        .arg(format!("{}m", (AGY_TURN_TIMEOUT.as_secs() / 60).max(1)))
        .envs(&process.env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Headless turns cannot answer agy's permission prompt, so without this
    // flag every gated tool (file reads included) is auto-denied and the
    // agent reports that it cannot access the files.
    if process.skip_permissions {
        command.arg("--dangerously-skip-permissions");
    }
    if let Some(model) = selected_model.as_deref().filter(|m| !m.is_empty()) {
        command.arg("--model").arg(model);
    }
    if let Some(conversation) = resume_session_id.as_deref().filter(|c| !c.is_empty()) {
        command.arg("--conversation").arg(conversation);
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "Failed to launch the Antigravity CLI at '{}'",
            process.command.display()
        )
    })?;
    let stdout = child.stdout.take().context("agy stdout was unavailable")?;
    let stderr = child.stderr.take().context("agy stderr was unavailable")?;

    let stderr_capture = Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_task = tokio::spawn(capture_stderr(stderr, Arc::clone(&stderr_capture)));

    let mut lines = BufReader::new(stdout).lines();
    let mut state = TurnState::default();
    let mut read_error: Option<String> = None;
    let mut cancelled = false;

    tokio::pin!(cancel_rx);
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                cancelled = true;
                let _ = child.kill().await;
                break;
            }
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                            continue;
                        };
                        let mut events = Vec::new();
                        let finished = translate_agy_event(&value, &mut state, &mut events);
                        for event in events {
                            if tx.send(Ok(event)).await.is_err() {
                                let _ = child.kill().await;
                                return Ok(());
                            }
                        }
                        if finished {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        read_error = Some(format!("Failed reading agy output: {error}"));
                        break;
                    }
                }
            }
        }
    }

    let status = child.wait().await.ok();
    stderr_task.abort();
    let stderr_text = stderr_capture
        .lock()
        .map(|captured| captured.trim().to_string())
        .unwrap_or_default();

    if cancelled {
        return Ok(());
    }

    let failure = state.error.clone().or(read_error).or_else(|| {
        (!state.saw_result).then(|| match &status {
            Some(status) if !stderr_text.is_empty() => stderr_text.clone(),
            Some(status) => format!("agy exited with {status} before returning a result"),
            None => "agy exited before returning a result".to_string(),
        })
    });
    if let Some(message) = failure {
        let _ = tx
            .send(Ok(StreamEvent::Error {
                message,
                retry_after_secs: None,
            }))
            .await;
    }

    let _ = tx
        .send(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        }))
        .await;
    Ok(())
}

/// Accumulated per-turn parsing state.
#[derive(Default)]
struct TurnState {
    saw_session: bool,
    saw_result: bool,
    saw_text: bool,
    error: Option<String>,
}

/// Translate one agy NDJSON event into Jcode stream events.
///
/// Returns `true` when the turn is finished (`result` event).
fn translate_agy_event(
    value: &Value,
    state: &mut TurnState,
    events: &mut Vec<StreamEvent>,
) -> bool {
    match value.get("event").and_then(Value::as_str).unwrap_or("") {
        "init" => {
            if !state.saw_session
                && let Some(conversation) = value
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
            {
                events.push(StreamEvent::SessionId(conversation.to_string()));
                state.saw_session = true;
            }
            if let Some(mode) = value
                .get("init")
                .and_then(|init| init.get("permission_mode"))
                .and_then(Value::as_str)
                .filter(|mode| !mode.is_empty())
            {
                events.push(StreamEvent::StatusDetail {
                    detail: format!("agy permission mode: {mode}"),
                });
            }
            false
        }
        "step_update" => {
            let Some(step) = value.get("step_update") else {
                return false;
            };
            let step_type = step.get("step_type").and_then(Value::as_str).unwrap_or("");
            // agy reports its own tool activity as `step_type: "tool"` ACTIVE/
            // DONE pairs. Dropping them made the entire tool phase invisible:
            // the agent looked silent until the final answer suddenly landed.
            if step_type == "tool" {
                push_tool_step(step, events);
            }
            if let Some(delta) = step
                .get("text_delta")
                .and_then(Value::as_str)
                .filter(|delta| !delta.is_empty())
            {
                if step_type.contains("think") {
                    events.push(StreamEvent::ThinkingDelta(delta.to_string()));
                } else {
                    events.push(StreamEvent::TextDelta(delta.to_string()));
                    state.saw_text = true;
                }
            }
            if let Some(usage) = step.get("usage") {
                push_usage(usage, events);
            }
            false
        }
        "result" => {
            state.saw_result = true;
            if let Some(result) = value.get("result") {
                let status = result
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("SUCCESS");
                if !status.eq_ignore_ascii_case("success") {
                    let detail = result
                        .get("error")
                        .and_then(Value::as_str)
                        .or_else(|| result.get("response").and_then(Value::as_str))
                        .unwrap_or(status);
                    state.error = Some(format!("agy turn failed ({status}): {detail}"));
                } else if !state.saw_text
                    && let Some(response) = result
                        .get("response")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                {
                    // No deltas were streamed (e.g. a cached/instant turn) —
                    // surface the final text so the turn is not empty.
                    events.push(StreamEvent::TextDelta(response.to_string()));
                    state.saw_text = true;
                }
                if let Some(usage) = result.get("usage") {
                    push_usage(usage, events);
                }
            }
            true
        }
        "error" => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .unwrap_or("agy reported an error");
            state.error = Some(message.to_string());
            false
        }
        _ => false,
    }
}

/// Argument keys agy's tools use for their most descriptive value, most
/// specific first. Unknown tools fall back to their first string argument.
const TOOL_ARG_KEYS: &[&str] = &[
    "AbsolutePath",
    "absolute_path",
    "TargetFile",
    "target_file",
    "FilePath",
    "file_path",
    "Path",
    "path",
    "DirectoryPath",
    "SearchDirectory",
    "directory",
    "Query",
    "query",
    "Pattern",
    "pattern",
    "Command",
    "command",
    "Url",
    "url",
    "Prompt",
];

/// Translate one agy `step_type: "tool"` update into Jcode tool events.
///
/// agy emits a single ACTIVE/DONE pair per call, both carrying the same
/// `step_index`, which is what lets us pair the start and the result:
///
/// ```json
/// {"step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"view_file",
///  "tool_info":{"name":"view_file","parameters":{"AbsolutePath":"/x/y.rs"}}}
/// {"step_index":2,"state":"DONE","step_type":"tool","tool_name":"view_file",
///  "duration_seconds":0.088,"tool_info":{"name":"view_file",
///  "parameters":{"AbsolutePath":"/x/y.rs"},"output":"1 lines, 113 bytes"}}
/// ```
///
/// This provider reports `handles_tools_internally() = true`, so Jcode keeps
/// only its own native tools (`JCODE_NATIVE_TOOLS` — `selfdev`,
/// `desktop_selfdev`, `communicate`) for local execution. None of agy's tool
/// names collide with those, so these blocks are rendered for the user but
/// never re-executed.
fn push_tool_step(step: &Value, events: &mut Vec<StreamEvent>) {
    let step_index = step.get("step_index").and_then(Value::as_u64).unwrap_or(0);
    let tool_use_id = jcode_message_types::sanitize_tool_id(&format!("agy-tool-{step_index}"));
    let state = step.get("state").and_then(Value::as_str).unwrap_or("");
    let info = step.get("tool_info");
    let tool_name = step
        .get("tool_name")
        .and_then(Value::as_str)
        .or_else(|| {
            info.and_then(|info| info.get("name"))
                .and_then(Value::as_str)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string();
    let parameters = info.and_then(|info| info.get("parameters"));
    let summary = tool_activity_summary(&tool_name, parameters);

    if state.eq_ignore_ascii_case("ACTIVE") {
        events.push(StreamEvent::ToolUseStart {
            id: tool_use_id,
            name: tool_name,
        });
        if let Some(params) = parameters
            && let Ok(encoded) = serde_json::to_string(params)
        {
            events.push(StreamEvent::ToolInputDelta(encoded));
        }
        events.push(StreamEvent::ToolUseEnd);
        // Drives the live status line while the tool runs.
        events.push(StreamEvent::StatusDetail { detail: summary });
        return;
    }

    let error = info
        .and_then(|info| info.get("error"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let output = info
        .and_then(|info| info.get("output"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let result_text = output.as_deref().unwrap_or(summary.as_str());
    let detail = match step.get("duration_seconds").and_then(Value::as_f64) {
        Some(seconds) => format!("{} · {seconds:.2}s", status_line(result_text, 110)),
        None => status_line(result_text, 110),
    };
    events.push(StreamEvent::ToolResult {
        tool_use_id,
        content: error.clone().or(output).unwrap_or_else(|| summary.clone()),
        is_error: error.is_some(),
    });
    events.push(StreamEvent::StatusDetail { detail });
}

/// Build the one-line activity label shown while a tool runs, e.g.
/// `view_file backend/src/main.rs`. Collapses whitespace so multi-line
/// commands do not break the status line, and trims very long arguments.
fn tool_activity_summary(tool_name: &str, parameters: Option<&Value>) -> String {
    let picked = parameters.and_then(|params| {
        TOOL_ARG_KEYS
            .iter()
            .find_map(|key| params.get(*key).and_then(Value::as_str))
            .or_else(|| {
                params
                    .as_object()
                    .and_then(|map| map.values().find_map(Value::as_str))
            })
    });
    let Some(value) = picked else {
        return tool_name.to_string();
    };
    let cleaned = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let shown: String = if cleaned.chars().count() > 96 {
        cleaned
            .chars()
            .take(95)
            .chain(std::iter::once('…'))
            .collect()
    } else {
        cleaned
    };
    format!("{tool_name} {shown}")
}

/// Collapse a tool result into a single short line for the status bar.
///
/// agy reports either a tidy summary (`1 lines, 113 bytes`) or the tool's raw
/// stdout (`run_command` returns the whole listing), and a status line must
/// never hold a multi-line payload. Takes the first non-blank line, collapses
/// whitespace, and ellipsises past `limit` characters.
fn status_line(text: &str, limit: usize) -> String {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let cleaned = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() > limit {
        cleaned
            .chars()
            .take(limit.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect()
    } else {
        cleaned
    }
}

fn push_usage(usage: &Value, events: &mut Vec<StreamEvent>) {
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
    let cache_read_input_tokens = usage.get("cache_read_tokens").and_then(Value::as_u64);
    if input_tokens.is_none() && output_tokens.is_none() && cache_read_input_tokens.is_none() {
        return;
    }
    events.push(StreamEvent::TokenUsage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens: None,
    });
}

/// Discover the models agy advertises (`agy models` prints `id<TAB>Label`).
fn discover_models(process: &AgyProcess) -> Result<Vec<String>> {
    let mut command = std::process::Command::new(&process.command);
    command.arg("models").envs(&process.env);
    let output = command
        .output()
        .with_context(|| format!("Failed to run '{} models'", process.command.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "'{} models' failed: {}",
            process.command.display(),
            stderr.trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut models = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Fetching") {
            continue;
        }
        let id = line.split('\t').next().unwrap_or("").trim();
        if !id.is_empty() && !models.iter().any(|known| known == id) {
            models.push(id.to_string());
        }
    }
    Ok(models)
}

async fn capture_stderr(
    mut stderr: tokio::process::ChildStderr,
    capture: Arc<std::sync::Mutex<String>>,
) {
    let mut buffer = [0_u8; 4096];
    loop {
        let Ok(read) = stderr.read(&mut buffer).await else {
            return;
        };
        if read == 0 {
            return;
        }
        let mut output = capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if output.len() < STDERR_LIMIT {
            let remaining = STDERR_LIMIT - output.len();
            output.push_str(&String::from_utf8_lossy(&buffer[..read.min(remaining)]));
        }
    }
}

/// Compose the prompt agy receives: outer system block, prior history (only on
/// a fresh conversation), then the latest user turn.
fn build_prompt(messages: &[Message], system: &str, resumed: bool) -> Result<String> {
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .map(message_text)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("No user prompt found for the Antigravity request"))?;

    let mut sections = Vec::new();
    if !system.trim().is_empty() {
        sections.push(format!("<system>\n{}\n</system>", system.trim()));
    }
    if !resumed {
        let history = messages
            .iter()
            .take(messages.len().saturating_sub(1))
            .filter_map(|message| {
                let text = message_text(message);
                (!text.trim().is_empty()).then(|| {
                    let role = match message.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    };
                    format!("<{role}>\n{text}\n</{role}>")
                })
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if !history.is_empty() {
            sections.push(history);
        }
    }
    sections.push(latest_user);
    Ok(sections.join("\n\n"))
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            JcodeContentBlock::Text { text, .. } => Some(text.clone()),
            JcodeContentBlock::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn translate(value: Value, state: &mut TurnState) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        translate_agy_event(&value, state, &mut events);
        events
    }

    #[test]
    fn init_emits_session_id_once() {
        let mut state = TurnState::default();
        let init = json!({
            "event": "init",
            "conversation_id": "abc-123",
            "init": { "permission_mode": "request-review" }
        });
        let events = translate(init.clone(), &mut state);
        assert!(matches!(&events[0], StreamEvent::SessionId(id) if id == "abc-123"));
        // A second init must not re-emit the session id.
        let again = translate(init, &mut state);
        assert!(!again.iter().any(|e| matches!(e, StreamEvent::SessionId(_))));
    }

    #[test]
    fn step_update_streams_text_and_usage() {
        let mut state = TurnState::default();
        let events = translate(
            json!({
                "event": "step_update",
                "step_update": {
                    "conversation_id": "abc",
                    "step_index": 1,
                    "state": "ACTIVE",
                    "step_type": "agent_response",
                    "text_delta": "PING"
                }
            }),
            &mut state,
        );
        assert!(matches!(&events[0], StreamEvent::TextDelta(t) if t == "PING"));

        let events = translate(
            json!({
                "event": "step_update",
                "step_update": {
                    "state": "DONE",
                    "step_type": "agent_response",
                    "text_delta": "\n",
                    "usage": { "input_tokens": 10, "output_tokens": 2, "cache_read_tokens": 0 }
                }
            }),
            &mut state,
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta(t) if t == "\n"))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                ..
            }
        )));
    }

    #[test]
    fn result_finishes_turn_and_backfills_missing_text() {
        let mut state = TurnState::default();
        let finished = translate_agy_event(
            &json!({
                "event": "result",
                "result": { "status": "SUCCESS", "response": "PING\n" }
            }),
            &mut state,
            &mut Vec::new(),
        );
        assert!(finished);
        assert!(state.saw_result);
    }

    #[test]
    fn result_error_is_not_success() {
        let mut state = TurnState::default();
        let mut events = Vec::new();
        translate_agy_event(
            &json!({
                "event": "result",
                "result": { "status": "ERROR", "response": "quota exhausted" }
            }),
            &mut state,
            &mut events,
        );
        assert!(
            state
                .error
                .as_deref()
                .unwrap_or("")
                .contains("quota exhausted")
        );
    }

    #[test]
    fn models_parse_id_tab_label() {
        // Exercises the same splitting `discover_models` performs on stdout.
        let stdout = "Fetching available models...\ngemini-3.6-flash-high\tGemini 3.6 Flash (High)\nclaude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\n";
        let mut models: Vec<String> = Vec::new();
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Fetching") {
                continue;
            }
            let id = line.split('\t').next().unwrap_or("").trim();
            if !id.is_empty() && !models.iter().any(|known| known == id) {
                models.push(id.to_string());
            }
        }
        assert_eq!(
            models,
            ["gemini-3.6-flash-high", "claude-opus-4-6-thinking"]
        );
    }

    #[test]
    fn set_model_rejects_unknown_when_catalog_known() {
        let provider = AgyProvider::new();
        provider.update_models(vec!["gemini-3.6-flash-high".to_string()]);
        assert!(provider.set_model("gemini-3.6-flash-high").is_ok());
        assert!(provider.set_model("agy:gemini-3.6-flash-low").is_err());
    }

    #[test]
    fn tool_step_active_emits_tool_use_and_status() {
        let mut state = TurnState::default();
        let events = translate(
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 2,
                    "state": "ACTIVE",
                    "step_type": "tool",
                    "tool_name": "view_file",
                    "tool_info": {
                        "name": "view_file",
                        "parameters": { "AbsolutePath": "/tmp/project/backend/build.gradle.kts" }
                    }
                }
            }),
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolUseStart { id, name } if id == "agy-tool-2" && name == "view_file"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolInputDelta(raw) if raw.contains("build.gradle.kts")
        )));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolUseEnd)));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::StatusDetail { detail }
                if detail == "view_file /tmp/project/backend/build.gradle.kts"
        )));
        // Tool activity must never be mistaken for assistant prose.
        assert!(!state.saw_text);
    }

    #[test]
    fn tool_step_done_emits_matching_result() {
        let mut state = TurnState::default();
        let events = translate(
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 2,
                    "state": "DONE",
                    "step_type": "tool",
                    "tool_name": "view_file",
                    "duration_seconds": 0.088131,
                    "tool_info": {
                        "name": "view_file",
                        "parameters": { "AbsolutePath": "/tmp/a.rs" },
                        "output": "1 lines, 113 bytes"
                    }
                }
            }),
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolResult { tool_use_id, content, is_error }
                if tool_use_id == "agy-tool-2" && content == "1 lines, 113 bytes" && !is_error
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::StatusDetail { detail } if detail == "1 lines, 113 bytes · 0.09s"
        )));
    }

    #[test]
    fn tool_error_marks_result_as_error() {
        let mut state = TurnState::default();
        let events = translate(
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 5,
                    "state": "DONE",
                    "step_type": "tool",
                    "tool_name": "grep_search",
                    "tool_info": { "name": "grep_search", "error": "no such directory" }
                }
            }),
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolResult { content, is_error: true, .. }
                if content == "no such directory"
        )));
    }

    #[test]
    fn activity_summary_falls_back_to_first_string_argument() {
        let params = json!({ "UnknownKey": "some-odd-value" });
        assert_eq!(
            tool_activity_summary("mystery_tool", Some(&params)),
            "mystery_tool some-odd-value"
        );
        assert_eq!(tool_activity_summary("mystery_tool", None), "mystery_tool");
    }

    #[test]
    fn status_line_collapses_multiline_tool_output() {
        // `run_command` returns raw stdout; the status line must stay one line.
        let raw = "total 360\r\ndrwxr-xr-x@ 18 shivank.rai  staff  576 Sep 18 01:06 .\r\n";
        assert_eq!(status_line(raw, 110), "total 360");
    }

    #[test]
    fn status_line_ellipsises_overlong_output() {
        let raw = "x".repeat(400);
        let rendered = status_line(&raw, 110);
        assert_eq!(rendered.chars().count(), 110);
        assert!(rendered.ends_with('…'));
    }
}
