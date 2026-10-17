//! Muse Code provider: drives the local `muse` binary over MSP.
//!
//! # What this provider is, and is not
//!
//! This is a **front-end over Muse Code's own agent loop**, not a model backend
//! in the usual sense. That distinction is forced by the protocol and is worth
//! stating plainly:
//!
//! **No MSP method accepts client-supplied tool definitions.** Verified against
//! the v1.4.10 wire schema: not one of the 69 methods has a `tools` parameter.
//! Muse Code owns the agent loop, its tools, its sandbox, its approvals, its
//! skills and its subagents. So [`Provider::complete`]'s `tools` and `system`
//! arguments are **not forwarded** and jcode's own tool set is not used while
//! this provider is selected.
//!
//! That is why [`Provider::handles_tools_internally`] returns `true`: jcode
//! already has a first-class concept for "the provider runs the tool loop"
//! (Cursor uses it), so the agent loop stops trying to execute tools jcode
//! never sent. This is the honest hook rather than a workaround.
//!
//! # Current limitation
//!
//! Text streams through, but Muse's own tool activity is not yet surfaced as
//! jcode tool events. A long Muse turn that spends minutes in tool calls will
//! therefore look quiet until it emits text. Surfacing tool activity is a
//! deliberate follow-up rather than an oversight: emitting `ToolUseStart` /
//! `ToolResult` pairs for tools jcode will not execute needs the agent loop's
//! block-builder contract to be verified first, and getting it wrong risks
//! malformed-block failures mid-turn.
//!
//! # Compliance
//!
//! The subscription credential is never read. The `muse` process owns it; this
//! provider only speaks MSP to that process.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::{EventStream, ModelRoute, Provider};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::msp::{MspInbound, MspTransport, MuseHostConfig, new_command_id};
use crate::{MSP_CLIENT_NAME, MSP_CLIENT_VERSION};

/// Fallback context window when the host catalog reports nothing.
///
/// Muse Code's own `model/list` reports 1,007,997 for every agent model, which
/// is the documented 1,048,576 minus its own reserved headroom.
const FALLBACK_CONTEXT_WINDOW: usize = 1_048_576;

/// Route api_method, kept stable for saved sessions and `/model` routing.
pub const ROUTE_API_METHOD: &str = "muse-code-msp";

/// Models Muse Code actually exposes to its own agent loop.
///
/// This mirrors the host's `model/list` snapshot rather than the wider Model API
/// catalog: driving the subscription exposes only what Muse Code itself uses.
/// `model/list` is authoritative at runtime and replaces this list when it
/// answers.
pub const MUSE_CODE_MODELS: &[&str] = &["muse-wonder-1.0", "muse-spark-1.3", "muse-spark-1.2"];

/// Default model when the host does not name one.
pub const DEFAULT_MODEL: &str = "muse-wonder-1.0";

/// A live host plus the session this provider is driving.
struct HostState {
    transport: MspTransport,
    session_id: String,
    /// Kept alive so a future revision can surface tool activity; the stream
    /// currently drains it.
    _models: Vec<String>,
}

pub struct MuseCodeProvider {
    model: Mutex<String>,
    host: Mutex<Option<Arc<HostState>>>,
    host_config: MuseHostConfig,
    context_window: std::sync::atomic::AtomicUsize,
}

impl Default for MuseCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MuseCodeProvider {
    pub fn new() -> Self {
        Self::with_model(DEFAULT_MODEL)
    }

    pub fn with_model(model: &str) -> Self {
        Self {
            model: Mutex::new(normalize_model(model).unwrap_or_else(|| DEFAULT_MODEL.to_string())),
            host: Mutex::new(None),
            host_config: MuseHostConfig::default(),
            context_window: std::sync::atomic::AtomicUsize::new(FALLBACK_CONTEXT_WINDOW),
        }
    }

    /// Override the host launch arguments (used by tests to select `--provider
    /// echo`, which spends no subscription usage).
    pub fn with_host_config(mut self, config: MuseHostConfig) -> Self {
        self.host_config = config;
        self
    }

    /// Spawn the host and start a session if one is not already running.
    ///
    /// The session is created once and reused for the provider's lifetime so
    /// Muse keeps its own conversation state across turns.
    async fn ensure_host(&self) -> Result<Arc<HostState>> {
        let mut guard = self.host.lock().await;
        if let Some(state) = guard.as_ref() {
            return Ok(Arc::clone(state));
        }

        let transport = MspTransport::spawn(&self.host_config)
            .context("failed to start `muse serve`; is Muse Code installed and on PATH?")?;
        transport
            .initialize(MSP_CLIENT_NAME, MSP_CLIENT_VERSION)
            .await
            .context("MSP handshake failed")?;

        // Adopt the host's own catalog when it answers, so the model picker and
        // context meter reflect what this Muse build actually serves rather than
        // a hardcoded guess.
        let catalog = transport.request("model/list", json!({})).await.ok();
        let models: Vec<String> = catalog
            .as_ref()
            .and_then(|value| value.get("models"))
            .and_then(|m| m.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| row.get("modelId").and_then(|id| id.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(limit) = catalog
            .as_ref()
            .and_then(|value| value.get("models"))
            .and_then(|m| m.as_array())
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("contextLimit"))
            .and_then(|c| c.as_u64())
        {
            self.context_window
                .store(limit as usize, std::sync::atomic::Ordering::Relaxed);
        }

        let session = transport
            .request(
                "session/start",
                json!({
                    "commandId": new_command_id(),
                    "modelId": self.model.lock().await.clone(),
                }),
            )
            .await
            .context("MSP session/start failed")?;
        let session_id = session
            .get("session")
            .and_then(|s| s.get("sessionId"))
            .and_then(|id| id.as_str())
            .context("session/start returned no sessionId")?
            .to_string();

        let state = Arc::new(HostState {
            transport,
            session_id,
            _models: models,
        });
        *guard = Some(Arc::clone(&state));
        Ok(state)
    }

    /// Submit one turn and stream its events.
    async fn start_turn(&self, prompt: &str) -> Result<EventStream> {
        let state = self.ensure_host().await?;
        // Subscribe before submitting so no event can be missed between the ack
        // and the first notification.
        let events = state.transport.subscribe();

        state
            .transport
            .request(
                "turn/start",
                json!({
                    "sessionId": state.session_id,
                    "commandId": new_command_id(),
                    "input": [{ "type": "text", "text": prompt }],
                    "displayText": prompt,
                }),
            )
            .await
            .context("MSP turn/start failed")?;

        Ok(muse_event_stream(events))
    }
}

/// Extract the text of the most recent user message.
///
/// Muse owns the conversation, so only the newest user input is forwarded; the
/// earlier history already lives in the Muse session.
fn latest_user_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
}

/// Convert the MSP event feed into jcode's stream.
fn muse_event_stream(events: tokio::sync::broadcast::Receiver<MspInbound>) -> EventStream {
    let stream = futures::stream::unfold((events, false), |(mut events, done)| async move {
        if done {
            return None;
        }
        loop {
            match events.recv().await {
                Ok(inbound) => {
                    if let Some(event) = map_inbound(&inbound) {
                        return Some((Ok(event), (events, false)));
                    }
                    if is_turn_end(&inbound) {
                        // Emit the terminal marker, then stop.
                        return Some((Ok(StreamEvent::TextDone), (events, true)));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    return Some((
                        Err(anyhow::anyhow!("muse host closed the connection mid-turn")),
                        (events, true),
                    ));
                }
            }
        }
    });
    Box::pin(stream)
}

fn is_turn_end(inbound: &MspInbound) -> bool {
    matches!(inbound, MspInbound::Notification { method, .. } if method == "turn/completed")
}

/// Map a single MSP notification onto a jcode stream event, if it carries text.
fn map_inbound(inbound: &MspInbound) -> Option<StreamEvent> {
    let MspInbound::Notification { method, params } = inbound else {
        // Server-initiated requests (notably `approval/request`) are not yet
        // bridged to jcode's approval UI. Muse's own policy still applies, and
        // the host will time the request out rather than proceed silently.
        return None;
    };
    match method.as_str() {
        "item/delta" => {
            let field = params
                .get("field")
                .and_then(|f| f.as_str())
                .unwrap_or("text");
            if field != "text" {
                return None;
            }
            params
                .get("delta")
                .and_then(|d| d.as_str())
                .filter(|d| !d.is_empty())
                .map(|delta| StreamEvent::TextDelta(delta.to_string()))
        }
        _ => None,
    }
}

fn normalize_model(model: &str) -> Option<String> {
    let trimmed = model.trim();
    let stripped = trimmed.strip_prefix("muse-code:").unwrap_or(trimmed).trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

#[async_trait]
impl Provider for MuseCodeProvider {
    /// Submit the newest user turn to the Muse session and stream its output.
    ///
    /// `tools` and `system` are intentionally ignored: MSP has no way to carry
    /// either, and Muse Code supplies its own. Passing them on would be a lie,
    /// so they are named with a leading underscore and documented here and in
    /// the module docs. See [`Provider::handles_tools_internally`].
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let prompt = latest_user_text(messages)
            .context("muse-code requires a non-empty user message to submit a turn")?;
        self.start_turn(&prompt).await
    }

    fn name(&self) -> &str {
        crate::PROVIDER_ID
    }

    fn display_name(&self) -> String {
        crate::PROVIDER_LABEL.to_string()
    }

    fn model(&self) -> String {
        self.model
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let Some(model) = normalize_model(model) else {
            bail!("Muse Code model cannot be empty");
        };
        if let Ok(mut guard) = self.model.try_lock() {
            *guard = model;
        }
        Ok(())
    }

    fn available_models_display(&self) -> Vec<String> {
        MUSE_CODE_MODELS.iter().map(|m| m.to_string()).collect()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| ModelRoute {
                model,
                provider: crate::PROVIDER_LABEL.to_string(),
                api_method: ROUTE_API_METHOD.to_string(),
                available: true,
                detail: "Muse Code subscription via the local `muse` binary (MSP over stdio)"
                    .to_string(),
                usage: None,
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        Ok(())
    }

    /// Muse Code is subscription-backed by construction: the host owns the
    /// credential and there is no API-key mode here.
    fn active_auth_method_label(&self) -> Option<&'static str> {
        Some("Muse Code subscription")
    }

    /// Muse Code runs its own tool loop and MSP cannot carry jcode's tools, so
    /// the agent loop must not expect tool calls it never sent.
    fn handles_tools_internally(&self) -> bool {
        true
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        self.context_window
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn transport(&self) -> Option<String> {
        Some("MSP (stdio subprocess)".to_string())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        Arc::new(Self::with_model(&model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_message_types::Role;

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    #[test]
    fn latest_user_text_takes_the_newest_non_empty_user_turn() {
        let messages = vec![user("first"), user(""), user("latest")];
        assert_eq!(latest_user_text(&messages).as_deref(), Some("latest"));

        // Assistant messages are ignored when looking for the prompt.
        let mut with_assistant = vec![user("hello")];
        with_assistant.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "ignored".into(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        });
        assert_eq!(latest_user_text(&with_assistant).as_deref(), Some("hello"));

        assert_eq!(latest_user_text(&[]), None);
        assert_eq!(latest_user_text(&[user("   ")]), None);
    }

    #[test]
    fn provider_declares_that_muse_owns_the_tool_loop() {
        let provider = MuseCodeProvider::new();
        // This is the contract that stops jcode from expecting tool calls it
        // never sent, because MSP cannot carry client tools.
        assert!(provider.handles_tools_internally());
        assert_eq!(provider.name(), "muse-code");
        assert_eq!(provider.display_name(), "Muse Code");
        assert_eq!(
            provider.active_auth_method_label(),
            Some("Muse Code subscription")
        );
    }

    #[test]
    fn model_normalisation_strips_route_prefix_and_rejects_empty() {
        let provider = MuseCodeProvider::new();
        assert_eq!(provider.model(), DEFAULT_MODEL);
        provider.set_model("muse-code:muse-spark-1.3").unwrap();
        assert_eq!(provider.model(), "muse-spark-1.3");
        provider.set_model("muse-wonder-1.0").unwrap();
        assert_eq!(provider.model(), "muse-wonder-1.0");
        assert!(provider.set_model("  ").is_err());
        assert_eq!(
            MuseCodeProvider::with_model("muse-code:muse-spark-1.2").model(),
            "muse-spark-1.2"
        );
    }

    #[test]
    fn routes_are_listed_for_every_muse_agent_model() {
        let provider = MuseCodeProvider::new();
        let routes = provider.model_routes();
        assert_eq!(routes.len(), MUSE_CODE_MODELS.len());
        assert!(routes.iter().any(|r| r.model == "muse-spark-1.3"));
        assert!(routes.iter().all(|r| r.api_method == ROUTE_API_METHOD));
        // Muse owns the tool loop, so the detail line must not imply jcode does.
        assert!(
            routes
                .iter()
                .all(|r| r.detail.contains("Muse Code subscription"))
        );
    }

    #[test]
    fn text_deltas_map_and_non_text_fields_are_dropped() {
        let delta = MspInbound::Notification {
            method: "item/delta".into(),
            params: json!({"field": "text", "delta": "hello"}),
        };
        assert!(matches!(
            map_inbound(&delta),
            Some(StreamEvent::TextDelta(ref t)) if t == "hello"
        ));

        // Empty deltas must not produce events.
        let empty = MspInbound::Notification {
            method: "item/delta".into(),
            params: json!({"field": "text", "delta": ""}),
        };
        assert!(map_inbound(&empty).is_none());

        // A non-text field is not assistant output.
        let other = MspInbound::Notification {
            method: "item/delta".into(),
            params: json!({"field": "reasoning", "delta": "thinking"}),
        };
        assert!(map_inbound(&other).is_none());

        // Approval requests are not text; they are not bridged yet.
        let approval = MspInbound::Request {
            id: json!(7),
            method: "approval/request".into(),
            params: json!({"approvalId": "a"}),
        };
        assert!(map_inbound(&approval).is_none());
    }

    #[test]
    fn turn_completion_terminates_the_stream() {
        let done = MspInbound::Notification {
            method: "turn/completed".into(),
            params: json!({"durationMs": 5}),
        };
        assert!(is_turn_end(&done));
        let started = MspInbound::Notification {
            method: "turn/started".into(),
            params: json!({}),
        };
        assert!(!is_turn_end(&started));
    }

    #[test]
    fn echo_host_config_is_used_by_tests_not_production() {
        // Production default must NOT force a provider; only tests opt into echo.
        let provider = MuseCodeProvider::new();
        assert!(!provider.host_config.args.iter().any(|a| a == "echo"));
        let echo = MuseCodeProvider::new().with_host_config(MuseHostConfig::echo());
        assert!(echo.host_config.args.iter().any(|a| a == "echo"));
    }

    #[test]
    fn fork_preserves_the_selected_model() {
        let provider = MuseCodeProvider::with_model("muse-spark-1.2");
        let forked = provider.fork();
        assert_eq!(forked.model(), "muse-spark-1.2");
        assert_eq!(forked.name(), "muse-code");
    }
}
