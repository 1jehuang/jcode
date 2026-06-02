use super::client_lifecycle::process_message_streaming_mpsc;
use super::state::{
    SessionControlHandle, SessionInterruptQueues, queue_soft_interrupt_for_session,
    session_event_fanout_sender,
};
use super::{SessionAgents, SwarmMember};
use crate::config::SafetyConfig;
use crate::session::Session;
use anyhow::{Context, Result};
use jcode_agent_runtime::{InterruptSignal, SoftInterruptSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const RELAY_LONG_POLL_SECONDS: u32 = 20;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const ERROR_BACKOFF: Duration = Duration::from_secs(10);
const LAUNCH_SESSION_WAIT: Duration = Duration::from_secs(45);
const MAX_RESPONSE_CHARS: usize = 12_000;
const CANCEL_SIGNAL_RESET: Duration = Duration::from_secs(5);

type SessionCancelSignals = Arc<RwLock<HashMap<String, InterruptSignal>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelayApiConfig {
    api_base: String,
    token: String,
    token_id: Option<String>,
    user_id: Option<String>,
    device_id: String,
}

impl RelayApiConfig {
    fn from_safety(safety: &SafetyConfig) -> Option<Self> {
        if !safety.jade_relay_enabled {
            return None;
        }
        let api_base = non_empty(safety.jade_relay_api_base.as_deref())?;
        let token = non_empty(safety.jade_relay_token.as_deref())?;
        Some(Self {
            api_base: normalize_api_base(api_base),
            token: token.to_string(),
            token_id: non_empty(safety.jade_relay_token_id.as_deref()).map(str::to_string),
            user_id: non_empty(safety.jade_relay_user_id.as_deref()).map(str::to_string),
            device_id: default_device_id(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path.trim_start_matches('/'))
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut req = req.header("Authorization", format!("Bearer {}", self.token));
        if let Some(token_id) = &self.token_id {
            req = req.header("x-jade-token-id", token_id);
        }
        req
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelayListenerConfig {
    api: RelayApiConfig,
    session_id: String,
    process_existing_prompts: bool,
}

impl RelayListenerConfig {
    fn from_safety(safety: &SafetyConfig) -> Option<Self> {
        if !safety.jade_relay_reply_enabled {
            return None;
        }
        let api = RelayApiConfig::from_safety(safety)?;
        let session_id = non_empty(safety.jade_relay_session_id.as_deref())?;
        Some(Self {
            api,
            session_id: session_id.to_string(),
            process_existing_prompts: env_flag("JCODE_JADE_RELAY_PROCESS_EXISTING_PROMPTS"),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelayLaunchConfig {
    api: RelayApiConfig,
    default_working_dir: Option<String>,
}

impl RelayLaunchConfig {
    fn from_safety(safety: &SafetyConfig) -> Option<Self> {
        if !safety.jade_relay_launch_enabled {
            return None;
        }
        Some(Self {
            api: RelayApiConfig::from_safety(safety)?,
            default_working_dir: non_empty(safety.jade_relay_launch_working_dir.as_deref())
                .map(str::to_string),
        })
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            let trimmed = value.trim();
            !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

fn normalize_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim();
    if trimmed.ends_with('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/")
    }
}

fn session_command_event_types_param() -> String {
    "types=prompt,cancel".to_string()
}

fn default_device_id() -> String {
    if let Ok(value) = std::env::var("JCODE_JADE_RELAY_DEVICE_ID")
        && !value.trim().is_empty()
    {
        return value.trim().to_string();
    }
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "device".to_string());
    format!("jcode-{host}")
}

pub(super) fn spawn_if_configured(
    safety: &SafetyConfig,
    sessions: SessionAgents,
    soft_interrupt_queues: SessionInterruptQueues,
    shutdown_signals: SessionCancelSignals,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    if let Some(config) = RelayListenerConfig::from_safety(safety) {
        crate::logging::info(&format!(
            "Starting Jade relay listener session={} user_id={}",
            config.session_id,
            config.api.user_id.as_deref().unwrap_or("<token-default>")
        ));
        let session_sessions = Arc::clone(&sessions);
        let session_interrupts = Arc::clone(&soft_interrupt_queues);
        let session_shutdown_signals = Arc::clone(&shutdown_signals);
        let session_swarm = Arc::clone(&swarm_members);
        tokio::spawn(async move {
            let client = RelayClient::new(config);
            client
                .run(
                    session_sessions,
                    session_interrupts,
                    session_shutdown_signals,
                    session_swarm,
                )
                .await;
        });
    }

    if let Some(config) = RelayLaunchConfig::from_safety(safety) {
        crate::logging::info(&format!(
            "Starting Jade relay launch listener device={} user_id={}",
            config.api.device_id,
            config.api.user_id.as_deref().unwrap_or("<token-default>")
        ));
        tokio::spawn(async move {
            let client = RelayLauncherClient::new(config);
            client
                .run(
                    sessions,
                    soft_interrupt_queues,
                    shutdown_signals,
                    swarm_members,
                )
                .await;
        });
    }
}

struct RelayClient {
    config: RelayListenerConfig,
    http: reqwest::Client,
}

impl RelayClient {
    fn new(config: RelayListenerConfig) -> Self {
        Self {
            config,
            http: crate::provider::shared_http_client(),
        }
    }

    fn url(&self, path: &str) -> String {
        self.config.api.url(path)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.config.api.auth(req)
    }

    async fn run(
        &self,
        sessions: SessionAgents,
        soft_interrupt_queues: SessionInterruptQueues,
        shutdown_signals: SessionCancelSignals,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) {
        let after = if self.config.process_existing_prompts {
            0
        } else {
            match self.poll_commands(0, 0).await {
                Ok(response) => response.next_after,
                Err(error) => {
                    crate::logging::warn(&format!("Jade relay initial poll failed: {error:#}"));
                    0
                }
            }
        };
        self.run_from_after(
            after,
            sessions,
            soft_interrupt_queues,
            shutdown_signals,
            swarm_members,
        )
        .await;
    }

    async fn run_from_after(
        &self,
        mut after: i64,
        sessions: SessionAgents,
        soft_interrupt_queues: SessionInterruptQueues,
        shutdown_signals: SessionCancelSignals,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) {
        let mut last_heartbeat = Instant::now()
            .checked_sub(HEARTBEAT_INTERVAL)
            .unwrap_or_else(Instant::now);

        loop {
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                if let Err(error) = self.heartbeat().await {
                    crate::logging::debug(&format!("Jade relay heartbeat failed: {error:#}"));
                }
                last_heartbeat = Instant::now();
            }

            match self.poll_commands(after, RELAY_LONG_POLL_SECONDS).await {
                Ok(response) => {
                    after = response.next_after;
                    for event in response.events {
                        if event.seq > after {
                            after = event.seq;
                        }
                        let result = match event.event_type() {
                            "prompt" => {
                                self.handle_prompt(
                                    event,
                                    &sessions,
                                    &soft_interrupt_queues,
                                    Arc::clone(&swarm_members),
                                )
                                .await
                            }
                            "cancel" => {
                                self.handle_cancel(
                                    event,
                                    &sessions,
                                    &soft_interrupt_queues,
                                    &shutdown_signals,
                                )
                                .await
                            }
                            other => {
                                crate::logging::debug(&format!(
                                    "Jade relay ignoring unsupported session event type={other}"
                                ));
                                Ok(())
                            }
                        };
                        if let Err(error) = result {
                            crate::logging::warn(&format!(
                                "Jade relay event handling failed: {error:#}"
                            ));
                        }
                    }
                }
                Err(error) => {
                    crate::logging::warn(&format!("Jade relay poll failed: {error:#}"));
                    tokio::time::sleep(ERROR_BACKOFF).await;
                }
            }
        }
    }

    async fn heartbeat(&self) -> Result<()> {
        let mut body = serde_json::json!({
            "device_id": &self.config.api.device_id,
            "label": &self.config.api.device_id,
            "platform": std::env::consts::OS,
            "session_id": &self.config.session_id,
            "app": "jcode",
            "capabilities": ["session"],
        });
        add_user_id(&mut body, self.config.api.user_id.as_deref());
        let response = self
            .auth(self.http.post(self.url("v1/devices")).json(&body))
            .send()
            .await?;
        ensure_success(response, "heartbeat").await.map(|_| ())
    }

    async fn poll_commands(&self, after: i64, wait: u32) -> Result<RelayEventsResponse> {
        let session = urlencoding_encode(&self.config.session_id);
        let mut params = vec![
            format!("after={}", after.max(0)),
            session_command_event_types_param(),
            format!("wait={wait}"),
            "limit=100".to_string(),
        ];
        if let Some(user_id) = &self.config.api.user_id {
            params.push(format!("user_id={}", urlencoding_encode(user_id)));
        }
        let url = self.url(&format!(
            "v1/sessions/{}/events?{}",
            session,
            params.join("&")
        ));
        let response = self.auth(self.http.get(url)).send().await?;
        let response = ensure_success(response, "poll").await?;
        response
            .json::<RelayEventsResponse>()
            .await
            .context("decode relay poll response")
    }

    async fn post_relay_event(
        &self,
        event_type: &str,
        text: &str,
        request_seq: i64,
        data: Option<serde_json::Value>,
    ) -> Result<i64> {
        let session = urlencoding_encode(&self.config.session_id);
        let mut body = serde_json::json!({
            "type": event_type,
            "text": truncate_chars(text, MAX_RESPONSE_CHARS),
            "request_seq": request_seq,
            "origin": &self.config.api.device_id,
        });
        if let Some(data) = data
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("data".to_string(), data);
        }
        add_user_id(&mut body, self.config.api.user_id.as_deref());
        let response = self
            .auth(
                self.http
                    .post(self.url(&format!("v1/sessions/{}/events", session)))
                    .json(&body),
            )
            .send()
            .await?;
        let response = ensure_success(response, "post relay event").await?;
        let event = response
            .json::<RelayEvent>()
            .await
            .context("decode relay event append response")?;
        Ok(event.seq)
    }

    async fn handle_prompt(
        &self,
        event: RelayEvent,
        sessions: &SessionAgents,
        soft_interrupt_queues: &SessionInterruptQueues,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) -> Result<()> {
        let text = event.text.unwrap_or_default();
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        crate::logging::info(&format!(
            "Jade relay delivering prompt seq={} session={} chars={}",
            event.seq,
            self.config.session_id,
            text.chars().count()
        ));

        let _ = self
            .post_relay_event(
                "status",
                "Jade relay prompt processing started",
                event.seq,
                Some(serde_json::json!({
                    "phase": "running",
                    "prompt_seq": event.seq,
                    "session_id": &self.config.session_id,
                })),
            )
            .await;

        match deliver_to_session(
            &self.config.session_id,
            text,
            sessions,
            soft_interrupt_queues,
            swarm_members,
        )
        .await
        {
            Ok(reply) => {
                let response_seq = self
                    .post_relay_event(
                        "response",
                        &reply,
                        event.seq,
                        Some(serde_json::json!({
                            "phase": "completed",
                            "prompt_seq": event.seq,
                            "session_id": &self.config.session_id,
                        })),
                    )
                    .await?;
                let _ = self
                    .post_relay_event(
                        "status",
                        "Jade relay prompt processing completed",
                        event.seq,
                        Some(serde_json::json!({
                            "phase": "completed",
                            "prompt_seq": event.seq,
                            "response_seq": response_seq,
                            "session_id": &self.config.session_id,
                        })),
                    )
                    .await;
                Ok(())
            }
            Err(error) => {
                let message = format!("delivery failed: {error:#}");
                let _ = self
                    .post_relay_event(
                        "error",
                        &message,
                        event.seq,
                        Some(serde_json::json!({
                            "phase": "failed",
                            "prompt_seq": event.seq,
                            "session_id": &self.config.session_id,
                        })),
                    )
                    .await;
                Err(error)
            }
        }
    }

    async fn handle_cancel(
        &self,
        event: RelayEvent,
        sessions: &SessionAgents,
        soft_interrupt_queues: &SessionInterruptQueues,
        shutdown_signals: &SessionCancelSignals,
    ) -> Result<()> {
        let reason = event
            .text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("Cancel requested via Jade relay");
        let interrupt = format!("[jade relay cancel] {reason}");
        crate::logging::info(&format!(
            "Jade relay cancel requested seq={} session={}",
            event.seq, self.config.session_id
        ));

        let live_agent = {
            let guard = sessions.read().await;
            guard.contains_key(&self.config.session_id)
        };
        let queue = {
            let guard = soft_interrupt_queues.read().await;
            guard.get(&self.config.session_id).cloned()
        };
        let stop_signal = {
            let guard = shutdown_signals.read().await;
            guard.get(&self.config.session_id).cloned()
        };

        let (mode, signal_reset_ms) = if let (Some(queue), Some(stop_signal)) = (queue, stop_signal)
        {
            let control = SessionControlHandle::cancel_only(
                self.config.session_id.clone(),
                queue,
                stop_signal,
            );
            if !control.queue_soft_interrupt(interrupt, true, SoftInterruptSource::User) {
                anyhow::bail!(
                    "session '{}' could not accept cancel interrupt",
                    self.config.session_id
                );
            }
            control.request_cancel();
            let reset_control = control.clone();
            tokio::spawn(async move {
                tokio::time::sleep(CANCEL_SIGNAL_RESET).await;
                reset_control.reset_cancel();
            });
            ("signalled", Some(CANCEL_SIGNAL_RESET.as_millis() as u64))
        } else if live_agent {
            if queue_soft_interrupt_for_session(
                &self.config.session_id,
                interrupt,
                true,
                SoftInterruptSource::User,
                soft_interrupt_queues,
                sessions,
            )
            .await
            {
                ("queued_no_signal", None)
            } else {
                anyhow::bail!(
                    "session '{}' could not accept cancel",
                    self.config.session_id
                )
            }
        } else if queue_soft_interrupt_for_session(
            &self.config.session_id,
            interrupt,
            true,
            SoftInterruptSource::User,
            soft_interrupt_queues,
            sessions,
        )
        .await
        {
            ("queued_offline", None)
        } else {
            anyhow::bail!(
                "session '{}' is not live and could not queue cancel",
                self.config.session_id
            )
        };

        self.post_relay_event(
            "status",
            "Jade relay cancel requested",
            event.seq,
            Some(serde_json::json!({
                "phase": "cancel_requested",
                "mode": mode,
                "signal_reset_ms": signal_reset_ms,
                "session_id": &self.config.session_id,
            })),
        )
        .await
        .map(|_| ())
    }
}

struct RelayLauncherClient {
    config: RelayLaunchConfig,
    http: reqwest::Client,
}

impl RelayLauncherClient {
    fn new(config: RelayLaunchConfig) -> Self {
        Self {
            config,
            http: crate::provider::shared_http_client(),
        }
    }

    fn url(&self, path: &str) -> String {
        self.config.api.url(path)
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.config.api.auth(req)
    }

    async fn run(
        &self,
        sessions: SessionAgents,
        soft_interrupt_queues: SessionInterruptQueues,
        shutdown_signals: SessionCancelSignals,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) {
        let mut after = match self.poll_launches(0, 0).await {
            Ok(response) => response.next_after,
            Err(error) => {
                crate::logging::warn(&format!("Jade relay launch initial poll failed: {error:#}"));
                0
            }
        };
        let mut last_heartbeat = Instant::now()
            .checked_sub(HEARTBEAT_INTERVAL)
            .unwrap_or_else(Instant::now);

        loop {
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                if let Err(error) = self.heartbeat().await {
                    crate::logging::debug(&format!(
                        "Jade relay launch heartbeat failed: {error:#}"
                    ));
                }
                last_heartbeat = Instant::now();
            }

            match self.poll_launches(after, RELAY_LONG_POLL_SECONDS).await {
                Ok(response) => {
                    after = response.next_after;
                    for event in response.events {
                        if event.seq > after {
                            after = event.seq;
                        }
                        if let Err(error) = self
                            .handle_launch(
                                event,
                                &sessions,
                                &soft_interrupt_queues,
                                &shutdown_signals,
                                Arc::clone(&swarm_members),
                            )
                            .await
                        {
                            crate::logging::warn(&format!(
                                "Jade relay launch handling failed: {error:#}"
                            ));
                        }
                    }
                }
                Err(error) => {
                    crate::logging::warn(&format!("Jade relay launch poll failed: {error:#}"));
                    tokio::time::sleep(ERROR_BACKOFF).await;
                }
            }
        }
    }

    async fn heartbeat(&self) -> Result<()> {
        let mut body = serde_json::json!({
            "device_id": &self.config.api.device_id,
            "label": &self.config.api.device_id,
            "platform": std::env::consts::OS,
            "app": "jcode",
            "capabilities": ["launch"],
        });
        add_user_id(&mut body, self.config.api.user_id.as_deref());
        let response = self
            .auth(self.http.post(self.url("v1/devices")).json(&body))
            .send()
            .await?;
        ensure_success(response, "launch heartbeat")
            .await
            .map(|_| ())
    }

    async fn poll_launches(&self, after: i64, wait: u32) -> Result<RelayEventsResponse> {
        let device = urlencoding_encode(&self.config.api.device_id);
        let mut params = vec![
            format!("after={}", after.max(0)),
            "types=launch".to_string(),
            format!("wait={wait}"),
            "limit=100".to_string(),
        ];
        if let Some(user_id) = &self.config.api.user_id {
            params.push(format!("user_id={}", urlencoding_encode(user_id)));
        }
        let url = self.url(&format!(
            "v1/devices/{}/events?{}",
            device,
            params.join("&")
        ));
        let response = self.auth(self.http.get(url)).send().await?;
        let response = ensure_success(response, "poll launch commands").await?;
        response
            .json::<RelayEventsResponse>()
            .await
            .context("decode relay launch poll response")
    }

    async fn post_device_event(
        &self,
        event_type: &str,
        text: &str,
        request_seq: i64,
        data: Option<serde_json::Value>,
    ) -> Result<i64> {
        let device = urlencoding_encode(&self.config.api.device_id);
        let mut body = serde_json::json!({
            "type": event_type,
            "text": truncate_chars(text, MAX_RESPONSE_CHARS),
            "request_seq": request_seq,
            "origin": &self.config.api.device_id,
        });
        if let Some(data) = data
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("data".to_string(), data);
        }
        add_user_id(&mut body, self.config.api.user_id.as_deref());
        let response = self
            .auth(
                self.http
                    .post(self.url(&format!("v1/devices/{}/events", device)))
                    .json(&body),
            )
            .send()
            .await?;
        let response = ensure_success(response, "post device event").await?;
        let event = response
            .json::<RelayEvent>()
            .await
            .context("decode device event append response")?;
        Ok(event.seq)
    }

    async fn post_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        text: &str,
        request_seq: i64,
        data: Option<serde_json::Value>,
    ) -> Result<i64> {
        let session = urlencoding_encode(session_id);
        let mut body = serde_json::json!({
            "type": event_type,
            "text": truncate_chars(text, MAX_RESPONSE_CHARS),
            "origin": &self.config.api.device_id,
        });
        if request_seq > 0
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert(
                "request_seq".to_string(),
                serde_json::Value::Number(serde_json::Number::from(request_seq)),
            );
        }
        if let Some(data) = data
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert("data".to_string(), data);
        }
        add_user_id(&mut body, self.config.api.user_id.as_deref());
        let response = self
            .auth(
                self.http
                    .post(self.url(&format!("v1/sessions/{}/events", session)))
                    .json(&body),
            )
            .send()
            .await?;
        let response = ensure_success(response, "post launched session event").await?;
        let event = response
            .json::<RelayEvent>()
            .await
            .context("decode session event append response")?;
        Ok(event.seq)
    }

    async fn handle_launch(
        &self,
        event: RelayEvent,
        sessions: &SessionAgents,
        soft_interrupt_queues: &SessionInterruptQueues,
        shutdown_signals: &SessionCancelSignals,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) -> Result<()> {
        let request =
            LaunchRequest::from_event(&event, self.config.default_working_dir.as_deref())?;
        crate::logging::info(&format!(
            "Jade relay launching headed session for device command seq={} chars={}",
            event.seq,
            request.text.chars().count()
        ));

        let (session_id, cwd) = create_launch_session(&request)?;
        let launched = spawn_launch_window(
            &session_id,
            &cwd,
            request.selfdev,
            request.provider_key.as_deref(),
        )?;
        if !launched {
            anyhow::bail!("no supported terminal found for headed Jcode launch")
        }

        let launched_data = serde_json::json!({
            "session_id": &session_id,
            "working_dir": cwd.display().to_string(),
            "phase": "launched",
        });
        let _ = self
            .post_device_event(
                "launch_status",
                &format!("Launched headed Jcode session {session_id}"),
                event.seq,
                Some(launched_data),
            )
            .await;

        if request.text.trim().is_empty() {
            self.spawn_session_listener(
                session_id,
                0,
                sessions,
                soft_interrupt_queues,
                shutdown_signals,
                swarm_members,
            );
            return Ok(());
        }

        if let Err(error) = wait_for_live_session(&session_id, sessions, LAUNCH_SESSION_WAIT).await
        {
            let message =
                format!("launched session {session_id} but it did not connect: {error:#}");
            let _ = self
                .post_device_event(
                    "launch_error",
                    &message,
                    event.seq,
                    Some(serde_json::json!({ "session_id": &session_id })),
                )
                .await;
            return Err(error);
        }

        let prompt_seq = self
            .post_session_event(
                &session_id,
                "prompt",
                &request.text,
                0,
                Some(serde_json::json!({
                    "source": "device_launch",
                    "launch_seq": event.seq,
                    "device_id": &self.config.api.device_id,
                })),
            )
            .await?;

        let _ = self
            .post_session_event(
                &session_id,
                "status",
                "Jade relay launch prompt processing started",
                prompt_seq,
                Some(serde_json::json!({
                    "phase": "running",
                    "prompt_seq": prompt_seq,
                    "session_id": &session_id,
                    "source": "device_launch",
                })),
            )
            .await;

        let after = match deliver_to_launched_session(
            &session_id,
            &request.text,
            sessions,
            Arc::clone(&swarm_members),
        )
        .await
        {
            Ok(reply) => {
                let response_seq = self
                    .post_session_event(
                        &session_id,
                        "response",
                        &reply,
                        prompt_seq,
                        Some(serde_json::json!({
                            "phase": "completed",
                            "prompt_seq": prompt_seq,
                            "session_id": &session_id,
                            "source": "device_launch",
                        })),
                    )
                    .await?;
                let status_seq = self
                    .post_session_event(
                        &session_id,
                        "status",
                        "Jade relay launch prompt processing completed",
                        prompt_seq,
                        Some(serde_json::json!({
                            "phase": "completed",
                            "prompt_seq": prompt_seq,
                            "response_seq": response_seq,
                            "session_id": &session_id,
                            "source": "device_launch",
                        })),
                    )
                    .await
                    .unwrap_or(response_seq);
                let _ = self
                    .post_device_event(
                        "launch_status",
                        &format!("Delivered launch prompt to {session_id}"),
                        event.seq,
                        Some(serde_json::json!({
                            "session_id": &session_id,
                            "prompt_seq": prompt_seq,
                            "response_seq": response_seq,
                            "phase": "processed",
                        })),
                    )
                    .await;
                status_seq
            }
            Err(error) => {
                let message = format!("delivery failed: {error:#}");
                let error_seq = self
                    .post_session_event(
                        &session_id,
                        "error",
                        &message,
                        prompt_seq,
                        Some(serde_json::json!({
                            "phase": "failed",
                            "prompt_seq": prompt_seq,
                            "session_id": &session_id,
                            "source": "device_launch",
                        })),
                    )
                    .await
                    .unwrap_or(prompt_seq);
                let _ = self
                    .post_device_event(
                        "launch_error",
                        &message,
                        event.seq,
                        Some(serde_json::json!({
                            "session_id": &session_id,
                            "prompt_seq": prompt_seq,
                            "error_seq": error_seq,
                        })),
                    )
                    .await;
                return Err(error);
            }
        };

        self.spawn_session_listener(
            session_id,
            after,
            sessions,
            soft_interrupt_queues,
            shutdown_signals,
            swarm_members,
        );
        Ok(())
    }

    fn spawn_session_listener(
        &self,
        session_id: String,
        after: i64,
        sessions: &SessionAgents,
        soft_interrupt_queues: &SessionInterruptQueues,
        shutdown_signals: &SessionCancelSignals,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) {
        let config = RelayListenerConfig {
            api: self.config.api.clone(),
            session_id: session_id.clone(),
            process_existing_prompts: false,
        };
        let sessions = Arc::clone(sessions);
        let soft_interrupt_queues = Arc::clone(soft_interrupt_queues);
        let shutdown_signals = Arc::clone(shutdown_signals);
        tokio::spawn(async move {
            crate::logging::info(&format!(
                "Starting Jade relay listener for launched session {session_id} after seq {after}"
            ));
            let client = RelayClient::new(config);
            client
                .run_from_after(
                    after,
                    sessions,
                    soft_interrupt_queues,
                    shutdown_signals,
                    swarm_members,
                )
                .await;
        });
    }
}

async fn ensure_success(response: reqwest::Response, action: &str) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    anyhow::bail!("jade relay {action} failed ({status}): {body}")
}

fn add_user_id(body: &mut serde_json::Value, user_id: Option<&str>) {
    if let Some(user_id) = user_id
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert(
            "user_id".to_string(),
            serde_json::Value::String(user_id.to_string()),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchRequest {
    text: String,
    working_dir: Option<String>,
    model: Option<String>,
    provider_key: Option<String>,
    selfdev: bool,
}

impl LaunchRequest {
    fn from_event(event: &RelayEvent, default_working_dir: Option<&str>) -> Result<Self> {
        let data = event.data.as_ref();
        let text = event.text.clone().unwrap_or_default();
        let working_dir = data_string(data, "working_dir")
            .or_else(|| data_string(data, "cwd"))
            .or_else(|| default_working_dir.map(str::to_string));
        let model = data_string(data, "model");
        let provider_key = provider_key_for_launch_model(
            model.as_deref(),
            data_string(data, "provider")
                .or_else(|| data_string(data, "provider_key"))
                .as_deref(),
        );
        Ok(Self {
            text,
            working_dir,
            model,
            provider_key,
            selfdev: data_bool(data, "selfdev"),
        })
    }
}

fn data_string(data: Option<&serde_json::Value>, key: &str) -> Option<String> {
    data.and_then(|value| value.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn data_bool(data: Option<&serde_json::Value>, key: &str) -> bool {
    data.and_then(|value| value.get(key))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn provider_key_for_launch_model(
    model: Option<&str>,
    provider_key_override: Option<&str>,
) -> Option<String> {
    if let Some(provider_key) = provider_key_override
        .map(str::trim)
        .filter(|provider_key| !provider_key.is_empty())
    {
        return Some(provider_key.to_string());
    }

    let model = model?.trim();
    if model.is_empty() {
        return None;
    }
    if let Some((prefix, _rest)) = model.split_once(':') {
        let prefix = prefix.trim();
        if crate::provider::provider_from_model_key(prefix).is_some()
            || crate::provider_catalog::resolve_openai_compatible_profile_selection(prefix)
                .is_some()
            || crate::config::config().providers.contains_key(prefix)
        {
            return Some(prefix.to_string());
        }
    }
    crate::provider::provider_for_model(model).map(str::to_string)
}

fn create_launch_session(request: &LaunchRequest) -> Result<(String, PathBuf)> {
    let cwd = request
        .working_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    if !cwd.is_dir() {
        anyhow::bail!("launch working_dir is not a directory: {}", cwd.display());
    }

    let mut session = Session::create(None, Some("Jade relay launch".to_string()));
    session.working_dir = Some(cwd.display().to_string());
    if let Some(model) = &request.model {
        session.model = Some(model.clone());
    }
    if let Some(provider_key) = &request.provider_key {
        session.provider_key = Some(provider_key.clone());
    }
    if request.selfdev {
        session.set_canary("self-dev");
    }
    session.save()?;
    Ok((session.id.clone(), cwd))
}

fn spawn_launch_window(
    session_id: &str,
    cwd: &Path,
    selfdev_requested: bool,
    provider_key: Option<&str>,
) -> Result<bool> {
    let exe = crate::build::client_update_candidate(selfdev_requested)
        .map(|(path, _label)| path)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("jcode"));
    if selfdev_requested {
        crate::session_launch::spawn_selfdev_in_new_terminal_with_provider(
            &exe,
            session_id,
            cwd,
            provider_key,
        )
    } else {
        crate::session_launch::spawn_resume_in_new_terminal_with_provider(
            &exe,
            session_id,
            cwd,
            provider_key,
        )
    }
}

async fn wait_for_live_session(
    session_id: &str,
    sessions: &SessionAgents,
    timeout: Duration,
) -> Result<()> {
    let started = Instant::now();
    loop {
        if sessions.read().await.contains_key(session_id) {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            anyhow::bail!(
                "session '{session_id}' did not connect within {}s",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn deliver_to_session(
    session_id: &str,
    text: &str,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Result<String> {
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    };
    let Some(agent) = agent else {
        anyhow::bail!("session '{session_id}' is not live in this Jcode server")
    };

    if agent.try_lock().is_err() {
        let queued = queue_soft_interrupt_for_session(
            session_id,
            format!("[jade relay message from user]\n{text}"),
            false,
            SoftInterruptSource::User,
            soft_interrupt_queues,
            sessions,
        )
        .await;
        if queued {
            return Ok("Message queued for the running session.".to_string());
        }
        anyhow::bail!("session '{session_id}' is busy and could not accept a queued interrupt")
    }

    let start_message_index = {
        let agent_guard = agent.lock().await;
        agent_guard.message_count()
    };
    let event_tx = session_event_fanout_sender(session_id.to_string(), swarm_members);
    process_message_streaming_mpsc(Arc::clone(&agent), text, Vec::new(), None, event_tx).await?;
    let reply = {
        let agent_guard = agent.lock().await;
        agent_guard.latest_assistant_text_after(start_message_index)
    };
    Ok(reply.unwrap_or_else(|| "Message processed; no assistant text was produced.".to_string()))
}

async fn deliver_to_launched_session(
    session_id: &str,
    text: &str,
    sessions: &SessionAgents,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Result<String> {
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    };
    let Some(agent) = agent else {
        anyhow::bail!("session '{session_id}' is not live in this Jcode server")
    };

    // A just-spawned headed TUI briefly owns the agent lock while it subscribes
    // and restores history. For launch commands, wait for that startup work and
    // run the first prompt as a normal turn instead of falling back to a soft
    // interrupt queue that may not be processed until a later turn.
    let start_message_index = {
        let agent_guard = agent.lock().await;
        agent_guard.message_count()
    };
    let event_tx = session_event_fanout_sender(session_id.to_string(), swarm_members);
    process_message_streaming_mpsc(Arc::clone(&agent), text, Vec::new(), None, event_tx).await?;
    let reply = {
        let agent_guard = agent.lock().await;
        agent_guard.latest_assistant_text_after(start_message_index)
    };
    Ok(reply.unwrap_or_else(|| "Message processed; no assistant text was produced.".to_string()))
}

#[derive(Debug, serde::Deserialize)]
struct RelayEventsResponse {
    #[serde(default)]
    events: Vec<RelayEvent>,
    #[serde(default)]
    next_after: i64,
}

#[derive(Debug, serde::Deserialize)]
struct RelayEvent {
    #[serde(default)]
    seq: i64,
    #[serde(default, rename = "type")]
    event_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

impl RelayEvent {
    fn event_type(&self) -> &str {
        if self.event_type.trim().is_empty() {
            "prompt"
        } else {
            self.event_type.trim()
        }
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

/// Minimal percent-encoding for path/query segments (alnum and -_.~ pass through).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_listener_config_is_opt_in_and_requires_credentials() {
        let cfg = SafetyConfig::default();
        assert!(RelayListenerConfig::from_safety(&cfg).is_none());

        let cfg = SafetyConfig {
            jade_relay_enabled: true,
            jade_relay_reply_enabled: true,
            ..SafetyConfig::default()
        };
        assert!(RelayListenerConfig::from_safety(&cfg).is_none());
    }

    #[test]
    fn relay_listener_config_accepts_complete_opt_in_config() {
        let cfg = SafetyConfig {
            jade_relay_enabled: true,
            jade_relay_reply_enabled: true,
            jade_relay_api_base: Some("https://example.com/api".to_string()),
            jade_relay_token: Some("tok".to_string()),
            jade_relay_token_id: Some("alice-token".to_string()),
            jade_relay_user_id: Some("alice".to_string()),
            jade_relay_session_id: Some("sess-1".to_string()),
            ..SafetyConfig::default()
        };
        let parsed = RelayListenerConfig::from_safety(&cfg).expect("complete config");
        assert_eq!(parsed.api.api_base, "https://example.com/api/");
        assert_eq!(parsed.api.token, "tok");
        assert_eq!(parsed.api.token_id.as_deref(), Some("alice-token"));
        assert_eq!(parsed.api.user_id.as_deref(), Some("alice"));
        assert_eq!(parsed.session_id, "sess-1");
    }

    #[test]
    fn relay_launch_config_is_separately_opt_in() {
        let cfg = SafetyConfig {
            jade_relay_enabled: true,
            jade_relay_api_base: Some("https://example.com".to_string()),
            jade_relay_token: Some("tok".to_string()),
            ..SafetyConfig::default()
        };
        assert!(RelayLaunchConfig::from_safety(&cfg).is_none());

        let cfg = SafetyConfig {
            jade_relay_launch_enabled: true,
            jade_relay_launch_working_dir: Some("/tmp/project".to_string()),
            ..cfg
        };
        let parsed = RelayLaunchConfig::from_safety(&cfg).expect("launch opt-in config");
        assert_eq!(parsed.api.api_base, "https://example.com/");
        assert_eq!(parsed.default_working_dir.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn launch_request_reads_structured_data() {
        let event = RelayEvent {
            seq: 7,
            event_type: "launch".to_string(),
            text: Some("hello from web".to_string()),
            data: Some(serde_json::json!({
                "working_dir": "/tmp/repo",
                "model": "openai:gpt-test",
                "provider": "openai",
                "selfdev": true,
            })),
        };
        let parsed = LaunchRequest::from_event(&event, Some("/fallback")).expect("launch request");
        assert_eq!(parsed.text, "hello from web");
        assert_eq!(parsed.working_dir.as_deref(), Some("/tmp/repo"));
        assert_eq!(parsed.model.as_deref(), Some("openai:gpt-test"));
        assert_eq!(parsed.provider_key.as_deref(), Some("openai"));
        assert!(parsed.selfdev);
    }

    #[test]
    fn relay_session_listener_polls_prompt_and_cancel_commands() {
        assert_eq!(session_command_event_types_param(), "types=prompt,cancel");
    }

    #[test]
    fn relay_event_type_defaults_to_prompt_for_legacy_events() {
        let legacy = RelayEvent {
            seq: 1,
            event_type: String::new(),
            text: Some("hello".to_string()),
            data: None,
        };
        assert_eq!(legacy.event_type(), "prompt");

        let cancel = RelayEvent {
            event_type: "cancel".to_string(),
            ..legacy
        };
        assert_eq!(cancel.event_type(), "cancel");
    }

    #[test]
    fn relay_url_encoding_matches_jade_api_expectations() {
        assert_eq!(urlencoding_encode("sess-relay-test"), "sess-relay-test");
        assert_eq!(urlencoding_encode("a/b c"), "a%2Fb%20c");
        assert_eq!(urlencoding_encode("user.name~1_2"), "user.name~1_2");
    }

    #[test]
    fn truncation_preserves_short_text_and_marks_long_text() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
    }
}
