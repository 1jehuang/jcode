//! Claude plugin command hooks. Discovery only reads trusted, enabled roots.
//!
//! SessionStart/SessionEnd are observers; PreToolUse is a gate/rewriter;
//! PostToolUse observes successful tools with full input and text response.
//! Stop and prompt/agent/http/async hooks require lifecycle semantics jcode
//! does not expose here and are diagnosed, never silently remapped. Observer
//! output cannot inject context. Tool input schemas remain jcode's schemas.
use super::{GateDecision, GateOutcome, HookEvent};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(super) struct CommandHook {
    event: String,
    command: String,
    args: Option<Vec<String>>,
    root: PathBuf,
    // Parsing alone is inert; production discovery binds the registry identity
    // before execution. Never derive a persistent identity from a cache path.
    plugin: Option<crate::plugin::PluginRoot>,
    matcher: Option<regex::Regex>,
    timeout: Duration,
}

pub(super) fn supported(event: &str) -> bool {
    matches!(
        event,
        "session_start" | "session_end" | "pre_tool" | "post_tool"
    )
}

fn claude_event(event: &str) -> Option<&'static str> {
    match event {
        "session_start" => Some("SessionStart"),
        "session_end" => Some("SessionEnd"),
        "pre_tool" => Some("PreToolUse"),
        "post_tool" => Some("PostToolUse"),
        _ => None,
    }
}

fn read_json(
    root: &Path,
    path: &Path,
    seen: &mut std::collections::HashSet<PathBuf>,
    diagnostics: &mut Vec<String>,
) -> Option<Value> {
    let canonical = match path.canonicalize() {
        Ok(path) if path.starts_with(root) => path,
        Ok(_) => {
            diagnostics.push(format!("hook file escapes plugin root: {}", path.display()));
            return None;
        }
        Err(error) => {
            diagnostics.push(format!("{}: {error}", path.display()));
            return None;
        }
    };
    if !seen.insert(canonical.clone()) {
        if !root
            .join("hooks/hooks.json")
            .canonicalize()
            .is_ok_and(|default| default == canonical)
        {
            diagnostics.push(format!("duplicate plugin hook file: {}", path.display()));
        }
        return None;
    }
    match std::fs::read(&canonical)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| Ok(serde_json::from_slice(&bytes)?))
    {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.push(format!("{}: {error}", path.display()));
            None
        }
    }
}

fn load_root(root: &Path) -> (Vec<CommandHook>, Vec<String>) {
    let mut diagnostics = Vec::new();
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) => return (Vec::new(), vec![format!("invalid plugin root: {error}")]),
    };
    let mut seen = std::collections::HashSet::new();
    let manifest_path = root.join(".claude-plugin/plugin.json");
    let manifest = if manifest_path.is_file() {
        read_json(&root, &manifest_path, &mut seen, &mut diagnostics)
    } else {
        None
    };
    let mut documents = Vec::new();
    let default = root.join("hooks/hooks.json");
    if default.is_file()
        && let Some(value) = read_json(&root, &default, &mut seen, &mut diagnostics)
    {
        documents.push(value);
    }
    if let Some(hooks) = manifest.as_ref().and_then(|m| m.get("hooks")) {
        let entries = hooks
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![hooks.clone()]);
        for entry in entries {
            if let Some(path) = entry.as_str() {
                let path = path
                    .replace("${CLAUDE_PLUGIN_ROOT}", &root.to_string_lossy())
                    .replace("$CLAUDE_PLUGIN_ROOT", &root.to_string_lossy());
                let path = root.join(path);
                if path == default {
                    continue;
                }
                if let Some(value) = read_json(&root, &path, &mut seen, &mut diagnostics) {
                    documents.push(value);
                }
            } else if entry.is_object() {
                documents.push(entry);
            } else {
                diagnostics
                    .push("unsupported manifest hooks entry (expected path or object)".into());
            }
        }
    }
    let mut commands = Vec::new();
    for document in documents {
        let Some(events) = document.get("hooks").unwrap_or(&document).as_object() else {
            diagnostics.push("hooks must be an event object".into());
            continue;
        };
        for (event, groups) in events {
            if !matches!(
                event.as_str(),
                "SessionStart" | "SessionEnd" | "PreToolUse" | "PostToolUse"
            ) {
                diagnostics.push(format!("unsupported plugin hook event {event}"));
                continue;
            }
            let Some(groups) = groups.as_array() else {
                diagnostics.push(format!("{event}: groups must be an array"));
                continue;
            };
            for group in groups {
                let matcher = match group.get("matcher") {
                    None => None,
                    Some(Value::String(s)) if s.is_empty() || s == "*" => None,
                    Some(Value::String(s))
                        if matches!(event.as_str(), "PreToolUse" | "PostToolUse") =>
                    {
                        match regex::Regex::new(s) {
                            Ok(r) => Some(r),
                            Err(e) => {
                                diagnostics.push(format!("invalid {event} matcher: {e}"));
                                continue;
                            }
                        }
                    }
                    _ => {
                        diagnostics.push(format!("unsupported {event} matcher"));
                        continue;
                    }
                };
                let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                    diagnostics.push(format!("{event}: missing hooks array"));
                    continue;
                };
                for hook in hooks {
                    if hook.get("type").and_then(Value::as_str) != Some("command")
                        || hook.get("async").and_then(Value::as_bool) == Some(true)
                    {
                        diagnostics.push(format!(
                            "{event}: only synchronous command hooks are supported"
                        ));
                        continue;
                    }
                    let Some(command) = hook
                        .get("command")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                    else {
                        diagnostics.push(format!("{event}: missing command"));
                        continue;
                    };
                    let args = match hook.get("args") {
                        None => None,
                        Some(value) => match serde_json::from_value::<Vec<String>>(value.clone()) {
                            Ok(args) => Some(args),
                            Err(error) => {
                                diagnostics
                                    .push(format!("{event}: args must be a string array: {error}"));
                                continue;
                            }
                        },
                    };
                    let timeout = match hook.get("timeout") {
                        // Until Claude's version-specific default is known, use
                        // jcode's existing bounded hook policy, not a guessed value.
                        None => {
                            crate::config::config().hooks.pre_tool_timeout_ms.max(1) as f64 / 1000.0
                        }
                        Some(v) => match v
                            .as_f64()
                            .filter(|n| n.is_finite() && *n > 0.0 && *n <= 3600.0)
                        {
                            Some(n) => n,
                            None => {
                                diagnostics.push(format!("{event}: invalid timeout"));
                                continue;
                            }
                        },
                    };
                    commands.push(CommandHook {
                        event: event.clone(),
                        command: command.into(),
                        args,
                        root: root.clone(),
                        plugin: None,
                        matcher: matcher.clone(),
                        timeout: Duration::from_secs_f64(timeout),
                    });
                }
            }
        }
    }
    (commands, diagnostics)
}

fn load_plugin(plugin: &crate::plugin::PluginRoot) -> (Vec<CommandHook>, Vec<String>) {
    let (mut hooks, diagnostics) = load_root(&plugin.root);
    for hook in &mut hooks {
        hook.plugin = Some(plugin.clone());
    }
    (hooks, diagnostics)
}

#[cfg(test)]
fn configured(event: &str, cwd: &str) -> bool {
    !commands_for_dir(event, Some(cwd)).is_empty()
}

pub(super) fn commands(event: &HookEvent) -> Vec<CommandHook> {
    commands_for_dir(event.event, event.cwd.as_deref())
}

pub(super) fn commands_for_dir(event: &str, cwd: Option<&str>) -> Vec<CommandHook> {
    if super::hooks_suppressed() || !supported(event) {
        return Vec::new();
    }
    // Never guess the daemon's cwd for a project-scoped event.
    let Some(cwd) = cwd else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for plugin in crate::plugin::enabled_plugins_for_dir(Path::new(cwd)) {
        let (hooks, diagnostics) = load_plugin(&plugin);
        for diagnostic in diagnostics {
            crate::logging::warn(&format!("Plugin {} hooks: {diagnostic}", plugin.name));
        }
        commands.extend(
            hooks
                .into_iter()
                .filter(|h| Some(h.event.as_str()) == claude_event(event)),
        );
    }
    commands
}

fn enabled(hook: &CommandHook, event: &HookEvent) -> bool {
    if super::hooks_suppressed() {
        return false;
    }
    let Some(plugin) = &hook.plugin else {
        return true;
    };
    let Some(cwd) = event.cwd.as_deref() else {
        return false;
    };
    crate::plugin::enabled_plugins_for_dir(Path::new(cwd))
        .iter()
        .any(|root| {
            root.name == plugin.name
                && root.marketplace == plugin.marketplace
                && root.scope == plugin.scope
                && root.root == plugin.root
        })
}

fn payload(event: &HookEvent, input: Option<&str>) -> Value {
    let mut value = json!({"hook_event_name": claude_event(event.event), "session_id": event.session_id, "cwd": event.cwd});
    for (key, text) in &event.fields {
        let key = key.to_ascii_lowercase();
        value[&key] = if key == "tool_input" {
            serde_json::from_str(text).unwrap_or(Value::Null)
        } else {
            Value::String(text.clone())
        };
    }
    if let Some(input) = input {
        value["tool_input"] = serde_json::from_str(input).unwrap_or(Value::Null);
    }
    if let Some((input, response)) = &event.plugin_tool_payload {
        value["tool_input"] = input.clone();
        value["tool_response"] = response.clone();
    }
    if let Some(tool) = value["tool_name"].as_str() {
        value["tool_name"] = Value::String(claude_tool_name(tool).to_owned());
    }
    value
}

fn claude_tool_name(tool: &str) -> &str {
    match tool {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "webfetch" => "WebFetch",
        _ => tool,
    }
}

fn expand_plugin_paths(text: &str, root: &Path, data: Option<&Path>) -> Option<String> {
    static VARIABLES: std::sync::LazyLock<Result<regex::Regex, regex::Error>> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\$\{CLAUDE_PLUGIN_(?:ROOT|DATA)\}|\$CLAUDE_PLUGIN_(?:ROOT|DATA)\b")
        });
    let variables = match &*VARIABLES {
        Ok(variables) => variables,
        Err(error) => {
            crate::logging::warn(&format!("Cannot expand plugin hook paths: {error}"));
            return None;
        }
    };
    // Substitute once. A path containing '$' or '${...}' is literal data, not
    // another expansion pass, and direct-exec arguments are never shell parsed.
    Some(
        variables
            .replace_all(text, |captures: &regex::Captures<'_>| match &captures[0] {
                "${CLAUDE_PLUGIN_ROOT}" | "$CLAUDE_PLUGIN_ROOT" => {
                    root.to_string_lossy().into_owned()
                }
                _ => data
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| captures[0].to_owned()),
            })
            .into_owned(),
    )
}

async fn execute(
    hook: &CommandHook,
    event: &HookEvent,
    input: Option<&str>,
) -> Option<super::sync_runner::RunOutput> {
    if !enabled(hook, event) {
        return None;
    }
    let tool = event
        .fields
        .iter()
        .find(|(key, _)| *key == "TOOL_NAME")
        .map(|(_, value)| value.as_str())
        .unwrap_or("");
    if hook
        .matcher
        .as_ref()
        .is_some_and(|m| !m.is_match(tool) && !m.is_match(claude_tool_name(tool)))
    {
        return None;
    }
    let data = match hook
        .plugin
        .as_ref()
        .map(crate::plugin::prepare_data_dir)
        .transpose()
    {
        Ok(data) => data,
        Err(error) => {
            crate::logging::warn(&format!("Plugin {} data directory: {error:#}", hook.event));
            return None;
        }
    };
    let mut cmd = if let Some(args) = &hook.args {
        let mut cmd = std::process::Command::new(expand_plugin_paths(
            &hook.command,
            &hook.root,
            data.as_deref(),
        )?);
        for arg in args {
            cmd.arg(expand_plugin_paths(arg, &hook.root, data.as_deref())?);
        }
        cmd
    } else {
        #[cfg(unix)]
        {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.arg("-c").arg(&hook.command);
            cmd
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut cmd = std::process::Command::new("cmd.exe");
            cmd.args(["/D", "/S", "/C"])
                .raw_arg(format!("\"{}\"", hook.command));
            cmd
        }
    };
    super::apply_event_env(&mut cmd, event);
    cmd.env("CLAUDE_PLUGIN_ROOT", &hook.root);
    if let Some(cwd) = &event.cwd {
        cmd.current_dir(cwd);
    }
    super::apply_current_client_terminal_env(&mut cmd);
    if let Some(data) = &data {
        // Override a daemon/terminal's stale value with this hook's identity.
        cmd.env("CLAUDE_PLUGIN_DATA", data);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let stdin = payload(event, input).to_string();
    let stderr = if event.event == "pre_tool" {
        super::sync_runner::StderrMode::Capture
    } else {
        super::sync_runner::StderrMode::Discard
    };
    match super::sync_runner::run_command(cmd, stdin.as_bytes(), hook.timeout, stderr).await {
        Ok(output) => Some(output),
        Err(error) => {
            crate::logging::warn(&format!("Plugin hook {} {error}", hook.event));
            None
        }
    }
}

pub(super) async fn gate_commands(
    hooks: Vec<CommandHook>,
    event: &HookEvent,
    input: &str,
) -> GateOutcome {
    let mut current = input.to_owned();
    let mut outcome = GateOutcome::allow();
    for hook in hooks {
        let Some(output) = execute(&hook, event, Some(&current)).await else {
            continue;
        };
        if output.stdout_truncated {
            outcome.decision = GateDecision::Block {
                reason: "plugin hook stdout exceeded 1 MiB; refusing truncated gate output".into(),
            };
            outcome.updated_input = None;
            return outcome;
        }
        let verdict = if output.status.code() == Some(2) {
            super::CommandVerdict::block(
                super::truncate_bytes(
                    String::from_utf8_lossy(&output.stderr).trim(),
                    super::BLOCK_REASON_LIMIT,
                )
                .into(),
            )
        } else if output.status.success() {
            let value = match serde_json::from_slice::<Value>(&output.stdout) {
                Ok(value) => value,
                Err(error) => {
                    if output.stdout.starts_with(b"{") {
                        crate::logging::warn(&format!(
                            "Plugin PreToolUse hook printed invalid JSON: {error}"
                        ));
                    }
                    continue;
                }
            };
            if value["continue"] == false {
                super::CommandVerdict::block(
                    super::truncate_bytes(
                        value["stopReason"]
                            .as_str()
                            .unwrap_or("blocked by plugin hook"),
                        super::BLOCK_REASON_LIMIT,
                    )
                    .into(),
                )
            } else if value["hookSpecificOutput"]
                .get("updatedInput")
                .is_some_and(|input| !input.is_object())
            {
                super::CommandVerdict::block(
                    "plugin hook updatedInput must be an object; refusing unapplied rewrite".into(),
                )
            } else {
                super::parse_gate_value(&value)
            }
        } else {
            crate::logging::warn(&format!(
                "Plugin PreToolUse hook exited with {:?}; allowing tool call",
                output.status.code()
            ));
            continue;
        };
        if let Some(reason) = verdict.block {
            outcome.decision = GateDecision::Block { reason };
            outcome.updated_input = None;
            return outcome;
        }
        if let Some(updated) = verdict.updated_input {
            current = updated.to_string();
            outcome.updated_input = Some(updated);
        }
        if let Some(context) = verdict.context {
            outcome.additional_context.push(context);
        }
    }
    outcome
}

mod observers;

pub(super) fn observe(event: &HookEvent, hooks: Vec<CommandHook>) {
    observers::observe(event, hooks);
}

#[cfg(all(test, unix))]
#[path = "plugin_hooks_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "plugin_hooks_tests_fixture.rs"]
mod test_fixture;
