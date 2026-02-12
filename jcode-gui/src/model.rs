use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChatEntry {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFeature {
    Memory,
    Swarm,
}

#[derive(Clone, Debug)]
pub enum BackendCommand {
    SendMessage(String),
    RefreshHistory,
    Cancel,
    SoftInterrupt {
        content: String,
        urgent: bool,
    },
    Clear,
    Reload,
    ResumeSession(String),
    CycleModel(i8),
    SetModel(String),
    SetFeature {
        feature: RuntimeFeature,
        enabled: bool,
    },
}

#[derive(Clone, Debug)]
pub enum BackendEvent {
    Connected,
    Disconnected {
        reason: String,
    },
    Status(String),
    SessionAssigned(String),
    HistoryLoaded {
        session_id: String,
        messages: Vec<ChatEntry>,
        provider_name: Option<String>,
        provider_model: Option<String>,
        available_models: Vec<String>,
        mcp_servers: Vec<String>,
        skills: Vec<String>,
        total_tokens: Option<(u64, u64)>,
        all_sessions: Vec<String>,
        client_count: Option<usize>,
        is_canary: Option<bool>,
        server_version: Option<String>,
        server_name: Option<String>,
        server_icon: Option<String>,
        server_has_update: Option<bool>,
    },
    TextDelta(String),
    ToolStart {
        id: String,
        name: String,
    },
    ToolExec {
        id: String,
        name: String,
    },
    ToolDone {
        id: String,
        name: String,
        output: String,
        error: Option<String>,
    },
    TokenUsage {
        input: u64,
        output: u64,
        cache_read_input: Option<u64>,
        cache_creation_input: Option<u64>,
    },
    UpstreamProvider(String),
    Notification(String),
    ModelChanged {
        model: String,
        provider_name: Option<String>,
        error: Option<String>,
    },
    Reloading,
    ReloadProgress {
        step: String,
        message: String,
        success: Option<bool>,
    },
    SoftInterruptInjected {
        content: String,
        point: String,
        tools_skipped: Option<usize>,
    },
    MemoryInjected {
        count: usize,
        prompt_chars: usize,
        computed_age_ms: u64,
    },
    SwarmStatus(Vec<String>),
    Done,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct GuiModel {
    pub connected: bool,
    pub connection_reason: Option<String>,
    pub session_id: Option<String>,
    pub provider_name: String,
    pub provider_model: String,
    pub available_models: Vec<String>,
    pub all_sessions: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub skills: Vec<String>,
    pub server_version: Option<String>,
    pub server_name: Option<String>,
    pub server_icon: Option<String>,
    pub server_has_update: Option<bool>,
    pub client_count: Option<usize>,
    pub is_canary: Option<bool>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub turn_input_tokens: u64,
    pub turn_output_tokens: u64,
    pub cache_read_input: Option<u64>,
    pub cache_creation_input: Option<u64>,
    pub upstream_provider: Option<String>,
    pub is_processing: bool,
    pub messages: Vec<ChatEntry>,
    pub queued_messages: Vec<String>,
    pub streaming_text: String,
    pub active_tool: Option<String>,
    pub activity_log: Vec<String>,
    pub composer: String,
    pub soft_interrupt: String,
    pub model_input: String,
    pub resume_session_input: String,
    pub memory_enabled: bool,
    pub swarm_enabled: bool,
    pub last_error: Option<String>,
}

impl Default for GuiModel {
    fn default() -> Self {
        Self {
            connected: false,
            connection_reason: None,
            session_id: None,
            provider_name: "unknown".to_string(),
            provider_model: "unknown".to_string(),
            available_models: Vec::new(),
            all_sessions: Vec::new(),
            mcp_servers: Vec::new(),
            skills: Vec::new(),
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            client_count: None,
            is_canary: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            turn_input_tokens: 0,
            turn_output_tokens: 0,
            cache_read_input: None,
            cache_creation_input: None,
            upstream_provider: None,
            is_processing: false,
            messages: Vec::new(),
            queued_messages: Vec::new(),
            streaming_text: String::new(),
            active_tool: None,
            activity_log: vec!["GUI initialized".to_string()],
            composer: String::new(),
            soft_interrupt: String::new(),
            model_input: String::new(),
            resume_session_input: String::new(),
            memory_enabled: true,
            swarm_enabled: false,
            last_error: None,
        }
    }
}

impl GuiModel {
    pub fn push_log(&mut self, line: impl Into<String>) {
        self.activity_log.push(line.into());
        if self.activity_log.len() > 250 {
            let drop_count = self.activity_log.len() - 250;
            self.activity_log.drain(0..drop_count);
        }
    }

    pub fn push_message(&mut self, role: impl Into<String>, content: impl Into<String>) {
        self.messages.push(ChatEntry {
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
        });
    }

    pub fn queue_message(&mut self, content: String) {
        let preview = shorten(&content, 100);
        self.queued_messages.push(content);
        self.push_log(format!("Queued: {}", preview));
    }

    pub fn dequeue_message(&mut self) -> Option<String> {
        if self.queued_messages.is_empty() {
            return None;
        }
        Some(self.queued_messages.remove(0))
    }

    fn finalize_streaming_assistant(&mut self) {
        if self.streaming_text.trim().is_empty() {
            self.streaming_text.clear();
            return;
        }

        let content = std::mem::take(&mut self.streaming_text);
        self.push_message("assistant", content);
    }

    pub fn apply_backend_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Connected => {
                self.connected = true;
                self.connection_reason = None;
                self.push_log("Connected to jcode server");
            }
            BackendEvent::Disconnected { reason } => {
                self.connected = false;
                self.connection_reason = Some(reason.clone());
                self.is_processing = false;
                self.active_tool = None;
                self.push_log(format!("Disconnected: {}", reason));
            }
            BackendEvent::Status(message) => {
                self.push_log(message);
            }
            BackendEvent::SessionAssigned(session_id) => {
                self.session_id = Some(session_id.clone());
                self.push_log(format!("Session assigned: {}", session_id));
            }
            BackendEvent::HistoryLoaded {
                session_id,
                messages,
                provider_name,
                provider_model,
                available_models,
                mcp_servers,
                skills,
                total_tokens,
                all_sessions,
                client_count,
                is_canary,
                server_version,
                server_name,
                server_icon,
                server_has_update,
            } => {
                self.session_id = Some(session_id);
                self.messages = messages;
                self.provider_name = provider_name.unwrap_or_else(|| "unknown".to_string());
                self.provider_model = provider_model.unwrap_or_else(|| "unknown".to_string());
                self.available_models = available_models;
                self.all_sessions = all_sessions;
                self.mcp_servers = mcp_servers;
                self.skills = skills;
                self.client_count = client_count;
                self.is_canary = is_canary;
                self.server_version = server_version;
                self.server_name = server_name;
                self.server_icon = server_icon;
                self.server_has_update = server_has_update;
                if let Some((input, output)) = total_tokens {
                    self.total_input_tokens = input;
                    self.total_output_tokens = output;
                }
                self.push_log("History synchronized".to_string());
            }
            BackendEvent::TextDelta(delta) => {
                self.is_processing = true;
                self.streaming_text.push_str(&delta);
            }
            BackendEvent::ToolStart { id, name } => {
                self.active_tool = Some(name.clone());
                self.push_log(format!("Tool start [{}]: {}", id, name));
            }
            BackendEvent::ToolExec { id, name } => {
                self.active_tool = Some(name.clone());
                self.push_log(format!("Tool exec  [{}]: {}", id, name));
            }
            BackendEvent::ToolDone {
                id,
                name,
                output,
                error,
            } => {
                self.active_tool = None;
                if let Some(error) = error {
                    self.push_log(format!("Tool done  [{}]: {} (error: {})", id, name, error));
                } else {
                    self.push_log(format!("Tool done  [{}]: {}", id, name));
                }
                let trimmed = output.trim();
                if !trimmed.is_empty() {
                    self.push_message("tool", format!("[{name}]\n{}", shorten(trimmed, 900)));
                }
            }
            BackendEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
            } => {
                self.turn_input_tokens = input;
                self.turn_output_tokens = output;
                self.cache_read_input = cache_read_input;
                self.cache_creation_input = cache_creation_input;
            }
            BackendEvent::UpstreamProvider(provider) => {
                self.upstream_provider = Some(provider);
            }
            BackendEvent::Notification(message) => {
                self.push_message("notification", message);
            }
            BackendEvent::ModelChanged {
                model,
                provider_name,
                error,
            } => {
                if let Some(error) = error {
                    self.last_error = Some(error.clone());
                    self.push_log(format!("Model change failed: {}", error));
                } else {
                    self.provider_model = model.clone();
                    if let Some(provider_name) = provider_name {
                        self.provider_name = provider_name;
                    }
                    self.push_log(format!("Model changed to {}", model));
                }
            }
            BackendEvent::Reloading => {
                self.push_log("Server reloading...".to_string());
            }
            BackendEvent::ReloadProgress {
                step,
                message,
                success,
            } => {
                let suffix = match success {
                    Some(true) => " [ok]",
                    Some(false) => " [failed]",
                    None => "",
                };
                self.push_log(format!("reload:{}:{}{}", step, message, suffix));
            }
            BackendEvent::SoftInterruptInjected {
                content,
                point,
                tools_skipped,
            } => {
                let skipped = tools_skipped
                    .map(|v| format!(", skipped {} tools", v))
                    .unwrap_or_default();
                self.push_log(format!(
                    "Soft interrupt injected at point {}{}: {}",
                    point,
                    skipped,
                    shorten(&content, 200)
                ));
            }
            BackendEvent::MemoryInjected {
                count,
                prompt_chars,
                computed_age_ms,
            } => {
                self.push_log(format!(
                    "Memory injected: {} entries, {} chars, age {}ms",
                    count, prompt_chars, computed_age_ms
                ));
            }
            BackendEvent::SwarmStatus(statuses) => {
                if !statuses.is_empty() {
                    self.push_log(format!("Swarm: {}", statuses.join(" | ")));
                }
            }
            BackendEvent::Done => {
                self.is_processing = false;
                self.active_tool = None;
                self.finalize_streaming_assistant();
                self.total_input_tokens = self
                    .total_input_tokens
                    .saturating_add(self.turn_input_tokens);
                self.total_output_tokens = self
                    .total_output_tokens
                    .saturating_add(self.turn_output_tokens);
            }
            BackendEvent::Error(message) => {
                self.is_processing = false;
                self.active_tool = None;
                self.last_error = Some(message.clone());
                self.finalize_streaming_assistant();
                self.push_message("error", message.clone());
                self.push_log(format!("Error: {}", message));
            }
        }
    }
}

fn shorten(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut out = String::with_capacity(max_chars + 1);
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}
