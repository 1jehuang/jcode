use super::dispatch;
use super::provider_init::ProviderChoice;
use crate::protocol::{Request, ServerEvent};
use crate::transport::{ReadHalf, WriteHalf};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

const ACP_PROTOCOL_VERSION: u64 = 1;

const JSONRPC_PARSE_ERROR: i64 = -32700;
const JSONRPC_INVALID_REQUEST: i64 = -32600;
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
const JSONRPC_INVALID_PARAMS: i64 = -32602;
const JSONRPC_INTERNAL_ERROR: i64 = -32603;
const JSONRPC_SERVER_ERROR: i64 = -32000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcpProfile {
    Standard,
    Extended,
    Full,
}

impl AcpProfile {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "extended" => Self::Extended,
            "full" => Self::Full,
            _ => Self::Standard,
        }
    }

    fn is_extended(self) -> bool {
        matches!(self, Self::Extended | Self::Full)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Extended => "extended",
            Self::Full => "full",
        }
    }
}

#[derive(Debug)]
struct JsonRpcMessage {
    id: Option<Value>,
    method: Option<String>,
    params: Value,
}

impl JsonRpcMessage {
    fn parse(line: &str) -> std::result::Result<Self, (i64, String)> {
        let value: Value =
            serde_json::from_str(line).map_err(|err| (JSONRPC_PARSE_ERROR, err.to_string()))?;
        let object = value.as_object().ok_or_else(|| {
            (
                JSONRPC_INVALID_REQUEST,
                "JSON-RPC message must be an object".to_string(),
            )
        })?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err((
                JSONRPC_INVALID_REQUEST,
                "JSON-RPC message must include jsonrpc=\"2.0\"".to_string(),
            ));
        }
        Ok(Self {
            id: object.get("id").cloned(),
            method: object
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })
    }
}

struct DaemonSession {
    session_id: String,
    reader: Mutex<BufReader<ReadHalf>>,
    writer: Mutex<WriteHalf>,
    next_request_id: AtomicU64,
    active_prompt_id: Mutex<Option<u64>>,
    prompt_running: AtomicBool,
    ui_state: Mutex<SessionUiState>,
}

/// Session-scoped provider/model state used to surface ACP `configOptions`
/// (model selector, reasoning effort) and `usage_update` notifications.
#[derive(Clone, Debug, Default)]
struct SessionUiState {
    provider_name: Option<String>,
    model: Option<String>,
    available_models: Vec<String>,
    reasoning_effort: Option<String>,
}

impl SessionUiState {
    fn from_history_fields(
        provider_name: Option<String>,
        provider_model: Option<String>,
        available_models: Vec<String>,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            provider_name,
            model: provider_model,
            available_models,
            reasoning_effort,
        }
    }

    fn context_limit(&self) -> u64 {
        self.model
            .as_deref()
            .and_then(|model| {
                crate::provider::context_limit_for_model_with_provider(
                    model,
                    self.provider_name.as_deref(),
                )
            })
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT) as u64
    }
}

impl DaemonSession {
    fn new(session_id: String, reader: ReadHalf, writer: WriteHalf, next_request_id: u64) -> Self {
        Self {
            session_id,
            reader: Mutex::new(BufReader::new(reader)),
            writer: Mutex::new(writer),
            next_request_id: AtomicU64::new(next_request_id),
            active_prompt_id: Mutex::new(None),
            prompt_running: AtomicBool::new(false),
            ui_state: Mutex::new(SessionUiState::default()),
        }
    }

    fn with_ui_state(self, state: SessionUiState) -> Self {
        Self {
            ui_state: Mutex::new(state),
            ..self
        }
    }

    fn next_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn send(&self, request: &Request) -> Result<()> {
        let mut json = serde_json::to_string(request)?;
        json.push('\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(json.as_bytes()).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn read_event(&self) -> Result<ServerEvent> {
        let mut line = String::new();
        let mut reader = self.reader.lock().await;
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("Jcode daemon disconnected");
        }
        let event = serde_json::from_str(&line)
            .with_context(|| format!("failed to decode Jcode daemon event: {}", line.trim_end()))?;
        Ok(event)
    }
}

#[derive(Clone)]
struct AcpRuntime {
    stdout: Arc<Mutex<tokio::io::Stdout>>,
    sessions: Arc<Mutex<HashMap<String, Arc<DaemonSession>>>>,
    profile: AcpProfile,
    provider_choice: ProviderChoice,
    model: Option<String>,
    provider_profile: Option<String>,
}

impl AcpRuntime {
    fn new(
        profile: AcpProfile,
        provider_choice: ProviderChoice,
        model: Option<String>,
        provider_profile: Option<String>,
    ) -> Self {
        Self {
            stdout: Arc::new(Mutex::new(tokio::io::stdout())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            profile,
            provider_choice,
            model,
            provider_profile,
        }
    }

    async fn run(self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(());
            }
            if line.trim().is_empty() {
                continue;
            }

            let message = match JsonRpcMessage::parse(&line) {
                Ok(message) => message,
                Err((code, message)) => {
                    self.write_error_value(
                        Value::Null,
                        code,
                        format!("Invalid JSON-RPC request: {message}"),
                    )
                    .await?;
                    continue;
                }
            };

            self.handle_message(message).await?;
        }
    }

    async fn handle_message(&self, message: JsonRpcMessage) -> Result<()> {
        let Some(method) = message.method.as_deref() else {
            if let Some(id) = message.id {
                self.write_error_value(
                    id,
                    JSONRPC_INVALID_REQUEST,
                    "JSON-RPC request missing method".to_string(),
                )
                .await?;
            }
            return Ok(());
        };

        match method {
            "initialize" => {
                if let Some(id) = message.id {
                    self.write_result(id, initialize_result(&message.params, self.profile))
                        .await?;
                }
            }
            "session/new" => self.handle_session_new(message).await?,
            "session/load" => self.handle_session_load(message, true).await?,
            "session/resume" => self.handle_session_load(message, false).await?,
            "session/prompt" => self.handle_session_prompt(message).await?,
            "session/cancel" => self.handle_session_cancel(message).await?,
            "session/close" => self.handle_session_close(message).await?,
            "session/set_config_option" => self.handle_set_config_option(message).await?,
            "session/set_model" => {
                self.handle_compat_config_option(
                    message,
                    CONFIG_ID_MODEL,
                    &["modelId", "model"],
                    "session/set_model",
                )
                .await?
            }
            "session/set_reasoning_effort" => {
                self.handle_compat_config_option(
                    message,
                    CONFIG_ID_EFFORT,
                    &["effort", "reasoningEffort"],
                    "session/set_reasoning_effort",
                )
                .await?
            }
            _ if method.starts_with('_') => {
                if let Some(id) = message.id {
                    self.write_error_value(
                        id,
                        JSONRPC_METHOD_NOT_FOUND,
                        format!("Unsupported Jcode ACP extension method: {method}"),
                    )
                    .await?;
                }
            }
            _ => {
                if let Some(id) = message.id {
                    self.write_error_value(
                        id,
                        JSONRPC_METHOD_NOT_FOUND,
                        format!("Unsupported ACP method: {method}"),
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_session_new(&self, message: JsonRpcMessage) -> Result<()> {
        let Some(id) = message.id else {
            return Ok(());
        };
        let cwd = match cwd_from_params(&message.params) {
            Ok(cwd) => cwd,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        if let Err(err) = ensure_no_acp_mcp_servers(&message.params) {
            self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                .await?;
            return Ok(());
        }

        match self.create_new_session(cwd).await {
            Ok(session) => {
                let session_id = session.session_id.clone();
                let state = session.ui_state.lock().await.clone();
                self.sessions
                    .lock()
                    .await
                    .insert(session_id.clone(), Arc::new(session));
                let mut result = json!({ "sessionId": session_id });
                insert_session_configuration(&mut result, &state);
                self.write_result(id, result).await?;
                self.write_available_commands(&session_id).await?;
            }
            Err(err) => {
                self.write_error_value(
                    id,
                    JSONRPC_INTERNAL_ERROR,
                    format!("Failed to create Jcode session: {err:#}"),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn handle_session_load(
        &self,
        message: JsonRpcMessage,
        replay_history: bool,
    ) -> Result<()> {
        let Some(id) = message.id else {
            return Ok(());
        };
        let session_id = match required_session_id(&message.params) {
            Ok(session_id) => session_id,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        let cwd = match cwd_from_params(&message.params) {
            Ok(cwd) => cwd,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        if let Err(err) = ensure_no_acp_mcp_servers(&message.params) {
            self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                .await?;
            return Ok(());
        }

        match self
            .attach_existing_session(session_id.clone(), cwd, replay_history)
            .await
        {
            Ok(session) => {
                let state = session.ui_state.lock().await.clone();
                self.sessions
                    .lock()
                    .await
                    .insert(session.session_id.clone(), Arc::new(session));
                let mut result = json!({});
                insert_session_configuration(&mut result, &state);
                self.write_result(id, result).await?;
                self.write_available_commands(&session_id).await?;
            }
            Err(err) => {
                self.write_error_value(
                    id,
                    JSONRPC_INTERNAL_ERROR,
                    format!("Failed to attach Jcode session '{session_id}': {err:#}"),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn handle_session_prompt(&self, message: JsonRpcMessage) -> Result<()> {
        let Some(id) = message.id else {
            return Ok(());
        };
        let session_id = match required_session_id(&message.params) {
            Ok(session_id) => session_id,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        let (text, images) = match prompt_from_params(&message.params) {
            Ok(prompt) => prompt,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).cloned()
        };
        let Some(session) = session else {
            self.write_error_value(
                id,
                JSONRPC_INVALID_PARAMS,
                format!("Unknown ACP session id: {session_id}"),
            )
            .await?;
            return Ok(());
        };

        if session
            .prompt_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.write_error_value(
                id,
                JSONRPC_SERVER_ERROR,
                format!("Session {session_id} is already processing a prompt"),
            )
            .await?;
            return Ok(());
        }

        let runtime = self.clone();
        tokio::spawn(async move {
            let result = runtime.run_prompt(id.clone(), session, text, images).await;
            if let Err(err) = result {
                let _ = runtime
                    .write_error_value(
                        id,
                        JSONRPC_INTERNAL_ERROR,
                        format!("Prompt failed: {err:#}"),
                    )
                    .await;
            }
        });
        Ok(())
    }

    async fn handle_session_cancel(&self, message: JsonRpcMessage) -> Result<()> {
        let session_id = match required_session_id(&message.params) {
            Ok(session_id) => session_id,
            Err(err) => {
                if let Some(id) = message.id {
                    self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                        .await?;
                }
                return Ok(());
            }
        };
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).cloned()
        };
        if let Some(session) = session {
            let cancel_id = session.next_id();
            let _ = session.send(&Request::Cancel { id: cancel_id }).await;
        }
        if let Some(id) = message.id {
            self.write_result(id, json!({})).await?;
        }
        Ok(())
    }

    async fn handle_session_close(&self, message: JsonRpcMessage) -> Result<()> {
        let Some(id) = message.id else {
            return Ok(());
        };
        let session_id = match required_session_id(&message.params) {
            Ok(session_id) => session_id,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        if let Some(session) = self.sessions.lock().await.remove(&session_id) {
            let cancel_id = session.next_id();
            let _ = session.send(&Request::Cancel { id: cancel_id }).await;
        }
        self.write_result(id, json!({})).await?;
        Ok(())
    }

    async fn handle_set_config_option(&self, message: JsonRpcMessage) -> Result<()> {
        let Some(id) = message.id else {
            return Ok(());
        };
        let session_id = match required_session_id(&message.params) {
            Ok(session_id) => session_id,
            Err(err) => {
                self.write_error_value(id, JSONRPC_INVALID_PARAMS, err)
                    .await?;
                return Ok(());
            }
        };
        let config_id = message
            .params
            .get("configId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let value = message
            .params
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string);
        let (Some(config_id), Some(value)) = (config_id, value) else {
            self.write_error_value(
                id,
                JSONRPC_INVALID_PARAMS,
                "session/set_config_option requires string configId and value".to_string(),
            )
            .await?;
            return Ok(());
        };

        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_id).cloned()
        };
        let Some(session) = session else {
            self.write_error_value(
                id,
                JSONRPC_INVALID_PARAMS,
                format!("Unknown ACP session id: {session_id}"),
            )
            .await?;
            return Ok(());
        };
        if session.prompt_running.load(Ordering::SeqCst) {
            self.write_error_value(
                id,
                JSONRPC_SERVER_ERROR,
                format!("Session {session_id} is processing a prompt; retry when it finishes"),
            )
            .await?;
            return Ok(());
        }

        let request_id = session.next_id();
        let apply_result = match config_id.as_str() {
            CONFIG_ID_MODEL => {
                session
                    .send(&Request::SetModel {
                        id: request_id,
                        model: value.clone(),
                    })
                    .await?;
                wait_for_model_changed(&session, request_id).await
            }
            CONFIG_ID_EFFORT => {
                session
                    .send(&Request::SetReasoningEffort {
                        id: request_id,
                        effort: value.clone(),
                        target_session_id: None,
                    })
                    .await?;
                wait_for_effort_changed(&session, request_id).await
            }
            other => Err(anyhow::anyhow!("Unknown config option id: {other}")),
        };

        match apply_result {
            Ok(()) => {
                let config_options = session_config_options(&*session.ui_state.lock().await);
                // The spec requires the full option set in the response itself.
                self.write_result(id, json!({ "configOptions": config_options }))
                    .await?;
                self.write_notification(
                    "session/update",
                    json!({
                        "sessionId": session.session_id,
                        "update": {
                            "sessionUpdate": "config_option_update",
                            "configOptions": config_options,
                        }
                    }),
                )
                .await?;
            }
            Err(err) => {
                self.write_error_value(
                    id,
                    JSONRPC_SERVER_ERROR,
                    format!("Failed to set {config_id}: {err:#}"),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Compatibility entry points used by ACP hosts that implemented the
    /// pre-configOptions model and reasoning controls. Normalize them through
    /// the standard config option path so both interfaces stay in sync.
    async fn handle_compat_config_option(
        &self,
        mut message: JsonRpcMessage,
        config_id: &str,
        value_fields: &[&str],
        method: &str,
    ) -> Result<()> {
        let value = match compatibility_option_value(&message.params, value_fields, method) {
            Ok(value) => value,
            Err(error) => {
                if let Some(id) = message.id {
                    self.write_error_value(id, JSONRPC_INVALID_PARAMS, error)
                        .await?;
                }
                return Ok(());
            }
        };
        let Some(params) = message.params.as_object_mut() else {
            if let Some(id) = message.id {
                self.write_error_value(
                    id,
                    JSONRPC_INVALID_PARAMS,
                    format!("{method} params must be an object"),
                )
                .await?;
            }
            return Ok(());
        };
        params.insert("configId".to_string(), Value::String(config_id.to_string()));
        params.insert("value".to_string(), Value::String(value));
        self.handle_set_config_option(message).await
    }

    async fn write_available_commands(&self, session_id: &str) -> Result<()> {
        self.write_notification(
            "session/update",
            json!({
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": acp_available_commands(),
                }
            }),
        )
        .await
    }

    async fn ensure_daemon(&self) -> Result<()> {
        if dispatch::server_is_running().await {
            return Ok(());
        }
        dispatch::spawn_server(
            &self.provider_choice,
            self.model.as_deref(),
            self.provider_profile.as_deref(),
        )
        .await
    }

    async fn connect_daemon(&self) -> Result<(ReadHalf, WriteHalf)> {
        self.ensure_daemon().await?;
        let stream = crate::server::connect_socket(&crate::server::socket_path()).await?;
        Ok(stream.into_split())
    }

    async fn create_new_session(&self, cwd: PathBuf) -> Result<DaemonSession> {
        let (reader, writer) = self.connect_daemon().await?;
        let session = DaemonSession::new(String::new(), reader, writer, 2);
        let subscribe_id = 1;
        session
            .send(&Request::Subscribe {
                id: subscribe_id,
                working_dir: Some(cwd.display().to_string()),
                selfdev: None,
                target_session_id: None,
                client_instance_id: Some("acp".to_string()),
                client_has_local_history: false,
                allow_session_takeover: false,
                terminal_env: crate::terminal_launch::snapshot_client_terminal_env(),
            })
            .await?;
        wait_for_done(&session, subscribe_id).await?;
        let history = request_history(&session).await?;
        let (session_id, ui_state) = match history {
            ServerEvent::History {
                session_id,
                provider_name,
                provider_model,
                available_models,
                reasoning_effort,
                ..
            } => (
                session_id,
                SessionUiState::from_history_fields(
                    provider_name,
                    provider_model,
                    available_models,
                    reasoning_effort,
                ),
            ),
            other => anyhow::bail!("expected history after session creation, got {other:?}"),
        };
        Ok(DaemonSession::new(
            session_id,
            session.reader.into_inner().into_inner(),
            session.writer.into_inner(),
            session.next_request_id.load(Ordering::Relaxed),
        )
        .with_ui_state(ui_state))
    }

    async fn attach_existing_session(
        &self,
        target_session_id: String,
        cwd: PathBuf,
        replay_history: bool,
    ) -> Result<DaemonSession> {
        let (reader, writer) = self.connect_daemon().await?;
        let session = DaemonSession::new(String::new(), reader, writer, 2);
        let resume_id = 1;
        session
            .send(&Request::Subscribe {
                id: resume_id,
                working_dir: Some(cwd.display().to_string()),
                selfdev: None,
                target_session_id: Some(target_session_id.clone()),
                client_instance_id: Some("acp".to_string()),
                client_has_local_history: false,
                allow_session_takeover: false,
                terminal_env: crate::terminal_launch::snapshot_client_terminal_env(),
            })
            .await?;

        let mut attached_id = target_session_id;
        let mut ui_state = SessionUiState::default();
        loop {
            let event = session.read_event().await?;
            match event {
                ServerEvent::Ack { .. } => {}
                ServerEvent::History {
                    session_id,
                    messages,
                    provider_name,
                    provider_model,
                    available_models,
                    reasoning_effort,
                    ..
                } => {
                    attached_id = session_id.clone();
                    ui_state = SessionUiState::from_history_fields(
                        provider_name,
                        provider_model,
                        available_models,
                        reasoning_effort,
                    );
                    if replay_history {
                        self.replay_history(&session_id, messages).await?;
                    }
                }
                ServerEvent::Done { id } if id == resume_id => break,
                ServerEvent::Error { id, message, .. } if id == resume_id => {
                    anyhow::bail!(message);
                }
                other => {
                    if self.profile.is_extended() {
                        self.write_jcode_extension_event(&attached_id, &other)
                            .await?;
                    }
                }
            }
        }

        Ok(DaemonSession::new(
            attached_id,
            session.reader.into_inner().into_inner(),
            session.writer.into_inner(),
            session.next_request_id.load(Ordering::Relaxed),
        )
        .with_ui_state(ui_state))
    }

    async fn replay_history(
        &self,
        session_id: &str,
        messages: Vec<crate::protocol::HistoryMessage>,
    ) -> Result<()> {
        for message in messages {
            let update_name = match message.role.as_str() {
                "user" => "user_message_chunk",
                "assistant" => "agent_message_chunk",
                _ => "agent_message_chunk",
            };
            self.write_notification(
                "session/update",
                json!({
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": update_name,
                        "content": {
                            "type": "text",
                            "text": message.content,
                        }
                    }
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn run_prompt(
        &self,
        rpc_id: Value,
        session: Arc<DaemonSession>,
        text: String,
        images: Vec<(String, String)>,
    ) -> Result<()> {
        if let Some(command) = parse_acp_slash_command(&text) {
            let response = match command {
                Ok(command) => self.run_session_command(&session, command).await,
                Err(err) => Err(err),
            };
            cleanup_prompt_state(&session).await;
            let response = response?;
            self.write_notification(
                "session/update",
                json!({
                    "sessionId": session.session_id,
                    "update": agent_message_chunk(response),
                }),
            )
            .await?;
            self.write_result(rpc_id, json!({ "stopReason": "end_turn" }))
                .await?;
            return Ok(());
        }

        let prompt_id = session.next_id();
        {
            let mut active = session.active_prompt_id.lock().await;
            *active = Some(prompt_id);
        }

        let send_result = session
            .send(&Request::Message {
                id: prompt_id,
                content: text,
                images,
                system_reminder: None,
                no_reply: false,
            })
            .await;
        if let Err(err) = send_result {
            cleanup_prompt_state(&session).await;
            return Err(err);
        }

        let mut mapper = EventMapper::new(session.session_id.clone(), self.profile);
        let mut stop_reason = "end_turn".to_string();
        loop {
            let event = match session.read_event().await {
                Ok(event) => event,
                Err(err) => {
                    cleanup_prompt_state(&session).await;
                    return Err(err);
                }
            };
            if self.profile.is_extended() {
                self.write_jcode_extension_event(&session.session_id, &event)
                    .await?;
            }
            match event {
                ServerEvent::Ack { .. } => {}
                ServerEvent::Done { id } if id == prompt_id => break,
                ServerEvent::Interrupted => {
                    stop_reason = "cancelled".to_string();
                }
                ServerEvent::Error { id, message, .. } if id == prompt_id => {
                    cleanup_prompt_state(&session).await;
                    self.write_error_value(rpc_id, JSONRPC_SERVER_ERROR, message)
                        .await?;
                    return Ok(());
                }
                ServerEvent::TokenUsage {
                    input,
                    output: _,
                    cache_read_input,
                    cache_creation_input,
                } => {
                    let (provider_name, context_limit) = {
                        let state = session.ui_state.lock().await;
                        (
                            state.provider_name.clone().unwrap_or_default(),
                            state.context_limit(),
                        )
                    };
                    let used = crate::compaction::effective_context_tokens_from_usage(
                        &provider_name,
                        input,
                        cache_read_input,
                        cache_creation_input,
                    );
                    self.write_notification(
                        "session/update",
                        json!({
                            "sessionId": session.session_id,
                            "update": {
                                "sessionUpdate": "usage_update",
                                "used": used,
                                "size": context_limit,
                            }
                        }),
                    )
                    .await?;
                }
                ServerEvent::ModelChanged {
                    model,
                    provider_name,
                    error,
                    ..
                } => {
                    // Mid-prompt model changes happen on provider failover;
                    // keep the selector in sync.
                    if error.is_none() {
                        let config_options = {
                            let mut state = session.ui_state.lock().await;
                            state.model = Some(model);
                            if provider_name.is_some() {
                                state.provider_name = provider_name;
                            }
                            session_config_options(&state)
                        };
                        if !config_options.is_empty() {
                            self.write_notification(
                                "session/update",
                                json!({
                                    "sessionId": session.session_id,
                                    "update": {
                                        "sessionUpdate": "config_option_update",
                                        "configOptions": config_options,
                                    }
                                }),
                            )
                            .await?;
                        }
                    }
                }
                other => {
                    for update in mapper.map_event(other) {
                        self.write_notification(
                            "session/update",
                            json!({
                                "sessionId": session.session_id,
                                "update": update,
                            }),
                        )
                        .await?;
                    }
                }
            }
        }

        cleanup_prompt_state(&session).await;
        self.write_result(rpc_id, json!({ "stopReason": stop_reason }))
            .await?;
        Ok(())
    }

    async fn run_session_command(
        &self,
        session: &DaemonSession,
        command: AcpSlashCommand,
    ) -> Result<String> {
        match command {
            AcpSlashCommand::Model(None) => {
                let state = session.ui_state.lock().await;
                Ok(match state.model.as_deref() {
                    Some(model) => format!("Current model: `{model}`"),
                    None => "The daemon did not report a current model.".to_string(),
                })
            }
            AcpSlashCommand::Model(Some(model)) => {
                let id = session.next_id();
                session
                    .send(&Request::SetModel {
                        id,
                        model: model.clone(),
                    })
                    .await?;
                wait_for_model_changed(session, id).await?;
                self.write_config_option_update(session).await?;
                let selected = session.ui_state.lock().await.model.clone().unwrap_or(model);
                Ok(format!("Switched model to `{selected}`."))
            }
            AcpSlashCommand::Models => {
                let event = request_model_catalog(session).await?;
                let ServerEvent::History {
                    provider_name,
                    provider_model,
                    available_models,
                    ..
                } = event
                else {
                    unreachable!("request_model_catalog only returns history")
                };
                let (current, models) = {
                    let mut state = session.ui_state.lock().await;
                    if provider_name.is_some() {
                        state.provider_name = provider_name;
                    }
                    if provider_model.is_some() {
                        state.model = provider_model;
                    }
                    state.available_models = available_models;
                    (state.model.clone(), state.available_models.clone())
                };
                self.write_config_option_update(session).await?;
                Ok(format_model_catalog(current.as_deref(), &models))
            }
            AcpSlashCommand::Effort(None) => {
                let state = session.ui_state.lock().await;
                let current = state
                    .reasoning_effort
                    .as_deref()
                    .unwrap_or("provider default");
                let available = available_efforts(&state);
                if available.is_empty() {
                    Ok(format!("Current reasoning effort: `{current}`."))
                } else {
                    Ok(format!(
                        "Current reasoning effort: `{current}`. Available: {}.",
                        available
                            .iter()
                            .map(|effort| format!("`{effort}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            AcpSlashCommand::Effort(Some(effort)) => {
                let id = session.next_id();
                session
                    .send(&Request::SetReasoningEffort {
                        id,
                        effort: effort.clone(),
                        target_session_id: None,
                    })
                    .await?;
                wait_for_effort_changed(session, id).await?;
                self.write_config_option_update(session).await?;
                let selected = session
                    .ui_state
                    .lock()
                    .await
                    .reasoning_effort
                    .clone()
                    .unwrap_or(effort);
                Ok(format!("Set reasoning effort to `{selected}`."))
            }
        }
    }

    async fn write_config_option_update(&self, session: &DaemonSession) -> Result<()> {
        let config_options = session_config_options(&*session.ui_state.lock().await);
        self.write_notification(
            "session/update",
            json!({
                "sessionId": session.session_id,
                "update": {
                    "sessionUpdate": "config_option_update",
                    "configOptions": config_options,
                }
            }),
        )
        .await
    }

    async fn write_result(&self, id: Value, result: Value) -> Result<()> {
        self.write_value(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
        .await
    }

    async fn write_error_value(&self, id: Value, code: i64, message: String) -> Result<()> {
        self.write_value(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message,
            }
        }))
        .await
    }

    async fn write_notification(&self, method: &str, params: Value) -> Result<()> {
        self.write_value(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn write_jcode_extension_event(
        &self,
        session_id: &str,
        event: &ServerEvent,
    ) -> Result<()> {
        self.write_notification(
            "_jcode/server_event",
            json!({
                "sessionId": session_id,
                "event": serde_json::to_value(event).unwrap_or(Value::Null),
            }),
        )
        .await
    }

    async fn write_value(&self, value: Value) -> Result<()> {
        let mut stdout = self.stdout.lock().await;
        let mut line = serde_json::to_string(&value)?;
        line.push('\n');
        stdout.write_all(line.as_bytes()).await?;
        stdout.flush().await?;
        Ok(())
    }
}

async fn cleanup_prompt_state(session: &DaemonSession) {
    {
        let mut active = session.active_prompt_id.lock().await;
        *active = None;
    }
    session.prompt_running.store(false, Ordering::SeqCst);
}

async fn wait_for_done(session: &DaemonSession, request_id: u64) -> Result<()> {
    loop {
        match session.read_event().await? {
            ServerEvent::Ack { .. } => {}
            ServerEvent::Done { id } if id == request_id => return Ok(()),
            ServerEvent::Error { id, message, .. } if id == request_id => anyhow::bail!(message),
            _ => {}
        }
    }
}

async fn request_history(session: &DaemonSession) -> Result<ServerEvent> {
    let id = session.next_id();
    session.send(&Request::GetHistory { id }).await?;
    loop {
        match session.read_event().await? {
            ServerEvent::Ack { .. } => {}
            event @ ServerEvent::History { id: event_id, .. } if event_id == id => {
                return Ok(event);
            }
            ServerEvent::Error {
                id: event_id,
                message,
                ..
            } if event_id == id => anyhow::bail!(message),
            _ => {}
        }
    }
}

async fn request_model_catalog(session: &DaemonSession) -> Result<ServerEvent> {
    let id = session.next_id();
    session.send(&Request::GetModelCatalog { id }).await?;
    loop {
        match session.read_event().await? {
            ServerEvent::Ack { .. } => {}
            event @ ServerEvent::History { id: event_id, .. } if event_id == id => {
                return Ok(event);
            }
            ServerEvent::Error {
                id: event_id,
                message,
                ..
            } if event_id == id => anyhow::bail!(message),
            _ => {}
        }
    }
}

const CONFIG_ID_MODEL: &str = "model";
const CONFIG_ID_EFFORT: &str = "reasoning_effort";

fn acp_available_commands() -> Vec<Value> {
    vec![
        json!({
            "name": "model",
            "description": "Switch the model for this session, or show the current model",
            "input": { "hint": "model id (optional)" },
        }),
        json!({
            "name": "models",
            "description": "List models available from the active provider",
        }),
        json!({
            "name": "effort",
            "description": "Set reasoning effort, or show the current effort",
            "input": { "hint": "none|minimal|low|medium|high|xhigh|max (optional)" },
        }),
    ]
}

fn insert_session_configuration(result: &mut Value, state: &SessionUiState) {
    let Some(object) = result.as_object_mut() else {
        return;
    };
    let config_options = session_config_options(state);
    if !config_options.is_empty() {
        object.insert("configOptions".to_string(), Value::Array(config_options));
    }
    if let Some(models) = session_models(state) {
        object.insert("models".to_string(), models);
    }
}

fn session_models(state: &SessionUiState) -> Option<Value> {
    let current = state.model.as_deref()?;
    let mut models = state.available_models.clone();
    if !models.iter().any(|candidate| candidate == current) {
        models.insert(0, current.to_string());
    }
    Some(json!({
        "availableModels": models
            .into_iter()
            .map(|model| json!({ "modelId": model, "name": model }))
            .collect::<Vec<_>>(),
        "currentModelId": current,
    }))
}

fn available_efforts(state: &SessionUiState) -> Vec<&'static str> {
    crate::provider::inferred_reasoning_efforts(
        state.provider_name.as_deref(),
        state.model.as_deref(),
    )
    .into_iter()
    // `swarm`/`swarm-deep` are TUI sentinels, not provider effort levels.
    .filter(|effort| !effort.starts_with("swarm"))
    .collect()
}

/// Build the ACP `configOptions` array (model selector plus reasoning effort)
/// from the current session provider state. Empty when the daemon reported no
/// usable model state.
fn session_config_options(state: &SessionUiState) -> Vec<Value> {
    let mut options = Vec::new();

    if let Some(model) = state.model.as_deref() {
        let mut models = state.available_models.clone();
        if !models.iter().any(|candidate| candidate == model) {
            models.insert(0, model.to_string());
        }
        let select_options: Vec<Value> = models
            .iter()
            .map(|name| json!({ "value": name, "name": name }))
            .collect();
        options.push(json!({
            "type": "select",
            "id": CONFIG_ID_MODEL,
            "name": "Model",
            "category": "model",
            "currentValue": model,
            "options": select_options,
        }));
    }

    let efforts = available_efforts(state);
    if !efforts.is_empty() {
        let current = state
            .reasoning_effort
            .as_deref()
            .filter(|effort| efforts.contains(effort))
            .unwrap_or_else(|| {
                if efforts.contains(&"medium") {
                    "medium"
                } else {
                    efforts[0]
                }
            });
        let select_options: Vec<Value> = efforts
            .iter()
            .map(|name| json!({ "value": name, "name": name }))
            .collect();
        options.push(json!({
            "type": "select",
            "id": CONFIG_ID_EFFORT,
            "name": "Reasoning effort",
            "category": "thought_level",
            "currentValue": current,
            "options": select_options,
        }));
    }

    options
}

async fn wait_for_model_changed(session: &DaemonSession, request_id: u64) -> Result<()> {
    loop {
        match session.read_event().await? {
            ServerEvent::Ack { .. } => {}
            ServerEvent::ModelChanged {
                id,
                model,
                provider_name,
                error,
            } if id == request_id => {
                if let Some(error) = error {
                    anyhow::bail!(error);
                }
                let mut state = session.ui_state.lock().await;
                state.model = Some(model);
                if provider_name.is_some() {
                    state.provider_name = provider_name;
                }
                return Ok(());
            }
            ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
}

async fn wait_for_effort_changed(session: &DaemonSession, request_id: u64) -> Result<()> {
    loop {
        match session.read_event().await? {
            ServerEvent::Ack { .. } => {}
            ServerEvent::ReasoningEffortChanged { id, effort, error } if id == request_id => {
                if let Some(error) = error {
                    anyhow::bail!(error);
                }
                let mut state = session.ui_state.lock().await;
                state.reasoning_effort = effort;
                return Ok(());
            }
            ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
}

struct EventMapper {
    session_id: String,
    profile: AcpProfile,
    current_tool_id: Option<String>,
    tool_inputs: HashMap<String, String>,
}

impl EventMapper {
    fn new(session_id: String, profile: AcpProfile) -> Self {
        Self {
            session_id,
            profile,
            current_tool_id: None,
            tool_inputs: HashMap::new(),
        }
    }

    fn map_event(&mut self, event: ServerEvent) -> Vec<Value> {
        match event {
            ServerEvent::TextDelta { text } => vec![agent_message_chunk(text)],
            ServerEvent::TextReplace { text } => vec![agent_message_chunk(text)],
            ServerEvent::ToolStart { id, name } => {
                self.current_tool_id = Some(id.clone());
                self.tool_inputs.entry(id.clone()).or_default();
                vec![json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": id,
                    "title": tool_title(&name),
                    "kind": tool_kind(&name),
                    "status": "pending",
                })]
            }
            ServerEvent::ToolInput { delta } => {
                let Some(tool_id) = self.current_tool_id.clone() else {
                    return Vec::new();
                };
                let buffer = self.tool_inputs.entry(tool_id.clone()).or_default();
                buffer.push_str(&delta);
                let mut update = json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_id,
                });
                if let Some(raw_input) = parse_json_object(buffer)
                    && let Some(object) = update.as_object_mut()
                {
                    object.insert("rawInput".to_string(), raw_input);
                }
                vec![update]
            }
            ServerEvent::ToolExec { id, name } => {
                self.current_tool_id = Some(id.clone());
                let mut update = json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": id,
                    "title": tool_title(&name),
                    "kind": tool_kind(&name),
                    "status": "in_progress",
                });
                if let Some(input) = self
                    .tool_inputs
                    .get(update["toolCallId"].as_str().unwrap_or_default())
                    && let Some(raw_input) = parse_json_object(input)
                    && let Some(object) = update.as_object_mut()
                {
                    object.insert("rawInput".to_string(), raw_input);
                }
                vec![update]
            }
            ServerEvent::ToolDone {
                id,
                name,
                output,
                error,
            } => vec![json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "title": tool_title(&name),
                "kind": tool_kind(&name),
                "status": if error.is_some() { "failed" } else { "completed" },
                "content": [{
                    "type": "content",
                    "content": {
                        "type": "text",
                        "text": output,
                    }
                }],
                "rawOutput": {
                    "output": output,
                    "error": error,
                }
            })],
            ServerEvent::GeneratedImage {
                id,
                path,
                output_format,
                revised_prompt,
                ..
            } => vec![json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "status": "completed",
                "content": [{
                    "type": "content",
                    "content": {
                        "type": "text",
                        "text": format!("Generated image: {path} ({output_format}){}", revised_prompt.map(|prompt| format!("\nRevised prompt: {prompt}")).unwrap_or_default()),
                    }
                }]
            })],
            ServerEvent::Compaction { trigger, .. } if self.profile.is_extended() => vec![json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": format!("\n[Jcode compacted context: {trigger}]\n"),
                }
            })],
            ServerEvent::SessionRenamed { display_title, .. } => vec![json!({
                "sessionUpdate": "session_info_update",
                "title": display_title,
            })],
            ServerEvent::McpStatus { servers } if self.profile.is_extended() => vec![json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": format!("\n[Jcode MCP status: {}]\n", servers.join(", ")),
                }
            })],
            _ => {
                let _ = &self.session_id;
                Vec::new()
            }
        }
    }
}

fn parse_json_object(input: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(input).ok()?;
    value.as_object()?;
    Some(value)
}

fn compatibility_option_value(
    params: &Value,
    value_fields: &[&str],
    method: &str,
) -> std::result::Result<String, String> {
    if !params.is_object() {
        return Err(format!("{method} params must be an object"));
    }
    value_fields
        .iter()
        .find_map(|field| {
            params
                .get(*field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .ok_or_else(|| {
            format!(
                "{method} requires a non-empty string {}",
                value_fields.join(" or ")
            )
        })
}

fn initialize_result(params: &Value, profile: AcpProfile) -> Value {
    // We only speak exactly ACP_PROTOCOL_VERSION; the response pins to our
    // version regardless of the `protocolVersion` the client requested.
    let _ = params;
    let protocol_version = ACP_PROTOCOL_VERSION;
    let mut agent_capabilities = json!({
        "loadSession": true,
        "promptCapabilities": {
            "image": true,
            "audio": false,
            "embeddedContext": true,
        },
        "mcpCapabilities": {
            "http": false,
            "sse": false,
        },
        "sessionCapabilities": {
            "close": {},
            "resume": {},
        }
    });

    if profile.is_extended()
        && let Some(object) = agent_capabilities.as_object_mut()
    {
        object.insert(
            "_meta".to_string(),
            json!({
                "jcode": {
                    "profile": profile.as_str(),
                    "extensions": ["raw_server_event"]
                }
            }),
        );
    }

    json!({
        "protocolVersion": protocol_version,
        "agentCapabilities": agent_capabilities,
        "agentInfo": {
            "name": "jcode",
            "title": "Jcode",
            "version": jcode_build_meta::pkg_version(),
        },
        "authMethods": [],
    })
}

fn cwd_from_params(params: &Value) -> std::result::Result<PathBuf, String> {
    let cwd = match params.get("cwd").and_then(Value::as_str) {
        Some(cwd) if !cwd.trim().is_empty() => PathBuf::from(cwd),
        _ => std::env::current_dir().map_err(|err| err.to_string())?,
    };
    if !cwd.is_absolute() {
        return Err(format!("ACP cwd must be absolute: {}", cwd.display()));
    }
    Ok(cwd)
}

fn required_session_id(params: &Value) -> std::result::Result<String, String> {
    params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Missing required sessionId".to_string())
}

fn ensure_no_acp_mcp_servers(params: &Value) -> std::result::Result<(), String> {
    match params.get("mcpServers") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(items)) if items.is_empty() => Ok(()),
        Some(_) => Err(
            "ACP mcpServers are not supported yet; configure MCP servers in ~/.jcode/mcp.json or a project-local .jcode/mcp.json/.mcp.json"
                .to_string(),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AcpSlashCommand {
    Model(Option<String>),
    Models,
    Effort(Option<String>),
}

fn parse_acp_slash_command(text: &str) -> Option<Result<AcpSlashCommand>> {
    // A leading space is the ACP client convention for escaping slash command
    // interpretation and sending the text to the model literally.
    let trimmed = text.trim_end();
    let body = trimmed.strip_prefix('/')?;
    let mut parts = body.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default();
    let argument = parts
        .next()
        .map(str::trim)
        .filter(|argument| !argument.is_empty())
        .map(str::to_string);
    match name {
        "model" => Some(Ok(AcpSlashCommand::Model(argument))),
        "models" if argument.is_none() => Some(Ok(AcpSlashCommand::Models)),
        "models" => Some(Err(anyhow::anyhow!("/models does not accept an argument"))),
        "effort" => Some(Ok(AcpSlashCommand::Effort(argument))),
        _ => None,
    }
}

fn format_model_catalog(current: Option<&str>, models: &[String]) -> String {
    if models.is_empty() {
        return match current {
            Some(current) => format!("Current model: `{current}`. No model catalog was reported."),
            None => "The active provider did not report a model catalog.".to_string(),
        };
    }
    let mut output = String::from("Available models:\n");
    for model in models {
        let selected = if Some(model.as_str()) == current {
            " (current)"
        } else {
            ""
        };
        output.push_str(&format!("- `{model}`{selected}\n"));
    }
    output.pop();
    output
}

fn prompt_from_params(
    params: &Value,
) -> std::result::Result<(String, Vec<(String, String)>), String> {
    let prompt = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| "Missing required prompt array".to_string())?;
    let mut text_parts = Vec::new();
    let mut images = Vec::new();

    for block in prompt {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_string());
                }
            }
            Some("image") => {
                let mime_type = block
                    .get("mimeType")
                    .or_else(|| block.get("mime_type"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Image content block missing mimeType".to_string())?;
                let data = block
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "Image content block missing data".to_string())?;
                images.push((mime_type.to_string(), data.to_string()));
            }
            Some("resource") => {
                if let Some(resource) = block.get("resource") {
                    text_parts.push(format_resource_block(resource));
                }
            }
            Some("resource_link") => {
                let uri = block.get("uri").and_then(Value::as_str).unwrap_or("");
                let name = block.get("name").and_then(Value::as_str).unwrap_or(uri);
                text_parts.push(format!("[Resource link: {name} <{uri}>]"));
            }
            Some(other) => {
                return Err(format!(
                    "Unsupported ACP prompt content block type: {other}"
                ));
            }
            None => return Err("Prompt content block missing type".to_string()),
        }
    }

    Ok((text_parts.join("\n\n"), images))
}

fn format_resource_block(resource: &Value) -> String {
    let uri = resource
        .get("uri")
        .and_then(Value::as_str)
        .unwrap_or("resource");
    if let Some(text) = resource.get("text").and_then(Value::as_str) {
        format!("[Embedded resource: {uri}]\n{text}")
    } else if let Some(blob) = resource.get("blob").and_then(Value::as_str) {
        let mime = resource
            .get("mimeType")
            .or_else(|| resource.get("mime_type"))
            .and_then(Value::as_str)
            .unwrap_or("application/octet-stream");
        format!(
            "[Embedded binary resource: {uri} ({mime}, {} base64 bytes)]",
            blob.len()
        )
    } else {
        format!("[Embedded resource: {uri}]")
    }
}

fn agent_message_chunk(text: String) -> Value {
    json!({
        "sessionUpdate": "agent_message_chunk",
        "content": {
            "type": "text",
            "text": text,
        }
    })
}

fn tool_title(name: &str) -> String {
    match name {
        "bash" => "Running shell command".to_string(),
        "read" => "Reading file".to_string(),
        "write" => "Writing file".to_string(),
        "edit" | "multiedit" | "patch" | "apply_patch" => "Editing files".to_string(),
        "agentgrep" | "grep" | "glob" | "ls" => "Searching workspace".to_string(),
        "webfetch" | "websearch" => "Fetching web content".to_string(),
        other => other.replace('_', " "),
    }
}

pub(crate) fn tool_kind(name: &str) -> &'static str {
    match name {
        "read" => "read",
        "write" | "edit" | "multiedit" | "patch" | "apply_patch" => "edit",
        "bash" | "bg" | "selfdev" => "execute",
        "agentgrep" | "grep" | "glob" | "ls" | "session_search" | "conversation_search" => "search",
        "webfetch" | "websearch" | "codesearch" => "fetch",
        _ => "other",
    }
}

pub(crate) async fn run_acp_command(
    provider_choice: ProviderChoice,
    model: Option<String>,
    provider_profile: Option<String>,
    explicit_tool_profile: bool,
) -> Result<()> {
    crate::env::set_var("JCODE_NON_INTERACTIVE", "1");
    let acp_config = crate::config::config().acp.clone();
    if !explicit_tool_profile {
        crate::env::set_var("JCODE_TOOL_PROFILE", acp_config.tool_profile.trim());
        crate::config::invalidate_config_cache();
    }
    let profile = AcpProfile::parse(&acp_config.profile);
    AcpRuntime::new(profile, provider_choice, model, provider_profile)
        .run()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn acp_tool_kind_maps_core_tools() {
        assert_eq!(tool_kind("read"), "read");
        assert_eq!(tool_kind("apply_patch"), "edit");
        assert_eq!(tool_kind("bash"), "execute");
        assert_eq!(tool_kind("agentgrep"), "search");
        assert_eq!(tool_kind("webfetch"), "fetch");
        assert_eq!(tool_kind("swarm"), "other");
    }

    #[test]
    fn json_rpc_parse_errors_use_standard_codes() {
        let (code, _) = JsonRpcMessage::parse("not json").unwrap_err();
        assert_eq!(code, JSONRPC_PARSE_ERROR);

        let (code, message) = JsonRpcMessage::parse(r#"{"method":"initialize"}"#).unwrap_err();
        assert_eq!(code, JSONRPC_INVALID_REQUEST);
        assert!(message.contains("jsonrpc"));
    }

    #[test]
    fn prompt_from_params_accepts_text_images_and_resources() {
        let params = json!({
            "sessionId": "s1",
            "prompt": [
                {"type": "text", "text": "hello"},
                {"type": "image", "mimeType": "image/png", "data": "abc"},
                {"type": "resource", "resource": {"uri": "file:///tmp/a.rs", "text": "fn main(){}"}},
                {"type": "resource_link", "uri": "file:///tmp/b.rs", "name": "b.rs"}
            ]
        });
        let (text, images) = prompt_from_params(&params).unwrap();
        assert!(text.contains("hello"));
        assert!(text.contains("Embedded resource: file:///tmp/a.rs"));
        assert!(text.contains("Resource link: b.rs"));
        assert_eq!(images, vec![("image/png".to_string(), "abc".to_string())]);
    }

    #[test]
    fn initialize_standard_omits_jcode_meta() {
        let result = initialize_result(&json!({"protocolVersion": 1}), AcpProfile::Standard);
        assert_eq!(result["protocolVersion"], 1);
        assert!(result["agentCapabilities"].get("_meta").is_none());
        assert_eq!(result["agentCapabilities"]["loadSession"], true);
    }

    #[test]
    fn initialize_full_advertises_jcode_extension_meta() {
        let result = initialize_result(&json!({"protocolVersion": 1}), AcpProfile::Full);
        assert_eq!(
            result["agentCapabilities"]["_meta"]["jcode"]["profile"],
            "full"
        );
    }

    #[test]
    fn event_mapper_maps_tool_lifecycle() {
        let mut mapper = EventMapper::new("session1".to_string(), AcpProfile::Standard);
        let start = mapper.map_event(ServerEvent::ToolStart {
            id: "tool1".to_string(),
            name: "bash".to_string(),
        });
        assert_eq!(start[0]["sessionUpdate"], "tool_call");
        assert_eq!(start[0]["kind"], "execute");

        let input = mapper.map_event(ServerEvent::ToolInput {
            delta: "{\"command\":\"true\"}".to_string(),
        });
        assert_eq!(input[0]["rawInput"]["command"], "true");

        let done = mapper.map_event(ServerEvent::ToolDone {
            id: "tool1".to_string(),
            name: "bash".to_string(),
            output: "ok".to_string(),
            error: None,
        });
        assert_eq!(done[0]["status"], "completed");
        assert_eq!(done[0]["content"][0]["content"]["text"], "ok");
    }

    #[test]
    fn non_empty_mcp_servers_rejected_until_session_scoped_mcp_is_supported() {
        let params = json!({"mcpServers": [{"name": "fs"}]});
        let error = ensure_no_acp_mcp_servers(&params).unwrap_err();
        assert!(error.contains("~/.jcode/mcp.json"));
        assert!(error.contains(".mcp.json"));
        assert!(!error.contains("config.toml"));
        let params = json!({"mcpServers": []});
        assert!(ensure_no_acp_mcp_servers(&params).is_ok());
    }

    #[test]
    fn advertised_commands_cover_all_acp_daemon_model_controls() {
        let commands = acp_available_commands();
        let names: Vec<&str> = commands
            .iter()
            .map(|command| command["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["model", "models", "effort"]);
        assert_eq!(commands[0]["input"]["hint"], "model id (optional)");
        assert!(commands[1].get("input").is_none());
        assert!(
            commands[2]["input"]["hint"]
                .as_str()
                .unwrap()
                .contains("high")
        );
    }

    #[test]
    fn advertised_commands_parse_to_real_dispatch_variants() {
        assert_eq!(
            parse_acp_slash_command("/model claude-sonnet-4-5")
                .unwrap()
                .unwrap(),
            AcpSlashCommand::Model(Some("claude-sonnet-4-5".to_string()))
        );
        assert_eq!(
            parse_acp_slash_command("/model ").unwrap().unwrap(),
            AcpSlashCommand::Model(None)
        );
        assert_eq!(
            parse_acp_slash_command("/models").unwrap().unwrap(),
            AcpSlashCommand::Models
        );
        assert_eq!(
            parse_acp_slash_command("/effort xhigh").unwrap().unwrap(),
            AcpSlashCommand::Effort(Some("xhigh".to_string()))
        );
        assert!(parse_acp_slash_command("/models now").unwrap().is_err());
        assert!(parse_acp_slash_command("/not-advertised").is_none());
        assert!(parse_acp_slash_command(" /model literal").is_none());
        assert!(parse_acp_slash_command("ordinary prompt").is_none());
    }

    #[test]
    fn compatibility_methods_accept_host_field_names_and_aliases() {
        assert_eq!(
            compatibility_option_value(
                &json!({"modelId": "deepseek-v4-flash"}),
                &["modelId", "model"],
                "session/set_model"
            )
            .unwrap(),
            "deepseek-v4-flash"
        );
        assert_eq!(
            compatibility_option_value(
                &json!({"reasoningEffort": "high"}),
                &["effort", "reasoningEffort"],
                "session/set_reasoning_effort"
            )
            .unwrap(),
            "high"
        );
        assert!(
            compatibility_option_value(
                &json!({"effort": ""}),
                &["effort", "reasoningEffort"],
                "session/set_reasoning_effort"
            )
            .unwrap_err()
            .contains("non-empty")
        );
    }

    #[test]
    fn cwd_must_be_absolute() {
        let params = json!({"cwd": "relative"});
        assert!(cwd_from_params(&params).is_err());
        let params = json!({"cwd": "/tmp"});
        assert_eq!(cwd_from_params(&params).unwrap(), Path::new("/tmp"));
    }

    #[test]
    fn config_options_include_model_selector_and_effort_ladder() {
        let state = SessionUiState {
            provider_name: Some("openai".to_string()),
            model: Some("gpt-5.2".to_string()),
            available_models: vec!["gpt-5.2".to_string(), "gpt-5.2-codex".to_string()],
            reasoning_effort: Some("high".to_string()),
        };
        let options = session_config_options(&state);
        assert_eq!(options.len(), 2);

        let model = &options[0];
        assert_eq!(model["id"], CONFIG_ID_MODEL);
        assert_eq!(model["category"], "model");
        assert_eq!(model["type"], "select");
        assert_eq!(model["currentValue"], "gpt-5.2");
        assert_eq!(model["options"].as_array().unwrap().len(), 2);

        let effort = &options[1];
        assert_eq!(effort["id"], CONFIG_ID_EFFORT);
        assert_eq!(effort["category"], "thought_level");
        assert_eq!(effort["currentValue"], "high");
        let effort_values: Vec<&str> = effort["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["value"].as_str().unwrap())
            .collect();
        assert!(effort_values.contains(&"medium"));
        assert!(
            !effort_values.iter().any(|value| value.starts_with("swarm")),
            "swarm sentinels are TUI-only and must not leak over ACP: {effort_values:?}"
        );
    }

    #[test]
    fn config_options_current_model_prepended_when_not_listed() {
        let state = SessionUiState {
            provider_name: Some("anthropic".to_string()),
            model: Some("claude-opus-4-6".to_string()),
            available_models: vec!["claude-sonnet-4-5".to_string()],
            reasoning_effort: None,
        };
        let options = session_config_options(&state);
        let model_values: Vec<&str> = options[0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["value"].as_str().unwrap())
            .collect();
        assert_eq!(model_values[0], "claude-opus-4-6");
        assert!(model_values.contains(&"claude-sonnet-4-5"));
    }

    #[test]
    fn legacy_models_catalog_is_emitted_alongside_config_options() {
        let state = SessionUiState {
            provider_name: Some("deepseek".to_string()),
            model: Some("deepseek-v4-flash".to_string()),
            available_models: vec!["deepseek-v4-pro".to_string()],
            reasoning_effort: Some("high".to_string()),
        };
        let mut result = json!({"sessionId": "s1"});
        insert_session_configuration(&mut result, &state);

        assert!(result["configOptions"].is_array());
        assert_eq!(result["models"]["currentModelId"], "deepseek-v4-flash");
        let ids: Vec<&str> = result["models"]["availableModels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["modelId"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["deepseek-v4-flash", "deepseek-v4-pro"]);
    }

    #[test]
    fn config_options_empty_without_model_state() {
        let options = session_config_options(&SessionUiState::default());
        assert!(options.is_empty());
    }

    #[test]
    fn context_limit_falls_back_to_default_for_unknown_models() {
        let state = SessionUiState {
            provider_name: Some("mystery".to_string()),
            model: Some("mystery-model-9000".to_string()),
            available_models: Vec::new(),
            reasoning_effort: None,
        };
        assert_eq!(
            state.context_limit(),
            crate::provider::DEFAULT_CONTEXT_LIMIT as u64
        );
    }
}
