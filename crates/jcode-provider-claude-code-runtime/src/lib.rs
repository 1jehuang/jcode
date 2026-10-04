//! Claude Code provider runtime for jcode.
//!
//! Claude turns run through the user's installed `claude` CLI over the Agent
//! SDK stream-json protocol, the same way t3code drives Claude:
//!
//! - one long-lived `claude` child per jcode session (resumed with `--resume`),
//! - Claude Code owns the agent loop, built-in tools, compaction, auth and
//!   retries,
//! - jcode-only tools are served back to Claude over an in-process SDK MCP
//!   server (`mcp__jcode__<tool>`) and executed by jcode through the
//!   `NativeToolCall` / [`Provider::native_result_sender`] bridge,
//! - accounts are instances, each an isolated `CLAUDE_CONFIG_DIR`. jcode never
//!   reads or stores Claude Code credentials.

pub mod control;
pub mod process;
pub mod prompt;
pub mod settings;
pub mod translate;
pub mod turn;

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use jcode_message_types::{Message, StreamEvent, ToolDefinition};
use jcode_provider_core::{
    AccountPin, AccountPinSlot, AccountProviderKind, EventStream, NativeToolResult,
    NativeToolResultSender, Provider,
};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use crate::process::{ChildHandle, InitInfo, LaunchConfig};
use crate::settings::{ClaudeCodeInstance, ClaudeCodeSettings, SessionStart, SpawnSpec};
pub use crate::translate::{ClaudeCodeRateLimits, RateLimitWindow};
use crate::turn::{TurnEnd, TurnRequest};

/// Provider name (`Provider::name`).
pub const PROVIDER_NAME: &str = "claude-code";
/// Identity probe cache lifetime.
const PROBE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// Identity probe deadline.
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);
/// One-shot (`complete_simple`) deadline.
const ONESHOT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Model ids offered before the CLI reported its own list.
const FALLBACK_MODELS: &[&str] = &[
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-fable-5-1",
    "claude-fable-5",
    "claude-sonnet-5",
    "claude-haiku-4-5",
];

/// Identity reported by an init-only probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeCodeIdentity {
    pub email: Option<String>,
    pub subscription: Option<String>,
    pub organization: Option<String>,
    pub models: Vec<String>,
}

/// Per-session state: the live child and the Claude session it continues.
#[derive(Default)]
struct SessionState {
    child: Option<Arc<ChildHandle>>,
    /// Claude session id to resume when the next child starts.
    resume_id: Option<String>,
    /// Whether the current Claude session already saw this conversation.
    seeded: bool,
}

/// Claude via the local Claude Code CLI.
pub struct ClaudeCodeProvider {
    settings: Arc<RwLock<ClaudeCodeSettings>>,
    model: Arc<RwLock<String>>,
    effort: Arc<RwLock<Option<String>>>,
    pin: AccountPinSlot,
    state: Arc<Mutex<SessionState>>,
    /// Turn lock: one in-flight turn per provider instance.
    turn_lock: Arc<Mutex<()>>,
    result_tx: mpsc::Sender<NativeToolResult>,
    result_rx: Arc<Mutex<mpsc::Receiver<NativeToolResult>>>,
    rate_limits: Arc<StdMutex<Option<ClaudeCodeRateLimits>>>,
    init: Arc<StdMutex<Option<InitInfo>>>,
    probe_cache: Arc<StdMutex<Vec<(String, Instant, ClaudeCodeIdentity)>>>,
}

impl Default for ClaudeCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn read<T: Clone>(lock: &RwLock<T>) -> T {
    lock.read().unwrap_or_else(|p| p.into_inner()).clone()
}

fn write<T>(lock: &RwLock<T>, value: T) {
    *lock.write().unwrap_or_else(|p| p.into_inner()) = value;
}

impl ClaudeCodeProvider {
    /// Provider configured from `[provider.claude_code]`.
    pub fn new() -> Self {
        Self::with_settings(jcode_base::config::config().provider.claude_code.clone())
    }

    /// Provider with explicit settings (tests, embedding).
    pub fn with_settings(settings: ClaudeCodeSettings) -> Self {
        let default_model = jcode_base::config::config()
            .provider
            .default_model
            .clone()
            .map(|m| settings::normalize_model_id(&m))
            .filter(|m| m.starts_with("claude-"))
            .unwrap_or_else(|| jcode_provider_core::DEFAULT_CLAUDE_MODEL.to_string());
        let (result_tx, result_rx) = mpsc::channel(64);
        Self {
            settings: Arc::new(RwLock::new(settings)),
            model: Arc::new(RwLock::new(default_model)),
            effort: Arc::new(RwLock::new(None)),
            pin: AccountPinSlot::default(),
            state: Arc::new(Mutex::new(SessionState::default())),
            turn_lock: Arc::new(Mutex::new(())),
            result_tx,
            result_rx: Arc::new(Mutex::new(result_rx)),
            rate_limits: Arc::new(StdMutex::new(None)),
            init: Arc::new(StdMutex::new(None)),
            probe_cache: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    fn settings(&self) -> ClaudeCodeSettings {
        read(&self.settings)
    }

    fn binary(&self) -> String {
        self.settings().resolved_binary()
    }

    /// Configured instances (implicit `default` when none).
    pub fn instances(&self) -> Vec<ClaudeCodeInstance> {
        self.settings().effective_instances()
    }

    /// Instance this session uses: the pin, else the default instance.
    pub fn active_instance(&self) -> ClaudeCodeInstance {
        let settings = self.settings();
        self.pin
            .get()
            .and_then(|pin| settings.instance(&pin.label))
            .or_else(|| settings.instance(&settings.resolved_default_instance()))
            .unwrap_or_else(ClaudeCodeInstance::implicit_default)
    }

    /// Latest rate-limit snapshot reported by the CLI.
    pub fn rate_limit_snapshot(&self) -> Option<ClaudeCodeRateLimits> {
        self.rate_limits
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn init_info(&self) -> Option<InitInfo> {
        self.init.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn login_hint(&self, instance: &ClaudeCodeInstance) -> String {
        settings::login_command(&self.binary(), instance)
    }

    fn expose_mcp(&self, tools: &[ToolDefinition]) -> bool {
        self.settings().expose_jcode_tools && !control::exposed_tools(tools).is_empty()
    }

    /// Stop the live child (the Claude session stays resumable).
    pub async fn shutdown(&self) {
        let child = self.state.lock().await.child.take();
        if let Some(child) = child {
            child.shutdown().await;
        }
    }

    /// Run an init-only probe for `instance` (no prompt is sent, no tokens are
    /// used). Cached for five minutes.
    pub async fn probe_identity(
        &self,
        instance: &ClaudeCodeInstance,
    ) -> Result<ClaudeCodeIdentity> {
        let key = instance
            .resolved_home()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if let Some((_, _, cached)) = self
            .probe_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(k, at, _)| *k == key && at.elapsed() < PROBE_CACHE_TTL)
        {
            return Ok(cached.clone());
        }
        let settings = self.settings();
        let mut command = tokio::process::Command::new(settings.resolved_binary());
        command
            .args(settings::build_probe_argv(&settings.setting_sources))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        for (k, v) in settings::instance_env(instance)
            .into_iter()
            .chain(settings::probe_env_extras())
        {
            command.env(k, v);
        }
        let identity = tokio::time::timeout(PROBE_TIMEOUT, async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let mut child = command.spawn().context("start Claude Code probe")?;
            let mut stdin = child.stdin.take().context("probe stdin")?;
            let stdout = child.stdout.take().context("probe stdout")?;
            let request =
                control::control_request("probe-init", control::initialize_request(false, ""));
            stdin.write_all(format!("{request}\n").as_bytes()).await?;
            stdin.flush().await?;
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await? {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let response = value.get("response");
                if value.get("type").and_then(Value::as_str) == Some("control_response")
                    && response
                        .and_then(|r| r.get("request_id"))
                        .and_then(Value::as_str)
                        == Some("probe-init")
                {
                    drop(stdin);
                    let _ = child.start_kill();
                    let info = InitInfo::from_response(
                        response
                            .and_then(|r| r.get("response"))
                            .unwrap_or(&Value::Null),
                    );
                    return Ok(ClaudeCodeIdentity {
                        email: info.email.clone(),
                        subscription: info.subscription.clone(),
                        organization: info.organization.clone(),
                        models: info.model_ids(),
                    });
                }
            }
            bail!("Claude Code probe exited without answering")
        })
        .await
        .map_err(|_| anyhow!("Claude Code probe timed out"))??;
        let mut cache = self.probe_cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.retain(|(k, _, _)| *k != key);
        cache.push((key, Instant::now(), identity.clone()));
        Ok(identity)
    }

    /// The live child for this session, spawning (or respawning with
    /// `--resume`) when needed.
    async fn ensure_child(
        &self,
        state: &mut SessionState,
        tools: &[ToolDefinition],
        cwd: Option<PathBuf>,
        resume_hint: Option<&str>,
    ) -> Result<Arc<ChildHandle>> {
        let instance = self.active_instance();
        let model = settings::normalize_model_id(&read(&self.model));
        let effort = read(&self.effort);
        let expose_mcp = self.expose_mcp(tools);
        let settings = self.settings();

        if let Some(child) = state.child.take() {
            let launch = &child.launch;
            let same_home = launch.instance.resolved_home() == instance.resolved_home()
                && launch.instance.env == instance.env
                && launch.instance.launch_args == instance.launch_args;
            let same_shape = launch.spec.expose_mcp == expose_mcp
                && launch.spec.effort == effort
                && launch.cwd == cwd;
            let mut reusable = child.is_alive() && same_home && same_shape;
            if reusable && child.model() != model {
                let request = serde_json::json!({"subtype": "set_model", "model": model});
                match child.control(request, Duration::from_secs(15)).await {
                    Ok(_) => child.set_model(&model),
                    Err(err) => {
                        jcode_base::logging::warn(&format!(
                            "Claude Code: set_model failed, restarting the CLI: {err}"
                        ));
                        reusable = false;
                    }
                }
            }
            if reusable {
                child.set_tools(tools);
                state.child = Some(child.clone());
                return Ok(child);
            }
            if same_home {
                state.resume_id = Some(child.session_id().to_string());
            } else {
                // Another account cannot resume this account's session.
                state.resume_id = None;
                state.seeded = false;
            }
            child.shutdown().await;
        }

        let resume = state
            .resume_id
            .clone()
            .or_else(|| resume_hint.map(str::to_string))
            .filter(|id| uuid::Uuid::parse_str(id).is_ok());
        let launch_args = instance
            .launch_args
            .as_deref()
            .map(settings::parse_launch_args)
            .unwrap_or_default();
        let make_launch = |session: SessionStart| LaunchConfig {
            binary: settings.resolved_binary(),
            instance: instance.clone(),
            spec: SpawnSpec {
                model: model.clone(),
                effort: effort.clone(),
                permission_mode: settings.permission_mode.clone(),
                setting_sources: settings.setting_sources.clone(),
                session,
                expose_mcp,
                launch_args: launch_args.clone(),
            },
            cwd: cwd.clone(),
            append_system_prompt: prompt::JCODE_APPEND_SYSTEM_PROMPT.to_string(),
        };

        let child = match resume {
            Some(id) => {
                match process::spawn(make_launch(SessionStart::Resume(id.clone())), tools).await {
                    Ok(child) => {
                        state.seeded = true;
                        child
                    }
                    Err(err) => {
                        jcode_base::logging::warn(&format!(
                            "Claude Code: could not resume session {id}, starting a new one: {err}"
                        ));
                        state.seeded = false;
                        process::spawn(
                            make_launch(SessionStart::New(uuid::Uuid::new_v4().to_string())),
                            tools,
                        )
                        .await?
                    }
                }
            }
            None => {
                state.seeded = false;
                process::spawn(
                    make_launch(SessionStart::New(uuid::Uuid::new_v4().to_string())),
                    tools,
                )
                .await?
            }
        };
        let info = child.init.lock().unwrap_or_else(|p| p.into_inner()).clone();
        *self.init.lock().unwrap_or_else(|p| p.into_inner()) = Some(info);
        self.pin.set_resolved_label(Some(instance.id.clone()));
        state.resume_id = Some(child.session_id().to_string());
        state.child = Some(child.clone());
        Ok(child)
    }

    async fn run_turn(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        cwd: Option<PathBuf>,
        resume_hint: Option<String>,
        out: mpsc::Sender<Result<StreamEvent>>,
    ) {
        let _turn_guard = self.turn_lock.lock().await;
        let mut results = self.result_rx.lock().await;
        let instance = self.active_instance();
        let login_hint = self.login_hint(&instance);
        let rate_limits = self.rate_limits.clone();
        let on_rate_limits = move |limits: ClaudeCodeRateLimits| {
            *rate_limits.lock().unwrap_or_else(|p| p.into_inner()) = Some(limits);
        };

        // One automatic restart when the child died before producing output.
        for attempt in 0..2 {
            let mut state = self.state.lock().await;
            let child = match self
                .ensure_child(&mut state, &tools, cwd.clone(), resume_hint.as_deref())
                .await
            {
                Ok(child) => child,
                Err(err) => {
                    let _ = out
                        .send(Ok(StreamEvent::Error {
                            message: format!("{err:#}"),
                            retry_after_secs: None,
                        }))
                        .await;
                    return;
                }
            };
            if attempt == 0 {
                let _ = out
                    .send(Ok(StreamEvent::ConnectionType {
                        connection: "claude code cli (stream-json)".into(),
                    }))
                    .await;
                let _ = out
                    .send(Ok(StreamEvent::SessionId(child.session_id().to_string())))
                    .await;
            }

            let input = prompt::turn_input(&messages);
            let transcript = if state.seeded {
                None
            } else {
                prompt::transcript(&messages[..input.history_len])
            };
            let turn_uuid = uuid::Uuid::new_v4().to_string();
            let user_line = prompt::user_line(&input, transcript.as_deref(), &turn_uuid);
            let permission_mode = self.settings().permission_mode;

            let outcome = turn::drive_turn(TurnRequest {
                child: &child,
                user_line,
                turn_uuid,
                tools: &tools,
                permission_mode: &permission_mode,
                login_hint: login_hint.clone(),
                out: &out,
                results: &mut results,
                on_rate_limits: &on_rate_limits,
            })
            .await;

            state.seeded = true;
            if let Some(session_id) = outcome.session_id {
                state.resume_id = Some(session_id);
            }
            if !outcome.child_usable
                && let Some(dead) = state.child.take()
            {
                state.resume_id = Some(dead.session_id().to_string());
                dead.shutdown().await;
            }
            match outcome.end {
                TurnEnd::Finished | TurnEnd::Cancelled | TurnEnd::RateLimited => return,
                TurnEnd::Exited(tail) => {
                    if attempt == 0 && !outcome.produced_output {
                        jcode_base::logging::warn(&format!(
                            "Claude Code exited mid-turn, restarting with --resume: {tail}"
                        ));
                        continue;
                    }
                    let detail = if tail.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", tail.trim())
                    };
                    let _ = out
                        .send(Ok(StreamEvent::Error {
                            message: format!("Claude Code exited unexpectedly{detail}"),
                            retry_after_secs: None,
                        }))
                        .await;
                    return;
                }
            }
        }
    }

    /// Independent provider for another session: same settings, model and
    /// pin, its own child and tool-result channel.
    fn fork_inner(&self) -> Self {
        let (result_tx, result_rx) = mpsc::channel(64);
        Self {
            settings: Arc::new(RwLock::new(self.settings())),
            model: Arc::new(RwLock::new(read(&self.model))),
            effort: Arc::new(RwLock::new(read(&self.effort))),
            pin: self.pin.fork(),
            state: Arc::new(Mutex::new(SessionState::default())),
            turn_lock: Arc::new(Mutex::new(())),
            result_tx,
            result_rx: Arc::new(Mutex::new(result_rx)),
            rate_limits: Arc::new(StdMutex::new(self.rate_limit_snapshot())),
            init: Arc::new(StdMutex::new(self.init_info())),
            probe_cache: self.probe_cache.clone(),
        }
    }
}

#[async_trait]
impl Provider for ClaudeCodeProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let cwd = prompt::working_dir_from_system(system).or_else(|| std::env::current_dir().ok());
        let (tx, rx) = mpsc::channel(256);
        let this = self.fork_shared();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        let resume = resume_session_id.map(str::to_string);
        tokio::spawn(async move {
            this.run_turn(messages, tools, cwd, resume, tx).await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn display_name(&self) -> String {
        "Claude Code".into()
    }

    fn model(&self) -> String {
        read(&self.model)
    }

    fn active_auth_method_label(&self) -> Option<&'static str> {
        Some("Claude Code CLI")
    }

    fn supports_image_input(&self) -> bool {
        true
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = settings::normalize_model_id(model);
        if model.is_empty() {
            bail!("Model id is empty");
        }
        write(&self.model, model);
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        FALLBACK_MODELS.to_vec()
    }

    fn available_models_display(&self) -> Vec<String> {
        let from_cli = self
            .init_info()
            .map(|info| info.model_ids())
            .unwrap_or_default();
        if from_cli.is_empty() {
            FALLBACK_MODELS.iter().map(|m| m.to_string()).collect()
        } else {
            from_cli
        }
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn reasoning_effort(&self) -> Option<String> {
        read(&self.effort)
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        let trimmed = effort.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
            write(&self.effort, None);
            return Ok(());
        }
        if settings::map_effort(trimmed).is_none() {
            bail!(
                "Unsupported Claude Code effort '{trimmed}'. Use one of: {}",
                settings::CLAUDE_EFFORTS.join(", ")
            );
        }
        write(&self.effort, Some(trimmed.to_ascii_lowercase()));
        Ok(())
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        settings::CLAUDE_EFFORTS.to_vec()
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn account_pin(&self, kind: AccountProviderKind) -> Option<AccountPin> {
        (kind == AccountProviderKind::ClaudeCode)
            .then(|| self.pin.get())
            .flatten()
    }

    fn set_account_pin(&self, kind: AccountProviderKind, pin: Option<AccountPin>) -> Result<()> {
        if kind != AccountProviderKind::ClaudeCode {
            bail!("Claude Code only manages Claude Code instances");
        }
        if let Some(pin) = pin.as_ref()
            && self.settings().instance(&pin.label).is_none()
        {
            bail!(
                "No Claude Code instance '{}' configured. Known instances: {}",
                pin.label,
                self.instances()
                    .into_iter()
                    .map(|i| i.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // The next turn notices the instance change and restarts the child
        // under the new CLAUDE_CONFIG_DIR.
        self.pin.set(pin);
        Ok(())
    }

    fn resolved_account_label(&self, kind: AccountProviderKind) -> Option<String> {
        if kind != AccountProviderKind::ClaudeCode {
            return None;
        }
        self.pin
            .resolved_label()
            .or_else(|| Some(self.active_instance().id))
    }

    fn supports_compaction(&self) -> bool {
        // Claude Code compacts its own transcript.
        false
    }

    fn context_window(&self) -> usize {
        jcode_provider_core::context_limit_for_model(&self.model())
            .unwrap_or(jcode_provider_core::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.fork_inner())
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        Some(self.result_tx.clone())
    }

    async fn complete_simple(&self, prompt_text: &str, system: &str) -> Result<String> {
        let instance = self.active_instance();
        let model = settings::normalize_model_id(&self.model());
        let mut command = tokio::process::Command::new(self.binary());
        command
            .args(settings::build_oneshot_argv(&model, system))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in settings::instance_env(&instance) {
            command.env(k, v);
        }
        let output = tokio::time::timeout(ONESHOT_TIMEOUT, async {
            use tokio::io::AsyncWriteExt;
            let mut child = command.spawn().context("start Claude Code")?;
            let mut stdin = child.stdin.take().context("Claude Code stdin")?;
            stdin.write_all(prompt_text.as_bytes()).await?;
            drop(stdin);
            child.wait_with_output().await.map_err(anyhow::Error::from)
        })
        .await
        .map_err(|_| anyhow!("Claude Code one-shot request timed out"))??;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let value: Value = serde_json::from_str(stdout.trim()).with_context(|| {
            format!(
                "Claude Code returned no JSON result: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
        if value.get("is_error").and_then(Value::as_bool) == Some(true) {
            let detail = value
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            if value.get("api_error_status").and_then(Value::as_u64) == Some(401) {
                bail!(
                    "Claude Code could not authenticate. Run `{}` and try again.",
                    self.login_hint(&instance)
                );
            }
            bail!("Claude Code error: {detail}");
        }
        Ok(value
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }
}

impl ClaudeCodeProvider {
    /// Clone sharing all session state (used to move the turn into a task).
    fn fork_shared(&self) -> Self {
        Self {
            settings: self.settings.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            pin: self.pin.clone(),
            state: self.state.clone(),
            turn_lock: self.turn_lock.clone(),
            result_tx: self.result_tx.clone(),
            result_rx: self.result_rx.clone(),
            rate_limits: self.rate_limits.clone(),
            init: self.init.clone(),
            probe_cache: self.probe_cache.clone(),
        }
    }
}
