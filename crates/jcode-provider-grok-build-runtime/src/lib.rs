//! Grok Build subscription runtime.
//!
//! When a scoped Grok CLI OIDC token is available, chat goes to the CLI HTTP
//! proxy and Jcode owns tools (same loop as Claude). Otherwise the official
//! `grok agent stdio` ACP backend is used. Auth is never `XAI_API_KEY`.

use acp::Agent as _;
use agent_client_protocol as acp;
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures::Stream;
use jcode_message_types::{
    ContentBlock as JcodeContentBlock, Message, Role, StreamEvent, ToolDefinition,
};
use jcode_provider_core::{EventStream, ModelRoute, Provider};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const DEFAULT_MODEL: &str = "grok-4.5";
const ACP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ACP_PROMPT_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const STDERR_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct GrokBuildProcess {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl GrokBuildProcess {
    pub fn from_env() -> Self {
        let command = std::env::var_os("JCODE_GROK_CLI_PATH")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("grok"));
        Self {
            command,
            args: vec!["agent".to_string(), "stdio".to_string()],
            env: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct GrokBuildProvider {
    process: GrokBuildProcess,
    model: Arc<RwLock<String>>,
    models: Arc<RwLock<Vec<String>>>,
    model_selected: Arc<AtomicBool>,
}

impl GrokBuildProvider {
    pub fn new() -> Self {
        Self::with_process(GrokBuildProcess::from_env())
    }

    pub fn with_process(process: GrokBuildProcess) -> Self {
        Self {
            process,
            model: Arc::new(RwLock::new(DEFAULT_MODEL.to_string())),
            models: Arc::new(RwLock::new(Vec::new())),
            model_selected: Arc::new(AtomicBool::new(false)),
        }
    }

    fn fake_acp(&self) -> bool {
        self.process.env.contains_key("JCODE_FAKE_GROK_ACP_LOG")
    }

    fn uses_http(&self) -> bool {
        !self.fake_acp() && jcode_base::auth::grok_build::has_cached_login()
    }

    async fn http_runtime(
        &self,
    ) -> Option<jcode_provider_openrouter_runtime::OpenRouterProvider> {
        if self.fake_acp() {
            return None;
        }
        let token = jcode_base::auth::grok_build::live_bearer_token().await?;
        jcode_provider_openrouter_runtime::OpenRouterProvider::new_grok_build_subscription(token)
            .ok()
    }

    async fn complete_http(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
        force_refresh: bool,
    ) -> Result<EventStream> {
        if force_refresh {
            jcode_base::auth::grok_build::refresh_bearer_token().await;
        }
        let http = self
            .http_runtime()
            .await
            .ok_or_else(|| anyhow!("Grok Build HTTP runtime requires a live OIDC token"))?;
        http.set_model(&self.model())?;
        let messages = sanitize_grok_turn_messages(messages);
        http.complete(&messages, tools, system, resume_session_id)
            .await
    }

    async fn complete_acp(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_prompt(messages, resume_session_id.is_some())?;
        let system = system.to_string();
        let process = self.process.clone();
        // Before catalog prefetch or an explicit `--model`/picker choice, let
        // Grok CLI keep its advertised current model instead of forcing our
        // display fallback onto a newer CLI catalog.
        let selected_model = self
            .model_selected
            .load(Ordering::Acquire)
            .then(|| self.model());
        let resume_session_id = resume_session_id.map(ToOwned::to_owned);
        let (tx, rx) = mpsc::channel(128);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let thread = std::thread::Builder::new()
            .name("jcode-grok-build-acp".to_string())
            .spawn(move || {
                if let Err(error) = run_turn_thread(
                    process,
                    selected_model,
                    resume_session_id,
                    system,
                    prompt,
                    tx.clone(),
                    cancel_rx,
                ) {
                    let _ = tx.blocking_send(Err(error));
                }
            })
            .context("Failed to start Grok Build ACP runtime thread")?;

        Ok(Box::pin(GrokEventStream {
            inner: ReceiverStream::new(rx),
            cancel: Some(cancel_tx),
            thread: Some(thread),
        }))
    }

    /// Verify that the CLI can initialize and authenticate with its own cached
    /// subscription credential. This never reads or forwards credential data.
    pub async fn authenticate_cached_cli(&self) -> Result<()> {
        let process = self.process.clone();
        run_on_acp_thread_with_process(process, move |connection| {
            Box::pin(async move {
                let initialized = initialize_and_authenticate(&connection).await?;
                Ok::<_, anyhow::Error>(models_from_initialize(&initialized))
            })
        })
        .await
        .map(|_| ())
        .with_context(|| cached_login_hint("Grok Build authentication failed"))
    }

    fn update_models(&self, discovered: DiscoveredModels) {
        if !discovered.available.is_empty() {
            *self
                .models
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = discovered.available;
        }
        if let Some(current) = discovered.current.filter(|model| !model.trim().is_empty()) {
            let mut selected = self
                .model
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if selected.as_str() == DEFAULT_MODEL
                || !self
                    .models
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(&*selected)
            {
                *selected = current;
            }
        }
    }
}

impl Default for GrokBuildProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GrokBuildProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        if grok_should_ignore_turn(messages) {
            // Jcode todo-gates inject `[auto] Continue the work…` after a finished
            // turn. Skipping them used to fall back to the last real user text
            // and Grok would redo/restate the same result (panda 339 vs 600).
            return Ok(skipped_auto_followup_stream());
        }
        if self.uses_http() {
            match self
                .complete_http(messages, tools, system, resume_session_id, false)
                .await
            {
                Ok(stream) => {
                    return Ok(wrap_repeat_guard(wrap_http_auth_retry(
                        stream,
                        self.clone(),
                        messages.to_vec(),
                        tools.to_vec(),
                        system.to_string(),
                        resume_session_id.map(ToOwned::to_owned),
                    )));
                }
                // Cached login can exist while the access token is expired and
                // refresh is missing/revoked. Do not fail the turn: ACP still
                // authenticates through Grok CLI.
                Err(_) => {}
            }
        }
        self.complete_acp(messages, tools, system, resume_session_id)
            .await
            .map(wrap_repeat_guard)
    }

    fn name(&self) -> &str {
        "grok-build"
    }

    fn display_name(&self) -> String {
        "Grok Build".to_string()
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = model.strip_prefix("grok-build:").unwrap_or(model).trim();
        if model.is_empty() {
            bail!("Grok Build model cannot be empty");
        }
        let available = self
            .models
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !available.is_empty() && !available.iter().any(|candidate| candidate == model) {
            bail!(
                "Model '{model}' is not advertised by Grok Build. Available models: {}",
                available.join(", ")
            );
        }
        drop(available);
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = model.to_string();
        self.model_selected.store(true, Ordering::Release);
        Ok(())
    }

    fn available_models_display(&self) -> Vec<String> {
        self.models
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: "Grok Build".to_string(),
                api_method: if self.uses_http() {
                    "grok-build".to_string()
                } else {
                    "grok-build-acp".to_string()
                },
                available: true,
                detail: if self.uses_http() {
                    "Grok Build subscription via Jcode tools".to_string()
                } else {
                    "Grok Build subscription via Jcode-managed ACP".to_string()
                },
                usage: None,
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        if self.uses_http() {
            self.update_models(DiscoveredModels {
                current: Some("grok-4.6".to_string()),
                available: vec![
                    "grok-4.6".to_string(),
                    "grok-4.5".to_string(),
                    "grok-code-fast-1".to_string(),
                ],
            });
            return Ok(());
        }
        let process = self.process.clone();
        let discovered = run_on_acp_thread_with_process(process, move |connection| {
            Box::pin(async move {
                let initialized = initialize_and_authenticate(&connection).await?;
                Ok::<_, anyhow::Error>(models_from_initialize(&initialized))
            })
        })
        .await
        .with_context(|| cached_login_hint("Failed to discover Grok Build models"))?;
        self.update_models(discovered);
        Ok(())
    }

    fn active_auth_method_label(&self) -> Option<&'static str> {
        Some("Grok Build subscription login")
    }

    fn handles_tools_internally(&self) -> bool {
        !self.uses_http()
    }

    fn supports_compaction(&self) -> bool {
        // HTTP path: Jcode owns history, same as Claude OAuth. ACP leaves
        // compaction to Grok CLI.
        self.uses_http()
    }

    fn transport(&self) -> Option<String> {
        if self.uses_http() {
            Some("Grok CLI subscription HTTP".to_string())
        } else {
            Some("ACP stdio".to_string())
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let fork = Self {
            process: self.process.clone(),
            model: Arc::new(RwLock::new(self.model())),
            models: Arc::new(RwLock::new(self.available_models_display())),
            model_selected: Arc::new(AtomicBool::new(
                self.model_selected.load(Ordering::Acquire),
            )),
        };
        Arc::new(fork)
    }
}

struct GrokEventStream {
    inner: ReceiverStream<Result<StreamEvent>>,
    cancel: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Stream for GrokEventStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Drop for GrokEventStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        // Joining can block while the child handles cancellation. Detach here;
        // dropping the child in the ACP thread has kill_on_drop enabled.
        self.thread.take();
    }
}

/// Stop a Grok turn that is stuck emitting the same sentence (panda 609-vs-297 loop).
const REPEAT_NEEDLE_CHARS: usize = 72;
const REPEAT_LIMIT: usize = 6;

struct RepeatGuardStream {
    inner: EventStream,
    buf: String,
    stopped: bool,
}

fn skipped_auto_followup_stream() -> EventStream {
    let (_tx, rx) = mpsc::channel(1);
    Box::pin(ReceiverStream::new(rx))
}

struct GrokHttpRetryArgs {
    provider: GrokBuildProvider,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    system: String,
    resume_session_id: Option<String>,
}

struct GrokHttpAuthRetryStream {
    inner: EventStream,
    pending: Option<Pin<Box<dyn std::future::Future<Output = Result<EventStream>> + Send>>>,
    retried: bool,
    retry: Option<GrokHttpRetryArgs>,
}

fn wrap_http_auth_retry(
    inner: EventStream,
    provider: GrokBuildProvider,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    system: String,
    resume_session_id: Option<String>,
) -> EventStream {
    Box::pin(GrokHttpAuthRetryStream {
        inner,
        pending: None,
        retried: false,
        retry: Some(GrokHttpRetryArgs {
            provider,
            messages,
            tools,
            system,
            resume_session_id,
        }),
    })
}

fn stream_item_is_unauthorized(item: &Result<StreamEvent>) -> bool {
    match item {
        Err(error) => grok_http_unauthorized(error),
        Ok(StreamEvent::Error { message, .. }) => grok_http_unauthorized_text(message),
        _ => false,
    }
}

fn start_http_auth_retry(
    args: GrokHttpRetryArgs,
) -> Pin<Box<dyn std::future::Future<Output = Result<EventStream>> + Send>> {
    Box::pin(async move {
        match args
            .provider
            .complete_http(
                &args.messages,
                &args.tools,
                &args.system,
                args.resume_session_id.as_deref(),
                true,
            )
            .await
        {
            Ok(stream) => Ok(stream),
            Err(_) => {
                args.provider
                    .complete_acp(
                        &args.messages,
                        &args.tools,
                        &args.system,
                        args.resume_session_id.as_deref(),
                    )
                    .await
            }
        }
    })
}

impl Stream for GrokHttpAuthRetryStream {
    type Item = Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(pending) = this.pending.as_mut() {
                match pending.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(stream)) => {
                        this.pending = None;
                        this.inner = stream;
                    }
                    Poll::Ready(Err(error)) => {
                        this.pending = None;
                        return Poll::Ready(Some(Err(error)));
                    }
                }
            }
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(item))
                    if !this.retried && stream_item_is_unauthorized(&item) =>
                {
                    this.retried = true;
                    let Some(args) = this.retry.take() else {
                        return Poll::Ready(Some(item));
                    };
                    this.pending = Some(start_http_auth_retry(args));
                }
                other => return other,
            }
        }
    }
}

fn wrap_repeat_guard(inner: EventStream) -> EventStream {
    Box::pin(RepeatGuardStream {
        inner,
        buf: String::new(),
        stopped: false,
    })
}

fn utf8_suffix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut idx = s.len() - max_bytes;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    &s[idx..]
}

fn repeated_assistant_phrase(buf: &str) -> bool {
    if buf.len() < REPEAT_NEEDLE_CHARS * REPEAT_LIMIT {
        return false;
    }
    let needle = utf8_suffix(buf, REPEAT_NEEDLE_CHARS);
    if needle.is_empty() || needle.chars().all(char::is_whitespace) {
        return false;
    }
    let mut count = 0usize;
    let mut rest = buf;
    while let Some(pos) = rest.find(needle) {
        count += 1;
        if count >= REPEAT_LIMIT {
            return true;
        }
        rest = &rest[pos + needle.len()..];
    }
    false
}

impl Stream for RepeatGuardStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        if self.stopped {
            return Poll::Ready(None);
        }
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(StreamEvent::TextDelta(text)))) => {
                self.buf.push_str(&text);
                if repeated_assistant_phrase(&self.buf) {
                    self.stopped = true;
                    Poll::Ready(Some(Ok(StreamEvent::TextDelta(
                        "\n\n[jcode stopped a repeated Grok output loop]".to_string(),
                    ))))
                } else {
                    Poll::Ready(Some(Ok(StreamEvent::TextDelta(text))))
                }
            }
            other => other,
        }
    }
}

#[derive(Default, Debug)]
struct DiscoveredModels {
    current: Option<String>,
    available: Vec<String>,
}

fn models_from_initialize(response: &acp::InitializeResponse) -> DiscoveredModels {
    let state = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("modelState"));
    models_from_value(state)
}

fn models_from_value(value: Option<&Value>) -> DiscoveredModels {
    let Some(object) = value.and_then(Value::as_object) else {
        return DiscoveredModels::default();
    };
    let current = object
        .get("currentModelId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut available = Vec::new();
    if let Some(models) = object.get("availableModels").and_then(Value::as_array) {
        for value in models {
            let id = value.as_str().or_else(|| {
                value.as_object().and_then(|model| {
                    ["modelId", "id", "name"]
                        .into_iter()
                        .find_map(|key| model.get(key).and_then(Value::as_str))
                })
            });
            if let Some(id) = id.filter(|id| !id.trim().is_empty())
                && !available.iter().any(|known| known == id)
            {
                available.push(id.to_string());
            }
        }
    }
    if let Some(current) = current.as_ref()
        && !available.iter().any(|model| model == current)
    {
        available.insert(0, current.clone());
    }
    DiscoveredModels { current, available }
}

fn select_subscription_auth_method(
    response: &acp::InitializeResponse,
) -> Result<acp::AuthMethodId> {
    let allowed = response.auth_methods.iter().filter(|method| {
        let id = method.id().0.as_ref().to_ascii_lowercase();
        id != "xai.api_key" && !id.contains("api_key") && !id.contains("api-key")
    });
    for preferred in ["cached_token", "grok.com"] {
        if let Some(method) = allowed
            .clone()
            .find(|method| method.id().0.as_ref() == preferred)
        {
            return Ok(method.id().clone());
        }
    }
    if let Some(method) = allowed.into_iter().find(|method| {
        let id = method.id().0.as_ref().to_ascii_lowercase();
        let name = method.name().to_ascii_lowercase();
        id.contains("grok") || id.contains("cached") || name.contains("grok")
    }) {
        return Ok(method.id().clone());
    }
    let advertised = response
        .auth_methods
        .iter()
        .map(|method| method.id().0.as_ref())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "Grok CLI did not advertise a cached subscription authentication method (advertised: {})",
        if advertised.is_empty() {
            "none"
        } else {
            &advertised
        }
    )
}

async fn initialize_and_authenticate(
    connection: &acp::ClientSideConnection,
) -> Result<acp::InitializeResponse> {
    let initialize = acp::InitializeRequest::new(acp::ProtocolVersion::V1)
        .client_info(acp::Implementation::new("jcode", env!("CARGO_PKG_VERSION")).title("Jcode"));
    let response = timeout_request("initialize", connection.initialize(initialize)).await?;
    if response.protocol_version != acp::ProtocolVersion::V1 {
        bail!(
            "Grok CLI negotiated unsupported ACP protocol version {:?}",
            response.protocol_version
        );
    }
    let auth_method = select_subscription_auth_method(&response)?;
    let mut meta = Map::new();
    meta.insert("headless".to_string(), Value::Bool(true));
    timeout_request(
        "authenticate",
        connection.authenticate(acp::AuthenticateRequest::new(auth_method).meta(meta)),
    )
    .await?;
    Ok(response)
}

fn resume_session_is_missing(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("FS_NOT_FOUND")
        || text.contains("Path not found")
        || text.contains("No such file or directory")
}

fn grok_sessions_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".grok").join("sessions"))
}

fn percent_decode_path(encoded: &str) -> PathBuf {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(high) = from_hex(bytes[index + 1])
            && let Some(low) = from_hex(bytes[index + 2])
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Grok CLI stores ACP sessions under `~/.grok/sessions/<percent-encoded-cwd>/<id>`.
/// Jcode's process cwd often differs (daemon at `$HOME`, TUI `/cd` drift), so
/// resume must use the workspace folder that actually contains the session.
fn grok_resume_cwd(session_id: &str) -> Option<PathBuf> {
    grok_resume_cwd_in(grok_sessions_root()?, session_id)
}

fn grok_resume_cwd_in(sessions_root: PathBuf, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(sessions_root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join(session_id).is_dir() {
            let encoded = entry.file_name();
            return Some(percent_decode_path(&encoded.to_string_lossy()));
        }
    }
    None
}

async fn timeout_request<T>(
    name: &'static str,
    future: impl std::future::Future<Output = acp::Result<T>>,
) -> Result<T> {
    tokio::time::timeout(ACP_REQUEST_TIMEOUT, future)
        .await
        .map_err(|_| {
            anyhow!(
                "Grok CLI ACP {name} timed out after {}s",
                ACP_REQUEST_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| anyhow!("Grok CLI ACP {name} failed: {error}"))
}

fn run_turn_thread(
    process: GrokBuildProcess,
    selected_model: Option<String>,
    resume_session_id: Option<String>,
    system: String,
    prompt: String,
    tx: mpsc::Sender<Result<StreamEvent>>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build Grok Build ACP Tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        with_connection(process, tx.clone(), async move |connection| {
            initialize_and_authenticate(&connection).await?;
            let process_cwd =
                std::env::current_dir().context("Failed to determine working directory")?;
            let (session_id, session_model) = if let Some(session_id) = resume_session_id {
                let resume_cwd = grok_resume_cwd(&session_id).unwrap_or_else(|| process_cwd.clone());
                match timeout_request(
                    "session/resume",
                    connection.resume_session(acp::ResumeSessionRequest::new(
                        session_id.clone(),
                        resume_cwd,
                    )),
                )
                .await
                {
                    Ok(response) => (acp::SessionId::new(session_id), response.models),
                    Err(error) if resume_session_is_missing(&error) => {
                        let response = timeout_request(
                            "session/new",
                            connection.new_session(grok_new_session_request(
                                process_cwd,
                                &system,
                            )),
                        )
                        .await?;
                        (response.session_id, response.models)
                    }
                    Err(error) => return Err(error),
                }
            } else {
                let response = timeout_request(
                    "session/new",
                    connection.new_session(grok_new_session_request(process_cwd, &system)),
                )
                .await?;
                (response.session_id, response.models)
            };

            tx.send(Ok(StreamEvent::SessionId(session_id.0.to_string())))
                .await
                .map_err(|_| anyhow!("Grok Build stream consumer closed"))?;

            let current_model = session_model
                .as_ref()
                .map(|models| models.current_model_id.0.as_ref());
            if let Some(selected_model) = selected_model
                && current_model != Some(selected_model.as_str())
            {
                timeout_request(
                    "session/set_model",
                    connection.set_session_model(acp::SetSessionModelRequest::new(
                        session_id.clone(),
                        selected_model,
                    )),
                )
                .await?;
            }

            let prompt_request = acp::PromptRequest::new(
                session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
            );
            tokio::pin!(cancel_rx);
            let response = tokio::select! {
                response = tokio::time::timeout(ACP_PROMPT_TIMEOUT, connection.prompt(prompt_request)) => {
                    response
                        .map_err(|_| anyhow!("Grok CLI ACP session/prompt timed out after {}s", ACP_PROMPT_TIMEOUT.as_secs()))?
                        .map_err(|error| anyhow!("Grok CLI ACP session/prompt failed: {error}"))?
                }
                _ = &mut cancel_rx => {
                    connection.cancel(acp::CancelNotification::new(session_id.clone())).await
                        .map_err(|error| anyhow!("Failed to cancel Grok CLI ACP prompt: {error}"))?;
                    // `cancel` queues a JSON-RPC notification. Give the local
                    // connection driver one scheduling turn to flush it before
                    // dropping the kill-on-drop child process.
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    return Ok(());
                }
            };
            tx.send(Ok(StreamEvent::MessageEnd {
                stop_reason: Some(format!("{:?}", response.stop_reason).to_ascii_lowercase()),
            }))
            .await
            .map_err(|_| anyhow!("Grok Build stream consumer closed"))?;
            Ok(())
        })
        .await
    })
}

type LocalConnectionFuture<T> = Pin<Box<dyn std::future::Future<Output = Result<T>> + 'static>>;

async fn run_on_acp_thread_with_process<T: Send + 'static>(
    process: GrokBuildProcess,
    operation: impl FnOnce(acp::ClientSideConnection) -> LocalConnectionFuture<T> + Send + 'static,
) -> Result<T> {
    let (result_tx, result_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("jcode-grok-build-acp-probe".to_string())
        .spawn(move || {
            let result = (|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let local = tokio::task::LocalSet::new();
                local.block_on(&runtime, async move {
                    with_connection(process, mpsc::channel(1).0, operation).await
                })
            })();
            let _ = result_tx.send(result);
        })
        .context("Failed to start Grok Build ACP probe thread")?;
    result_rx
        .await
        .context("Grok Build ACP probe thread exited without a result")?
}

async fn with_connection<T, F, Fut>(
    process: GrokBuildProcess,
    event_tx: mpsc::Sender<Result<StreamEvent>>,
    operation: F,
) -> Result<T>
where
    F: FnOnce(acp::ClientSideConnection) -> Fut,
    Fut: std::future::Future<Output = Result<T>> + 'static,
{
    let mut command = Command::new(&process.command);
    command
        .args(&process.args)
        .envs(&process.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().with_context(|| {
        format!(
            "Failed to launch Jcode's managed Grok Build backend at '{}'",
            process.command.display()
        )
    })?;
    let stdin = child
        .stdin
        .take()
        .context("Grok CLI stdin was unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Grok CLI stdout was unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Grok CLI stderr was unavailable")?;
    let stderr_capture = Arc::new(std::sync::Mutex::new(String::new()));
    let mut stderr_task =
        tokio::task::spawn_local(capture_stderr(stderr, Arc::clone(&stderr_capture)));

    let received_message = Arc::new(AtomicBool::new(false));
    let client = GrokAcpClient {
        tx: event_tx,
        received_message: Arc::clone(&received_message),
        tools: Mutex::new(HashMap::new()),
    };
    let (connection, io) =
        acp::ClientSideConnection::new(client, stdin.compat_write(), stdout.compat(), |future| {
            tokio::task::spawn_local(future);
        });
    let io_task = tokio::task::spawn_local(io);
    let result = operation(connection).await;
    let _ = child.kill().await;
    io_task.abort();
    let _ = tokio::time::timeout(Duration::from_millis(100), &mut stderr_task).await;
    stderr_task.abort();
    let stderr = stderr_capture
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .trim()
        .to_string();
    if result.is_ok()
        && !received_message.load(Ordering::Acquire)
        && stderr_reports_provider_failure(&stderr)
    {
        bail!("Grok CLI provider request failed: {stderr}");
    }
    result.map_err(|error| {
        if stderr.is_empty() {
            error
        } else {
            error.context(format!("Grok CLI stderr: {stderr}"))
        }
    })
}

fn stderr_reports_provider_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    [
        "api error",
        "payment required",
        "balance exhausted",
        "quota exhausted",
        "too many requests",
        "rate limit",
        "http_status",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
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

struct GrokAcpClient {
    tx: mpsc::Sender<Result<StreamEvent>>,
    received_message: Arc<AtomicBool>,
    tools: Mutex<HashMap<String, TrackedAcpTool>>,
}

#[derive(Default)]
struct TrackedAcpTool {
    name: String,
    file_path: Option<String>,
    diffs: Vec<(String, Option<String>, String)>,
    result_text: Option<String>,
    title: Option<String>,
    raw_input: Option<Value>,
    started: bool,
    finished: bool,
    emitted_input: Option<String>,
}

#[async_trait(?Send)]
impl acp::Client for GrokAcpClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        let action = grok_permission_action(&request);
        let description = grok_permission_description(&request);
        let approved = if jcode_base::safety::SafetySystem::new().classify(&action)
            == jcode_base::safety::ActionTier::AutoAllowed
        {
            true
        } else {
            let _ = self.tx.try_send(Ok(StreamEvent::StatusDetail {
                detail: format!(
                    "Grok ACP wants permission for '{action}'. Approve in /permissions."
                ),
            }));
            wait_for_jcode_permission(&action, &description).await
        };
        Ok(acp::RequestPermissionResponse::new(
            acp_permission_outcome(approved, &request.options),
        ))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        let events = match notification.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                self.received_message.store(true, Ordering::Release);
                text_from_acp_content(chunk.content)
                    .map(StreamEvent::TextDelta)
                    .into_iter()
                    .collect()
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => text_from_acp_content(chunk.content)
                .map(StreamEvent::ThinkingDelta)
                .into_iter()
                .collect(),
            acp::SessionUpdate::ToolCall(call) => {
                let id = call.tool_call_id.0.to_string();
                self.events_for_acp_tool(
                    id,
                    acp_nonzero_kind(call.kind),
                    Some(call.title),
                    acp_nonzero_status(call.status),
                    call.content,
                    call.locations,
                    call.raw_input,
                    call.meta,
                )
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                self.events_for_acp_tool(
                    id,
                    update.fields.kind,
                    update.fields.title,
                    update.fields.status,
                    update.fields.content.unwrap_or_default(),
                    update.fields.locations.unwrap_or_default(),
                    update.fields.raw_input,
                    update.meta,
                )
            }
            _ => Vec::new(),
        };
        for event in events {
            let _ = self.tx.send(Ok(event)).await;
        }
        Ok(())
    }
}

impl GrokAcpClient {
    fn events_for_acp_tool(
        &self,
        id: String,
        kind: Option<acp::ToolKind>,
        title: Option<String>,
        status: Option<acp::ToolCallStatus>,
        content: Vec<acp::ToolCallContent>,
        locations: Vec<acp::ToolCallLocation>,
        raw_input: Option<Value>,
        meta: Option<acp::Meta>,
    ) -> Vec<StreamEvent> {
        let incoming_diffs = {
            let mut diffs = acp_tool_diffs(&content);
            if diffs.is_empty() {
                diffs = diffs_from_raw_input(raw_input.as_ref());
            }
            diffs
        };
        let incoming_path = incoming_diffs
            .first()
            .map(|(path, _, _)| path.clone())
            .or_else(|| {
                locations
                    .first()
                    .map(|location| location.path.display().to_string())
            })
            .or_else(|| raw_input_path(raw_input.as_ref()));
        let incoming_name = acp_tool_name(
            kind,
            title.as_deref(),
            meta.as_ref(),
            raw_input.as_ref(),
            &incoming_diffs,
        );
        let incoming_text = acp_tool_content_text(&content);
        let failed = matches!(status, Some(acp::ToolCallStatus::Failed));
        let status_done = matches!(
            status,
            Some(acp::ToolCallStatus::Completed | acp::ToolCallStatus::Failed)
        );

        let mut tools = self
            .tools
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tracked = tools.entry(id.clone()).or_default();
        if tracked.finished {
            return Vec::new();
        }
        if incoming_name != "tool" {
            tracked.name = incoming_name.to_string();
        }
        if let Some(path) = incoming_path {
            tracked.file_path = Some(path);
        }
        if !incoming_diffs.is_empty() {
            tracked.diffs = incoming_diffs;
        }
        if let Some(text) = incoming_text {
            tracked.result_text = Some(text);
        }
        if let Some(title) = title.clone() {
            tracked.title = Some(title);
        }
        merge_raw_input(&mut tracked.raw_input, raw_input);
        if tracked.name == "edit"
            && tracked.diffs.len() == 1
            && tracked.diffs[0].1.as_deref().unwrap_or("").is_empty()
        {
            tracked.name = "write".to_string();
        }

        // Preview diffs while InProgress, but only finalize on Completed/Failed.
        // Finalizing from a non-empty diff made later Failed updates a no-op.
        let completed = status_done;
        if tracked.name.is_empty() {
            tracked.name = "tool".to_string();
        }
        let ready = tracked.name != "tool" || completed || !tracked.diffs.is_empty();

        let mut events = Vec::new();
        if let Some(detail) = tracked.title.clone()
            && !tracked.started
            && !ready
        {
            events.push(StreamEvent::StatusDetail { detail });
        }
        if ready && !tracked.started {
            tracked.started = true;
            events.push(StreamEvent::ToolUseStart {
                id: id.clone(),
                name: tracked.name.clone(),
            });
        }
        if tracked.started && !tracked.finished {
            let input = acp_tool_input(
                &tracked.name,
                tracked.file_path.as_deref(),
                &tracked.diffs,
                tracked.raw_input.clone(),
            );
            let serialized = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
            if tracked.emitted_input.as_deref() != Some(serialized.as_str()) {
                events.push(StreamEvent::ToolInputDelta(serialized.clone()));
                tracked.emitted_input = Some(serialized);
            }
            if completed {
                tracked.finished = true;
                let result = acp_tool_result_text(
                    tracked.title.as_deref(),
                    &tracked.diffs,
                    tracked.result_text.as_deref(),
                );
                events.push(StreamEvent::ToolUseEnd);
                events.push(StreamEvent::ToolResult {
                    tool_use_id: id,
                    content: result,
                    is_error: failed,
                });
            }
        }
        events
    }
}

fn grok_permission_action(request: &acp::RequestPermissionRequest) -> String {
    let title = request
        .tool_call
        .fields
        .title
        .as_deref()
        .unwrap_or("tool")
        .trim();
    grok_tool_alias(title)
        .or_else(|| (!title.is_empty()).then_some(title))
        .unwrap_or("tool")
        .to_string()
}

fn grok_permission_description(request: &acp::RequestPermissionRequest) -> String {
    let title = request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "tool".to_string());
    let path = request
        .tool_call
        .fields
        .locations
        .as_ref()
        .and_then(|locations| locations.first())
        .map(|location| location.path.display().to_string());
    match path {
        Some(path) => format!("{title} {path}"),
        None => title,
    }
}

fn acp_permission_outcome(
    approved: bool,
    options: &[acp::PermissionOption],
) -> acp::RequestPermissionOutcome {
    let preferred = if approved {
        acp::PermissionOptionKind::AllowOnce
    } else {
        acp::PermissionOptionKind::RejectOnce
    };
    let selected = options.iter().find(|option| option.kind == preferred).or_else(|| {
        if approved {
            None
        } else {
            options.iter().find(|option| {
                matches!(
                    option.kind,
                    acp::PermissionOptionKind::RejectOnce | acp::PermissionOptionKind::RejectAlways
                )
            })
        }
    });
    match selected {
        Some(option) => acp::RequestPermissionOutcome::Selected(
            acp::SelectedPermissionOutcome::new(option.option_id.clone()),
        ),
        None => acp::RequestPermissionOutcome::Cancelled,
    }
}

async fn wait_for_jcode_permission(action: &str, description: &str) -> bool {
    let request_id = jcode_base::safety::enqueue_tool_permission(
        action,
        description,
        "Grok CLI ACP requested permission before running a tool.",
    );
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    while tokio::time::Instant::now() < deadline {
        if let Some(approved) = jcode_base::safety::decision_via_file(&request_id) {
            return approved;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let _ = jcode_base::safety::record_permission_via_file(
        &request_id,
        false,
        "grok_acp_timeout",
        Some("Timed out waiting for /permissions".to_string()),
    );
    false
}

fn grok_http_unauthorized(error: &anyhow::Error) -> bool {
    grok_http_unauthorized_text(&format!("{error:#}"))
}

fn grok_http_unauthorized_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("status: 401")
        || (text.contains("401")
            && (text.contains("unauthorized")
                || text.contains("invalid token")
                || text.contains("expired")
                || text.contains("unauthenticated")))
}

fn acp_nonzero_kind(kind: acp::ToolKind) -> Option<acp::ToolKind> {
    match kind {
        acp::ToolKind::Other => None,
        other => Some(other),
    }
}

fn acp_nonzero_status(status: acp::ToolCallStatus) -> Option<acp::ToolCallStatus> {
    match status {
        acp::ToolCallStatus::Pending => None,
        other => Some(other),
    }
}

fn grok_meta_tool_name(meta: Option<&acp::Meta>) -> Option<&str> {
    meta.and_then(|meta| meta.get("x.ai/tool"))
        .and_then(Value::as_object)
        .and_then(|tool| tool.get("name"))
        .and_then(Value::as_str)
}

fn grok_tool_alias(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "search_replace" | "str_replace" | "strreplace" | "file_edit" | "edit" => Some("edit"),
        "write_file" | "file_write" | "write" => Some("write"),
        "read_file" | "file_read" | "read" => Some("read"),
        "run_terminal_command" | "shell_exec" | "bash" => Some("bash"),
        "list_dir" | "ls" => Some("ls"),
        "todo_write" | "todo_read" | "todo" => Some("todo"),
        "grep" | "file_grep" => Some("grep"),
        "glob" | "file_glob" => Some("glob"),
        _ => None,
    }
}

fn acp_tool_name(
    kind: Option<acp::ToolKind>,
    title: Option<&str>,
    meta: Option<&acp::Meta>,
    raw_input: Option<&Value>,
    diffs: &[(String, Option<String>, String)],
) -> &'static str {
    if let Some(name) = grok_meta_tool_name(meta).and_then(grok_tool_alias) {
        return name;
    }
    if let Some(name) = title.and_then(grok_tool_alias) {
        return name;
    }
    if let Some(variant) = raw_input
        .and_then(|value| value.get("variant"))
        .and_then(Value::as_str)
        && variant.eq_ignore_ascii_case("searchreplace")
    {
        return "edit";
    }
    if diffs.iter().any(|(_, old, _)| old.is_none()) && diffs.len() == 1 {
        return "write";
    }
    if !diffs.is_empty() {
        return "edit";
    }
    match kind {
        Some(acp::ToolKind::Edit | acp::ToolKind::Delete | acp::ToolKind::Move) => "edit",
        Some(acp::ToolKind::Read) => "read",
        Some(acp::ToolKind::Execute) => "bash",
        Some(acp::ToolKind::Search) => "grep",
        Some(acp::ToolKind::Fetch) => "webfetch",
        _ => {
            let title = title.unwrap_or_default().to_ascii_lowercase();
            if title.contains("edit") || title.contains("write") || title.contains("patch") {
                "edit"
            } else if title.contains("read") {
                "read"
            } else if title.contains("bash") || title.contains("shell") || title.contains("run") {
                "bash"
            } else {
                "tool"
            }
        }
    }
}

fn acp_tool_diffs(content: &[acp::ToolCallContent]) -> Vec<(String, Option<String>, String)> {
    content
        .iter()
        .filter_map(|block| match block {
            acp::ToolCallContent::Diff(diff) => Some((
                diff.path.display().to_string(),
                diff.old_text.clone(),
                diff.new_text.clone(),
            )),
            _ => None,
        })
        .collect()
}

fn raw_input_path(raw_input: Option<&Value>) -> Option<String> {
    raw_input.and_then(|value| {
        value
            .get("file_path")
            .or_else(|| value.get("target_file"))
            .or_else(|| value.get("path"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn diffs_from_raw_input(raw_input: Option<&Value>) -> Vec<(String, Option<String>, String)> {
    let Some(value) = raw_input else {
        return Vec::new();
    };
    let old = value.get("old_string").and_then(Value::as_str);
    let new = value
        .get("new_string")
        .and_then(Value::as_str)
        .or_else(|| value.get("contents").and_then(Value::as_str))
        .or_else(|| value.get("content").and_then(Value::as_str));
    let Some(new) = new else {
        return Vec::new();
    };
    let Some(path) = raw_input_path(Some(value)) else {
        return Vec::new();
    };
    vec![(path, old.map(ToOwned::to_owned), new.to_string())]
}

fn acp_tool_content_text(content: &[acp::ToolCallContent]) -> Option<String> {
    let parts: Vec<String> = content
        .iter()
        .filter_map(|block| match block {
            acp::ToolCallContent::Content(block) => match &block.content {
                acp::ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            },
            _ => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect();
    (!parts.is_empty()).then_some(parts.join("\n"))
}

fn merge_raw_input(existing: &mut Option<Value>, incoming: Option<Value>) {
    let Some(incoming) = incoming else {
        return;
    };
    match (existing.take(), incoming) {
        (Some(Value::Object(mut current)), Value::Object(next)) => {
            current.extend(next);
            *existing = Some(Value::Object(current));
        }
        (_, incoming) => *existing = Some(incoming),
    }
}

fn acp_tool_input(
    name: &str,
    file_path: Option<&str>,
    diffs: &[(String, Option<String>, String)],
    raw_input: Option<Value>,
) -> Value {
    if let Some(Value::Object(mut object)) = raw_input {
        if let Some(path) = file_path
            && !object.contains_key("file_path")
            && !object.contains_key("path")
            && !object.contains_key("target_file")
        {
            object.insert("file_path".to_string(), Value::String(path.to_string()));
        }
        if let Some(target) = object.remove("target_file")
            && !object.contains_key("file_path")
        {
            object.insert("file_path".to_string(), target);
        }
        if name == "edit"
            && let Some((_, old, new)) = diffs.first()
        {
            object
                .entry("old_string")
                .or_insert_with(|| Value::String(old.clone().unwrap_or_default()));
            object
                .entry("new_string")
                .or_insert_with(|| Value::String(new.clone()));
        }
        if name == "write"
            && let Some((_, _, new)) = diffs.first()
        {
            object
                .entry("content")
                .or_insert_with(|| Value::String(new.clone()));
        }
        return Value::Object(object);
    }

    let mut object = Map::new();
    if let Some(path) = file_path {
        object.insert("file_path".to_string(), Value::String(path.to_string()));
    }
    match (name, diffs.first()) {
        ("edit", Some((_, old, new))) => {
            object.insert(
                "old_string".to_string(),
                Value::String(old.clone().unwrap_or_default()),
            );
            object.insert("new_string".to_string(), Value::String(new.clone()));
        }
        ("write", Some((_, _, new))) => {
            object.insert("content".to_string(), Value::String(new.clone()));
        }
        _ => {}
    }
    Value::Object(object)
}

fn acp_tool_result_text(
    title: Option<&str>,
    diffs: &[(String, Option<String>, String)],
    result_text: Option<&str>,
) -> String {
    if !diffs.is_empty() {
        return diffs
            .iter()
            .map(|(path, old, new)| format_unified_diff(path, old.as_deref().unwrap_or(""), new))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(text) = result_text.map(str::trim).filter(|text| !text.is_empty()) {
        return text.to_string();
    }
    title.unwrap_or("ok").to_string()
}

fn format_unified_diff(path: &str, old: &str, new: &str) -> String {
    // Leading newline so the TUI's `[edit] {output}` wrapper does not glue onto
    // the `--- a/` header (that made collect_diff_lines miss the first hunk).
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = format!("\n--- a/{path}\n+++ b/{path}\n");
    let mut hunks = 0usize;
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        hunks += 1;
        out.push_str(&format!("{hunk}"));
    }
    if hunks == 0 && old != new {
        for line in new.lines() {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod acp_tool_display_tests {
    use super::*;

    #[test]
    fn edit_diff_becomes_edit_tool_with_old_and_new_strings() {
        let diffs = vec![(
            "src/lib.rs".to_string(),
            Some("fn old() {}\n".to_string()),
            "fn new() {}\n".to_string(),
        )];
        assert_eq!(
            acp_tool_name(
                Some(acp::ToolKind::Edit),
                Some("Editing"),
                None,
                None,
                &diffs
            ),
            "edit"
        );
        let input = acp_tool_input("edit", Some("src/lib.rs"), &diffs, None);
        assert_eq!(input["file_path"], "src/lib.rs");
        assert_eq!(input["old_string"], "fn old() {}\n");
        assert_eq!(input["new_string"], "fn new() {}\n");
        let result = acp_tool_result_text(Some("Editing"), &diffs, None);
        assert!(result.contains("-fn old() {}"));
        assert!(result.contains("+fn new() {}"));
    }

    #[test]
    fn new_file_diff_becomes_write_tool() {
        let diffs = vec![("new.rs".to_string(), None, "hello\n".to_string())];
        assert_eq!(
            acp_tool_name(Some(acp::ToolKind::Edit), None, None, None, &diffs),
            "write"
        );
        let input = acp_tool_input("write", Some("new.rs"), &diffs, None);
        assert_eq!(input["content"], "hello\n");
    }

    #[test]
    fn grok_search_replace_raw_input_is_an_edit_tool() {
        let raw = serde_json::json!({
            "file_path": "src/lib.rs",
            "old_string": "fn old() {}\n",
            "new_string": "fn new() {}\n"
        });
        let diffs = diffs_from_raw_input(Some(&raw));
        assert_eq!(diffs[0].0, "src/lib.rs");
        let mut meta = acp::Meta::new();
        meta.insert(
            "x.ai/tool".to_string(),
            serde_json::json!({"name": "search_replace", "kind": "edit"}),
        );
        assert_eq!(
            acp_tool_name(None, Some("search_replace"), Some(&meta), None, &[]),
            "edit"
        );
        assert_eq!(grok_tool_alias("search_replace"), Some("edit"));
        assert_eq!(grok_tool_alias("read_file"), Some("read"));
        assert_eq!(grok_tool_alias("run_terminal_command"), Some("bash"));
    }

    #[test]
    fn grok_resume_cwd_decodes_percent_encoded_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let encoded = temp.path().join("%2Fmnt%2Fc%2FUsers%2FHASAKI");
        std::fs::create_dir_all(encoded.join("01a0b27f-session")).unwrap();
        assert_eq!(
            grok_resume_cwd_in(temp.path().to_path_buf(), "01a0b27f-session"),
            Some(PathBuf::from("/mnt/c/Users/HASAKI"))
        );
        assert!(resume_session_is_missing(&anyhow!(
            "Grok CLI ACP session/resume failed: Path not found.: {{\"code\":\"FS_NOT_FOUND\"}}"
        )));
    }

    #[test]
    fn grok_live_search_replace_session_update_deserializes_and_emits_diff() {
        let json = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-9f471bec-7f60-4839-977d-7647d60f7a7d-120",
            "title": "search_replace",
            "rawInput": {
                "file_path": "consts/call_bot.consts.go",
                "old_string": "\tCallBotResultSuccess = \"success\"\n\tCallBotResultFail    = \"fail\"\n)",
                "new_string": "\tCallBotResultSuccess = \"success\"\n\tCallBotResultFail    = \"fail\"\n\tCallBotScenarioClinicNoShow = \"clinic_no_show\"\n)"
            },
            "_meta": {
                "x.ai/tool": {
                    "name": "search_replace",
                    "kind": "edit"
                }
            }
        });
        let update: acp::SessionUpdate =
            serde_json::from_value(json).expect("Grok tool_call JSON must deserialize");
        let acp::SessionUpdate::ToolCall(call) = update else {
            panic!("expected ToolCall, got {update:?}");
        };
        assert!(
            call.raw_input.is_some(),
            "rawInput dropped during deserialize: {call:?}"
        );

        let (tx, _rx) = mpsc::channel(8);
        let client = GrokAcpClient {
            tx,
            received_message: Arc::new(AtomicBool::new(false)),
            tools: Mutex::new(HashMap::new()),
        };
        let events = client.events_for_acp_tool(
            call.tool_call_id.0.to_string(),
            acp_nonzero_kind(call.kind),
            Some(call.title),
            acp_nonzero_status(call.status),
            call.content,
            call.locations,
            call.raw_input,
            call.meta,
        );
        let input = events.iter().find_map(|event| match event {
            StreamEvent::ToolInputDelta(delta) => Some(delta.as_str()),
            _ => None,
        });
        let input = input.expect(&format!("missing ToolInputDelta in {events:?}"));
        assert!(
            input.contains("old_string"),
            "input missing old_string: {input}"
        );
        assert!(
            input.contains("new_string"),
            "input missing new_string: {input}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolResult { .. })),
            "in-progress edit must not finalize: {events:?}"
        );
        let done = client.events_for_acp_tool(
            call.tool_call_id.0.to_string(),
            None,
            None,
            Some(acp::ToolCallStatus::Completed),
            Vec::new(),
            Vec::new(),
            None,
            None,
        );
        let result = done.iter().find_map(|event| match event {
            StreamEvent::ToolResult {
                content,
                is_error,
                ..
            } if !is_error => Some(content.as_str()),
            _ => None,
        });
        let result = result.expect("missing completed ToolResult");
        assert!(
            result.contains("CallBotScenarioClinicNoShow")
                && result.contains('-')
                && result.contains('+'),
            "result is not a diff: {result:?}"
        );
        let dump = format_unified_diff(
            "consts/call_bot.consts.go",
            "\tCallBotResultSuccess = \"success\"\n\tCallBotResultFail    = \"fail\"\n)",
            "\tCallBotResultSuccess = \"success\"\n\tCallBotResultFail    = \"fail\"\n\tCallBotScenarioClinicNoShow = \"clinic_no_show\"\n)",
        );
        assert!(
            dump.contains("CallBotScenarioClinicNoShow"),
            "similar produced no hunks: {dump:?}"
        );
        assert!(dump.contains('\n'));
    }

    #[test]
    fn in_progress_edit_failed_update_is_not_dropped() {
        let (tx, _rx) = mpsc::channel(8);
        let client = GrokAcpClient {
            tx,
            received_message: Arc::new(AtomicBool::new(false)),
            tools: Mutex::new(HashMap::new()),
        };
        let preview = client.events_for_acp_tool(
            "edit-fail".to_string(),
            Some(acp::ToolKind::Edit),
            Some("search_replace".to_string()),
            Some(acp::ToolCallStatus::InProgress),
            Vec::new(),
            Vec::new(),
            Some(serde_json::json!({
                "file_path": "src/lib.rs",
                "old_string": "a",
                "new_string": "b"
            })),
            None,
        );
        assert!(
            preview
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolUseStart { .. })),
            "{preview:?}"
        );
        assert!(
            !preview
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolResult { .. })),
            "in-progress must not emit ToolResult: {preview:?}"
        );
        let failed = client.events_for_acp_tool(
            "edit-fail".to_string(),
            None,
            None,
            Some(acp::ToolCallStatus::Failed),
            vec![acp::ToolCallContent::from("write failed")],
            Vec::new(),
            None,
            None,
        );
        assert!(
            failed.iter().any(|event| matches!(
                event,
                StreamEvent::ToolResult { is_error: true, content, .. }
                    if content.contains("write failed") || content.contains("src/lib.rs")
            )),
            "failed update was dropped: {failed:?}"
        );
    }

    #[test]
    fn workspace_mcp_overrides_global_duplicate_and_can_disable() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global.json");
        let project = temp.path().join("project.json");
        std::fs::write(
            &global,
            r#"{"servers":{"dup":{"command":"global-bin"},"keep":{"command":"keep-bin"}}}"#,
        )
        .unwrap();
        std::fs::write(
            &project,
            r#"{"servers":{"dup":{"command":"project-bin"},"keep":{"disabled":true}}}"#,
        )
        .unwrap();
        let servers = merge_mcp_server_files(&[global, project]);
        let names: Vec<_> = servers
            .iter()
            .filter_map(mcp_server_name)
            .collect();
        assert_eq!(names, vec!["dup".to_string()]);
        let acp::McpServer::Stdio(stdio) = &servers[0] else {
            panic!("expected stdio");
        };
        assert_eq!(stdio.command, PathBuf::from("project-bin"));
    }

    #[test]
    fn grok_read_file_update_keeps_target_path_and_body() {
        let (tx, _rx) = mpsc::channel(8);
        let client = GrokAcpClient {
            tx,
            received_message: Arc::new(AtomicBool::new(false)),
            tools: Mutex::new(HashMap::new()),
        };
        let first = client.events_for_acp_tool(
            "read-1".to_string(),
            None,
            Some("read_file".to_string()),
            None,
            Vec::new(),
            Vec::new(),
            Some(serde_json::json!({"target_file": "src/main.go"})),
            None,
        );
        assert!(
            first.iter().any(|event| matches!(
                event,
                StreamEvent::ToolUseStart { name, .. } if name == "read"
            )),
            "missing read start: {first:?}"
        );
        assert!(
            !first
                .iter()
                .any(|event| matches!(event, StreamEvent::ToolResult { .. })),
            "read should not complete before content: {first:?}"
        );

        let mut meta = acp::Meta::new();
        meta.insert(
            "x.ai/tool".to_string(),
            serde_json::json!({"name": "read_file", "kind": "read"}),
        );
        let content = vec![acp::ToolCallContent::from("package main\n")];
        let second = client.events_for_acp_tool(
            "read-1".to_string(),
            None,
            None,
            Some(acp::ToolCallStatus::Completed),
            content,
            Vec::new(),
            None,
            Some(meta),
        );
        let input = first
            .iter()
            .chain(second.iter())
            .find_map(|event| match event {
                StreamEvent::ToolInputDelta(delta) => Some(delta.as_str()),
                _ => None,
            });
        let input = input.expect("missing ToolInputDelta for read");
        assert!(
            input.contains("src/main.go"),
            "read lost target_file: {input}"
        );
        assert!(
            second.iter().any(|event| matches!(
                event,
                StreamEvent::ToolResult { content, .. } if content.contains("package main")
            )),
            "read lost body: {second:?}"
        );
    }

    #[test]
    fn grok_new_file_search_replace_is_write_with_content() {
        let (tx, _rx) = mpsc::channel(8);
        let client = GrokAcpClient {
            tx,
            received_message: Arc::new(AtomicBool::new(false)),
            tools: Mutex::new(HashMap::new()),
        };
        let events = client.events_for_acp_tool(
            "write-1".to_string(),
            None,
            Some("search_replace".to_string()),
            None,
            Vec::new(),
            Vec::new(),
            Some(serde_json::json!({
                "file_path": "watch.sh",
                "old_string": "",
                "new_string": "#!/bin/bash\necho hi\n"
            })),
            None,
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                StreamEvent::ToolUseStart { name, .. } if name == "write"
            )),
            "missing write start: {events:?}"
        );
        let input = events.iter().find_map(|event| match event {
            StreamEvent::ToolInputDelta(delta) => Some(delta.as_str()),
            _ => None,
        });
        let input = input.expect("missing write input");
        assert!(input.contains("watch.sh"), "{input}");
        assert!(
            input.contains("echo hi") || input.contains("new_string") || input.contains("content"),
            "{input}"
        );
    }
}

fn text_from_acp_content(content: acp::ContentBlock) -> Option<String> {
    match content {
        acp::ContentBlock::Text(text) => Some(text.text),
        _ => None,
    }
}

fn grok_new_session_request(cwd: PathBuf, system: &str) -> acp::NewSessionRequest {
    acp::NewSessionRequest::new(cwd)
        .mcp_servers(load_forwarded_mcp_servers())
        .meta(grok_session_meta(system))
}

fn grok_session_meta(system: &str) -> acp::Meta {
    let mut meta = acp::Meta::new();
    // Append Jcode's coordinator prompt (skills, MCP catalog, TUI conventions)
    // onto Grok's own agent system prompt. Do not use systemPromptOverride —
    // that would strip Grok's harness.
    if !system.trim().is_empty() {
        meta.insert(
            "rules".to_string(),
            Value::String(system.trim().to_string()),
        );
    }
    meta.insert("yoloMode".to_string(), Value::Bool(false));
    meta
}

fn load_forwarded_mcp_servers() -> Vec<acp::McpServer> {
    if std::env::var_os("JCODE_GROK_ACP_DISABLE_MCP").is_some() {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".jcode").join("mcp.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".jcode").join("mcp.json"));
        paths.push(cwd.join(".mcp.json"));
    }
    merge_mcp_server_files(&paths)
}

/// Later files win. A later `disabled`/`enabled: false` entry removes a name.
fn merge_mcp_server_files(paths: &[PathBuf]) -> Vec<acp::McpServer> {
    let mut by_name = std::collections::BTreeMap::new();
    for path in paths {
        for (name, server) in mcp_entries_from_file(path) {
            match server {
                Some(server) => {
                    by_name.insert(name, server);
                }
                None => {
                    by_name.remove(&name);
                }
            }
        }
    }
    by_name.into_values().collect()
}

fn mcp_server_name(server: &acp::McpServer) -> Option<String> {
    match server {
        acp::McpServer::Stdio(stdio) => Some(stdio.name.clone()),
        acp::McpServer::Http(http) => Some(http.name.clone()),
        acp::McpServer::Sse(sse) => Some(sse.name.clone()),
        _ => None,
    }
}

fn mcp_servers_from_file(path: &Path) -> Vec<acp::McpServer> {
    mcp_entries_from_file(path)
        .into_iter()
        .filter_map(|(_, server)| server)
        .collect()
}

fn mcp_entries_from_file(path: &Path) -> Vec<(String, Option<acp::McpServer>)> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(servers) = value
        .get("servers")
        .or_else(|| value.get("mcpServers"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|(name, config)| {
            if config.get("disabled").and_then(Value::as_bool) == Some(true)
                || config.get("enabled").and_then(Value::as_bool) == Some(false)
            {
                return Some((name.clone(), None));
            }
            let command = config.get("command").and_then(Value::as_str)?.trim();
            if command.is_empty() {
                return None;
            }
            let args = config
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let env = config
                .get("env")
                .and_then(Value::as_object)
                .map(|env| {
                    env.iter()
                        .filter_map(|(key, value)| {
                            Some(acp::EnvVariable::new(key, value.as_str()?.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some((
                name.clone(),
                Some(acp::McpServer::Stdio(
                    acp::McpServerStdio::new(name, command).args(args).env(env),
                )),
            ))
        })
        .collect()
}

fn build_prompt(messages: &[Message], resumed: bool) -> Result<String> {
    let latest_user = latest_user_text(messages)
        .ok_or_else(|| anyhow!("No user prompt found for Grok Build request"))?;

    let mut sections = Vec::new();
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

fn latest_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if is_synthetic_user_turn(message) {
            return None;
        }
        if message.role != Role::User {
            return None;
        }
        let text = user_plain_text(message);
        (!text.is_empty()).then_some(text)
    })
}

fn triggering_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != Role::User {
            return None;
        }
        if message
            .content
            .iter()
            .any(|block| matches!(block, JcodeContentBlock::ToolResult { .. }))
            && user_plain_text(message).is_empty()
        {
            return None;
        }
        let text = user_plain_text(message);
        (!text.is_empty()).then_some(text)
    })
}

fn grok_should_ignore_turn(messages: &[Message]) -> bool {
    triggering_user_text(messages).is_some_and(|text| {
        jcode_base::todo::is_auto_poke_message(&text)
            && text
                .to_ascii_lowercase()
                .contains("do not reply or wait for the user")
    })
}

fn user_plain_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            JcodeContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .trim()
        .to_string()
}

fn is_synthetic_user_turn(message: &Message) -> bool {
    if message.role != Role::User {
        return false;
    }
    if message
        .content
        .iter()
        .any(|block| matches!(block, JcodeContentBlock::ToolResult { .. }))
    {
        return false;
    }
    let text = user_plain_text(message);
    text.is_empty() || jcode_base::todo::is_auto_poke_message(&text)
}

/// Drop empty / `[auto] Re-read…` user turns so they are not the latest instruction.
fn sanitize_grok_turn_messages(messages: &[Message]) -> Vec<Message> {
    let filtered: Vec<Message> = messages
        .iter()
        .filter(|message| !is_synthetic_user_turn(message))
        .cloned()
        .collect();
    if latest_user_text(&filtered).is_some() {
        filtered
    } else {
        messages.to_vec()
    }
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

fn cached_login_hint(prefix: &str) -> String {
    format!(
        "{prefix}. Grok Build uses subscription login, not XAI_API_KEY. Run `jcode login --provider grok-build` and retry"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grok_acp_process_does_not_pass_always_approve() {
        let process = GrokBuildProcess::from_env();
        assert!(
            !process.args.iter().any(|arg| arg == "--always-approve"),
            "ACP fallback must not skip Jcode permission: {:?}",
            process.args
        );
    }

    #[test]
    fn acp_permission_outcome_never_selects_allow_always() {
        let allow_always = acp::PermissionOption::new(
            acp::PermissionOptionId::new("allow-always"),
            "Allow always",
            acp::PermissionOptionKind::AllowAlways,
        );
        let allow_once = acp::PermissionOption::new(
            acp::PermissionOptionId::new("allow-once"),
            "Allow once",
            acp::PermissionOptionKind::AllowOnce,
        );
        let reject_once = acp::PermissionOption::new(
            acp::PermissionOptionId::new("reject-once"),
            "Reject",
            acp::PermissionOptionKind::RejectOnce,
        );
        let options = vec![allow_always.clone(), allow_once.clone(), reject_once.clone()];
        match acp_permission_outcome(true, &options) {
            acp::RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.0.as_ref(), "allow-once");
            }
            other => panic!("expected AllowOnce, got {other:?}"),
        }
        match acp_permission_outcome(false, &options) {
            acp::RequestPermissionOutcome::Selected(selected) => {
                assert_eq!(selected.option_id.0.as_ref(), "reject-once");
            }
            other => panic!("expected RejectOnce, got {other:?}"),
        }
        match acp_permission_outcome(true, std::slice::from_ref(&allow_always)) {
            acp::RequestPermissionOutcome::Cancelled => {}
            other => panic!("AllowAlways-only must cancel, got {other:?}"),
        }
    }

    #[test]
    fn unauthorized_classifier_matches_proxy_status_line() {
        let error = anyhow!(
            "OpenAI-compatible chat request failed\n  status: 401 Unauthorized\n  response: {{\"error\":\"invalid token\"}}"
        );
        assert!(grok_http_unauthorized(&error));
        assert!(stream_item_is_unauthorized(&Ok(StreamEvent::Error {
            message: "status: 401 Unauthorized".to_string(),
            retry_after_secs: None,
        })));
        assert!(!grok_http_unauthorized(&anyhow!(
            "status: 403 Forbidden spending-limit"
        )));
    }

    #[test]
    fn live_oidc_setup_error_is_not_a_hard_failure_message() {
        let error = anyhow!("Grok Build HTTP runtime requires a live OIDC token");
        assert!(
            !grok_http_unauthorized(&error),
            "missing live token is not 401; complete() must fall back to ACP instead of retrying HTTP"
        );
    }

    #[test]
    fn acp_transport_leaves_compaction_to_grok_cli() {
        let mut process = GrokBuildProcess::from_env();
        process
            .env
            .insert("JCODE_FAKE_GROK_ACP_LOG".into(), "1".into());
        let provider = GrokBuildProvider::with_process(process);
        assert!(!provider.supports_compaction());
        assert!(provider.handles_tools_internally());
    }

    #[test]
    fn chooses_cached_subscription_auth_and_rejects_api_key_only() {
        let response = acp::InitializeResponse::new(acp::ProtocolVersion::V1).auth_methods(vec![
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("xai.api_key", "xAI API key")),
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("grok.com", "Grok.com")),
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("cached_token", "Cached token")),
        ]);
        assert_eq!(
            select_subscription_auth_method(&response)
                .unwrap()
                .0
                .as_ref(),
            "cached_token"
        );

        let grok_com_only =
            acp::InitializeResponse::new(acp::ProtocolVersion::V1).auth_methods(vec![
                acp::AuthMethod::Agent(acp::AuthMethodAgent::new("grok.com", "Grok.com")),
            ]);
        assert_eq!(
            select_subscription_auth_method(&grok_com_only)
                .unwrap()
                .0
                .as_ref(),
            "grok.com"
        );

        let api_only = acp::InitializeResponse::new(acp::ProtocolVersion::V1).auth_methods(vec![
            acp::AuthMethod::Agent(acp::AuthMethodAgent::new("xai.api_key", "xAI API key")),
        ]);
        assert!(select_subscription_auth_method(&api_only).is_err());
    }

    #[test]
    fn parses_dynamic_models_from_initialize_meta() {
        let state = json!({
            "currentModelId": "grok-4.5",
            "availableModels": [
                {"modelId": "grok-4.5", "name": "Grok 4.5"},
                "grok-code-fast-1",
                {"id": "grok-4.5"}
            ]
        });
        let models = models_from_value(Some(&state));
        assert_eq!(models.current.as_deref(), Some("grok-4.5"));
        assert_eq!(models.available, ["grok-4.5", "grok-code-fast-1"]);
    }

    #[test]
    fn resumed_prompt_sends_only_latest_user_text() {
        let messages = vec![
            Message::user("old"),
            Message::assistant_text("old answer"),
            Message::user("new"),
        ];
        let prompt = build_prompt(&messages, true).unwrap();
        assert!(!prompt.contains("outer"), "{prompt}");
        assert!(!prompt.contains("<system>"), "{prompt}");
        assert_eq!(prompt, "new");
    }

    #[test]
    fn prompt_ignores_tool_result_user_messages() {
        let messages = vec![
            Message::user("print degrees"),
            Message::assistant_text("ok"),
            Message {
                role: Role::User,
                content: vec![JcodeContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "print degrees".to_string(),
                    is_error: None,
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
        ];
        let prompt = build_prompt(&messages, true).unwrap();
        assert_eq!(prompt, "print degrees");
    }

    #[test]
    fn ignores_ownership_auto_poke_instead_of_redoing_last_user_request() {
        let messages = vec![
            Message::user("rồi quay lại vụ dashboard"),
            Message::assistant_text("339 khớp ô 338. List 600 là hai NV."),
            Message::user(
                "[auto] Continue the work below. Keep the todo up to date; do not reply or wait for the user.\n- Goal \"ungrouped goal\": clarify the goal and track the work.",
            ),
        ];
        assert!(grok_should_ignore_turn(&messages));
        assert_eq!(
            latest_user_text(&messages).as_deref(),
            Some("rồi quay lại vụ dashboard")
        );
    }

    #[test]
    fn still_runs_real_user_turns_and_final_response_handoff() {
        assert!(!grok_should_ignore_turn(&[Message::user(
            "rồi quay lại vụ dashboard"
        )]));
        assert!(!grok_should_ignore_turn(&[Message::user(
            jcode_base::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE
        )]));
    }

    #[test]
    fn prompt_skips_auto_poke_and_empty_user_messages() {
        let messages = vec![
            Message::user("booking Huyen cardigan 609 vs 297"),
            Message::user(jcode_base::todo::TODO_LONG_SESSION_REVIEW_MESSAGE),
            Message::user(""),
        ];
        assert_eq!(
            latest_user_text(&messages).as_deref(),
            Some("booking Huyen cardigan 609 vs 297")
        );
        let prompt = build_prompt(&messages, true).unwrap();
        assert_eq!(prompt, "booking Huyen cardigan 609 vs 297");
        let sanitized = sanitize_grok_turn_messages(&messages);
        assert_eq!(sanitized.len(), 1);
        assert_eq!(
            user_plain_text(&sanitized[0]),
            "booking Huyen cardigan 609 vs 297"
        );
    }

    #[test]
    fn sanitize_keeps_tool_result_user_turns() {
        let messages = vec![
            Message::user("print degrees"),
            Message {
                role: Role::User,
                content: vec![JcodeContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "Updating plan".to_string(),
                    is_error: None,
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
        ];
        let sanitized = sanitize_grok_turn_messages(&messages);
        assert_eq!(sanitized.len(), 2);
    }

    #[test]
    fn repeated_assistant_phrase_trips_on_looped_dump_sentence() {
        let sentence = "I'll dump the 312 excluded rows, then rebuild the Excel so 609 vs 297 is explicit.";
        let looping = sentence.repeat(8);
        assert!(repeated_assistant_phrase(&looping));
        assert!(!repeated_assistant_phrase("I'll dump the 312 excluded rows once."));
    }

    #[test]
    fn repeated_assistant_phrase_does_not_panic_inside_vietnamese_char() {
        // 'ú' is 2 bytes. A 72-byte suffix that starts on its second byte used
        // to panic: "start byte index 389 is not a char boundary".
        let mut buf = "n".repeat(388);
        buf.push('ú');
        buf.push_str(&"y".repeat(71));
        assert_eq!(buf.len(), 461);
        assert!(!buf.is_char_boundary(buf.len() - REPEAT_NEEDLE_CHARS));
        let _ = repeated_assistant_phrase(&buf);

        let sentence = "Huyền xử lý cardigan, khách đã đến, số liệu vẫn lệch.";
        assert!(repeated_assistant_phrase(&sentence.repeat(10)));
    }

    #[test]
    fn first_prompt_does_not_wrap_jcode_system_as_user_query() {
        let prompt = build_prompt(&[Message::user("hello")], false).unwrap();
        assert_eq!(prompt, "hello");
        assert!(!prompt.contains("<system>"));
        let meta = grok_session_meta("Jcode coordinator\nUse skills when relevant.");
        assert_eq!(
            meta.get("rules").and_then(Value::as_str),
            Some("Jcode coordinator\nUse skills when relevant.")
        );
        assert_eq!(meta.get("yoloMode").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn mcp_json_stdio_servers_forward_to_acp() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{
              "servers": {
                "session-graph": {
                  "command": "python3",
                  "args": ["/tmp/mcp_server.py"]
                },
                "off": { "command": "x", "disabled": true }
              }
            }"#,
        )
        .unwrap();
        let servers = mcp_servers_from_file(&path);
        assert_eq!(servers.len(), 1);
        let acp::McpServer::Stdio(stdio) = &servers[0] else {
            panic!("expected stdio");
        };
        assert_eq!(stdio.name, "session-graph");
        assert_eq!(stdio.command, PathBuf::from("python3"));
        assert_eq!(stdio.args, ["/tmp/mcp_server.py"]);
    }
}
