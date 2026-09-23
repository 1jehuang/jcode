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
        let mut state = STATE.write().unwrap_or_else(|e| e.into_inner());
        for overlay in state.overlays.values_mut().filter(|o| o.owner == self.0) {
            // Retain definitions and denies, but never fall back to a built-in override.
            overlay.sender = None;
        }
        state.pending.retain(|_, pending| pending.owner != self.0);
    }
}

pub(crate) fn remove_session(session: &str) {
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

pub(crate) fn config(session: &str) -> Option<SessionToolConfig> {
    STATE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .overlays
        .get(session)
        .map(|o| o.config.clone())
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
