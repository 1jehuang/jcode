//! Grok Build subscription provider over the installed Grok CLI's ACP server.
//!
//! This runtime deliberately has no xAI HTTP or API-key path. Authentication is
//! delegated to `grok agent stdio`, which consumes the Grok CLI's own cached
//! login after ACP `initialize` advertises the supported subscription method.

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
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
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
            args: vec![
                "agent".to_string(),
                "stdio".to_string(),
                "--no-auto-update".to_string(),
            ],
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
        _tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = build_prompt(messages, system, resume_session_id.is_some())?;
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
                    prompt,
                    tx.clone(),
                    cancel_rx,
                ) {
                    let _ = tx.blocking_send(Ok(StreamEvent::Error {
                        message: format!("{error:#}"),
                        retry_after_secs: None,
                    }));
                }
            })
            .context("Failed to start Grok Build ACP runtime thread")?;

        Ok(Box::pin(GrokEventStream {
            inner: ReceiverStream::new(rx),
            cancel: Some(cancel_tx),
            thread: Some(thread),
        }))
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
                api_method: "grok-build-acp".to_string(),
                available: true,
                detail: "Grok Build subscription via Grok CLI ACP".to_string(),
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
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
        Some("Grok CLI cached login")
    }

    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn transport(&self) -> Option<String> {
        Some("ACP stdio".to_string())
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
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.available_models_display();
        fork.model_selected.store(
            self.model_selected.load(Ordering::Acquire),
            Ordering::Release,
        );
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
            let cwd = std::env::current_dir().context("Failed to determine working directory")?;
            let (session_id, session_model) = if let Some(session_id) = resume_session_id {
                let response = timeout_request(
                    "session/resume",
                    connection.resume_session(acp::ResumeSessionRequest::new(
                        session_id.clone(),
                        cwd,
                    )),
                )
                .await?;
                (acp::SessionId::new(session_id), response.models)
            } else {
                let response = timeout_request(
                    "session/new",
                    connection.new_session(acp::NewSessionRequest::new(cwd).mcp_servers(Vec::new())),
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
            "Failed to launch '{}'. Install the Grok CLI and run `grok login` first",
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
    let stderr_task = tokio::task::spawn_local(capture_stderr(stderr, Arc::clone(&stderr_capture)));

    let client = GrokAcpClient { tx: event_tx };
    let (connection, io) =
        acp::ClientSideConnection::new(client, stdin.compat_write(), stdout.compat(), |future| {
            tokio::task::spawn_local(future);
        });
    let io_task = tokio::task::spawn_local(io);
    let result = operation(connection).await;
    drop(child);
    io_task.abort();
    stderr_task.abort();
    result.map_err(|error| {
        let stderr = stderr_capture
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if stderr.trim().is_empty() {
            error
        } else {
            error.context(format!("Grok CLI stderr: {}", stderr.trim()))
        }
    })
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
}

#[async_trait(?Send)]
impl acp::Client for GrokAcpClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        let selected = request.options.iter().find(|option| {
            matches!(
                option.kind,
                acp::PermissionOptionKind::AllowOnce | acp::PermissionOptionKind::AllowAlways
            )
        });
        let outcome = match selected {
            Some(option) => acp::RequestPermissionOutcome::Selected(
                acp::SelectedPermissionOutcome::new(option.option_id.clone()),
            ),
            None => acp::RequestPermissionOutcome::Cancelled,
        };
        Ok(acp::RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        let event = match notification.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                text_from_acp_content(chunk.content).map(StreamEvent::TextDelta)
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                text_from_acp_content(chunk.content).map(StreamEvent::ThinkingDelta)
            }
            acp::SessionUpdate::ToolCall(call) => {
                Some(StreamEvent::StatusDetail { detail: call.title })
            }
            acp::SessionUpdate::ToolCallUpdate(update) => update
                .fields
                .title
                .map(|detail| StreamEvent::StatusDetail { detail }),
            _ => None,
        };
        if let Some(event) = event {
            let _ = self.tx.send(Ok(event)).await;
        }
        Ok(())
    }
}

fn text_from_acp_content(content: acp::ContentBlock) -> Option<String> {
    match content {
        acp::ContentBlock::Text(text) => Some(text.text),
        _ => None,
    }
}

fn build_prompt(messages: &[Message], system: &str, resumed: bool) -> Result<String> {
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .map(message_text)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("No user prompt found for Grok Build request"))?;

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

fn cached_login_hint(prefix: &str) -> String {
    format!(
        "{prefix}. Grok Build uses the Grok CLI subscription login, not XAI_API_KEY. Run `grok login` and retry"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn resumed_prompt_sends_only_outer_system_and_latest_user() {
        let messages = vec![
            Message::user("old"),
            Message::assistant_text("old answer"),
            Message::user("new"),
        ];
        let prompt = build_prompt(&messages, "outer", true).unwrap();
        assert!(prompt.contains("outer"));
        assert!(prompt.ends_with("new"));
        assert!(!prompt.contains("old answer"));
    }
}
