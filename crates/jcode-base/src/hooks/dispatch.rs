//! Discover once before building a hook payload, then dispatch that snapshot.
use super::*;

pub struct PreparedHooks {
    plugin: Vec<plugin_hooks::CommandHook>,
}

/// Reads current scope on every call; no cached enable/approval decisions.
pub fn prepare_hooks(event: &str, cwd: Option<&str>) -> Option<PreparedHooks> {
    let plugin = plugin_hooks::commands_for_dir(event, cwd);
    (hook_configured(event) || !plugin.is_empty()).then_some(PreparedHooks { plugin })
}

/// Fire an observer hook for `event` if one is configured.
///
/// Detached and fire-and-forget: failures are logged, never propagated, and
/// the hook process cannot block the agent.
pub fn dispatch_observer(event: HookEvent) {
    let plugin = plugin_hooks::commands(&event);
    PreparedHooks { plugin }.dispatch_observer(event);
}

impl PreparedHooks {
    pub fn dispatch_observer(self, event: HookEvent) {
        plugin_hooks::observe(&event, self.plugin);
        let command_lines = hook_commands(event.event);
        if command_lines.is_empty() {
            return;
        }
        let event_name = event.event;
        for command_line in command_lines {
            match build_hook_process(&command_line, &event) {
                Ok(mut cmd) => {
                    cmd.stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    match crate::platform::spawn_detached_without_console_window(&mut cmd) {
                        Ok(child) => {
                            crate::platform::reap_detached(child);
                            crate::logging::debug(&format!(
                                "Hook '{event_name}' dispatched to '{command_line}' (session={:?})",
                                event.session_id
                            ));
                        }
                        Err(error) => crate::logging::warn(&format!(
                            "Hook '{event_name}' command '{command_line}' failed to start: {error}"
                        )),
                    }
                }
                Err(error) => crate::logging::warn(&format!(
                    "Hook '{event_name}' command '{command_line}' is invalid: {error}"
                )),
            }
        }
    }
}

/// Run the `pre_tool` gate hook for a tool call, if configured.
///
/// The hook receives `JCODE_HOOK_TOOL_NAME` plus the full tool input JSON on
/// stdin (and truncated in `JCODE_HOOK_TOOL_INPUT`). Contract:
///
/// - exit 0: allow the tool call, optionally with a JSON object on stdout
///   (see [`parse_gate_stdout`]) that can deny, rewrite input, or add context
/// - exit 2: block it; stderr becomes the error shown to the model
/// - anything else (other exits, timeout, spawn failure): fail open
pub async fn run_pre_tool_gate(
    session_id: &str,
    working_dir: Option<&str>,
    tool_name: &str,
    tool_input_json: &str,
) -> GateOutcome {
    let plugin = plugin_hooks::commands_for_dir("pre_tool", working_dir);
    PreparedHooks { plugin }
        .run_pre_tool_gate(session_id, working_dir, tool_name, tool_input_json)
        .await
}

impl PreparedHooks {
    pub async fn run_pre_tool_gate(
        self,
        session_id: &str,
        working_dir: Option<&str>,
        tool_name: &str,
        tool_input_json: &str,
    ) -> GateOutcome {
        let command_lines = hook_commands("pre_tool");
        let mut event = HookEvent::new("pre_tool")
            .session_id(session_id)
            .field("TOOL_NAME", tool_name);
        if let Some(cwd) = working_dir {
            event = event.cwd(cwd);
        }
        let mut outcome = plugin_hooks::gate_commands(self.plugin, &event, tool_input_json).await;
        let mut current_input = match &outcome.updated_input {
            Some(updated) => updated.to_string(),
            None => tool_input_json.to_string(),
        };
        for command_line in command_lines {
            let mut event = HookEvent::new("pre_tool")
                .session_id(session_id)
                .field("TOOL_NAME", tool_name)
                .field(
                    "TOOL_INPUT",
                    truncate_bytes(&current_input, TOOL_INPUT_ENV_LIMIT),
                );
            if let Some(cwd) = working_dir {
                event = event.cwd(cwd);
            }
            let verdict =
                run_pre_tool_command(&command_line, &event, tool_name, &current_input).await;
            if let Some(reason) = verdict.block {
                if outcome.decision == GateDecision::Allow {
                    outcome.decision = GateDecision::Block { reason };
                }
            } else if outcome.decision == GateDecision::Allow
                && let Some(updated) = verdict.updated_input
            {
                current_input = updated.to_string();
                outcome.updated_input = Some(updated);
            }
            if let Some(context) = verdict.context {
                outcome.additional_context.push(context);
            }
        }
        if outcome.decision != GateDecision::Allow {
            outcome.updated_input = None;
        }
        outcome
    }
}
