//! Session-local SDK overlays and owner-scoped callback rendezvous.
use super::*;
use crate::protocol::{ServerEvent, SessionToolConfig, SessionToolDefinition};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
struct Overlay {
    config: SessionToolConfig,
    owner: String,
    sender: Option<mpsc::UnboundedSender<ServerEvent>>,
}
struct Pending {
    owner: String,
    session: String,
    tx: oneshot::Sender<Result<ToolOutput>>,
}
#[derive(Default)]
struct State {
    overlays: HashMap<String, Overlay>,
    pending: HashMap<String, Pending>,
}
static STATE: LazyLock<StdRwLock<State>> = LazyLock::new(|| StdRwLock::new(State::default()));
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

pub(crate) struct ConnectionGuard(pub String);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        crate::screen_context::remove_owner(&self.0);
        let mut state = STATE.write().unwrap_or_else(|e| e.into_inner());
        // Instructions are authorized only while their client is attached.
        // Tool restrictions below remain fail-closed after disconnection.
        for overlay in state.overlays.values_mut().filter(|o| o.owner == self.0) {
            overlay.config.system_prompt_append = None;
        }
        // Prompt-only overlays and temporary bridges have no built-in override
        // to protect after disconnect. Keeping them would permanently block
        // reload if their grant expires before reconnect. `all` intentionally
        // includes the empty custom-tool list of a prompt-only overlay.
        state.overlays.retain(|_, overlay| {
            !(overlay.owner == self.0
                && overlay.config.enabled.is_none()
                && overlay.config.disabled.is_empty()
                && overlay.config.custom.iter().all(|tool| {
                    matches!(
                        tool.name.as_str(),
                        "cloud_environment" | "speak" | "screen_context"
                    )
                }))
        });
        for overlay in state.overlays.values_mut().filter(|o| o.owner == self.0) {
            // Retain definitions and denies, but never fall back to a built-in override.
            overlay.sender = None;
        }
        state.pending.retain(|_, pending| pending.owner != self.0);
    }
}

pub(crate) fn remove_session(session: &str) {
    crate::screen_context::remove_session(session);
    let mut state = STATE.write().unwrap_or_else(|e| e.into_inner());
    state.overlays.remove(session);
    state
        .pending
        .retain(|_, pending| pending.session != session);
}

pub(crate) fn configure(
    session: &str,
    owner: &str,
    config: SessionToolConfig,
    sender: mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    if let Some(prompt) = &config.system_prompt_append {
        anyhow::ensure!(
            !prompt.is_empty()
                && prompt.trim() == prompt
                && prompt.len() <= 16 * 1024
                && !prompt.contains('\0'),
            "System prompt append must be nonempty, trimmed, NUL-free, and at most 16 KiB"
        );
    }
    let valid_name = |name: &str| {
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    };
    let mut names = HashSet::new();
    for tool in &config.custom {
        anyhow::ensure!(
            valid_name(&tool.name),
            "Invalid custom tool name: {}",
            tool.name
        );
        anyhow::ensure!(
            tool.parameters.is_object()
                && tool
                    .parameters
                    .get("type")
                    .is_none_or(|kind| kind == "object"),
            "Tool parameters must be a JSON object schema"
        );
        anyhow::ensure!(
            names.insert(&tool.name),
            "Duplicate custom tool: {}",
            tool.name
        );
    }
    for name in config
        .enabled
        .iter()
        .flatten()
        .chain(config.disabled.iter())
    {
        anyhow::ensure!(valid_name(name), "Invalid tool name: {name}");
    }
    let mut state = STATE.write().unwrap_or_else(|e| e.into_inner());
    if config.enabled.is_none()
        && config.disabled.is_empty()
        && config.custom.is_empty()
        && config.system_prompt_append.is_none()
    {
        if let Some(previous) = state.overlays.get(session) {
            anyhow::ensure!(
                previous.owner == owner,
                "Session tools are owned by another client"
            );
            state.overlays.remove(session);
            state
                .pending
                .retain(|_, pending| pending.session != session);
        }
        crate::screen_context::remove_session(session);
        return Ok(());
    }
    if let Some(previous) = state.overlays.get(session) {
        anyhow::ensure!(
            previous.owner == owner || previous.sender.is_none(),
            "Session tools are owned by another client"
        );
    }
    anyhow::ensure!(
        !state.pending.values().any(|p| p.session == session),
        "Session tool callbacks are busy"
    );
    if config.system_prompt_append.is_none()
        || !config.custom.iter().any(|tool| tool.name == "speak")
    {
        crate::screen_context::remove_session(session);
    }
    state.overlays.insert(
        session.into(),
        Overlay {
            config,
            owner: owner.into(),
            sender: Some(sender),
        },
    );
    Ok(())
}

/// Screen opt-in is available only to the live Jarvis overlay owner.
pub(crate) fn owns_jarvis(session: &str, owner: &str) -> bool {
    let state = STATE.read().unwrap_or_else(|e| e.into_inner());
    state.overlays.get(session).is_some_and(|overlay| {
        overlay.owner == owner
            && overlay.sender.is_some()
            && overlay.config.system_prompt_append.is_some()
            && overlay
                .config
                .custom
                .iter()
                .any(|tool| tool.name == "speak")
    })
}

pub(crate) fn config(session: &str) -> Option<SessionToolConfig> {
    STATE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .overlays
        .get(session)
        .map(|o| o.config.clone())
}

/// These owner-scoped callbacks and tool restrictions are intentionally not
/// persisted. An automatic upgrade must not silently discard or broaden them.
pub(crate) fn has_runtime_overlays() -> bool {
    let state = STATE.read().unwrap_or_else(|e| e.into_inner());
    !state.overlays.is_empty() || !state.pending.is_empty()
}

pub(crate) fn custom(session: &str, name: &str) -> bool {
    config(session).is_some_and(|c| c.custom.iter().any(|t| t.name == name))
}

pub(crate) fn apply_definitions(
    session: &str,
    mut tools: Vec<ToolDefinition>,
) -> Vec<ToolDefinition> {
    if let Some(config) = config(session) {
        let disabled: HashSet<_> = config.disabled.into_iter().collect();
        for custom in config.custom {
            tools.retain(|t| t.name != custom.name);
            tools.push(ToolDefinition {
                name: custom.name,
                description: custom.description,
                input_schema: custom.parameters,
                defer_loading: false,
            });
        }
        tools.retain(|t| !tool_name_is_disabled(&disabled, &t.name));
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

pub(crate) fn wire_definitions(tools: Vec<ToolDefinition>) -> Vec<SessionToolDefinition> {
    tools
        .into_iter()
        .map(|t| SessionToolDefinition {
            name: t.name,
            description: t.description,
            parameters: t.input_schema,
        })
        .collect()
}

struct PendingGuard(String);
impl Drop for PendingGuard {
    fn drop(&mut self) {
        STATE
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .remove(&self.0);
    }
}

pub(crate) async fn execute(session: &str, name: &str, input: Value) -> Result<ToolOutput> {
    execute_with_timeout(session, name, input, Duration::from_secs(120)).await
}

async fn execute_with_timeout(
    session: &str,
    name: &str,
    input: Value,
    timeout: Duration,
) -> Result<ToolOutput> {
    let call_id = format!("sdk-{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = oneshot::channel();
    {
        let mut state = STATE.write().unwrap_or_else(|e| e.into_inner());
        let overlay = state
            .overlays
            .get(session)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("SDK tool configuration disappeared"))?;
        anyhow::ensure!(
            overlay.config.custom.iter().any(|tool| tool.name == name)
                && !tool_name_is_disabled(&overlay.config.disabled.iter().cloned().collect(), name),
            "SDK tool is no longer enabled: {name}"
        );
        let sender = overlay
            .sender
            .ok_or_else(|| anyhow::anyhow!("SDK tool owner disconnected"))?;
        state.pending.insert(
            call_id.clone(),
            Pending {
                owner: overlay.owner,
                session: session.into(),
                tx,
            },
        );
        if sender
            .send(ServerEvent::ToolCall {
                session_id: session.into(),
                call_id: call_id.clone(),
                name: name.into(),
                input,
            })
            .is_err()
        {
            state.pending.remove(&call_id);
            anyhow::bail!("SDK tool owner disconnected");
        }
    }
    let _guard = PendingGuard(call_id);
    tokio::time::timeout(timeout, rx)
        .await
        .map_err(|_| anyhow::anyhow!("SDK tool callback timed out after 120 seconds"))?
        .map_err(|_| anyhow::anyhow!("SDK tool owner disconnected"))?
}

pub(crate) fn complete(
    owner: &str,
    session: &str,
    call_id: &str,
    output: String,
    error: Option<String>,
) -> Result<()> {
    let mut state = STATE.write().unwrap_or_else(|e| e.into_inner());
    let pending = state
        .pending
        .get(call_id)
        .ok_or_else(|| anyhow::anyhow!("Unknown or expired SDK tool call"))?;
    anyhow::ensure!(
        pending.owner == owner && pending.session == session,
        "SDK tool call belongs to another owner or session"
    );
    let pending = state
        .pending
        .remove(call_id)
        .expect("pending checked under lock");
    let result = match error {
        Some(error) => Err(anyhow::anyhow!("{error}")),
        None => Ok(ToolOutput::new(output)),
    };
    pending
        .tx
        .send(result)
        .map_err(|_| anyhow::anyhow!("SDK tool call was cancelled"))
}

/// Execution adapter only, never installed in the shared registry. This keeps
/// hooks, telemetry and output limits identical to built-in tools.
pub(super) struct CallbackTool(pub String);
#[async_trait::async_trait]
impl Tool for CallbackTool {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "SDK callback"
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        execute(&ctx.session_id, &self.0, input).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn custom_definition(name: &str) -> SessionToolDefinition {
        SessionToolDefinition {
            name: name.into(),
            description: "test callback".into(),
            parameters: serde_json::json!({"type":"object"}),
        }
    }

    #[tokio::test]
    async fn disconnect_removes_temporary_speech_but_preserves_other_overlays() {
        let owner = "speech-disconnect-owner";
        let (tx, _rx) = mpsc::unbounded_channel();
        let cases = [
            (
                "sdk-speech-only-disconnect",
                vec!["speak"],
                None,
                vec![],
                false,
            ),
            (
                "sdk-speech-cloud-disconnect",
                vec!["speak", "cloud_environment"],
                None,
                vec![],
                false,
            ),
            (
                "sdk-speech-enabled-disconnect",
                vec!["speak"],
                Some(vec!["speak".into()]),
                vec![],
                true,
            ),
            (
                "sdk-speech-disabled-disconnect",
                vec!["speak"],
                None,
                vec!["bash".into()],
                true,
            ),
            (
                "sdk-speech-unrelated-disconnect",
                vec!["speak", "unrelated"],
                None,
                vec![],
                true,
            ),
            (
                "sdk-unrelated-disconnect",
                vec!["unrelated"],
                None,
                vec![],
                true,
            ),
        ];
        for (session, names, enabled, disabled, _) in &cases {
            configure(
                session,
                owner,
                SessionToolConfig {
                    custom: names.iter().map(|name| custom_definition(name)).collect(),
                    enabled: enabled.clone(),
                    disabled: disabled.clone(),
                    system_prompt_append: None,
                },
                tx.clone(),
            )
            .unwrap();
        }
        let other_session = "sdk-speech-other-owner-disconnect";
        configure(
            other_session,
            "speech-unaffected-owner",
            SessionToolConfig {
                custom: vec![custom_definition("speak")],
                ..Default::default()
            },
            tx,
        )
        .unwrap();

        drop(ConnectionGuard(owner.into()));
        for (session, names, enabled, disabled, retained) in cases {
            assert_eq!(config(session).is_some(), retained, "{session}");
            if retained {
                let config = config(session).unwrap();
                assert_eq!(config.enabled, enabled);
                assert_eq!(config.disabled, disabled);
                assert_eq!(
                    config
                        .custom
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>(),
                    names
                );
                let error = execute(session, names[0], serde_json::json!({}))
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains("owner disconnected"), "{error}");
            }
            remove_session(session);
        }
        assert!(config(other_session).is_some());
        remove_session(other_session);
    }

    #[tokio::test]
    async fn speak_callback_roundtrip_requires_matching_owner_and_session() {
        let session = "sdk-speak-roundtrip-test";
        let owner = "speak-roundtrip-owner";
        let (tx, mut rx) = mpsc::unbounded_channel();
        configure(
            session,
            owner,
            SessionToolConfig {
                custom: vec![custom_definition("speak")],
                ..Default::default()
            },
            tx,
        )
        .unwrap();
        let input = serde_json::json!({"text": "Hello from speech"});
        let task = tokio::spawn(execute(session, "speak", input.clone()));
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let ServerEvent::ToolCall {
            session_id,
            call_id,
            name,
            input: received,
        } = event
        else {
            panic!("Expected speech callback");
        };
        assert_eq!(session_id, session);
        assert_eq!(name, "speak");
        assert_eq!(received, input);
        assert!(complete("other-owner", session, &call_id, "forged".into(), None).is_err());
        assert!(complete(owner, "other-session", &call_id, "forged".into(), None).is_err());
        complete(owner, session, &call_id, "spoken".into(), None).unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(output.output, "spoken");
        assert!(complete(owner, session, &call_id, "duplicate".into(), None).is_err());
        remove_session(session);
    }

    #[test]
    fn disconnect_removes_only_temporary_cloud_overlay() {
        let (tx, _) = mpsc::unbounded_channel();
        configure(
            "sdk-cloud-disconnect-test",
            "cloud-disconnect-owner",
            SessionToolConfig {
                custom: vec![SessionToolDefinition {
                    name: "cloud_environment".into(),
                    description: "temporary".into(),
                    parameters: serde_json::json!({"type":"object"}),
                }],
                ..Default::default()
            },
            tx.clone(),
        )
        .unwrap();
        configure(
            "sdk-other-disconnect-test",
            "cloud-disconnect-owner",
            SessionToolConfig {
                disabled: vec!["bash".into()],
                ..Default::default()
            },
            tx,
        )
        .unwrap();
        drop(ConnectionGuard("cloud-disconnect-owner".into()));
        assert!(config("sdk-cloud-disconnect-test").is_none());
        assert!(config("sdk-other-disconnect-test").is_some());
        remove_session("sdk-other-disconnect-test");
    }
    #[tokio::test]
    async fn owner_clear_cancels_pending_callbacks() {
        let session = "sdk-clear-busy-test";
        let (tx, mut rx) = mpsc::unbounded_channel();
        configure(
            session,
            "clear-owner",
            SessionToolConfig {
                custom: vec![SessionToolDefinition {
                    name: "callback".into(),
                    description: "test".into(),
                    parameters: serde_json::json!({"type":"object"}),
                }],
                ..Default::default()
            },
            tx.clone(),
        )
        .unwrap();
        let task = tokio::spawn(execute(session, "callback", Value::Null));
        let ServerEvent::ToolCall { call_id, .. } = rx.recv().await.unwrap() else {
            unreachable!()
        };
        configure(session, "clear-owner", SessionToolConfig::default(), tx).unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(config(session).is_none());
        assert!(complete("clear-owner", session, &call_id, "late".into(), None).is_err());
    }
    #[test]
    fn empty_config_only_clears_same_owner() {
        let session = "sdk-clear-owner-test";
        let (tx, _) = mpsc::unbounded_channel();
        configure(
            session,
            "owner",
            SessionToolConfig {
                disabled: vec!["bash".into()],
                ..Default::default()
            },
            tx.clone(),
        )
        .unwrap();
        assert!(configure(session, "other", SessionToolConfig::default(), tx.clone()).is_err());
        drop(ConnectionGuard("owner".into()));
        assert!(configure(session, "other", SessionToolConfig::default(), tx.clone()).is_err());
        assert!(config(session).is_some());
        configure(session, "owner", SessionToolConfig::default(), tx).unwrap();
        assert!(config(session).is_none());
    }
    #[tokio::test]
    async fn sdk_callback_timeout_removes_pending() {
        let session = "sdk-timeout-test";
        let (tx, mut rx) = mpsc::unbounded_channel();
        configure(
            session,
            "timeout-owner",
            SessionToolConfig {
                custom: vec![SessionToolDefinition {
                    name: "callback".into(),
                    description: "test callback".into(),
                    parameters: serde_json::json!({"type":"object"}),
                }],
                ..Default::default()
            },
            tx.clone(),
        )
        .unwrap();
        let error =
            execute_with_timeout(session, "callback", Value::Null, Duration::from_millis(1))
                .await
                .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        let ServerEvent::ToolCall { call_id, .. } = rx.recv().await.unwrap() else {
            unreachable!()
        };
        assert!(complete("timeout-owner", session, &call_id, "late".into(), None).is_err());
        configure(session, "timeout-owner", SessionToolConfig::default(), tx).unwrap();
        remove_session(session);
    }
}
