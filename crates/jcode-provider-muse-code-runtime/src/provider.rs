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
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

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
    transport: Arc<MspTransport>,
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
            transport: Arc::new(transport),
            session_id,
            _models: models,
        });

        // Muse owns the agent loop, so it raises its own approvals. MSP defines
        // `approval/request` as a **must-answer** server request, and a request
        // left unanswered stalls the turn until it times out. Answering is
        // therefore a protocol obligation rather than an optional feature.
        spawn_server_request_responder(
            Arc::downgrade(&state.transport),
            state.transport.subscribe(),
        );

        *guard = Some(Arc::clone(&state));
        Ok(state)
    }

    /// Submit one turn and stream its events.
    async fn start_turn(&self, prompt: &str) -> Result<EventStream> {
        let state = self.ensure_host().await?;
        // Subscribe before submitting so no event can be missed between the ack
        // and the first notification.
        let events = state.transport.subscribe();

        let command_id = new_command_id();
        state
            .transport
            .request(
                "turn/start",
                json!({
                    "sessionId": state.session_id,
                    "commandId": command_id,
                    "input": [{ "type": "text", "text": prompt }],
                    "displayText": prompt,
                }),
            )
            .await
            .context("MSP turn/start failed")?;

        // `turnId` is the commandId we minted, so the guard knows exactly which
        // turn to cancel if jcode stops reading.
        Ok(muse_event_stream(
            events,
            TurnCancelGuard {
                transport: Arc::downgrade(&state.transport),
                session_id: state.session_id.clone(),
                turn_id: command_id,
            },
        ))
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

/// Cancels the Muse turn when jcode stops reading the reply.
///
/// jcode aborts a reply by dropping the stream. Dropping a broadcast receiver
/// cancels nothing on the host side, so without this Muse would keep making
/// model calls and running tools after the user pressed stop, and the only
/// evidence would be side effects in the workspace.
struct TurnCancelGuard {
    transport: std::sync::Weak<MspTransport>,
    session_id: String,
    turn_id: String,
}

impl TurnCancelGuard {
    /// Mark the turn finished, so dropping the stream does not cancel it.
    fn disarm(&mut self) {
        self.turn_id.clear();
    }
}

impl Drop for TurnCancelGuard {
    fn drop(&mut self) {
        if self.turn_id.is_empty() {
            return;
        }
        let Some(transport) = self.transport.upgrade() else {
            return;
        };
        let session_id = std::mem::take(&mut self.session_id);
        let turn_id = std::mem::take(&mut self.turn_id);
        // `Drop` cannot await, so hand the cancellation to the runtime.
        tokio::spawn(async move {
            let _ = transport
                .request(
                    "turn/cancel",
                    json!({
                        "sessionId": session_id,
                        "commandId": new_command_id(),
                        "turnId": turn_id,
                    }),
                )
                .await;
        });
    }
}

/// Convert the MSP event feed into jcode's stream.
fn muse_event_stream(
    events: tokio::sync::broadcast::Receiver<MspInbound>,
    guard: TurnCancelGuard,
) -> EventStream {
    let stream = futures::stream::unfold(
        (events, false, guard),
        |(mut events, done, mut guard)| async move {
            if done {
                return None;
            }
            loop {
                match events.recv().await {
                    Ok(inbound) => {
                        // The host exiting arrives as an explicit `Closed` message,
                        // not as `RecvError::Closed`: the transport still holds a
                        // sender, so the channel stays open and a bare `recv()` would
                        // block forever. Without this arm a Muse crash left the turn
                        // waiting indefinitely.
                        if matches!(inbound, MspInbound::Closed) {
                            return Some((
                                Err(anyhow::anyhow!(
                                    "the muse host exited before the turn completed"
                                )),
                                (events, true, guard),
                            ));
                        }
                        if let Some(event) = map_inbound(&inbound) {
                            return Some((Ok(event), (events, false, guard)));
                        }
                        if is_turn_end(&inbound) {
                            // The turn reached its own end, so the guard must not
                            // cancel it on drop.
                            guard.disarm();
                            return Some((Ok(StreamEvent::TextDone), (events, true, guard)));
                        }
                    }
                    // Skipping dropped events would present a truncated reply as a
                    // complete one, which is worse than failing: the caller would
                    // believe the turn succeeded. The channel buffers 256 messages
                    // and a burst can overrun it.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        return Some((
                            Err(anyhow::anyhow!(
                                "muse emitted more events than jcode could consume ({missed} dropped), so the reply is incomplete"
                            )),
                            (events, true, guard),
                        ));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Some((
                            Err(anyhow::anyhow!("muse host closed the connection mid-turn")),
                            (events, true, guard),
                        ));
                    }
                }
            }
        },
    );
    Box::pin(stream)
}

fn is_turn_end(inbound: &MspInbound) -> bool {
    matches!(inbound, MspInbound::Notification { method, .. } if method == "turn/completed")
}

/// Why jcode declines a Muse user-input prompt.
const USER_INPUT_DECLINE_REASON: &str =
    "jcode has no prompt surface wired for Muse Code yet, so no user is available to answer";

/// Answer MSP's server-initiated requests.
///
/// Both `approval/request` and `userInput/request` are **must-answer** requests.
/// The response is a presentation receipt only; the decision travels separately
/// (`approval/decide`, `userInput/answer` or `userInput/cancel`). An unanswered
/// request tells the host this connection could not present the prompt, so it
/// stays pending and the turn stalls.
///
/// jcode has no approval or prompt surface wired for Muse yet, so this
/// acknowledges presentation and then **declines**, which is the fail-safe
/// direction in both cases: nothing executes that a user did not approve, and
/// no answer is fabricated on the user's behalf. The model sees an explicit
/// denial or cancellation rather than a hang.
///
/// The honest cost is that Muse cannot run commands outside its own permission
/// profile, and cannot ask a clarifying question, while this stands. Bridging
/// these into jcode's approval and input UI is the follow-up.
fn spawn_server_request_responder(
    transport: std::sync::Weak<MspTransport>,
    mut events: broadcast::Receiver<MspInbound>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(MspInbound::Request { id, method, params }) => {
                    // Weak, not Arc: a strong reference here would be a cycle.
                    // The task would hold the transport, the transport holds the
                    // broadcast sender for this very channel, so the receiver
                    // would never see `Closed`, the task would never exit, and
                    // the child process would outlive the provider. In a
                    // long-lived daemon that accumulates Muse processes.
                    let Some(transport) = transport.upgrade() else {
                        break;
                    };
                    // Acknowledge presentation first, so the host is never left
                    // waiting on a receipt that will not arrive.
                    let _ = transport.respond(id, json!({})).await;
                    if method == "approval/request"
                        && let Some(choice_id) = deny_choice_id(&params)
                    {
                        let _ = transport
                            .request(
                                "approval/decide",
                                json!({
                                    "sessionId": params.get("sessionId"),
                                    "commandId": new_command_id(),
                                    "approvalId": params.get("approvalId"),
                                    "choiceId": choice_id,
                                }),
                            )
                            .await;
                    }
                    // A clarifying question has the same failure mode: left
                    // unanswered it stalls the turn. Declining is honest (no
                    // answer is invented) and the model sees a cancelled
                    // result, so it can proceed or report rather than hang.
                    if method == "userInput/request"
                        && let Some(cancel) = user_input_cancel_request(&params)
                    {
                        let _ = transport.request("userInput/cancel", cancel).await;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Build the `userInput/cancel` request for a Muse clarifying prompt.
///
/// Returns `None` when the payload lacks the identifiers the host requires, in
/// which case the prompt is left alone rather than cancelled with missing
/// fields.
fn user_input_cancel_request(params: &Value) -> Option<Value> {
    let session_id = params.get("sessionId")?.as_str()?;
    let user_input_id = params.get("userInputId")?.as_str()?;
    Some(json!({
        "sessionId": session_id,
        "commandId": new_command_id(),
        "userInputId": user_input_id,
        "reason": USER_INPUT_DECLINE_REASON,
    }))
}

/// Choose the deny option from an `approval/request` payload.
///
/// Preference order: a `denied` decision scoped to a single use, then any
/// `denied` variant, then `abort`. Returns `None` when the host offered no
/// deniable choice, in which case the request is deliberately left pending
/// rather than guessing at a choice id the host never advertised.
fn deny_choice_id(params: &Value) -> Option<String> {
    let choices = params.get("availableChoices")?.as_array()?;
    let find = |decision: &str, scope: Option<&str>| {
        choices.iter().find_map(|choice| {
            if choice.get("decision")?.as_str()? != decision {
                return None;
            }
            if let Some(scope) = scope
                && choice.get("scope").and_then(Value::as_str) != Some(scope)
            {
                return None;
            }
            choice.get("choiceId")?.as_str().map(str::to_string)
        })
    };
    find("denied", Some("once"))
        .or_else(|| find("denied", None))
        .or_else(|| find("abort", None))
}

/// Map a single MSP notification onto a jcode stream event, if it carries text.
fn map_inbound(inbound: &MspInbound) -> Option<StreamEvent> {
    let MspInbound::Notification { method, params } = inbound else {
        // Server-initiated requests are handled by
        // `spawn_server_request_responder`, which answers them whether or not a
        // stream is active. They carry no assistant text, so nothing is emitted
        // here.
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
        // Muse runs its own tools, so its tool activity never reaches jcode as a
        // tool call and cannot become transcript content: emitting
        // `ToolUseStart` for tools jcode will not execute would leave unbalanced
        // tool blocks in the transcript.
        //
        // A silent turn is still a bad turn, though. MSP gives every tool-ish
        // item a `displayLabel` in three plain-text forms keyed off the item's
        // status, so surface that on jcode's **status line** instead.
        // `StatusDetail` is transport detail for the status bar rather than
        // transcript content, which makes it the right surface for activity
        // jcode does not own.
        "item/started" | "item/updated" | "item/completed" => tool_status_detail(params),
        _ => None,
    }
}

/// Render a Muse tool item as a status-line detail.
///
/// Gated on the presence of `displayLabel` rather than an enumerated set of item
/// kinds: the schema documents that field as `toolCall.displayLabel`, and
/// message and reasoning items do not carry it, so keying on it stays correct if
/// Muse adds kinds later.
fn tool_status_detail(params: &Value) -> Option<StreamEvent> {
    let item = params.get("item")?;
    let label = item.get("displayLabel")?;

    // The item's status picks which of the three forms applies. Anything that is
    // not a clear success uses `neutral`, which never implies success.
    let text = match item.get("status").and_then(Value::as_str) {
        Some("inProgress") => label.get("active"),
        Some("completed") => label.get("done"),
        _ => label.get("neutral"),
    }
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|text| !text.is_empty())?;

    Some(StreamEvent::StatusDetail {
        detail: format!("muse: {text}"),
    })
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
            *guard = model.clone();
        }

        // A live Muse session keeps its own model selection, so changing only
        // the local name would leave jcode displaying one model while Muse
        // answered with another. `set_model` is synchronous, so the update is
        // handed to the runtime instead of awaited; a failure is logged rather
        // than silently dropped.
        let live = self.host.try_lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .map(|state| (Arc::clone(&state.transport), state.session_id.clone()))
        });
        if let Some((transport, session_id)) = live {
            tokio::spawn(async move {
                if let Err(error) = transport
                    .request(
                        "session/setModel",
                        json!({
                            "sessionId": session_id,
                            "commandId": new_command_id(),
                            "model": { "modelId": model },
                        }),
                    )
                    .await
                {
                    jcode_logging::warn(&format!(
                        "muse-code: session/setModel failed, the live session is still on its previous model: {error:#}"
                    ));
                }
            });
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
    fn deny_choice_prefers_single_use_denial() {
        let params = json!({
            "availableChoices": [
                {"choiceId": "a", "decision": "approved", "scope": "once"},
                {"choiceId": "b", "decision": "denied", "scope": "session"},
                {"choiceId": "c", "decision": "denied", "scope": "once"},
            ]
        });
        assert_eq!(deny_choice_id(&params).as_deref(), Some("c"));
    }

    #[test]
    fn deny_choice_falls_back_and_refuses_to_guess() {
        // No single-use denial: take any denial rather than an approval.
        let any_denied = json!({
            "availableChoices": [
                {"choiceId": "a", "decision": "approved", "scope": "once"},
                {"choiceId": "b", "decision": "denied", "scope": "session"},
            ]
        });
        assert_eq!(deny_choice_id(&any_denied).as_deref(), Some("b"));

        // Nothing deniable: `abort` is the next fail-safe. Never an approval.
        let abort = json!({
            "availableChoices": [
                {"choiceId": "a", "decision": "approved", "scope": "once"},
                {"choiceId": "z", "decision": "abort", "scope": "once"},
            ]
        });
        assert_eq!(deny_choice_id(&abort).as_deref(), Some("z"));

        // Only approvals offered: decline to choose rather than approve.
        let only_approved = json!({
            "availableChoices": [
                {"choiceId": "a", "decision": "approved", "scope": "once"},
                {"choiceId": "b", "decision": "approvedPolicyAmendment", "scope": "localPersistent"},
            ]
        });
        assert_eq!(deny_choice_id(&only_approved), None);

        // Malformed payloads must not panic.
        assert_eq!(deny_choice_id(&json!({})), None);
        assert_eq!(deny_choice_id(&json!({"availableChoices": "nope"})), None);
        assert_eq!(
            deny_choice_id(&json!({"availableChoices": [{"decision": "denied"}]})),
            None
        );
    }

    #[test]
    fn muse_tool_activity_lands_on_the_status_line_not_the_transcript() {
        // A running tool call shows its `active` form.
        let running = json!({
            "sessionId": "s",
            "item": {
                "kind": "toolCall",
                "status": "inProgress",
                "displayLabel": {
                    "active": "Running the test suite",
                    "done": "Ran the test suite",
                    "neutral": "Test suite",
                }
            }
        });
        match map_inbound(&MspInbound::Notification {
            method: "item/started".into(),
            params: running,
        }) {
            Some(StreamEvent::StatusDetail { detail }) => {
                assert_eq!(detail, "muse: Running the test suite");
            }
            other => panic!("expected a status detail, got {other:?}"),
        }

        // A finished call shows its `done` form.
        let done = json!({
            "item": {
                "kind": "toolCall",
                "status": "completed",
                "displayLabel": {"active": "a", "done": "Ran the test suite", "neutral": "n"}
            }
        });
        match map_inbound(&MspInbound::Notification {
            method: "item/completed".into(),
            params: done,
        }) {
            Some(StreamEvent::StatusDetail { detail }) => {
                assert_eq!(detail, "muse: Ran the test suite");
            }
            other => panic!("expected a status detail, got {other:?}"),
        }

        // A rejected call must never read as success: `neutral` is used.
        let rejected = json!({
            "item": {
                "kind": "toolCall",
                "status": "rejected",
                "displayLabel": {"active": "a", "done": "d", "neutral": "Command rejected"}
            }
        });
        match map_inbound(&MspInbound::Notification {
            method: "item/completed".into(),
            params: rejected,
        }) {
            Some(StreamEvent::StatusDetail { detail }) => {
                assert_eq!(detail, "muse: Command rejected");
            }
            other => panic!("expected a status detail, got {other:?}"),
        }
    }

    #[test]
    fn non_tool_items_do_not_produce_status_details() {
        // Assistant text arrives as a delta, never as a status line entry.
        let assistant = json!({
            "item": {"kind": "agentMessage", "status": "inProgress", "text": "hi"}
        });
        assert!(
            map_inbound(&MspInbound::Notification {
                method: "item/started".into(),
                params: assistant,
            })
            .is_none()
        );

        // A user message likewise.
        let user = json!({"item": {"kind": "userMessage", "status": "completed"}});
        assert!(
            map_inbound(&MspInbound::Notification {
                method: "item/completed".into(),
                params: user,
            })
            .is_none()
        );

        // Blank labels must not produce empty status noise.
        let blank = json!({
            "item": {"displayLabel": {"active": "  ", "done": "", "neutral": ""},
                    "status": "inProgress"}
        });
        assert!(
            map_inbound(&MspInbound::Notification {
                method: "item/started".into(),
                params: blank,
            })
            .is_none()
        );

        // Malformed payloads must not panic or fabricate a status.
        assert!(
            map_inbound(&MspInbound::Notification {
                method: "item/started".into(),
                params: json!({}),
            })
            .is_none()
        );
        assert!(
            map_inbound(&MspInbound::Notification {
                method: "item/started".into(),
                params: json!({"item": {"displayLabel": "nope"}}),
            })
            .is_none()
        );
    }

    #[test]
    fn user_input_prompts_are_declined_with_both_required_ids() {
        let params = json!({
            "sessionId": "s-1",
            "userInputId": "u-1",
            "toolName": "ask",
            "questions": [{"questionId": "q"}],
        });
        let cancel = user_input_cancel_request(&params).expect("should build a cancel request");
        assert_eq!(cancel["sessionId"], json!("s-1"));
        assert_eq!(cancel["userInputId"], json!("u-1"));
        // The idempotency handle is mandatory on every MSP mutation.
        let command_id = cancel["commandId"].as_str().expect("commandId present");
        assert_eq!(
            uuid::Uuid::parse_str(command_id).unwrap().get_version_num(),
            7
        );
        // The reason tells the model why, so a decline is not a silent failure.
        assert!(cancel["reason"].as_str().unwrap().contains("jcode"));

        // Missing identifiers must not produce a malformed mutation.
        assert!(user_input_cancel_request(&json!({"sessionId": "s"})).is_none());
        assert!(user_input_cancel_request(&json!({"userInputId": "u"})).is_none());
        assert!(user_input_cancel_request(&json!({})).is_none());
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
