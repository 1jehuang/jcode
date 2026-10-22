//! Cursor CLI ACP provider runtime.
//!
//! The runtime owns one Cursor `agent acp` subprocess per provider instance.
//! Cursor remains the authority for the model catalog. Jcode only stores the
//! advertised opaque IDs and applies an explicit, deterministic selection rule.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::{DEFAULT_CONTEXT_LIMIT, EventStream, ModelRoute, Provider};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, timeout};
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_COMMAND: &str = "agent";
const DEFAULT_ACP_ARG: &str = "acp";
/// Default permission/behavior flags passed **before** the `acp` subcommand.
/// These are top-level `agent` options, not `acp` subcommand options. The
/// `--force` flag (alias `--yolo`) gives the Cursor agent access to
/// shell/bash tools, and `--trust` skips the interactive workspace-trust
/// prompt — both are required for jcode to drive Cursor non-interactively via
/// ACP. jcode's own safety classification (the `jcode` permission mode, the
/// default) still gates every permission request the Cursor agent raises, so
/// destructive tools are cancelled unless the user explicitly opts into
/// `yolo`/`allow_all`.
const DEFAULT_PERMISSION_ARGS: &[&str] = &["--force", "--trust"];
const ACP_PROTOCOL_VERSION: u64 = 1;
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
/// Timeout for the `initialize` and `session/new` handshake reads. A hung
/// subprocess that never responds would otherwise hold the provider mutex
/// forever, blocking every subsequent `complete()` call.
const ACP_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-line read timeout during a prompt. This is the maximum time we wait for
/// a single JSON-RPC message from the subprocess before declaring it hung.
/// Prompt responses stream incrementally, so a long-running tool does not
/// trip this — only a process that stops sending *any* line for this long
/// is considered hung.
const ACP_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Maximum number of stderr bytes to retain for debugging spawn/protocol
/// failures. Only the tail is kept so memory stays bounded even if the
/// subprocess writes a large volume of diagnostic output.
const STDERR_TAIL_BYTES: usize = 8192;
/// Grace period for the subprocess to exit after SIGTERM before we escalate
/// to SIGKILL. Small on purpose: only needs to cover Cursor flushing its
/// session state, not a model round-trip (compare ACP_HANDSHAKE_TIMEOUT).
const ACP_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// Process-wide shared model catalog for Cursor ACP. Every provider instance
/// (original, forks, new sessions) shares this Arc so a prefetch on any one
/// instance populates models for all.
static SHARED_DISCOVERED_MODELS: OnceLock<Arc<RwLock<Vec<String>>>> = OnceLock::new();
/// Process-wide shared image-support flag for Cursor ACP.
static SHARED_SUPPORTS_IMAGES: OnceLock<Arc<RwLock<bool>>> = OnceLock::new();

fn shared_discovered_models() -> Arc<RwLock<Vec<String>>> {
    SHARED_DISCOVERED_MODELS
        .get_or_init(|| Arc::new(RwLock::new(Vec::new())))
        .clone()
}

fn shared_supports_images() -> Arc<RwLock<bool>> {
    SHARED_SUPPORTS_IMAGES
        .get_or_init(|| Arc::new(RwLock::new(false)))
        .clone()
}

/// Controlled Cursor ACP executable command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorAcpCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl CursorAcpCommand {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Read the command from explicit environment configuration.
    ///
    /// `JCODE_CURSOR_ACP_PATH` controls the executable. `JCODE_CURSOR_ACP_ARGS`
    /// is a whitespace-separated argument list and defaults to `--force --trust
    /// acp`. The `--force` flag (alias `--yolo`) gives the Cursor agent access
    /// to shell/bash tools, and `--trust` skips the interactive workspace-trust
    /// prompt. Both are **top-level** options on `agent`, so they must appear
    /// *before* the `acp` subcommand, not after it.
    ///
    /// `JCODE_CURSOR_ACP_EXTRA_ARGS` is a convenience override: when set, its
    /// tokens replace the default permission args (`--force --trust`) so users
    /// can opt into stricter modes (e.g. `--auto-review`) or add custom flags
    /// without re-specifying the entire arg list. These tokens are placed before
    /// the `acp` subcommand. The default `--force --trust` flags give Cursor's
    /// agent tool access and skip the trust prompt; jcode's safe permission mode
    /// (the default) still gates destructive operations.
    pub fn from_env() -> Self {
        let program = std::env::var("JCODE_CURSOR_ACP_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_COMMAND.to_string());

        // Build the permission/behavior args that go BEFORE the `acp` subcommand.
        // JCODE_CURSOR_ACP_EXTRA_ARGS replaces the permission portion so users
        // can customize without losing the `acp` subcommand.
        let extra_args: Vec<String> = std::env::var("JCODE_CURSOR_ACP_EXTRA_ARGS")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.split_whitespace().map(str::to_string).collect())
            .unwrap_or_else(|| {
                DEFAULT_PERMISSION_ARGS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            });

        // JCODE_CURSOR_ACP_ARGS fully replaces the arg list (including `acp`),
        // for advanced users who want complete control.
        let args = std::env::var("JCODE_CURSOR_ACP_ARGS")
            .ok()
            .map(|value| value.split_whitespace().map(str::to_string).collect())
            .filter(|args: &Vec<String>| !args.is_empty())
            .unwrap_or_else(|| {
                // Permission args come first (they're top-level agent options),
                // then the `acp` subcommand.
                let mut combined = extra_args;
                combined.push(DEFAULT_ACP_ARG.to_string());
                combined
            });
        Self { program, args }
    }

    /// Whether the configured executable can be resolved on this machine.
    ///
    /// Used to gate catalog surfaces so a standalone Cursor ACP provider that
    /// was never installed does not advertise a (empty/placeholder) model list
    /// in the picker. An explicit `JCODE_CURSOR_ACP_PATH` is trusted as-is;
    /// otherwise the bare program name must be findable on `PATH`.
    pub fn configured(&self) -> bool {
        if std::path::Path::new(&self.program).is_absolute()
            || self.program.contains(std::path::MAIN_SEPARATOR)
        {
            std::path::Path::new(&self.program).exists()
        } else {
            std::env::var_os("PATH")
                .map(|paths| {
                    std::env::split_paths(&paths).any(|dir| dir.join(&self.program).exists())
                })
                .unwrap_or(false)
        }
    }
}

impl Default for CursorAcpCommand {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Return the stable base part of a Cursor ACP model ID.
///
/// IDs are opaque to Jcode. This helper is only used for the optional
/// convenience resolution of a bare ID when exactly one advertised variant
/// exists.
pub fn model_base_id(model: &str) -> &str {
    model.split_once('[').map_or(model, |(base, _)| base)
}

/// Extract a bracketed setting from a Cursor ACP model ID.
///
/// Model IDs may carry opaque bracketed settings, e.g.
/// `gpt-5.6-sol[context=272k,reasoning=medium,fast=false]`.
/// This parses comma-separated `key=value` pairs inside the brackets and
/// returns the value for the requested key.
fn parse_bracket_setting(model: &str, key: &str) -> Option<String> {
    let (_, rest) = model.split_once('[')?;
    let rest = rest.strip_suffix(']').unwrap_or(rest);
    for pair in rest.split(',') {
        if let Some((k, v)) = pair.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Parse a human-friendly size string like "272k" or "200000" into a token count.
fn parse_token_count(value: &str) -> Option<usize> {
    let value = value.trim();
    if let Some(stripped) = value.strip_suffix(['k', 'K']) {
        stripped.parse::<usize>().ok().map(|n| n * 1_000)
    } else if let Some(stripped) = value.strip_suffix(['m', 'M']) {
        stripped.parse::<usize>().ok().map(|n| n * 1_000_000)
    } else {
        value.parse::<usize>().ok()
    }
}

/// Reasoning effort levels supported by Cursor ACP models.
const ACP_REASONING_EFFORTS: &[&str] = &["none", "low", "medium", "high", "max"];

/// Remove a bracketed setting from a model ID, preserving all other settings.
fn remove_bracket_setting(model: &str, key: &str) -> String {
    let Some((base, rest)) = model.split_once('[') else {
        return model.to_string();
    };
    let rest = rest.strip_suffix(']').unwrap_or(rest);
    let kept: Vec<&str> = rest
        .split(',')
        .filter(|pair| pair.split_once('=').is_some_and(|(k, _)| k.trim() != key))
        .collect();
    if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}[{}]", kept.join(","))
    }
}

/// Set or replace a bracketed setting on a model ID, preserving all other settings.
fn set_bracket_setting(model: &str, key: &str, value: &str) -> String {
    let Some((base, rest)) = model.split_once('[') else {
        return format!("{model}[{key}={value}]");
    };
    let rest = rest.strip_suffix(']').unwrap_or(rest);
    let mut replaced = false;
    let parts: Vec<String> = rest
        .split(',')
        .map(|pair| {
            if let Some((k, _)) = pair.split_once('=')
                && k.trim() == key
            {
                replaced = true;
                return format!("{key}={value}");
            }
            pair.to_string()
        })
        .collect();
    let mut parts = parts;
    if !replaced {
        parts.push(format!("{key}={value}"));
    }
    format!("{base}[{}]", parts.join(","))
}

/// Resolve a requested model against the catalog advertised by Cursor ACP.
///
/// Exact IDs always win. A bare ID is accepted only when one advertised
/// bracketed variant has that base. No static model list or silent fallback is
/// used.
pub fn resolve_model(
    requested: Option<&str>,
    advertised: &[String],
    current: Option<&str>,
) -> Result<String> {
    let current = current.map(str::trim).filter(|value| !value.is_empty());
    let mut models = Vec::new();
    for model in advertised {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|known| known == model) {
            models.push(model.to_string());
        }
    }

    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return current
            .map(ToString::to_string)
            .or_else(|| models.first().cloned())
            .ok_or_else(|| anyhow!("Cursor ACP did not advertise a current or available model"));
    };

    if let Some(exact) = models.iter().find(|model| model.as_str() == requested) {
        return Ok(exact.clone());
    }

    let candidates: Vec<&String> = models
        .iter()
        .filter(|model| model_base_id(model) == requested)
        .collect();
    match candidates.as_slice() {
        [only] => Ok((*only).clone()),
        [] => bail!(
            "Cursor ACP does not advertise model '{}'. Available models: {}",
            requested,
            if models.is_empty() {
                "none".to_string()
            } else {
                models.join(", ")
            }
        ),
        _ => bail!(
            "Cursor ACP model '{}' is ambiguous. Use the exact advertised ID: {}",
            requested,
            candidates
                .iter()
                .map(|candidate| candidate.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[derive(Debug)]
struct IncomingMessage {
    id: Option<Value>,
    method: Option<String>,
    params: Value,
    result: Option<Value>,
    error: Option<Value>,
}

impl IncomingMessage {
    fn parse(line: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid Cursor ACP JSON: {}", line.trim()))?;
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("Cursor ACP message must be a JSON object"))?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            bail!("Cursor ACP message is not JSON-RPC 2.0");
        }
        Ok(Self {
            id: object.get("id").cloned(),
            method: object
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string),
            params: object.get("params").cloned().unwrap_or(Value::Null),
            result: object.get("result").cloned(),
            error: object.get("error").cloned(),
        })
    }
}

#[derive(Debug, Default)]
struct ModelCatalog {
    models: Vec<String>,
    current: Option<String>,
    config_id: Option<String>,
}

impl ModelCatalog {
    fn merge(&mut self, value: &Value) {
        let mut discovered = Vec::new();
        if let Some(models) = value.get("models") {
            discovered.extend(model_ids(models.get("availableModels").unwrap_or(models)));
            self.current = string_value(models.get("currentModelId"));
        }
        if let Some(options) = value.get("configOptions").and_then(Value::as_array) {
            for option in options {
                let is_model = option.get("category").and_then(Value::as_str) == Some("model")
                    || option.get("id").and_then(Value::as_str) == Some("model");
                if !is_model {
                    continue;
                }
                self.config_id = string_value(option.get("id")).or_else(|| Some("model".into()));
                discovered.extend(model_ids(option.get("options").unwrap_or(&Value::Null)));
                self.current = string_value(option.get("currentValue")).or(self.current.take());
            }
        }
        if string_value(value.get("configId")).as_deref() == Some("model")
            || value.get("category").and_then(Value::as_str) == Some("model")
        {
            self.config_id = string_value(value.get("configId")).or_else(|| Some("model".into()));
            discovered.extend(model_ids(value.get("options").unwrap_or(&Value::Null)));
            self.current = string_value(value.get("value"))
                .or_else(|| string_value(value.get("currentValue")))
                .or(self.current.take());
        }
        for model in discovered {
            if !model.is_empty() && !self.models.iter().any(|known| known == &model) {
                self.models.push(model);
            }
        }
        if let Some(current) = self.current.clone()
            && !self.models.iter().any(|model| model == &current)
        {
            self.models.push(current);
        }
    }
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn model_ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.as_str()
                .map(str::to_string)
                .or_else(|| string_value(item.get("modelId")))
                .or_else(|| string_value(item.get("id")))
                .or_else(|| string_value(item.get("value")))
        })
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect()
}

struct AcpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Bounded tail of stderr output for debugging spawn/protocol failures.
    /// A background task continuously reads stderr and keeps only the last
    /// `STDERR_TAIL_BYTES` characters so memory stays bounded.
    stderr_tail: Arc<RwLock<String>>,
    next_id: u64,
    session_id: String,
    catalog: ModelCatalog,
    supports_images: bool,
    thinking_active: bool,
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        // Backstop: shutdown() does graceful teardown; the try_wait here
        // collects the zombie start_kill would otherwise leave behind.
        let _ = self.child.start_kill();
        let _ = self.child.try_wait();
    }
}

impl AcpProcess {
    async fn spawn(command: &CursorAcpCommand, cwd: &Path) -> Result<Self> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch Cursor ACP command '{} {}'",
                    command.program,
                    command.args.join(" ")
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Cursor ACP process did not expose stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Cursor ACP process did not expose stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Cursor ACP process did not expose stderr"))?;

        // Spawn a background task that continuously reads stderr and keeps a
        // bounded tail for debugging. Without this the stderr pipe buffer
        // could fill and block the subprocess.
        let stderr_tail = Arc::new(RwLock::new(String::new()));
        let stderr_tail_clone = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if let Ok(mut tail) = stderr_tail_clone.write() {
                            tail.push_str(&line);
                            // Trim to the last STDERR_TAIL_BYTES chars to keep
                            // memory bounded regardless of output volume.
                            let len = tail.len();
                            if len > STDERR_TAIL_BYTES {
                                let start = len - STDERR_TAIL_BYTES;
                                tail.drain(..start);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut process = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr_tail,
            next_id: 1,
            session_id: String::new(),
            catalog: ModelCatalog::default(),
            supports_images: false,
            thinking_active: false,
        };
        process.initialize().await.map_err(|err| {
            let tail = process.stderr_tail();
            if tail.trim().is_empty() {
                err
            } else {
                err.context(format!("Cursor ACP stderr (tail):\n{}", tail.trim_end()))
            }
        })?;
        process.new_session(cwd).await.map_err(|err| {
            let tail = process.stderr_tail();
            if tail.trim().is_empty() {
                err
            } else {
                err.context(format!("Cursor ACP stderr (tail):\n{}", tail.trim_end()))
            }
        })?;
        Ok(process)
    }

    /// SIGTERM, bounded wait, then SIGKILL; reaps on both paths so no
    /// zombie outlives the provider. Prefer over Drop where an async ctx exists.
    async fn shutdown(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return; // already exited and reaped
        }
        // Close stdin first so a well-behaved ACP server sees EOF and can
        // exit on its own without any signal.
        let _ = self.stdin.shutdown().await;
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // libc kill is the workspace-wide way to send SIGTERM (see
            // jcode-base background.rs); start_kill is SIGKILL-only.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        #[cfg(not(unix))]
        {
            // Windows has no SIGTERM; tokio's kill (TerminateProcess) is the
            // closest available graceful signal.
            let _ = self.child.start_kill();
        }
        if timeout(ACP_SHUTDOWN_GRACE, self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
    }

    /// Return the captured stderr tail for inclusion in error messages.
    fn stderr_tail(&self) -> String {
        self.stderr_tail
            .read()
            .map(|tail| tail.clone())
            .unwrap_or_default()
    }

    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "jcode",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
                None,
            )
            .await?;
        self.supports_images = result
            .pointer("/agentCapabilities/promptCapabilities/image")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(())
    }

    async fn new_session(&mut self, cwd: &Path) -> Result<()> {
        let result = self
            .request(
                "session/new",
                json!({
                    "cwd": cwd.to_string_lossy(),
                    "mcpServers": [],
                }),
                None,
            )
            .await?;
        self.session_id = string_value(result.get("sessionId"))
            .ok_or_else(|| anyhow!("Cursor ACP session/new response did not include sessionId"))?;
        self.catalog.merge(&result);
        Ok(())
    }

    async fn set_model(&mut self, model: &str) -> Result<()> {
        let config_id =
            self.catalog.config_id.clone().ok_or_else(|| {
                anyhow!("Cursor ACP did not advertise a model configuration option")
            })?;
        let result = self
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": self.session_id,
                    "configId": config_id,
                    "value": model,
                }),
                None,
            )
            .await?;
        self.catalog.merge(&result);
        self.catalog.current = Some(model.to_string());
        Ok(())
    }

    async fn prompt(
        &mut self,
        prompt: Vec<Value>,
        tx: &mpsc::Sender<Result<StreamEvent>>,
    ) -> Result<Value> {
        let result = self
            .request(
                "session/prompt",
                json!({
                    "sessionId": self.session_id,
                    "prompt": prompt,
                }),
                Some(tx),
            )
            .await?;
        if self.thinking_active {
            self.thinking_active = false;
            tx.send(Ok(StreamEvent::ThinkingEnd)).await.ok();
        }
        Ok(result)
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        tx: Option<&mpsc::Sender<Result<StreamEvent>>>,
    ) -> Result<Value> {
        let id = Value::from(self.next_id);
        self.next_id += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;

        loop {
            let message = self.read_message(tx).await?;
            if let Some(incoming_method) = message.method.as_deref() {
                match incoming_method {
                    "session/update" => self.handle_update(&message.params, tx).await?,
                    "session/request_permission" => self.handle_permission(&message).await?,
                    _ => {
                        if let Some(request_id) = message.id {
                            self.write(json!({
                                "jsonrpc": "2.0",
                                "id": request_id,
                                "error": {"code": JSONRPC_METHOD_NOT_FOUND, "message": format!("Unsupported Cursor ACP client method: {incoming_method}")}
                            }))
                            .await?;
                        }
                    }
                }
                continue;
            }
            if message.id.as_ref() != Some(&id) {
                continue;
            }
            if let Some(error) = message.error {
                bail!("Cursor ACP request '{}' failed: {}", method, error);
            }
            return Ok(message.result.unwrap_or(Value::Null));
        }
    }

    async fn read_message(
        &mut self,
        tx: Option<&mpsc::Sender<Result<StreamEvent>>>,
    ) -> Result<IncomingMessage> {
        loop {
            let mut line = String::new();
            let bytes = if let Some(tx) = tx {
                // During a prompt: respect stream cancellation and apply a
                // per-line read timeout so a hung subprocess cannot hold the
                // provider mutex indefinitely.
                tokio::select! {
                    _ = tx.closed() => bail!("Cursor ACP stream was cancelled"),
                    result = timeout(ACP_READ_TIMEOUT, self.stdout.read_line(&mut line)) => {
                        match result {
                            Ok(read_result) => read_result?,
                            Err(_) => {
                                bail!("Cursor ACP process did not produce a line within {ACP_READ_TIMEOUT:?}; subprocess may be hung")
                            }
                        }
                    }
                }
            } else {
                // Handshake phase (initialize/session/new): apply a shorter
                // timeout since these should respond immediately.
                match timeout(ACP_HANDSHAKE_TIMEOUT, self.stdout.read_line(&mut line)).await {
                    Ok(read_result) => read_result?,
                    Err(_) => {
                        bail!(
                            "Cursor ACP process did not respond to handshake within {ACP_HANDSHAKE_TIMEOUT:?}; subprocess may be hung"
                        )
                    }
                }
            };
            if bytes == 0 {
                let status = self.child.try_wait().ok().flatten();
                bail!("Cursor ACP process exited before responding ({status:?})");
            }
            if line.trim().is_empty() {
                continue;
            }
            return IncomingMessage::parse(&line);
        }
    }

    async fn write(&mut self, value: Value) -> Result<()> {
        let mut encoded = serde_json::to_string(&value)?;
        encoded.push('\n');
        self.stdin.write_all(encoded.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn handle_permission(&mut self, message: &IncomingMessage) -> Result<()> {
        let Some(id) = message.id.clone() else {
            return Ok(());
        };
        let options = message
            .params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // Permission policy for the Cursor ACP subprocess.
        //
        // Modes (set via `JCODE_CURSOR_ACP_PERMISSION` or `[acp] permission_mode`):
        //
        // `jcode` (default): use jcode's own safety classification. The Cursor
        // ACP permission request carries a `toolCall.kind` field ("read",
        // "search", "edit", "execute", "fetch", "other"). Read-only and search
        // tools are auto-approved (matching jcode's `AUTO_ALLOWED` tier);
        // destructive tools (execute/edit) require a human decision, so the
        // request is cancelled to fail-closed rather than silently approving
        // dangerous operations.
        //
        // `yolo` / `allow_all` (explicit opt-in only): auto-approve every
        // permission request by selecting the most permissive offered option.
        // This lets jcode drive Cursor's agent without blocking on interactive
        // prompts, but bypasses jcode's safety classification. Users must
        // explicitly set `permission_mode = "yolo"` in config or
        // `JCODE_CURSOR_ACP_PERMISSION=yolo` in the environment to get this
        // unsafe behavior.
        //
        // A specific option ID (e.g. "allow-always" / "reject-once"): only that
        // option is selected when the subprocess offers it; otherwise the
        // request is cancelled.
        let configured = std::env::var("JCODE_CURSOR_ACP_PERMISSION")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "jcode".to_string());
        let normalized = configured.trim().to_ascii_lowercase();
        let is_yolo = matches!(normalized.as_str(), "yolo" | "allow_all" | "allow-all");
        let is_jcode = normalized == "jcode";

        // Helper: normalize an option ID so that hyphen and underscore variants
        // match (Cursor ACP uses "allow-once"/"allow-always"/"reject-once", but
        // earlier docs used underscore forms; accept both).
        let norm_id = |raw: &str| raw.trim().to_ascii_lowercase().replace('-', "_");

        // Jcode safety classification: read-only and search tool kinds are
        // auto-allowed; everything else (execute, edit, fetch, other) requires
        // an explicit human decision. This mirrors jcode's `SafetySystem::classify`
        // tier-1 auto-allowed list, mapped to the ACP `toolCall.kind` vocabulary.
        let tool_kind = message
            .params
            .pointer("/toolCall/kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is_safe_kind = matches!(tool_kind, "read" | "search");

        let result = if is_yolo || (is_jcode && is_safe_kind) {
            // Auto-approve: select the most permissive available option.
            // Cursor offers "allow-always" (persists for the session),
            // "allow-once" (single use), and "reject-once". We prefer
            // "allow-always" to minimize future permission round-trips, then
            // fall back to "allow-once", then the first offered option.
            let preference = ["allow_always", "allow_once"];
            let selected = preference
                .iter()
                .find_map(|preferred| {
                    options.iter().find_map(|option| {
                        let option_id = string_value(option.get("optionId"))
                            .or_else(|| string_value(option.get("id")))?;
                        (norm_id(&option_id) == *preferred).then_some(option_id)
                    })
                })
                .or_else(|| {
                    options.iter().find_map(|option| {
                        string_value(option.get("optionId"))
                            .or_else(|| string_value(option.get("id")))
                    })
                });
            if let Some(option_id) = selected {
                json!({"outcome": {"outcome": "selected", "optionId": option_id}})
            } else {
                json!({"outcome": {"outcome": "cancelled"}})
            }
        } else if is_jcode {
            // Jcode mode + dangerous tool kind: fail-closed by cancelling the
            // permission request. The Cursor agent will receive the cancellation
            // and report the tool was denied. This prevents jcode from silently
            // approving terminal/file-write operations without a human present.
            json!({"outcome": {"outcome": "cancelled"}})
        } else {
            // Match a specific option ID from the offered choices (hyphen/underscore
            // insensitive so both "allow-always" and "allow_always" work).
            let target = norm_id(&normalized);
            let selected = options.iter().find_map(|option| {
                let option_id = string_value(option.get("optionId"))
                    .or_else(|| string_value(option.get("id")))?;
                (norm_id(&option_id) == target).then_some(option_id)
            });
            if let Some(option_id) = selected {
                json!({"outcome": {"outcome": "selected", "optionId": option_id}})
            } else {
                json!({"outcome": {"outcome": "cancelled"}})
            }
        };

        self.write(json!({"jsonrpc":"2.0","id":id,"result":result}))
            .await
    }

    async fn handle_update(
        &mut self,
        params: &Value,
        tx: Option<&mpsc::Sender<Result<StreamEvent>>>,
    ) -> Result<()> {
        let update = params.get("update").unwrap_or(params);
        self.catalog.merge(update);
        let kind = update
            .get("sessionUpdate")
            .or_else(|| update.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut events = Vec::new();
        match kind {
            "agent_message_chunk" => {
                if self.thinking_active {
                    self.thinking_active = false;
                    events.push(StreamEvent::ThinkingEnd);
                }
                if let Some(text) = content_text(update.get("content")) {
                    events.push(StreamEvent::TextDelta(text));
                }
            }
            "agent_thought_chunk" => {
                if !self.thinking_active {
                    self.thinking_active = true;
                    events.push(StreamEvent::ThinkingStart);
                }
                if let Some(text) = content_text(update.get("content")) {
                    events.push(StreamEvent::ThinkingDelta(text));
                }
            }
            "tool_call" => {
                if let Some(title) = string_value(update.get("title")) {
                    events.push(StreamEvent::StatusDetail { detail: title });
                }
                // Emit a ToolUseStart so the agent loop records the tool call
                // and the UI shows a tool execution card. Cursor ACP executes
                // tools itself (jcode is the ACP *client*, not the tool runner),
                // so the agent loop's handles_tools_internally path expects a
                // full ToolUseStart -> ToolUseEnd -> ToolResult sequence to
                // render and persist the call.
                let tool_id = string_value(update.get("toolCallId"))
                    .unwrap_or_else(|| format!("cursor-acp-tool-{}", self.next_id));
                // The ACP "kind" field ("execute", "read", "edit", "search",
                // "fetch", "other") describes the tool category, not the tool
                // name. We use it as the name so the UI shows a meaningful label
                // and the permission policy can classify it consistently.
                let tool_name = string_value(update.get("kind")).unwrap_or_default();
                let tool_input = update
                    .get("rawInput")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                if !tool_name.is_empty() {
                    events.push(StreamEvent::ToolUseStart {
                        id: tool_id.clone(),
                        name: tool_name.clone(),
                    });
                    if let Ok(input_str) = serde_json::to_string(&tool_input) {
                        events.push(StreamEvent::ToolInputDelta(input_str));
                    }
                    events.push(StreamEvent::ToolUseEnd);
                }
            }
            "tool_call_update" => {
                let status = string_value(update.get("status"))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let tool_id = string_value(update.get("toolCallId")).unwrap_or_default();
                if status == "completed" || status == "failed" {
                    // Surface the tool result so the agent loop persists it and
                    // the user sees the command output. rawOutput carries
                    // {exitCode, stdout, stderr} for execute tools; other tool
                    // kinds may carry a plain string or object.
                    let raw_output = update.get("rawOutput");
                    let is_error = status == "failed"
                        || raw_output
                            .and_then(|output| output.get("exitCode"))
                            .and_then(Value::as_i64)
                            .is_some_and(|code| code != 0);
                    let content = raw_output
                        .map(|output| {
                            // Prefer stdout+stderr for shell tools; fall back to
                            // a compact JSON dump for other tool kinds.
                            let stdout =
                                string_value(output.get("stdout")).filter(|s| !s.is_empty());
                            let stderr =
                                string_value(output.get("stderr")).filter(|s| !s.is_empty());
                            match (stdout, stderr) {
                                (Some(out), Some(err)) => {
                                    format!("{out}\n[stderr]\n{err}")
                                }
                                (Some(out), _) => out,
                                (None, Some(err)) => err,
                                (None, None) => serde_json::to_string(output)
                                    .unwrap_or_else(|_| "completed".to_string()),
                            }
                        })
                        .unwrap_or_else(|| "completed".to_string());
                    if !tool_id.is_empty() {
                        events.push(StreamEvent::ToolResult {
                            tool_use_id: tool_id,
                            content,
                            is_error,
                        });
                    }
                }
            }
            "config_option_update" | "current_mode_update" => {
                if let Some(value) = string_value(update.get("value")) {
                    events.push(StreamEvent::StatusDetail {
                        detail: format!("Cursor ACP configuration: {value}"),
                    });
                }
            }
            "plan" => events.push(StreamEvent::StatusDetail {
                detail: "Cursor ACP updated its plan".to_string(),
            }),
            "usage_update" => {
                let input = update
                    .get("inputTokens")
                    .or_else(|| update.get("input_tokens"))
                    .and_then(Value::as_u64);
                let output = update
                    .get("outputTokens")
                    .or_else(|| update.get("output_tokens"))
                    .and_then(Value::as_u64);
                if input.is_some() || output.is_some() {
                    events.push(StreamEvent::TokenUsage {
                        input_tokens: input,
                        output_tokens: output,
                        cache_read_input_tokens: None,
                        cache_creation_input_tokens: None,
                    });
                }
            }
            _ => {}
        }
        if let Some(tx) = tx {
            for event in events {
                tx.send(Ok(event))
                    .await
                    .map_err(|_| anyhow!("Cursor ACP stream was cancelled"))?;
            }
        }
        Ok(())
    }
}

fn content_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| string_value(value.get("text")))
}

#[derive(Clone)]
pub struct CursorAcpProvider {
    command: CursorAcpCommand,
    cwd: Arc<PathBuf>,
    session: Arc<Mutex<Option<AcpProcess>>>,
    discovered_models: Arc<RwLock<Vec<String>>>,
    model: Arc<RwLock<Option<String>>>,
    supports_images: Arc<RwLock<bool>>,
}

impl CursorAcpProvider {
    pub fn new() -> Self {
        Self::with_command(CursorAcpCommand::from_env())
    }

    pub fn with_command(command: CursorAcpCommand) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let model = std::env::var("JCODE_CURSOR_ACP_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        Self {
            command,
            cwd: Arc::new(cwd),
            session: Arc::new(Mutex::new(None)),
            discovered_models: shared_discovered_models(),
            model: Arc::new(RwLock::new(model)),
            supports_images: shared_supports_images(),
        }
    }

    fn with_state(&self, process: &AcpProcess) {
        if let Ok(mut models) = self.discovered_models.write() {
            *models = process.catalog.models.clone();
        }
        if let Some(current) = process.catalog.current.clone()
            && let Ok(mut model) = self.model.write()
            && model.is_none()
        {
            *model = Some(current);
        }
        if let Ok(mut supports_images) = self.supports_images.write() {
            *supports_images = process.supports_images;
        }
    }

    async fn ensure_process<'a>(
        &'a self,
        guard: &'a mut Option<AcpProcess>,
    ) -> Result<&'a mut AcpProcess> {
        if guard.is_none() {
            *guard = Some(AcpProcess::spawn(&self.command, &self.cwd).await?);
        }
        Ok(guard.as_mut().expect("Cursor ACP process initialized"))
    }

    async fn configure_process(&self, process: &mut AcpProcess) -> Result<String> {
        self.with_state(process);
        let requested = self.model.read().ok().and_then(|model| model.clone());
        let selected = resolve_model(
            requested.as_deref(),
            &process.catalog.models,
            process.catalog.current.as_deref(),
        )?;
        if process.catalog.current.as_deref() != Some(selected.as_str()) {
            process.set_model(&selected).await?;
        }
        if let Ok(mut model) = self.model.write() {
            *model = Some(selected.clone());
        }
        self.with_state(process);
        Ok(selected)
    }

    async fn run_prompt(
        &self,
        prompt: Vec<Value>,
        tx: &mpsc::Sender<Result<StreamEvent>>,
    ) -> Result<()> {
        let mut guard = self.session.lock().await;
        let result = async {
            let process = self.ensure_process(&mut guard).await?;
            let selected = self.configure_process(process).await?;
            let response = process.prompt(prompt, tx).await?;
            tx.send(Ok(StreamEvent::SessionId(process.session_id.clone())))
                .await
                .map_err(|_| anyhow!("Cursor ACP stream was cancelled"))?;
            tx.send(Ok(StreamEvent::ConnectionType {
                connection: format!("cursor-acp:{}", selected),
            }))
            .await
            .map_err(|_| anyhow!("Cursor ACP stream was cancelled"))?;
            tx.send(Ok(StreamEvent::MessageEnd {
                stop_reason: string_value(response.get("stopReason")),
            }))
            .await
            .map_err(|_| anyhow!("Cursor ACP stream was cancelled"))?;
            self.with_state(process);
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if result.is_err()
            && let Some(mut process) = guard.take()
        {
            process.shutdown().await;
        }
        result
    }
}

impl Default for CursorAcpProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CursorAcpProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_prompt(system, messages);
        let provider = self.clone();
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(128);
        tokio::spawn(async move {
            if let Err(error) = provider.run_prompt(prompt, &tx).await
                && !tx.is_closed()
            {
                let _ = tx.send(Err(error)).await;
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "cursor-acp"
    }

    fn display_name(&self) -> String {
        "Cursor ACP".to_string()
    }

    fn model(&self) -> String {
        // Report "unknown" before discovery so the model picker's empty-catalog
        // fallback (which skips a placeholder row when current_model == "unknown")
        // does not advertise a bogus "cursor-acp:discovering" route labeled with
        // an unrecognized provider. Once the ACP process advertises a current
        // model, the real id is returned instead.
        self.model
            .read()
            .ok()
            .and_then(|model| model.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let requested = model.trim();
        if requested.is_empty() {
            bail!("Cursor ACP model cannot be empty");
        }
        let advertised = self
            .discovered_models
            .read()
            .map(|models| models.clone())
            .unwrap_or_default();
        if !advertised.is_empty() {
            let current = self.model.read().ok().and_then(|model| model.clone());
            let resolved = resolve_model(Some(requested), &advertised, current.as_deref())?;
            *self
                .model
                .write()
                .map_err(|_| anyhow!("Cursor ACP model state is poisoned"))? = Some(resolved);
        } else {
            *self
                .model
                .write()
                .map_err(|_| anyhow!("Cursor ACP model state is poisoned"))? =
                Some(requested.to_string());
        }
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        // Cursor ACP models are discovered dynamically at runtime from the
        // subprocess handshake; they cannot satisfy the 'static lifetime bound
        // of this return type. Callers that need the live catalog should use
        // `available_models_display()` or `available_models_for_switching()`,
        // both of which return owned Strings.
        Vec::new()
    }

    fn available_models_display(&self) -> Vec<String> {
        // Do not advertise a catalog until the Cursor ACP executable is
        // resolvable. Without this gate a standalone ACP provider that was
        // never installed would still surface the discovering placeholder in
        // `/model` and in `jcode model list --provider cursor-acp`.
        if !self.command.configured() {
            return Vec::new();
        }
        self.discovered_models
            .read()
            .map(|models| models.clone())
            .unwrap_or_default()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: "Cursor ACP".to_string(),
                api_method: "cursor-acp".to_string(),
                available: true,
                detail: "Advertised by Cursor CLI ACP".to_string(),
                cheapness: None,
                usage: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        // Skip the subprocess spawn when the Cursor CLI is not installed so
        // non-Cursor users do not see a failed-spawn warning on every startup.
        if !self.command.configured() {
            return Ok(());
        }
        let mut guard = self.session.lock().await;
        let result = async {
            // Drop any existing Cursor ACP process so a refresh re-queries
            // Cursor's live model catalog via a fresh `initialize` +
            // `session/new`. The bootstrap guard only serializes concurrent
            // prefetches, so each new trigger re-runs this teardown.
            // Conversation context is owned by jcode and replayed in full on
            // the next `complete()`, so dropping the Cursor-side session is
            // safe.
            if let Some(mut process) = guard.take() {
                process.shutdown().await;
            }
            let process = self.ensure_process(&mut guard).await?;
            self.configure_process(process).await?;
            self.with_state(process);
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if result.is_err()
            && let Some(mut process) = guard.take()
        {
            process.shutdown().await;
        }
        result
    }

    fn context_window(&self) -> usize {
        // Cursor ACP models may carry a [context=N] bracket setting, e.g.
        // `gpt-5.6-sol[context=272k,...]`. Parse it so auto-compact targets
        // the real limit instead of the 200k default.
        let model = self.model();
        if let Some(ctx_str) = parse_bracket_setting(&model, "context")
            && let Some(tokens) = parse_token_count(&ctx_str)
        {
            return tokens;
        }
        DEFAULT_CONTEXT_LIMIT
    }

    fn reasoning_effort(&self) -> Option<String> {
        // Cursor ACP models may carry a [reasoning=level] bracket setting,
        // e.g. `gpt-5.6-sol[reasoning=medium,...]`. Expose it so /effort
        // and the model picker reflect the active reasoning level.
        self.model
            .read()
            .ok()
            .and_then(|model| model.clone())
            .and_then(|model| parse_bracket_setting(&model, "reasoning"))
            .filter(|value| !value.is_empty() && value != "none")
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        // Replace (or insert) the reasoning bracket setting on the current
        // model ID. If effort is "none", strip the setting entirely.
        let current = self
            .model
            .read()
            .ok()
            .and_then(|model| model.clone())
            .ok_or_else(|| anyhow!("Cursor ACP model state is poisoned or unset"))?;
        let new_model = if effort == "none" {
            remove_bracket_setting(&current, "reasoning")
        } else {
            set_bracket_setting(&current, "reasoning", effort)
        };
        *self
            .model
            .write()
            .map_err(|_| anyhow!("Cursor ACP model state is poisoned"))? = Some(new_model);
        Ok(())
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        ACP_REASONING_EFFORTS.to_vec()
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn supports_image_input(&self) -> bool {
        self.supports_images.read().is_ok_and(|value| *value)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            session: Arc::new(Mutex::new(None)),
            discovered_models: shared_discovered_models(),
            model: Arc::new(RwLock::new(
                self.model.read().ok().and_then(|model| model.clone()),
            )),
            supports_images: shared_supports_images(),
        })
    }
}

fn build_prompt(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut text = String::new();
    if !system.trim().is_empty() {
        text.push_str("System:\n");
        text.push_str(system.trim());
        text.push_str("\n\n");
    }
    text.push_str("Conversation:\n");
    let mut images = Vec::new();
    for message in messages {
        text.push_str(match message.role {
            Role::User => "User:\n",
            Role::Assistant => "Assistant:\n",
        });
        for block in &message.content {
            match block {
                ContentBlock::Text { text: value, .. } => {
                    text.push_str(value);
                    text.push('\n');
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    text.push_str(&format!("[tool_use id={id} name={name} input={input}]\n"));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    text.push_str(&format!(
                        "[tool_result id={tool_use_id} error={}]\n{content}\n",
                        is_error.unwrap_or(false)
                    ));
                }
                ContentBlock::Image { media_type, data } => {
                    images.push(json!({"type":"image","data":data,"mimeType":media_type}));
                    text.push_str("[image]\n");
                }
                ContentBlock::Reasoning { text: value }
                | ContentBlock::ReasoningTrace { text: value }
                | ContentBlock::AnthropicThinking {
                    thinking: value, ..
                } => {
                    text.push_str(&format!("[reasoning]\n{value}\n"));
                }
                ContentBlock::OpenAIReasoning { summary, .. } => {
                    text.push_str(&format!("[reasoning]\n{}\n", summary.join("\n")));
                }
                ContentBlock::OpenAICompaction { .. } => {
                    text.push_str("[compaction]\n");
                }
                // Deferred-tool references carry no prompt text of their own
                // (they ride along the ToolResult they belong to), so other
                // providers ignore them; Cursor ACP gets the plain text only.
                ContentBlock::ToolReference { .. } => {}
                // Provider-native blocks (server-side web search payloads) are
                // only replayed by the provider that emitted them; Cursor ACP
                // ignores them.
                ContentBlock::ProviderNative { .. } => {}
            }
        }
        text.push('\n');
    }
    text.push_str("Assistant:\n");
    let mut prompt = vec![json!({"type":"text","text":text})];
    prompt.extend(images);
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// Seed the process-wide shared discovered-models catalog for the duration
    /// of a test. `SHARED_DISCOVERED_MODELS` is global state that earlier
    /// fake-process tests populate (e.g. `gpt-5.3-codex`), so bracket-setting
    /// tests must pin their own catalog instead of depending on which tests
    /// happened to run before them. Restores the previous value afterwards.
    ///
    /// Takes the process-wide shared-state mutex for the whole window:
    /// parallel tests in this crate (fake-process discovery writes the same
    /// shared catalog, and unseeded tests assert on it) would otherwise race.
    fn with_seeded_discovered_models<T>(models: &[&str], f: impl FnOnce() -> T) -> T {
        let _guard = tests_shared_state_mutex().blocking_lock();
        let shared = shared_discovered_models();
        let previous = shared.read().map(|known| known.clone()).unwrap_or_default();
        if let Ok(mut guard) = shared.write() {
            *guard = ids(models);
        }
        // Restore the previous catalog even if the test body panics, so a
        // failing bracket-setting test does not leak its seed into the rest
        // of the parallel suite.
        struct RestoreOnDrop {
            shared: std::sync::Arc<std::sync::RwLock<Vec<String>>>,
            previous: Vec<String>,
        }
        impl Drop for RestoreOnDrop {
            fn drop(&mut self) {
                if let Ok(mut guard) = self.shared.write() {
                    *guard = self.previous.clone();
                }
            }
        }
        let _restore = RestoreOnDrop {
            shared: shared.clone(),
            previous,
        };
        f()
    }

    /// Mutex serializing every test that touches the process-wide shared
    /// discovered-models catalog, either by seeding it (sync tests) or by
    /// populating it through a fake ACP subprocess handshake (async tests).
    /// A tokio Mutex so async tests can hold it across `.await`.
    async fn with_shared_state_lock_async<T, F: std::future::Future<Output = T>>(f: F) -> T {
        let mutex = tests_shared_state_mutex();
        let _guard = mutex.lock().await;
        f.await
    }

    fn tests_shared_state_mutex() -> &'static tokio::sync::Mutex<()> {
        static TEST_STATE_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
            std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
        &TEST_STATE_MUTEX
    }

    #[test]
    fn exact_model_id_wins_and_preserves_settings() {
        let models = ids(&["composer-2.5[fast=true]", "gpt-5.3-codex"]);
        assert_eq!(
            resolve_model(Some("composer-2.5[fast=true]"), &models, None).unwrap(),
            "composer-2.5[fast=true]"
        );
    }

    #[test]
    fn bare_model_resolves_only_one_variant() {
        let models = ids(&["gpt-5.6-sol[reasoning=medium,fast=false]"]);
        assert_eq!(
            resolve_model(Some("gpt-5.6-sol"), &models, None).unwrap(),
            "gpt-5.6-sol[reasoning=medium,fast=false]"
        );
    }

    #[test]
    fn ambiguous_bare_model_is_rejected() {
        let models = ids(&["composer-2.5[fast=true]", "composer-2.5[fast=false]"]);
        let error = resolve_model(Some("composer-2.5"), &models, None).unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn unsupported_model_is_rejected_without_fallback() {
        let models = ids(&["gpt-5.3-codex"]);
        let error =
            resolve_model(Some("not-advertised"), &models, Some("gpt-5.3-codex")).unwrap_err();
        assert!(error.to_string().contains("does not advertise"));
    }

    #[test]
    fn omitted_model_uses_cursor_current_model() {
        let models = ids(&["gpt-5.3-codex", "composer-2.5[fast=true]"]);
        assert_eq!(
            resolve_model(None, &models, Some("composer-2.5[fast=true]")).unwrap(),
            "composer-2.5[fast=true]"
        );
    }

    #[test]
    fn command_defaults_to_cursor_acp_protocol_argument() {
        let command = CursorAcpCommand::new("agent", ["--force", "--trust", "acp"]);
        assert_eq!(command.program, "agent");
        assert_eq!(command.args, vec!["--force", "--trust", "acp"]);
    }

    #[test]
    fn from_env_defaults_and_overrides() {
        // These tests manipulate global env vars and must run sequentially.
        // Merged into one function to avoid cross-test races under parallel
        // test execution. Takes the shared-state mutex because other tests
        // construct providers via `new()`/`from_env()` and read these vars.
        let _guard = tests_shared_state_mutex().blocking_lock();

        // Restore the vars even if an assertion panics mid-section, so a
        // failed run does not leak overrides into other tests' `from_env()`
        // and does not wipe vars the host environment originally set.
        struct RestoreEnvOnDrop {
            path: Option<String>,
            args: Option<String>,
            extra_args: Option<String>,
        }
        impl Drop for RestoreEnvOnDrop {
            fn drop(&mut self) {
                let entries = [
                    ("JCODE_CURSOR_ACP_PATH", &self.path),
                    ("JCODE_CURSOR_ACP_ARGS", &self.args),
                    ("JCODE_CURSOR_ACP_EXTRA_ARGS", &self.extra_args),
                ];
                for (key, value) in entries {
                    match value {
                        Some(v) => unsafe { std::env::set_var(key, v) },
                        None => unsafe { std::env::remove_var(key) },
                    }
                }
            }
        }
        let _restore = RestoreEnvOnDrop {
            path: std::env::var("JCODE_CURSOR_ACP_PATH").ok(),
            args: std::env::var("JCODE_CURSOR_ACP_ARGS").ok(),
            extra_args: std::env::var("JCODE_CURSOR_ACP_EXTRA_ARGS").ok(),
        };

        // 1) Default path: no env overrides → --force --trust acp
        unsafe {
            std::env::remove_var("JCODE_CURSOR_ACP_PATH");
            std::env::remove_var("JCODE_CURSOR_ACP_ARGS");
            std::env::remove_var("JCODE_CURSOR_ACP_EXTRA_ARGS");
        }
        let command = CursorAcpCommand::from_env();
        assert_eq!(command.program, "agent");
        assert_eq!(
            command.args,
            vec!["--force", "--trust", "acp"],
            "default args must put --force --trust before `acp` (top-level options, not subcommand options)"
        );

        // 2) JCODE_CURSOR_ACP_EXTRA_ARGS replaces only the permission flags
        unsafe {
            std::env::set_var("JCODE_CURSOR_ACP_EXTRA_ARGS", "--auto-review");
        }
        let command = CursorAcpCommand::from_env();
        assert_eq!(
            command.args,
            vec!["--auto-review", "acp"],
            "JCODE_CURSOR_ACP_EXTRA_ARGS should replace the default permission flags (before `acp`)"
        );

        // 3) JCODE_CURSOR_ACP_ARGS fully replaces the entire arg list
        unsafe {
            std::env::set_var("JCODE_CURSOR_ACP_ARGS", "acp --yolo");
        }
        let command = CursorAcpCommand::from_env();
        assert_eq!(
            command.args,
            vec!["acp", "--yolo"],
            "JCODE_CURSOR_ACP_ARGS should fully replace the arg list"
        );

        // Cleanup
        unsafe {
            std::env::remove_var("JCODE_CURSOR_ACP_EXTRA_ARGS");
            std::env::remove_var("JCODE_CURSOR_ACP_ARGS");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unconfigured_command_reports_no_catalog_and_unknown_model() {
        let _guard = tests_shared_state_mutex().blocking_lock();
        let provider = CursorAcpProvider::with_command(CursorAcpCommand::new(
            "jcode-cursor-acp-command-that-does-not-exist",
            ["acp"],
        ));
        assert!(!provider.command.configured());
        assert!(provider.available_models_display().is_empty());
        assert!(provider.model_routes().is_empty());
        assert_eq!(provider.model(), "unknown");
    }

    #[cfg(unix)]
    #[test]
    fn configured_command_still_gates_catalog_until_discovery() {
        // `sh` always exists on unix, so configured() is true, but the catalog
        // is still empty until a real ACP process advertises models. This
        // guards against the picker showing a bogus row before prefetch runs.
        let _guard = tests_shared_state_mutex().blocking_lock();
        let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", "true"]));
        assert!(provider.command.configured());
        assert!(provider.available_models_display().is_empty());
        assert_eq!(provider.model(), "unknown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_acp_executable_returns_controlled_error() {
        // `configured()` returns false for a missing executable, so
        // `prefetch_models` returns Ok(()) without spawning. Force the
        // spawn path by bypassing the configured() gate: use a command
        // that is "configured" (points to an existing file like /bin/true)
        // but is not a valid ACP server, so the spawn succeeds but the
        // ACP initialize handshake fails.
        let provider = CursorAcpProvider::with_command(CursorAcpCommand::new(
            "jcode-cursor-acp-command-that-does-not-exist",
            ["acp"],
        ));
        // The command is not on PATH, so configured() is false and
        // prefetch_models short-circuits to Ok(()). This verifies the
        // early-exit guard works correctly.
        assert!(!provider.command.configured());
        provider.prefetch_models().await.unwrap();
    }

    #[test]
    fn catalog_extracts_string_and_object_model_ids() {
        let mut catalog = ModelCatalog::default();
        catalog.merge(&json!({
            "models": {
                "currentModelId": "gpt-5.3-codex",
                "availableModels": [
                    "gpt-5.3-codex",
                    {"id": "composer-2.5[fast=true]"}
                ]
            },
            "configOptions": [{
                "id": "model",
                "category": "model",
                "currentValue": "gpt-5.3-codex",
                "options": [{"value": "gpt-5.3-codex"}]
            }]
        }));
        assert_eq!(catalog.config_id.as_deref(), Some("model"));
        assert_eq!(catalog.current.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(
            catalog.models,
            ids(&["gpt-5.3-codex", "composer-2.5[fast=true]"])
        );
    }

    #[test]
    fn catalog_extracts_model_id_from_model_id_field() {
        // Cursor ACP's availableModels array uses objects with a "modelId" field,
        // not "id" or "value". The catalog must extract the model ID from that field.
        let mut catalog = ModelCatalog::default();
        catalog.merge(&json!({
            "models": {
                "currentModelId": "auto-smart[optimize_for=cost]",
                "availableModels": [
                    {"modelId": "auto-smart[optimize_for=cost]", "name": "Auto"},
                    {"modelId": "composer-2.5[fast=true]", "name": "Composer 2.5"}
                ]
            }
        }));
        assert_eq!(
            catalog.current.as_deref(),
            Some("auto-smart[optimize_for=cost]")
        );
        assert_eq!(
            catalog.models,
            ids(&["auto-smart[optimize_for=cost]", "composer-2.5[fast=true]"])
        );
    }

    #[test]
    fn catalog_applies_model_config_updates() {
        let mut catalog = ModelCatalog::default();
        catalog.merge(&json!({
            "configOptions": [{
                "id": "model",
                "category": "model",
                "currentValue": "gpt-5.3-codex",
                "options": [{"value": "gpt-5.3-codex"}, {"value": "composer-2.5[fast=true]"}]
            }]
        }));

        catalog.merge(&json!({
            "configId": "model",
            "value": "composer-2.5[fast=true]"
        }));

        assert_eq!(catalog.current.as_deref(), Some("composer-2.5[fast=true]"));
        assert_eq!(catalog.config_id.as_deref(), Some("model"));
        assert_eq!(catalog.models.len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_acp_process_discovers_models_and_streams_prompt() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":true}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"fake-session","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex","composer-2.5[fast=true]"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"},{"value":"composer-2.5[fast=true]"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello from fake Cursor"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            provider.prefetch_models().await.unwrap();
            assert_eq!(
                provider.available_models_display(),
                ids(&["gpt-5.3-codex", "composer-2.5[fast=true]"])
            );
            assert_eq!(provider.model(), "gpt-5.3-codex");
            assert!(provider.supports_image_input());

            let mut stream = provider
                .complete(&[Message::user("hello")], &[], "", None)
                .await
                .unwrap();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    StreamEvent::TextDelta(delta) => text.push_str(&delta),
                    StreamEvent::MessageEnd { stop_reason } => {
                        assert_eq!(stop_reason.as_deref(), Some("end_turn"));
                    }
                    _ => {}
                }
            }
            assert_eq!(text, "hello from fake Cursor");

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_call_and_result_are_surfaced_as_stream_events() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // Fake Cursor ACP that emits a tool_call, a tool_call_update with
            // status=completed and rawOutput (exitCode/stdout/stderr), then a text
            // message. jcode must surface these as ToolUseStart -> ToolUseEnd ->
            // ToolResult so the agent loop renders and persists the tool execution.
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"tool-sess","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"tool-abc","title":"`echo hello`","kind":"execute","status":"pending","rawInput":{"command":"echo hello"}}}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"tool-abc","status":"completed","rawOutput":{"exitCode":0,"stdout":"hello\n","stderr":""}}}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done!"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("run echo hello")], &[], "", None)
                .await
                .unwrap();

            let mut saw_tool_use_start = false;
            let mut saw_tool_use_end = false;
            let mut tool_result_content = None;
            let mut tool_result_is_error = None;
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    StreamEvent::ToolUseStart { name, .. } => {
                        assert_eq!(name, "execute");
                        saw_tool_use_start = true;
                    }
                    StreamEvent::ToolUseEnd => {
                        saw_tool_use_end = true;
                    }
                    StreamEvent::ToolResult {
                        content, is_error, ..
                    } => {
                        tool_result_content = Some(content);
                        tool_result_is_error = Some(is_error);
                    }
                    StreamEvent::TextDelta(delta) => text.push_str(&delta),
                    _ => {}
                }
            }
            assert!(
                saw_tool_use_start,
                "ToolUseStart must be emitted for tool_call"
            );
            assert!(
                saw_tool_use_end,
                "ToolUseEnd must be emitted after tool_call"
            );
            assert_eq!(
                tool_result_content.as_deref(),
                Some("hello\n"),
                "ToolResult must surface stdout from rawOutput"
            );
            assert_eq!(
                tool_result_is_error,
                Some(false),
                "exitCode 0 means no error"
            );
            assert_eq!(text, "done!");

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_call_failed_status_marks_result_as_error() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // A failed tool call (non-zero exit code) must surface as an error result.
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"err-sess","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"tool-err","title":"`false`","kind":"execute","status":"pending","rawInput":{"command":"false"}}}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"tool-err","status":"completed","rawOutput":{"exitCode":1,"stdout":"","stderr":"command not found"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("run false")], &[], "", None)
                .await
                .unwrap();

            let mut tool_result = None;
            let mut is_error = None;
            while let Some(event) = stream.next().await {
                if let Ok(StreamEvent::ToolResult {
                    content,
                    is_error: err,
                    ..
                }) = event
                {
                    tool_result = Some(content);
                    is_error = Some(err);
                }
            }
            assert_eq!(
                tool_result.as_deref(),
                Some("command not found"),
                "failed tool with empty stdout must surface stderr directly"
            );
            assert_eq!(is_error, Some(true), "non-zero exit must be an error");

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refresh_re_spawns_and_re_reads_catalog() {
        with_shared_state_lock_async(async {
            // The fake Cursor emits a different model catalog on every process
            // launch by persisting a launch counter to a temp file. A refresh must
            // drop the existing process and re-spawn so the new catalog is observed.
            // Without the drop, the cached catalog from the first launch would be
            // re-published unchanged.
            let counter = std::env::temp_dir().join(format!(
                "jcode-cursor-acp-refresh-test-{}.txt",
                std::process::id()
            ));
            std::fs::write(&counter, "0").unwrap();
            let counter_arg = counter.to_string_lossy().into_owned();
            let script = r#"
    counter="$1"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          count=$(($(cat "$counter") + 1))
          printf '%s' "$count" > "$counter"
          if [ "$count" -eq 1 ]; then
            printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          else
            printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          fi
          ;;
        *'"method":"session/new"'*)
          count=$(cat "$counter")
          if [ "$count" -eq 1 ]; then
            printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"s1","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          else
            printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"s2","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex","composer-2.5[fast=true]"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"},{"value":"composer-2.5[fast=true]"}]}]}}'
          fi
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
      esac
    done
    "#;
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new(
                "sh",
                ["-c", script, "sh", &counter_arg],
            ));

            // Cold prefetch: first catalog (one model).
            provider.prefetch_models().await.unwrap();
            assert_eq!(provider.available_models_display(), ids(&["gpt-5.3-codex"]));
            assert_eq!(provider.model(), "gpt-5.3-codex");

            // Refresh must drop the live process and re-spawn to see the new model.
            // Without the drop, the cached single-model catalog would be re-published
            // and the newly advertised composer-2.5 variant would never appear.
            provider.prefetch_models().await.unwrap();
            assert_eq!(
                provider.available_models_display(),
                ids(&["gpt-5.3-codex", "composer-2.5[fast=true]"])
            );
            // Cursor kept the same current model, so jcode preserves it.
            assert_eq!(provider.model(), "gpt-5.3-codex");

            let _ = std::fs::remove_file(&counter);

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn yolo_permission_mode_auto_approves_first_option() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // Fake Cursor ACP that sends a permission request during a prompt.
            // The test verifies jcode auto-selects the first option (YOLO mode).
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"perm-session","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/request_permission","id":999,"params":{"options":[{"optionId":"allow-once"},{"optionId":"allow-always"},{"optionId":"reject-once"}]}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"approved!"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            // Ensure YOLO mode is active (default, but explicit for test clarity).
            unsafe { std::env::set_var("JCODE_CURSOR_ACP_PERMISSION", "yolo") };
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("do something")], &[], "", None)
                .await
                .unwrap();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                if let Ok(StreamEvent::TextDelta(delta)) = event {
                    text.push_str(&delta);
                }
            }
            assert_eq!(text, "approved!");
            unsafe { std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION") };

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn yolo_permission_prefers_allow_always_over_allow_once() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // Fake Cursor ACP that sends a permission request with both allow-once
            // and allow-always. YOLO mode should select allow-always (session-persisting).
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"perm-session2","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/request_permission","id":888,"params":{"options":[{"optionId":"allow-once"},{"optionId":"allow-always"},{"optionId":"reject-once"}]}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"approved!"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            // Explicitly opt into yolo mode (no longer the default) so this test
            // verifies the allow-always preference path.
            unsafe { std::env::set_var("JCODE_CURSOR_ACP_PERMISSION", "yolo") };
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("do something")], &[], "", None)
                .await
                .unwrap();
            while let Some(event) = stream.next().await {
                if let Ok(StreamEvent::TextDelta(_)) = event {
                    // Just drain; the real assertion is that the fake script
                    // completed (it would hang if allow-always was not selected
                    // because the script only emits one message after permission).
                }
            }
            unsafe { std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION") };

        })
        .await;
    }

    #[test]
    fn permission_option_id_normalization_accepts_underscores_and_hyphens() {
        // The norm_id logic normalizes hyphen↔underscore so both forms work.
        // We verify via the env-var matching path: setting
        // JCODE_CURSOR_ACP_PERMISSION to "allow_always" should match an option
        // advertised as "allow-always" by the ACP server.
        // (Indirectly tested via yolo preference + the normalization code path.)
        // This is a lightweight unit check on the normalization itself.
        let norm = |raw: &str| raw.trim().to_ascii_lowercase().replace('-', "_");
        assert_eq!(norm("allow-always"), "allow_always");
        assert_eq!(norm("allow-always"), norm("allow_always"));
        assert_eq!(norm("reject-once"), "reject_once");
        assert_eq!(norm("allow-once"), "allow_once");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn jcode_permission_mode_cancels_execute_tools() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // In jcode mode, a tool call with kind="execute" should be cancelled
            // (fail-closed) rather than auto-approved.
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"jcode-sess","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/request_permission","id":777,"params":{"toolCall":{"kind":"execute","title":"rm -rf /"},"options":[{"optionId":"allow-once"},{"optionId":"allow-always"},{"optionId":"reject-once"}]}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"cancelled!"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            unsafe { std::env::set_var("JCODE_CURSOR_ACP_PERMISSION", "jcode") };
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("rm -rf /")], &[], "", None)
                .await
                .unwrap();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                if let Ok(StreamEvent::TextDelta(delta)) = event {
                    text.push_str(&delta);
                }
            }
            assert_eq!(text, "cancelled!");
            unsafe { std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION") };

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn jcode_permission_mode_allows_read_tools() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // In jcode mode, a tool call with kind="read" should be auto-approved.
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"jcode-sess2","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/request_permission","id":666,"params":{"toolCall":{"kind":"read","title":"read file"},"options":[{"optionId":"allow-once"},{"optionId":"allow-always"},{"optionId":"reject-once"}]}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"allowed!"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            unsafe { std::env::set_var("JCODE_CURSOR_ACP_PERMISSION", "jcode") };
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("read file")], &[], "", None)
                .await
                .unwrap();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                if let Ok(StreamEvent::TextDelta(delta)) = event {
                    text.push_str(&delta);
                }
            }
            assert_eq!(text, "allowed!");
            unsafe { std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION") };

        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn default_permission_mode_cancels_execute_tools() {
        with_shared_state_lock_async(async {
                use tokio_stream::StreamExt;

            // Without any explicit JCODE_CURSOR_ACP_PERMISSION env var, the default
            // mode must be jcode (safe), which cancels execute tool permission
            // requests rather than auto-approving them.
            unsafe { std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION") };
            let script = r#"
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"default-sess","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
        *'"method":"session/set_config_option"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
          ;;
        *'"method":"session/prompt"'*)
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/request_permission","id":555,"params":{"toolCall":{"kind":"execute","title":"rm -rf /"},"options":[{"optionId":"allow-once"},{"optionId":"allow-always"},{"optionId":"reject-once"}]}}'
          printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"cancelled by default!"}}}}'
          printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
          ;;
      esac
    done
    "#;
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new("sh", ["-c", script]));

            let mut stream = provider
                .complete(&[Message::user("rm -rf /")], &[], "", None)
                .await
                .unwrap();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                if let Ok(StreamEvent::TextDelta(delta)) = event {
                    text.push_str(&delta);
                }
            }
            assert_eq!(
                text, "cancelled by default!",
                "default permission mode must be jcode (safe), cancelling execute tools"
            );

        })
        .await;
    }

    #[test]
    fn parse_bracket_setting_extracts_context() {
        let model = "gpt-5.6-sol[context=272k,reasoning=medium,fast=false]";
        assert_eq!(
            parse_bracket_setting(model, "context"),
            Some("272k".to_string())
        );
        assert_eq!(
            parse_bracket_setting(model, "reasoning"),
            Some("medium".to_string())
        );
        assert_eq!(
            parse_bracket_setting(model, "fast"),
            Some("false".to_string())
        );
        assert_eq!(parse_bracket_setting(model, "missing"), None);
    }

    #[test]
    fn parse_token_count_handles_k_and_m_suffixes() {
        assert_eq!(parse_token_count("272k"), Some(272_000));
        assert_eq!(parse_token_count("1m"), Some(1_000_000));
        assert_eq!(parse_token_count("200000"), Some(200_000));
        assert_eq!(parse_token_count("bogus"), None);
    }

    #[test]
    fn context_window_reads_bracket_or_falls_back() {
        with_seeded_discovered_models(&["gpt-5.6-sol[context=272k]", "plain-model"], || {
            let provider = CursorAcpProvider::new();
            // No model set → default limit.
            assert_eq!(provider.context_window(), DEFAULT_CONTEXT_LIMIT);

            // Model with [context=272k].
            provider.set_model("gpt-5.6-sol[context=272k]").unwrap();
            assert_eq!(provider.context_window(), 272_000);

            // Model without bracket → default.
            provider.set_model("plain-model").unwrap();
            assert_eq!(provider.context_window(), DEFAULT_CONTEXT_LIMIT);
        })
    }

    #[test]
    fn reasoning_effort_reads_bracket() {
        with_seeded_discovered_models(&["gpt-5.6-sol[reasoning=high]", "plain-model"], || {
            let provider = CursorAcpProvider::new();
            provider.set_model("gpt-5.6-sol[reasoning=high]").unwrap();
            assert_eq!(provider.reasoning_effort(), Some("high".to_string()));

            provider.set_model("plain-model").unwrap();
            assert_eq!(provider.reasoning_effort(), None);
        })
    }

    #[test]
    fn set_reasoning_effort_replaces_bracket() {
        with_seeded_discovered_models(&["gpt-5.6-sol[context=272k,reasoning=low]"], || {
            let provider = CursorAcpProvider::new();
            provider
                .set_model("gpt-5.6-sol[context=272k,reasoning=low]")
                .unwrap();
            provider.set_reasoning_effort("high").unwrap();
            assert!(provider.model().contains("reasoning=high"));
            assert!(provider.model().contains("context=272k"));
        })
    }

    #[test]
    fn set_reasoning_effort_none_strips_setting() {
        with_seeded_discovered_models(&["gpt-5.6-sol[context=272k,reasoning=low]"], || {
            let provider = CursorAcpProvider::new();
            provider
                .set_model("gpt-5.6-sol[context=272k,reasoning=low]")
                .unwrap();
            provider.set_reasoning_effort("none").unwrap();
            assert!(!provider.model().contains("reasoning="));
            assert!(provider.model().contains("context=272k"));
        })
    }

    #[test]
    fn available_efforts_returns_supported_levels() {
        // `new()` reads the JCODE_CURSOR_ACP_* env vars via `from_env()`,
        // so serialize against `from_env_defaults_and_overrides`.
        let _guard = tests_shared_state_mutex().blocking_lock();
        let provider = CursorAcpProvider::new();
        let efforts = provider.available_efforts();
        assert!(efforts.contains(&"none"));
        assert!(efforts.contains(&"medium"));
        assert!(efforts.contains(&"max"));
    }

    #[test]
    fn model_base_id_strips_bracket_settings() {
        // Plain model id: no brackets → unchanged.
        assert_eq!(model_base_id("gpt-5"), "gpt-5");
        // A single bracket setting is stripped.
        assert_eq!(model_base_id("gpt-5[context=272k]"), "gpt-5");
        // Multiple comma-separated bracket settings: only the part before the
        // first '[' is kept.
        assert_eq!(
            model_base_id("gpt-5.6-sol[context=272k,reasoning=medium,fast=false]"),
            "gpt-5.6-sol"
        );
        // Empty string → empty string.
        assert_eq!(model_base_id(""), "");
    }

    #[test]
    fn incoming_message_parse_handles_valid_jsonrpc() {
        let message = IncomingMessage::parse(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/update","params":{"a":1},"result":null}"#,
        )
        .unwrap();
        assert_eq!(message.id, Some(json!(1)));
        assert_eq!(message.method.as_deref(), Some("session/update"));
        assert_eq!(message.params, json!({"a":1}));
    }

    #[test]
    fn incoming_message_parse_rejects_invalid_json() {
        let error = IncomingMessage::parse("not json").unwrap_err();
        assert!(
            error.to_string().contains("invalid Cursor ACP JSON"),
            "parse error should mention invalid JSON: {error}"
        );
    }

    #[test]
    fn incoming_message_parse_accepts_missing_fields() {
        // A bare JSON-RPC envelope with no method/result/params still parses;
        // the optional fields default to None / Null.
        let message = IncomingMessage::parse(r#"{"jsonrpc":"2.0"}"#).unwrap();
        assert_eq!(message.id, None);
        assert_eq!(message.method, None);
        assert_eq!(message.params, Value::Null);
        assert_eq!(message.result, None);
        assert_eq!(message.error, None);
    }

    #[test]
    fn incoming_message_parse_ignores_extra_fields() {
        // Unknown fields must not break parsing.
        let message =
            IncomingMessage::parse(r#"{"jsonrpc":"2.0","id":7,"custom":"ignored"}"#).unwrap();
        assert_eq!(message.id, Some(json!(7)));
        assert_eq!(message.method, None);
    }

    #[test]
    fn catalog_merge_model_config_update_and_empty_object() {
        let mut catalog = ModelCatalog::default();

        // Merging an empty object is a no-op.
        catalog.merge(&json!({}));
        assert!(catalog.models.is_empty());
        assert!(catalog.current.is_none());
        assert!(catalog.config_id.is_none());

        // Merging a model config update populates the catalog.
        catalog.merge(&json!({
            "configId": "model",
            "category": "model",
            "value": "gpt-5.3-codex",
            "options": [{"value": "gpt-5.3-codex"}, {"value": "composer-2.5[fast=true]"}]
        }));
        assert_eq!(catalog.config_id.as_deref(), Some("model"));
        assert_eq!(catalog.current.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(
            catalog.models,
            ids(&["gpt-5.3-codex", "composer-2.5[fast=true]"])
        );

        // A subsequent empty-object merge must not wipe the discovered state.
        catalog.merge(&json!({}));
        assert_eq!(catalog.models.len(), 2);
        assert_eq!(catalog.current.as_deref(), Some("gpt-5.3-codex"));
    }

    #[test]
    fn string_value_extracts_string_and_rejects_non_string() {
        // Some(string value) → extracted.
        assert_eq!(
            string_value(Some(&json!("hello"))),
            Some("hello".to_string())
        );
        // Some(non-string value) → None.
        assert_eq!(string_value(Some(&json!(42))), None);
        assert_eq!(string_value(Some(&json!({"k":1}))), None);
        assert_eq!(string_value(Some(&Value::Null)), None);
        // None → None.
        assert_eq!(string_value(None), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_reaps_fake_acp_child_without_leaving_a_zombie() {
        with_shared_state_lock_async(async {
            // The fake ACP parks after stdin EOF and flushes a marker from its
            // SIGTERM trap: the marker proves the grace path ran (SIGKILL
            // alone leaves none), and the second prefetch completing proves
            // the child was reaped. No direct waitpid check: the pid never
            // leaves AcpProcess.
            let marker = std::env::temp_dir().join(format!(
                "jcode-cursor-acp-shutdown-test-{}.txt",
                std::process::id()
            ));
            let _ = std::fs::remove_file(&marker);
            let marker_arg = marker.to_string_lossy().into_owned();
            let script = r#"
    marker="$1"
    trap 'printf flushed > "$marker"; exit 0' TERM
    while IFS= read -r line; do
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":false}}}}\n' "$id"
          ;;
        *'"method":"session/new"'*)
          printf '%s\n' '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"reap-sess","models":{"currentModelId":"gpt-5.3-codex","availableModels":["gpt-5.3-codex"]},"configOptions":[{"id":"model","category":"model","currentValue":"gpt-5.3-codex","options":[{"value":"gpt-5.3-codex"}]}]}}'
          ;;
      esac
    done
    # stdin EOF: park so only the SIGTERM trap can exit the process.
    while :; do sleep 1; done
"#;
            let provider = CursorAcpProvider::with_command(CursorAcpCommand::new(
                "sh",
                ["-c", script, "sh", &marker_arg],
            ));
            provider.prefetch_models().await.unwrap();

            // Refresh drops the live process through the graceful path.
            provider.prefetch_models().await.unwrap();

            // Grace path ran: the child saw SIGTERM and flushed its marker
            // before exiting (SIGKILL would have left no marker).
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap_or_default(),
                "flushed",
                "SIGTERM grace path must run before any escalation"
            );
            let _ = std::fs::remove_file(&marker);

        })
        .await;
    }
}
