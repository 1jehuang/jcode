use super::*;
use serde_json::json;

fn fixture(root: &Path, file: &str, value: Value) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn source(root: &Path, id: &str, project_scoped: bool) -> PluginMcpSource {
    PluginMcpSource {
        id: id.into(),
        root: root.into(),
        plugin: None,
        scope: if project_scoped {
            crate::plugin::PluginScope::Project
        } else {
            crate::plugin::PluginScope::User
        },
    }
}

#[test]
fn plugin_mcp_dotfile_namespaces_and_preserves_provenance_without_execution() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must-not-exist");
    fixture(
        root.path(),
        ".mcp.json",
        json!({"mcpServers": {
            "shell": {"command":"sh", "args":["-c",format!("touch {}", marker.display())]},
            "off": {"command":"do-not-execute", "enabled":false},
            "disabled": {"url":"https://example.invalid/mcp", "disabled":true}
        }}),
    );
    let config = load_sources([source(root.path(), "demo@market", true)]);
    assert_eq!(config.servers.len(), 3);
    assert!(config.servers.contains_key("plugin:demo@market:shell"));
    assert!(!config.servers.contains_key("shell"));
    assert!(!config.servers["plugin:demo@market:off"].is_enabled());
    assert!(!config.servers["plugin:demo@market:disabled"].is_enabled());
    let provenance = &config.sources["plugin:demo@market:shell"];
    assert!(super::super::is_project_mcp_source(provenance));
    assert!(provenance.contains(".mcp.json"));
    assert!(
        !marker.exists(),
        "discovery must never run configured commands"
    );
}

#[test]
fn plugin_mcp_manifest_inline_and_paths_extend_default_and_track_winning_file() {
    let root = tempfile::tempdir().unwrap();
    fixture(
        root.path(),
        ".mcp.json",
        json!({"mcpServers": {
            "default":{"command":"default"}, "overridden":{"command":"old"}
        }}),
    );
    fixture(
        root.path(),
        "config/remote.json",
        json!({"mcpServers": {
            "overridden":{"url":"https://example.invalid/${CLAUDE_PLUGIN_ROOT}"},
            "remote":{"type":"http", "url":"https://example.invalid/mcp"}
        }}),
    );
    fixture(
        root.path(),
        ".claude-plugin/plugin.json",
        json!({"mcpServers": [
            "./config/remote.json", {"inline":{"command":"${CLAUDE_PLUGIN_ROOT}/server"}}
        ]}),
    );
    let config = load_sources([source(root.path(), "demo", false)]);
    assert_eq!(config.servers.len(), 4);
    assert!(config.servers["plugin:demo:overridden"].is_remote());
    assert!(config.sources["plugin:demo:overridden"].contains("config/remote.json"));
    assert!(config.sources["plugin:demo:inline"].contains(".claude-plugin/plugin.json"));
    assert!(!super::super::is_project_mcp_source(
        &config.sources["plugin:demo:inline"]
    ));
    assert_eq!(
        config.servers["plugin:demo:inline"].env["CLAUDE_PLUGIN_ROOT"],
        root.path().canonicalize().unwrap().to_string_lossy()
    );
}

#[test]
fn plugin_mcp_manifest_single_path_and_inline_map_are_supported() {
    for manifest in [
        json!("./servers.json"),
        json!({"from-manifest":{"command":"server"}}),
    ] {
        let root = tempfile::tempdir().unwrap();
        fixture(
            root.path(),
            "servers.json",
            json!({"from-manifest":{"command":"server"}}),
        );
        fixture(
            root.path(),
            ".claude-plugin/plugin.json",
            json!({"mcpServers":manifest}),
        );
        let config = load_sources([source(root.path(), "demo", false)]);
        assert_eq!(
            config.servers["plugin:demo:from-manifest"].command,
            "server"
        );
    }
}

#[test]
fn plugin_mcp_bad_entries_do_not_hide_valid_siblings() {
    let root = tempfile::tempdir().unwrap();
    fixture(
        root.path(),
        ".mcp.json",
        json!({"mcpServers": {
            "valid":{"command":"server"}, "invalid":{"args":"not an array"},
            "unknown":{"type":"unsupported", "url":"https://example.invalid"},
            "incomplete":{"type":"http"}
        }}),
    );
    std::fs::create_dir_all(root.path().join(".claude-plugin")).unwrap();
    std::fs::write(root.path().join(".claude-plugin/plugin.json"), "bad json").unwrap();
    let config = load_sources([source(root.path(), "demo", false)]);
    assert_eq!(config.servers.len(), 1);
    assert!(config.servers.contains_key("plugin:demo:valid"));
}

#[test]
fn plugin_mcp_namespaces_are_injective_and_separate_plugins() {
    assert_ne!(server_name("a:b", "c"), server_name("a", "b:c"));
    assert_ne!(server_name("a%3Ab", "c"), server_name("a:b", "c"));
    let root = tempfile::tempdir().unwrap();
    fixture(
        root.path(),
        ".mcp.json",
        json!({"same":{"command":"server"}}),
    );
    let config = load_sources([
        source(root.path(), "a@one", false),
        source(root.path(), "a@two", false),
    ]);
    assert_eq!(config.servers.len(), 2);
}

#[test]
fn plugin_mcp_paths_cannot_escape_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("plugin");
    std::fs::create_dir(&root).unwrap();
    fixture(
        parent.path(),
        "outside.json",
        json!({"outside":{"command":"outside"}}),
    );
    fixture(
        &root,
        ".claude-plugin/plugin.json",
        json!({"mcpServers":"../outside.json"}),
    );
    assert!(
        load_sources([source(&root, "demo", false)])
            .servers
            .is_empty()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(parent.path().join("outside.json"), root.join(".mcp.json"))
            .unwrap();
        assert!(
            load_sources([source(&root, "demo", false)])
                .servers
                .is_empty()
        );
    }
}

#[test]
fn plugin_mcp_reload_drops_removed_sources_and_deleted_files() {
    let root = tempfile::tempdir().unwrap();
    fixture(
        root.path(),
        ".mcp.json",
        json!({"server":{"command":"server"}}),
    );
    assert_eq!(
        load_sources([source(root.path(), "demo", false)])
            .servers
            .len(),
        1
    );
    assert!(load_sources([]).servers.is_empty());
    std::fs::remove_file(root.path().join(".mcp.json")).unwrap();
    let reloaded = load_sources([source(root.path(), "demo", false)]);
    assert!(reloaded.servers.is_empty());
    assert!(reloaded.sources.is_empty());
}

struct DataFixture {
    temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl DataFixture {
    fn new() -> Self {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let home = temp
            .path()
            .join("home-${JCODE_DATA_MUST_STAY_LITERAL:-literal}");
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let previous = [
            "JCODE_HOME",
            "JCODE_DISABLE_CLAUDE_MCP",
            "CLAUDE_PLUGIN_DATA",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect();
        crate::env::set_var("JCODE_HOME", &home);
        crate::env::set_var("JCODE_DISABLE_CLAUDE_MCP", "1");
        crate::env::set_var("CLAUDE_PLUGIN_DATA", "/spoofed-parent-data");
        fixture(&home, "mcp.json", json!({"mcpServers":{}}));
        Self {
            temp,
            home,
            project,
            previous,
            _guard: guard,
        }
    }

    fn register(&self, version: &str, scope: &str, servers: Value) -> crate::plugin::PluginRoot {
        let root = self.home.join("plugins/cache/market/demo").join(version);
        fixture(&root, ".claude-plugin/plugin.json", json!({"name":"demo"}));
        fixture(&root, ".mcp.json", json!({"mcpServers":servers}));
        fixture(
            &self.home,
            "plugins/registry.json",
            json!({"installed":[{
                "name":"demo", "marketplace":"market", "root":root, "version":version,
                "scope":scope, "enabled":true,
                "project":if scope == "user" { Value::Null } else { json!(self.project.canonicalize().unwrap()) }
            }]}),
        );
        crate::plugin::enabled_plugins_for_dir(&self.project).remove(0)
    }
}

impl Drop for DataFixture {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[test]
fn plugin_mcp_data_is_intrinsic_single_pass_and_discovery_is_read_only() {
    let fixture = DataFixture::new();
    let plugin = fixture.register("v1", "user", json!({
        "stdio":{"command":"${CLAUDE_PLUGIN_DATA}/server", "args":["${CLAUDE_PLUGIN_DATA:-fallback}"],
            "env":{"CLAUDE_PLUGIN_DATA":"/spoofed-manifest", "DATA":"${CLAUDE_PLUGIN_DATA}"}},
        "remote":{"url":"https://example.invalid/${CLAUDE_PLUGIN_DATA}", "headers":{"X-Data":"${CLAUDE_PLUGIN_DATA:-fallback}"}}
    }));
    let data = crate::plugin::data_path(&plugin).unwrap();
    let config = McpConfig::load_for_dir(Some(&fixture.project));
    let stdio = &config.servers["plugin:demo@market:stdio"];
    assert_eq!(stdio.command, format!("{}/server", data.display()));
    assert_eq!(stdio.args, [data.to_string_lossy().into_owned()]);
    assert_eq!(stdio.env["CLAUDE_PLUGIN_DATA"], data.to_string_lossy());
    assert_eq!(stdio.env["DATA"], data.to_string_lossy());
    assert_eq!(
        config.servers["plugin:demo@market:remote"].headers["X-Data"],
        data.to_string_lossy()
    );
    assert!(!fixture.home.join("plugins/data").exists());
    assert_eq!(
        std::env::var("CLAUDE_PLUGIN_DATA").unwrap(),
        "/spoofed-parent-data"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn plugin_mcp_data_fake_peer_persists_across_cache_update_without_global_env_leak() {
    let fixture = DataFixture::new();
    let peer = r#"import json,os,sys
data=os.environ['CLAUDE_PLUGIN_DATA']
marker=os.path.join(data,'mcp-fixture-marker')
previous=os.path.isfile(marker)
with open(marker,'w') as f: f.write('persistent')
for line in sys.stdin:
 m=json.loads(line); method=m.get('method')
 if method=='shutdown': break
 if 'id' not in m: continue
 if method=='initialize': result={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'data-fixture'}}
 elif method=='tools/list': result={'tools':[]}
 else: result={'content':[{'type':'text','text':json.dumps({'data':data,'arg':sys.argv[1],'exists':os.path.isdir(data),'previous':previous})}]}
 print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':result}),flush=True)
"#;
    let servers = json!({"peer":{"command":"python3", "args":["-u","-c",peer,"${CLAUDE_PLUGIN_DATA}"],
        "env":{"CLAUDE_PLUGIN_DATA":"/spoofed-manifest", "ROOT":"${CLAUDE_PLUGIN_ROOT}"}}});
    let plugin = fixture.register("v1", "user", servers.clone());
    let data = crate::plugin::data_path(&plugin).unwrap();
    assert!(!data.exists());
    for (version, previous) in [("v1", false), ("v2", true)] {
        let plugin = fixture.register(version, "user", servers.clone());
        assert_eq!(crate::plugin::data_path(&plugin).unwrap(), data);
        let manager = super::super::McpManager::for_dir(Some(fixture.project.clone()));
        let config = McpConfig::load_for_dir(Some(&fixture.project));
        manager
            .connect(
                "plugin:demo@market:peer",
                &config.servers["plugin:demo@market:peer"],
            )
            .await
            .unwrap();
        let response = manager
            .call_tool("plugin:demo@market:peer", "report", json!({}))
            .await
            .unwrap();
        let super::super::protocol::ContentBlock::Text { text } = &response.content[0] else {
            panic!("expected text")
        };
        let report: Value = serde_json::from_str(text).unwrap();
        assert_eq!(report["data"], data.to_string_lossy().as_ref());
        assert_eq!(report["arg"], data.to_string_lossy().as_ref());
        assert_eq!(report["exists"], true);
        assert_eq!(report["previous"], previous);
        manager.disconnect_all().await;
    }
    assert_eq!(
        std::env::var("CLAUDE_PLUGIN_DATA").unwrap(),
        "/spoofed-parent-data"
    );
    assert!(fixture.temp.path().exists());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn plugin_mcp_data_approval_and_mutated_config_fail_before_mkdir() {
    let fixture = DataFixture::new();
    let servers = json!({"peer":{"command":"${CLAUDE_PLUGIN_ROOT}/missing-peer"}});
    fixture.register("v1", "project", servers.clone());
    let name = "plugin:demo@market:peer";
    let manager = super::super::McpManager::for_dir(Some(fixture.project.clone()));
    let config = McpConfig::load_for_dir(Some(&fixture.project));
    assert!(manager.connect(name, &config.servers[name]).await.is_err());
    assert!(!fixture.home.join("plugins/data").exists());
    fixture.register("v1", "user", servers);
    let manager = super::super::McpManager::for_dir(Some(fixture.project.clone()));
    let mut config = McpConfig::load_for_dir(Some(&fixture.project)).servers[name].clone();
    config
        .env
        .insert("CLAUDE_PLUGIN_DATA".into(), "/spoofed-at-connect".into());
    assert!(manager.connect(name, &config).await.is_err());
    assert!(!fixture.home.join("plugins/data").exists());
}
