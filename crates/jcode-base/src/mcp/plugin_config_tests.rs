use super::*;
use serde_json::json;
use std::path::{Path, PathBuf};

struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvRestore {
    fn isolate(home: &Path) -> Self {
        let snapshot = [
            "JCODE_HOME",
            "JCODE_DISABLE_CLAUDE_MCP",
            "CLAUDE_PLUGIN_ROOT",
        ]
        .map(|key| (key, std::env::var_os(key)));
        crate::env::set_var("JCODE_HOME", home);
        crate::env::set_var("JCODE_DISABLE_CLAUDE_MCP", "1");
        crate::env::set_var("CLAUDE_PLUGIN_ROOT", "/wrong-parent-environment");
        Self(snapshot.into())
    }
}
impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}
fn write_json(path: &Path, value: Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}
fn register(home: &Path, project: &Path, scope: &str, servers: Value) -> PathBuf {
    let root = home.join("plugins/cache/market/demo/v1");
    write_json(
        &root.join(".claude-plugin/plugin.json"),
        json!({"name":"demo"}),
    );
    write_json(&root.join(".mcp.json"), json!({"mcpServers":servers}));
    write_json(
        &home.join("plugins/registry.json"),
        json!({"installed":[{
            "name":"demo", "marketplace":"market", "root":root, "scope":scope,
            "enabled":true, "project":if scope == "user" { Value::Null } else { json!(project.canonicalize().unwrap()) }
        }]}),
    );
    // Prevent one-time legacy import from being part of these fixtures.
    write_json(
        &home.join("mcp.json"),
        json!({"mcpServers":{"ordinary":{"command":"ordinary-server", "enabled":false}}}),
    );
    root
}

#[test]
fn plugin_mcp_expands_intrinsic_root_once_in_all_fields() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    // Literal environment syntax in a filesystem name must stay literal.
    let home = temp.path().join("home-${DO_NOT_EXPAND}");
    let project = tempfile::tempdir().unwrap();
    let _env = EnvRestore::isolate(&home);
    let root = register(&home, project.path(), "user", json!({
        "stdio":{"command":"${CLAUDE_PLUGIN_ROOT}/bin/server", "args":["${CLAUDE_PLUGIN_ROOT}/arg", "${UNSET_TEST_VAR:-fallback}"],
            "env":{"CLAUDE_PLUGIN_ROOT":"/spoofed", "DATA":"${CLAUDE_PLUGIN_ROOT}/data"}},
        "remote":{"url":"https://example.invalid/${CLAUDE_PLUGIN_ROOT}", "headers":{"X-Root":"${CLAUDE_PLUGIN_ROOT}"}}
    })).canonicalize().unwrap();
    let config = McpConfig::load_for_dir(Some(project.path()));
    let root = root.to_string_lossy();
    let stdio = &config.servers["plugin:demo@market:stdio"];
    assert_eq!(stdio.command, format!("{root}/bin/server"));
    assert_eq!(stdio.args, [format!("{root}/arg"), "fallback".to_owned()]);
    assert_eq!(stdio.env["CLAUDE_PLUGIN_ROOT"], root);
    assert_eq!(stdio.env["DATA"], format!("{root}/data"));
    let remote = &config.servers["plugin:demo@market:remote"];
    assert_eq!(
        remote.url.as_deref(),
        Some(format!("https://example.invalid/{root}").as_str())
    );
    assert_eq!(remote.headers["X-Root"], root);
    assert!(stdio.is_enabled());
}

#[test]
fn plugin_mcp_project_and_local_scope_need_approval_and_cannot_self_enable() {
    let _lock = crate::storage::lock_test_env();
    for scope in ["project", "local"] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let _env = EnvRestore::isolate(home.path());
        register(
            home.path(),
            project.path(),
            scope,
            json!({"server":{"command":"server", "disabled":false}}),
        );
        let name = "plugin:demo@market:server";
        McpConfig::set_enabled_override(name, true).unwrap();
        let pending = McpConfig::load_for_dir(Some(project.path()));
        assert!(pending.pending_approval.contains(name));
        assert!(!pending.servers[name].is_enabled());
        assert!(pending.sources[name].starts_with(&format!("plugin:{scope}:")));
        assert!(!McpConfig::load_for_dir(None).servers.contains_key(name));
        assert!(
            !McpConfig::load_for_dir(Some(other.path()))
                .servers
                .contains_key(name)
        );
        McpConfig::set_project_approval(project.path(), name, McpApproval::Approve).unwrap();
        let approved = McpConfig::load_for_dir(Some(project.path()));
        assert!(approved.servers[name].is_enabled());
        assert!(!approved.pending_approval.contains(name));
        McpConfig::set_project_approval(project.path(), name, McpApproval::Reject).unwrap();
        let rejected = McpConfig::load_for_dir(Some(project.path()));
        assert!(!rejected.servers[name].is_enabled());
        assert!(!rejected.pending_approval.contains(name));
    }
}

#[test]
fn plugin_mcp_registry_disable_and_uninstall_remove_only_plugin_servers_on_reload() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let _env = EnvRestore::isolate(home.path());
    register(
        home.path(),
        project.path(),
        "user",
        json!({"server":{"command":"server"}}),
    );
    let name = "plugin:demo@market:server";
    let manager = crate::plugin::PluginManager::for_dir(project.path()).unwrap();
    assert!(McpConfig::load_for_dir(None).servers.contains_key(name));
    manager
        .disable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    let disabled = McpConfig::load_for_dir(Some(project.path()));
    assert!(!disabled.servers.contains_key(name));
    assert!(!disabled.sources.contains_key(name));
    assert!(disabled.servers.contains_key("ordinary"));
    manager
        .enable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    assert!(
        McpConfig::load_for_dir(Some(project.path()))
            .servers
            .contains_key(name)
    );
    McpConfig::set_enabled_override(name, false).unwrap();
    assert!(!McpConfig::load_for_dir(Some(project.path())).servers[name].is_enabled());
    manager
        .uninstall("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    let removed = McpConfig::load_for_dir(Some(project.path()));
    assert!(!removed.servers.contains_key(name));
    assert!(removed.servers.contains_key("ordinary"));
}

#[test]
fn plugin_mcp_explicit_config_wins_without_losing_its_source() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let _env = EnvRestore::isolate(home.path());
    register(
        home.path(),
        project.path(),
        "user",
        json!({"server":{"command":"plugin-command"}}),
    );
    let name = "plugin:demo@market:server";
    write_json(
        &home.path().join("mcp.json"),
        json!({"mcpServers":{name:{"command":"explicit-command"}}}),
    );
    let config = McpConfig::load_for_dir(Some(project.path()));
    assert_eq!(config.servers[name].command, "explicit-command");
    assert_eq!(config.sources[name], "~/.jcode/mcp.json");
}

#[tokio::test]
async fn plugin_mcp_targeted_reload_preserves_dynamic_entries_and_removes_disabled_plugin() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let _env = EnvRestore::isolate(home.path());
    register(
        home.path(),
        project.path(),
        "user",
        json!({"server":{"command":"not-executed", "enabled":false}}),
    );
    let name = "plugin:demo@market:server";
    let mut config = McpConfig::load_for_dir(None);
    let dynamic = config.servers["ordinary"].clone();
    config.servers.insert("dynamic".into(), dynamic.clone());
    config
        .sources
        .insert("dynamic".into(), "added this session".into());
    let mut manager = crate::mcp::McpManager::with_config(config);
    let plugins = crate::plugin::PluginManager::for_dir(project.path()).unwrap();
    plugins
        .disable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    assert_eq!(
        manager.reload_plugin_sources().await.unwrap(),
        (0, Vec::new())
    );
    assert!(!manager.config().servers.contains_key(name));
    assert!(!manager.config().sources.contains_key(name));
    assert!(manager.config().servers.contains_key("ordinary"));
    assert_eq!(manager.config().sources["dynamic"], "added this session");
    assert_eq!(
        serde_json::to_value(&manager.config().servers["dynamic"]).unwrap(),
        serde_json::to_value(dynamic).unwrap()
    );
    plugins
        .enable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    manager.reload_plugin_sources().await.unwrap();
    assert!(manager.config().servers.contains_key(name));
    assert!(!manager.config().servers[name].is_enabled());
    assert!(manager.connected_servers().await.is_empty());
    plugins
        .uninstall("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    manager.reload_plugin_sources().await.unwrap();
    assert!(!manager.config().servers.contains_key(name));
    assert!(manager.config().servers.contains_key("dynamic"));
}

#[test]
fn plugin_mcp_manual_connection_cannot_bypass_project_approval() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let _env = EnvRestore::isolate(home.path());
    register(
        home.path(),
        project.path(),
        "project",
        json!({"server":{"command":"never-spawned"}}),
    );
    let config = McpConfig::load_for_dir(Some(project.path()));
    let manager = crate::mcp::McpManager::with_config(config);
    let name = "plugin:demo@market:server";
    assert_eq!(
        manager.project_server_state(name),
        Some(ProjectServerState::Pending)
    );
    assert!(manager.connect_refusal(name).unwrap().contains("approval"));
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_mcp_live_reload_activates_and_revokes_only_plugin_connections() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let _env = EnvRestore::isolate(home.path());
    let root = register(
        home.path(),
        project.path(),
        "user",
        json!({"server":{
            "command":"/bin/sh", "args":["${CLAUDE_PLUGIN_ROOT}/server.sh", "${CLAUDE_PLUGIN_ROOT}/starts"],
            "timeout_secs":5
        }}),
    );
    let script = root.join("server.sh");
    std::fs::write(&script, r##"printf 'start\n' >> "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | grep -o '"id":[0-9]*' | grep -o '[0-9]*' | head -1)
  case "$line" in
    *'"initialize"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}' ;;
    *'"tools/list"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"tools":[{"name":"probe","inputSchema":{"type":"object"}}]}}' ;;
    *'"tools/call"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"content":[{"type":"text","text":"ok"}]}}' ;;
    *'"shutdown"'*) exit 0 ;;
  esac
done
"##).unwrap();
    let plugins = crate::plugin::PluginManager::for_dir(project.path()).unwrap();
    plugins
        .disable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    let mut config = McpConfig::load_for_dir(None);
    let dynamic: McpServerConfig = serde_json::from_value(json!({
        "command":"/bin/sh", "args":[script, root.join("starts")], "shared":false, "timeout_secs":5
    }))
    .unwrap();
    config.servers.insert("dynamic".into(), dynamic.clone());
    config
        .sources
        .insert("dynamic".into(), "added this session".into());
    let mut manager = crate::mcp::McpManager::with_config(config);
    manager.connect("dynamic", &dynamic).await.unwrap();
    let name = "plugin:demo@market:server";
    plugins
        .enable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    assert_eq!(
        manager.reload_plugin_sources().await.unwrap(),
        (1, Vec::new())
    );
    assert!(
        manager
            .connected_servers()
            .await
            .contains(&name.to_string())
    );
    assert!(
        manager
            .all_tools()
            .await
            .iter()
            .any(|(server, tool)| server == name && tool.name == "probe")
    );
    assert_eq!(
        manager.reload_plugin_sources().await.unwrap(),
        (0, Vec::new())
    );
    assert_eq!(
        std::fs::read_to_string(root.join("starts"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    plugins
        .disable("demo@market", crate::plugin::PluginScope::User)
        .unwrap();
    assert_eq!(
        manager.reload_plugin_sources().await.unwrap(),
        (0, Vec::new())
    );
    assert_eq!(
        manager.connected_servers().await,
        vec!["dynamic".to_string()]
    );
    assert!(
        manager
            .all_tools()
            .await
            .iter()
            .all(|(server, _)| server == "dynamic")
    );
    assert!(
        manager
            .call_tool("dynamic", "probe", json!({}))
            .await
            .is_ok()
    );
    assert_eq!(
        std::fs::read_to_string(root.join("starts"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    manager.disconnect_all().await;
}
