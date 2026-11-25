use super::*;

fn fixture(document: Value) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("hooks")).unwrap();
    std::fs::write(temp.path().join("hooks/hooks.json"), document.to_string()).unwrap();
    temp
}

#[tokio::test]
async fn plugin_data_discovery_is_inert_and_runtime_env_and_argv_are_qualified() {
    use self::fixture::{Fixture, write};
    use crate::plugin::PluginScope;
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
    use self::fixture::{Fixture, write};
    use crate::plugin::PluginScope;
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
        gate_commands(hooks, &event, r#"{"command":"echo hello"}"#)
            .await
            .decision,
        GateDecision::Block {
            reason: "denied".into()
        }
    );
    let payload: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("payload.json")).unwrap()).unwrap();
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
        .await
        .decision,
        GateDecision::Allow
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!temp.path().join("ran").exists());
}

#[tokio::test]
async fn gate_reads_a_hook_that_logs_before_reading_a_large_request() {
    let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command",
        "command":"head -c 200000 /dev/zero >&2; cat >/dev/null; exit 2","timeout":10}]}]}}));
    let (hooks, _) = load_root(temp.path());
    let event = HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy());
    let input = json!({"data":"x".repeat(1024 * 1024)}).to_string();
    assert!(matches!(
        gate_commands(hooks, &event, &input).await.decision,
        GateDecision::Block { .. }
    ));
}

#[tokio::test]
async fn hook_timeout_stops_the_hooks_descendants() {
    let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command",
        "command":"sleep 30 & echo $! > \"$CLAUDE_PLUGIN_ROOT/child.pid\"; wait","timeout":0.5}]}]}}));
    let (hooks, _) = load_root(temp.path());
    let event = HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy());
    assert_eq!(
        gate_commands(hooks, &event, "{}").await.decision,
        GateDecision::Allow
    );
    let pid: u32 = std::fs::read_to_string(temp.path().join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::platform::is_process_running(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "hook child {pid} survived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
        gate_commands(hooks, &event, "{}").await.decision,
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
        gate_commands(hooks, &event, r#"{"command":"original"}"#)
            .await
            .updated_input,
        Some(json!({"command":"final"}))
    );
    let payload: Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("second.json")).unwrap()).unwrap();
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
            .await
            .decision,
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
        .await
        .decision,
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
    let root = home.path().join("plugin-catalogs/cache/test");
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
    let registry = home.path().join("plugin-catalogs/registry.json");
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
        super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}")
            .await
            .decision,
        GateDecision::Block {
            reason: "scoped".into()
        }
    );
    assert_eq!(
        super::super::run_pre_tool_gate("test", Some(other.path().to_str().unwrap()), "bash", "{}")
            .await
            .decision,
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
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()));
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
        super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}")
            .await
            .decision,
        GateDecision::Block {
            reason: "native-policy".into()
        }
    );
    let native_input: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("native-input.json")).unwrap())
            .unwrap();
    assert_eq!(native_input, json!({"command":"changed"}));
    crate::env::remove_var("JCODE_HOOK_PRE_TOOL");
    let outcome = super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await;
    assert_eq!(outcome.decision, GateDecision::Allow);
    assert_eq!(outcome.updated_input, Some(json!({"command":"changed"})));
    std::fs::write(root.join("hooks/hooks.json"), json!({"hooks":{"PreToolUse":[{"hooks":[{
        "type":"command", "command":"printf '%s' '{\"hookSpecificOutput\":{\"permissionDecision\":\"ask\"}}'"
    }]}]}}).to_string()).unwrap();
    assert!(
        matches!(super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}").await.decision,
        GateDecision::Block { reason } if reason.contains("requires user approval"))
    );
    write_registry(false);
    assert_eq!(
        super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}")
            .await
            .decision,
        GateDecision::Allow
    );
    write_registry(true);
    crate::env::set_var("JCODE_HOOKS_DISABLED", "1");
    assert!(!super::super::hook_configured_for_dir(
        "pre_tool",
        Some(cwd)
    ));
    assert_eq!(
        super::super::run_pre_tool_gate("test", Some(cwd), "bash", "{}")
            .await
            .decision,
        GateDecision::Allow
    );
}

#[path = "plugin_hooks_tests_fixture.rs"]
mod fixture;

#[tokio::test]
async fn hook_output_capture_is_bounded_for_both_streams() {
    for redirect in ["", ">&2"] {
        let command = format!("head -c 2097152 /dev/zero {redirect}");
        let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{
            "type":"command", "command":command, "timeout":5
        }]}]}}));
        let (hooks, _) = load_root(temp.path());
        let event = HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy());
        let output = execute(&hooks[0], &event, Some("{}")).await.unwrap();
        assert!(output.status.success());
        if redirect.is_empty() {
            assert_eq!(output.stdout.len(), 1024 * 1024);
            assert!(output.stdout_truncated);
        } else {
            assert_eq!(output.stderr.len(), 1024 * 1024);
            assert!(output.stdout.is_empty());
        }
    }
}

#[tokio::test]
async fn cancelling_a_hook_while_it_ignores_stdin_stops_its_descendants() {
    let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{
        "type":"command", "command":"sleep 30 & echo $! > \"$CLAUDE_PLUGIN_ROOT/child.pid\"; wait", "timeout":10
    }]}]}}));
    let (mut hooks, _) = load_root(temp.path());
    let hook = hooks.remove(0);
    let event = HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy());
    let task = tokio::spawn(async move {
        execute(
            &hook,
            &event,
            Some(&json!({"data":"x".repeat(1024 * 1024)}).to_string()),
        )
        .await
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(temp.path().join("child.pid")) {
            if let Ok(pid) = text.trim().parse() {
                break pid;
            }
        }
        assert!(std::time::Instant::now() < deadline, "hook did not start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_process_stops(pid).await;
}

async fn assert_process_stops(pid: u32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::platform::is_process_running(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "hook child {pid} survived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn truncated_gate_output_never_allows_an_unapplied_rewrite() {
    let temp = fixture(json!({"hooks":{"PreToolUse":[{"hooks":[{
        "type":"command", "command":"cat \"$CLAUDE_PLUGIN_ROOT/response.json\"", "timeout":5
    }]}]}}));
    std::fs::write(
        temp.path().join("response.json"),
        json!({
            "hookSpecificOutput":{"updatedInput":{"command":"x".repeat(2 * 1024 * 1024)}}
        })
        .to_string(),
    )
    .unwrap();
    let (hooks, _) = load_root(temp.path());
    let event = HookEvent::new("pre_tool").cwd(temp.path().to_string_lossy());
    assert!(matches!(
        gate_commands(hooks, &event, "{}").await.decision,
        GateDecision::Block { .. }
    ));
}
