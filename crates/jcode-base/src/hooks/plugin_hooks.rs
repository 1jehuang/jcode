//! Claude plugin command hooks. Discovery only reads trusted, enabled roots.
//!
//! SessionStart/SessionEnd are observers; PreToolUse is a gate/rewriter;
//! PostToolUse observes successful tools with full jcode input and response.
//! Stop and prompt/agent/http/async hooks require lifecycle semantics jcode
//! does not expose here and are diagnosed, never silently remapped. Observer
//! output cannot inject context. Tool input schemas remain jcode's schemas.
use super::{GateDecision, HookEvent};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
struct CommandHook {
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
        if root.join("hooks/hooks.json").canonicalize().ok().as_ref() != Some(&canonical) {
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
    if default.is_file() {
        if let Some(value) = read_json(&root, &default, &mut seen, &mut diagnostics) {
            documents.push(value);
        }
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

pub(super) fn configured(event: &str, cwd: &str) -> bool {
    let name = match event {
        "session_start" => "session_start",
        "session_end" => "session_end",
        "pre_tool" => "pre_tool",
        "post_tool" => "post_tool",
        _ => return false,
    };
    !commands(&HookEvent::new(name).cwd(cwd)).is_empty()
}

fn commands(event: &HookEvent) -> Vec<CommandHook> {
    if super::hooks_suppressed() || !supported(event.event) {
        return Vec::new();
    }
    // Never guess the daemon's cwd for a project-scoped event.
    let Some(cwd) = event.cwd.as_deref() else {
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
                .filter(|h| Some(h.event.as_str()) == claude_event(event.event)),
        );
    }
    commands
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

fn expand_plugin_paths(text: &str, root: &Path, data: Option<&Path>) -> String {
    static VARIABLES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\$\{CLAUDE_PLUGIN_(?:ROOT|DATA)\}|\$CLAUDE_PLUGIN_(?:ROOT|DATA)\b")
            .expect("valid plugin variable pattern")
    });
    // Substitute once. A path containing '$' or '${...}' is literal data, not
    // another expansion pass, and direct-exec arguments are never shell parsed.
    VARIABLES
        .replace_all(text, |captures: &regex::Captures<'_>| match &captures[0] {
            "${CLAUDE_PLUGIN_ROOT}" | "$CLAUDE_PLUGIN_ROOT" => root.to_string_lossy().into_owned(),
            _ => data
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| captures[0].to_owned()),
        })
        .into_owned()
}

async fn execute(
    hook: &CommandHook,
    event: &HookEvent,
    input: Option<&str>,
) -> Option<std::process::Output> {
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
        ));
        cmd.args(
            args.iter()
                .map(|arg| expand_plugin_paths(arg, &hook.root, data.as_deref())),
        );
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
            let mut cmd = std::process::Command::new("cmd.exe");
            cmd.arg("/D").arg("/S").arg("/C").arg(&hook.command);
            cmd
        }
    };
    super::apply_event_env(&mut cmd, event);
    cmd.env("CLAUDE_PLUGIN_ROOT", &hook.root);
    if let Some(cwd) = &event.cwd {
        cmd.current_dir(cwd);
    }
    let _ = super::CLIENT_TERMINAL_ENV.try_with(|env| {
        crate::terminal_launch::apply_client_terminal_env(&mut cmd, env);
    });
    if let Some(data) = &data {
        // Override a daemon/terminal's stale value with this hook's identity.
        cmd.env("CLAUDE_PLUGIN_DATA", data);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let result = tokio::time::timeout(hook.timeout, async {
        let mut child = cmd.spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin
                .write_all(payload(event, input).to_string().as_bytes())
                .await;
        }
        child.wait_with_output().await
    })
    .await;
    match result {
        Ok(Ok(output)) => Some(output),
        other => {
            crate::logging::warn(&format!("Plugin hook {} failed: {other:?}", hook.event));
            None
        }
    }
}

pub(super) async fn gate(event: &HookEvent, input: &str) -> GateDecision {
    gate_commands(commands(event), event, input).await
}

async fn gate_commands(hooks: Vec<CommandHook>, event: &HookEvent, input: &str) -> GateDecision {
    let mut current = input.to_owned();
    let mut rewritten = None;
    for hook in hooks {
        if let Some(output) = execute(&hook, event, Some(&current)).await {
            if output.status.code() == Some(2) {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return GateDecision::Block {
                    reason: super::truncate_bytes(stderr.trim(), super::BLOCK_REASON_LIMIT)
                        .to_owned(),
                };
            }
            if output.status.success() {
                if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) {
                    if value["decision"] == "block" || value["continue"] == false {
                        return GateDecision::Block {
                            reason: super::truncate_bytes(
                                value["reason"]
                                    .as_str()
                                    .or_else(|| value["stopReason"].as_str())
                                    .unwrap_or("blocked by plugin hook"),
                                super::BLOCK_REASON_LIMIT,
                            )
                            .to_owned(),
                        };
                    }
                    let specific = &value["hookSpecificOutput"];
                    if specific["permissionDecision"] == "deny" {
                        return GateDecision::Block {
                            reason: super::truncate_bytes(
                                specific["permissionDecisionReason"]
                                    .as_str()
                                    .unwrap_or("blocked by plugin hook"),
                                super::BLOCK_REASON_LIMIT,
                            )
                            .to_owned(),
                        };
                    }
                    if specific["permissionDecision"] == "ask" {
                        // There is no interactive permission prompt at this seam.
                        // Do not silently convert an explicit ask to permission.
                        return GateDecision::Block {
                            reason: specific["permissionDecisionReason"]
                                .as_str()
                                .unwrap_or(
                                    "plugin hook requires user approval (ask is not supported)",
                                )
                                .to_owned(),
                        };
                    }
                    if let Some(input) = specific.get("updatedInput") {
                        if input.is_object() {
                            current = input.to_string();
                            rewritten = Some(input.clone());
                        } else {
                            return GateDecision::Block {
                                reason: "plugin hook updatedInput must be an object; refusing unapplied rewrite".into(),
                            };
                        }
                    }
                }
            }
        }
    }
    rewritten.map_or(GateDecision::Allow, |input| GateDecision::Rewrite { input })
}

pub(super) fn observe(event: &HookEvent) {
    // Claude's PostToolUse excludes failed tools; jcode's native observer does not.
    if event.event == "post_tool"
        && event
            .fields
            .iter()
            .any(|(key, value)| *key == "STATUS" && value != "ok")
    {
        return;
    }
    let hooks = commands(event);
    if hooks.is_empty() {
        return;
    }
    let event = event.clone();
    let terminal_env = super::CLIENT_TERMINAL_ENV
        .try_with(Clone::clone)
        .unwrap_or_default();
    // A bounded runner also works for session teardown outside a Tokio runtime.
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(super::with_client_terminal_env(terminal_env, async {
                for hook in hooks {
                    if let Some(output) = execute(&hook, &event, None).await {
                        if output.status.code() == Some(2) || !output.stdout.is_empty() {
                            crate::logging::warn(&format!("Plugin {} observer output cannot gate or inject context; output ignored", hook.event));
                        }
                    }
                }
            })),
            Err(error) => crate::logging::warn(&format!("Plugin hook runtime: {error}")),
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fixture(document: Value) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("hooks")).unwrap();
        std::fs::write(temp.path().join("hooks/hooks.json"), document.to_string()).unwrap();
        temp
    }

    #[tokio::test]
    async fn plugin_data_discovery_is_inert_and_runtime_env_and_argv_are_qualified() {
        use crate::plugin::PluginScope;
        use crate::plugin_components::fixture::{Fixture, write};
        let f = Fixture::new();
        let installed = f
            .manager
            .install("registry-fixture", PluginScope::User)
            .unwrap();
        write(&installed.root.join("hooks/hooks.json"), &json!({"hooks":{"PreToolUse":[{
            "matcher":"^Bash$", "hooks":[
                {"type":"command", "command":"printf '%s' \"$CLAUDE_PLUGIN_DATA\""},
                {"type":"command", "command":"/bin/echo", "args":["${CLAUDE_PLUGIN_DATA}/a file; $(touch should-not-exist)"]}
            ]
        }]}}).to_string());
        let root = f.manager.enabled_roots().unwrap().remove(0);
        let data = crate::plugin::data_path(&root).unwrap();
        let event = HookEvent::new("pre_tool")
            .cwd(f.project.to_string_lossy())
            .field("TOOL_NAME", "bash");
        assert!(configured("pre_tool", f.project.to_str().unwrap()));
        let hooks = commands(&event);
        assert_eq!(hooks.len(), 2);
        assert!(!data.exists(), "listing must not prepare data");
        let unmatched = HookEvent::new("pre_tool")
            .cwd(f.project.to_string_lossy())
            .field("TOOL_NAME", "read");
        assert!(execute(&hooks[0], &unmatched, Some("{}")).await.is_none());
        assert!(!data.exists(), "a nonmatching hook must not prepare data");

        struct ParentData(Option<std::ffi::OsString>);
        impl Drop for ParentData {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => crate::env::set_var("CLAUDE_PLUGIN_DATA", value),
                    None => crate::env::remove_var("CLAUDE_PLUGIN_DATA"),
                }
            }
        }
        let _parent = ParentData(std::env::var_os("CLAUDE_PLUGIN_DATA"));
        crate::env::set_var("CLAUDE_PLUGIN_DATA", "/wrong-parent-data");
        let output = execute(&hooks[0], &event, Some("{}")).await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            data.to_string_lossy()
        );
        assert!(data.is_dir());
        let output = execute(&hooks[1], &event, Some("{}")).await.unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}/a file; $(touch should-not-exist)\n", data.display())
        );
        assert!(!f.project.join("should-not-exist").exists());
    }

    #[tokio::test]
    async fn unsafe_plugin_data_directory_prevents_hook_spawn() {
        use crate::plugin::PluginScope;
        use crate::plugin_components::fixture::{Fixture, write};
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let installed = f
            .manager
            .install("registry-fixture", PluginScope::User)
            .unwrap();
        write(
            &installed.root.join("hooks/hooks.json"),
            &json!({"hooks":{"PreToolUse":[{"hooks":[{
                "type":"command", "command":"touch \"$CLAUDE_PLUGIN_ROOT/should-not-run\""
            }]}]}})
            .to_string(),
        );
        let root = f.manager.enabled_roots().unwrap().remove(0);
        let data = crate::plugin::data_path(&root).unwrap();
        std::fs::create_dir_all(data.parent().unwrap()).unwrap();
        symlink(&f.other, &data).unwrap();
        let (hooks, diagnostics) = load_plugin(&root);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let event = HookEvent::new("pre_tool").cwd(f.project.to_string_lossy());
        assert!(execute(&hooks[0], &event, Some("{}")).await.is_none());
        assert!(!root.root.join("should-not-run").exists());
        assert!(std::fs::read_dir(&f.other).unwrap().next().is_none());
    }

    #[test]
    fn argv_plugin_paths_are_one_pass_and_do_not_expand_prefix_variables() {
        let root = Path::new("/code/${CLAUDE_PLUGIN_DATA}");
        let data = Path::new("/data/$CLAUDE_PLUGIN_ROOT");
        assert_eq!(
            expand_plugin_paths(
                "${CLAUDE_PLUGIN_ROOT}:$CLAUDE_PLUGIN_DATA:$CLAUDE_PLUGIN_DATA_EXTRA",
                root,
                Some(data)
            ),
            "/code/${CLAUDE_PLUGIN_DATA}:/data/$CLAUDE_PLUGIN_ROOT:$CLAUDE_PLUGIN_DATA_EXTRA"
        );
    }

    #[tokio::test]
    async fn discovery_does_not_execute_and_gate_blocks_with_json_stdin() {
        let temp = fixture(json!({"hooks":{"PreToolUse":[{"matcher":"^bash$","hooks":[{
            "type":"command", "command":"cat > \"$CLAUDE_PLUGIN_ROOT/payload.json\"; printf denied >&2; exit 2"
        }]}]}}));
        let (hooks, diagnostics) = load_root(temp.path());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(!temp.path().join("payload.json").exists());
        let event = HookEvent::new("pre_tool")
            .cwd(temp.path().to_string_lossy())
            .session_id("test")
            .field("TOOL_NAME", "bash");
        assert_eq!(
            gate_commands(hooks, &event, r#"{"command":"echo hello"}"#).await,
            GateDecision::Block {
                reason: "denied".into()
            }
        );
        let payload: Value =
            serde_json::from_slice(&std::fs::read(temp.path().join("payload.json")).unwrap())
                .unwrap();
        assert_eq!(payload["hook_event_name"], "PreToolUse");
        assert_eq!(payload["tool_input"]["command"], "echo hello");
        assert_eq!(payload["session_id"], "test");
    }

    #[tokio::test]
    async fn nonmatching_tool_does_not_execute_and_timeout_fails_open() {
        let temp = fixture(
            json!({"hooks":{"PreToolUse":[{"matcher":"^write$","hooks":[{
            "type":"command", "command":"touch \"$CLAUDE_PLUGIN_ROOT/ran\"; exit 2"
        }]},{"hooks":[{"type":"command","command":"exec sleep 10","timeout":0.03}]}]}}),
        );
        let (hooks, diagnostics) = load_root(temp.path());
        assert!(diagnostics.is_empty());
        let event = HookEvent::new("pre_tool")
            .cwd(temp.path().to_string_lossy())
            .field("TOOL_NAME", "read");
        let start = std::time::Instant::now();
        assert_eq!(
            gate_commands(
                hooks,
                &event,
                &json!({"data":"x".repeat(1024 * 1024)}).to_string()
            )
            .await,
            GateDecision::Allow
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(!temp.path().join("ran").exists());
    }

    #[tokio::test]
    async fn argv_hooks_preserve_literal_arguments_and_expand_plugin_root() {
        use std::os::unix::fs::PermissionsExt;
        let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{
            "type":"command", "command":"${CLAUDE_PLUGIN_ROOT}/runner with spaces.sh",
            "args":["${CLAUDE_PLUGIN_ROOT}/a file; $(touch should-not-exist)"]
        }]}]}}));
        let script = temp.path().join("runner with spaces.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s' \"$1\" > \"$CLAUDE_PLUGIN_ROOT/argv.txt\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (hooks, diagnostics) = load_root(temp.path());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let event = HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy());
        assert_eq!(
            gate_commands(hooks, &event, "{}").await,
            GateDecision::Allow
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("argv.txt")).unwrap(),
            format!(
                "{}/a file; $(touch should-not-exist)",
                temp.path().canonicalize().unwrap().display()
            )
        );
        assert!(!temp.path().join("should-not-exist").exists());
    }

    #[tokio::test]
    async fn updated_input_is_passed_to_later_hooks_and_returned_to_executor() {
        let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[
            {"type":"command","command":"printf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"command\":\"rewritten\"}}}'"},
            {"type":"command","command":"cat > \"$CLAUDE_PLUGIN_ROOT/second.json\"; printf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"command\":\"final\"}}}'"}
        ]}]}}));
        let (hooks, diagnostics) = load_root(temp.path());
        assert!(diagnostics.is_empty());
        let event = HookEvent::new("pre_tool")
            .cwd(temp.path().to_string_lossy())
            .field("TOOL_NAME", "bash");
        assert_eq!(
            gate_commands(hooks, &event, r#"{"command":"original"}"#).await,
            GateDecision::Rewrite {
                input: json!({"command":"final"})
            }
        );
        let payload: Value =
            serde_json::from_slice(&std::fs::read(temp.path().join("second.json")).unwrap())
                .unwrap();
        assert_eq!(payload["tool_input"], json!({"command":"rewritten"}));
    }

    #[tokio::test]
    async fn invalid_rewrites_and_explicit_blocks_never_allow_execution() {
        for output in [
            json!({"hookSpecificOutput":{"updatedInput":"invalid"}}),
            json!({"decision":"block","reason":"legacy policy"}),
            json!({"continue":false,"stopReason":"stop policy"}),
        ] {
            let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{
                "type":"command", "command":format!("printf '%s' '{}'", output)
            }]}]}}));
            let (hooks, diagnostics) = load_root(temp.path());
            assert!(diagnostics.is_empty());
            assert!(matches!(
                gate_commands(
                    hooks,
                    &HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy()),
                    "{}"
                )
                .await,
                GateDecision::Block { .. }
            ));
        }
    }

    #[tokio::test]
    async fn manifest_inline_and_path_hooks_execute_shell_root_expansion() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".claude-plugin")).unwrap();
        std::fs::write(temp.path().join("extra.json"), json!({"hooks":{"SessionStart":[{"hooks":[{
            "type":"command", "command":"printf '%s' \"${CLAUDE_PLUGIN_ROOT}\" > \"$CLAUDE_PLUGIN_ROOT/root.txt\""
        }]}]}}).to_string()).unwrap();
        std::fs::write(temp.path().join(".claude-plugin/plugin.json"), json!({"hooks":["./extra.json",{"hooks":{"PreToolUse":[{"hooks":[{
            "type":"command", "command":"printf '%s' '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"policy\"}}'"
        }]}]}}]}).to_string()).unwrap();
        let (hooks, diagnostics) = load_root(temp.path());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let event = HookEvent::new("session_start").cwd(temp.path().to_string_lossy());
        assert!(
            execute(&hooks[0], &event, None)
                .await
                .unwrap()
                .status
                .success()
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("root.txt")).unwrap(),
            temp.path().canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(
            gate_commands(
                vec![hooks[1].clone()],
                &HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy()),
                "{}"
            )
            .await,
            GateDecision::Block {
                reason: "policy".into()
            }
        );
    }

    #[test]
    fn unsupported_constructs_are_diagnosed_not_executed() {
        let temp = fixture(json!({"hooks":{
            "Stop":[{"hooks":[{"type":"command","command":"exit 2"}]}],
            "PreToolUse":[{"hooks":[{"type":"prompt","prompt":"deny"},{"type":"command","command":"exit 2","async":true}]}]
        }}));
        let (hooks, diagnostics) = load_root(temp.path());
        assert!(hooks.is_empty());
        assert_eq!(diagnostics.len(), 3);
        assert!(diagnostics.iter().any(|d| d.contains("Stop")));
    }

    #[test]
    fn external_documents_are_rejected_and_canonical_aliases_deduplicated() {
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("external.json");
        std::fs::write(
            &outside_file,
            json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"exit 2"}]}]}})
                .to_string(),
        )
        .unwrap();
        let temp = fixture(
            json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"exit 0"}]}]}}),
        );
        std::fs::create_dir(temp.path().join(".claude-plugin")).unwrap();
        std::os::unix::fs::symlink(&outside_file, temp.path().join("external-link.json")).unwrap();
        std::fs::write(temp.path().join(".claude-plugin/plugin.json"), json!({"hooks":[
            "./hooks/hooks.json", "${CLAUDE_PLUGIN_ROOT}/hooks/hooks.json", "./hooks/../hooks/hooks.json",
            outside_file, "./external-link.json", "../external.json"
        ]}).to_string()).unwrap();
        let (hooks, diagnostics) = load_root(temp.path());
        assert_eq!(hooks.len(), 1);
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.contains("escapes plugin root"))
                .count(),
            2
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn public_dispatch_honors_enabled_scope_and_recursion_guard() {
        let _lock = crate::storage::lock_test_env();
        struct EnvReset(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for EnvReset {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    match value {
                        Some(value) => crate::env::set_var(key, value),
                        None => crate::env::remove_var(key),
                    }
                }
            }
        }
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let keys = ["JCODE_HOME", "JCODE_HOOKS_DISABLED", "JCODE_HOOK_PRE_TOOL"];
        let _env = EnvReset(
            keys.into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        crate::env::set_var("JCODE_HOME", home.path());
        crate::env::remove_var("JCODE_HOOKS_DISABLED");
        crate::env::remove_var("JCODE_HOOK_PRE_TOOL");
        let root = home.path().join("plugins/cache/test");
        std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        std::fs::create_dir(root.join("hooks")).unwrap();
        std::fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"name":"test"}"#,
        )
        .unwrap();
        std::fs::write(root.join("hooks/hooks.json"), json!({"hooks":{
            "PreToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"echo scoped >&2; exit 2"}]}],
            "SessionStart":[{"hooks":[{"type":"command","command":"cat > \"$CLAUDE_PLUGIN_ROOT/session.json\""}]}]
        }}).to_string()).unwrap();
        let registry = home.path().join("plugins/registry.json");
        let write_registry = |enabled| {
            std::fs::write(&registry, json!({"installed":[{
            "name":"test", "marketplace":"test", "root":root, "scope":"project", "enabled":enabled,
            "project":project.path().canonicalize().unwrap()
        }]}).to_string()).unwrap()
        };
        write_registry(true);
        let cwd = project.path().to_str().unwrap();
        assert!(super::super::hook_configured_for_dir("pre_tool", Some(cwd)));
        assert!(!root.join("session.json").exists());
        assert_eq!(
            super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await,
            GateDecision::Block {
                reason: "scoped".into()
            }
        );
        assert_eq!(
            super::super::run_pre_tool_gate(
                "test",
                Some(other.path().to_str().unwrap()),
                "bash",
                "{}"
            )
            .await,
            GateDecision::Allow
        );
        super::super::dispatch_observer(
            HookEvent::new("session_start")
                .session_id("observed")
                .cwd(cwd),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let observed: Value = loop {
            let result = std::fs::read(root.join("session.json"))
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
                });
            match result {
                Ok(value) => break value,
                Err(error) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "observer did not publish complete JSON before deadline: {error}"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        };
        assert_eq!(observed["session_id"], "observed");
        // A plugin rewrite reaches the executor, but never bypasses a native
        // policy gate. Native gates must see the rewritten request too.
        std::fs::write(root.join("hooks/hooks.json"), json!({"hooks":{"PreToolUse":[{"hooks":[{
            "type":"command", "command":"printf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"command\":\"changed\"}}}'"
        }]}]}}).to_string()).unwrap();
        let native = home.path().join("native.sh");
        std::fs::write(
            &native,
            format!(
                "#!/bin/sh\ncat > '{}'\necho native-policy >&2\nexit 2\n",
                home.path().join("native-input.json").display()
            ),
        )
        .unwrap();
        crate::env::set_var(
            "JCODE_HOOK_PRE_TOOL",
            format!("/bin/sh '{}'", native.display()),
        );
        assert_eq!(
            super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await,
            GateDecision::Block {
                reason: "native-policy".into()
            }
        );
        let native_input: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("native-input.json")).unwrap())
                .unwrap();
        assert_eq!(native_input, json!({"command":"changed"}));
        crate::env::remove_var("JCODE_HOOK_PRE_TOOL");
        assert_eq!(
            super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await,
            GateDecision::Rewrite {
                input: json!({"command":"changed"})
            }
        );
        std::fs::write(root.join("hooks/hooks.json"), json!({"hooks":{"PreToolUse":[{"hooks":[{
            "type":"command", "command":"printf '%s' '{\"hookSpecificOutput\":{\"permissionDecision\":\"ask\"}}'"
        }]}]}}).to_string()).unwrap();
        assert!(
            matches!(super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await,
            GateDecision::Block { reason } if reason.contains("ask is not supported"))
        );
        write_registry(false);
        assert_eq!(
            super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await,
            GateDecision::Allow
        );
        write_registry(true);
        crate::env::set_var("JCODE_HOOKS_DISABLED", "1");
        assert!(!super::super::hook_configured_for_dir(
            "pre_tool",
            Some(cwd)
        ));
        assert_eq!(
            super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await,
            GateDecision::Allow
        );
    }
}
