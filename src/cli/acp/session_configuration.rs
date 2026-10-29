//! ACP model and reasoning effort configuration presentation.

use super::*;

pub(super) const CONFIG_ID_MODEL: &str = "model";
pub(super) const CONFIG_ID_EFFORT: &str = "reasoning_effort";

/// ACP permission-mode selector.
///
/// ACP v1 hosts that gate tool calls through the agent's own session
/// configuration (Kiro Crew's `codex` slot is one) look for a config option
/// whose id is `mode` and that advertises the values the host may select.
/// Advertising it is a PROMISE: the value must actually restrict the session,
/// or a host that selected it would believe the session cannot write.
pub(super) const CONFIG_ID_MODE: &str = "mode";

/// The only permission mode this agent advertises.
///
/// `read-only` disables every tool that can change the workspace or reach the
/// network, and leaves the read-only ones mounted. It is the mode a host
/// selects when it wants the agent to look but not touch.
pub(super) const MODE_READ_ONLY: &str = "read-only";

/// Tools that can change the workspace, the machine, or the outside world.
///
/// A `read-only` session disables exactly these and keeps the rest, so the
/// session can still read the repository and search it. The set is
/// deliberately over-inclusive: a tool that spawns other tools (`batch`),
/// spawns agents (`swarm`), or opens a UI that can drive the machine is in it
/// even though its own name does not say "write". A name that is merely
/// unfamiliar is also in it, because an unfamiliar tool is one whose behaviour
/// nobody has verified to be read-only.
pub(super) const READ_ONLY_DENIED_TOOLS: &[&str] = &[
    // Filesystem mutation.
    "write",
    "edit",
    "replace",
    "apply_patch",
    // Process execution and its relatives.
    "bash",
    "bg",
    "compile_remote",
    // Outbound network and desktop control.
    "browser",
    "webfetch",
    "websearch",
    "macos_computer_use",
    "open",
    // Session and memory mutation.
    "todo",
    "initiative",
    "memory",
    "session_search",
    "gmail",
    "calendar",
    "schedule",
    "selfdev",
    "desktop_selfdev",
    // Tools that run other tools, spawn agents, or open a machine-driving UI.
    "batch",
    "swarm",
    "applet",
    "panel",
    "side_panel",
    "skill_manage",
    "integration_tools",
    "elicitate_mcp",
];

pub(super) fn acp_available_commands() -> Vec<Value> {
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

pub(super) fn insert_session_configuration(result: &mut Value, state: &SessionUiState) {
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

pub(super) fn session_models(state: &SessionUiState) -> Option<Value> {
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

pub(super) fn available_efforts(state: &SessionUiState) -> Vec<&'static str> {
    crate::provider::inferred_reasoning_efforts(
        state.provider_name.as_deref(),
        state.model.as_deref(),
    )
    .into_iter()
    // `swarm` is a TUI sentinel, not provider effort levels.
    .filter(|effort| !effort.starts_with("swarm"))
    .collect()
}

/// Build the ACP `configOptions` array (permission mode, model selector, and
/// reasoning effort) from the current session provider state.
///
/// The permission mode is always present; the model and effort entries are
/// empty when the daemon reported no usable state for them.
pub(super) fn session_config_options(state: &SessionUiState) -> Vec<Value> {
    let mut options = Vec::new();

    // The permission mode is advertised FIRST and unconditionally. A host that
    // gates tool calls through the agent's session configuration looks for this
    // option by id, and a session that only advertised it once a host had
    // already asked for it would be a session the host had to trust blind.
    options.push(json!({
        "type": "select",
        "id": CONFIG_ID_MODE,
        "name": "Permission mode",
        "category": "permission",
        "currentValue": state.permission_mode.as_deref().unwrap_or(MODE_READ_ONLY),
        "options": [{
            "value": MODE_READ_ONLY,
            "name": "Read-only",
        }],
    }));

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
