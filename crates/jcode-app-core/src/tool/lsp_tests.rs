use super::*;

fn python_available() -> bool {
    let available = std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        eprintln!("SKIP: python3 unavailable for plugin LSP fixture");
    }
    available
}

struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl Drop for Restore {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn plugin_lsp_data_is_pure_until_spawn_and_persists_across_cache_update() {
    if !python_available() {
        return;
    }
    let _guard = crate::storage::lock_test_env();
    let _restore = Restore(
        ["JCODE_HOME", "CLAUDE_PLUGIN_DATA"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    let home = tempfile::tempdir().unwrap();
    let data_home = home.path().join("home-${JCODE_LSP_DATA_LITERAL}");
    crate::env::set_var("JCODE_HOME", &data_home);
    crate::env::set_var("CLAUDE_PLUGIN_DATA", "/spoofed-parent-data");
    let manifest = json!({"lspServers":{"peer":{
        "command":"python3", "args":["-u","-c",include_str!("lsp_fixture.py"),"${CLAUDE_PLUGIN_DATA}"],
        "env":{"CLAUDE_PLUGIN_DATA":"${JCODE_LSP_DATA_SPOOF_UNSET_956028}", "FIXTURE_DATA_REPORT":"1"},
        "extensionToLanguage":{".rs":"rust"},
        "initializationOptions":{"data":"${CLAUDE_PLUGIN_DATA}"},
        "settings":{"data":"${CLAUDE_PLUGIN_DATA}"}
    }}});
    let old = fixture_plugin(manifest.clone());
    let new = fixture_plugin(manifest);
    let mut expected = None;
    for (dir, previous) in [(&old, false), (&new, true)] {
        let plugin = jcode_base::plugin::PluginRoot {
            name: "demo".into(),
            marketplace: "market".into(),
            root: dir.path().canonicalize().unwrap(),
            version: Some(if previous { "v2" } else { "v1" }.into()),
            scope: jcode_base::plugin::PluginScope::User,
        };
        let data = jcode_base::plugin::data_path(&plugin).unwrap();
        if let Some(expected) = &expected {
            assert_eq!(&data, expected);
        } else {
            expected = Some(data.clone());
        }
        let server = matching_plugin_server(dir.path(), ".rs", None, [plugin]).unwrap();
        assert_eq!(
            server.config.env["CLAUDE_PLUGIN_DATA"],
            data.to_string_lossy()
        );
        assert_eq!(
            server.config.settings.as_ref().unwrap()["data"],
            data.to_string_lossy().as_ref()
        );
        if !previous {
            assert!(!data_home.join("plugin-data").exists());
        }
        let mut client = Client::spawn(&server, dir.path()).unwrap();
        let result = client
            .request(
                "initialize",
                json!({"initializationOptions":server.config.initialization_options}),
            )
            .await
            .unwrap();
        assert_eq!(
            result["fixtureData"]["path"],
            data.to_string_lossy().as_ref()
        );
        assert_eq!(
            result["fixtureData"]["arg"],
            data.to_string_lossy().as_ref()
        );
        assert_eq!(result["fixtureData"]["exists"], true);
        assert_eq!(result["fixtureData"]["previousMarker"], previous);
        assert_eq!(
            result["fixtureData"]["params"]["initializationOptions"]["data"],
            data.to_string_lossy().as_ref()
        );
        client.shutdown().await;
        assert_eq!(
            std::env::var("CLAUDE_PLUGIN_DATA").unwrap(),
            "/spoofed-parent-data"
        );
    }
}

fn fixture_plugin(manifest: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".claude-plugin")).unwrap();
    std::fs::write(
        dir.path().join(".claude-plugin/plugin.json"),
        manifest.to_string(),
    )
    .unwrap();
    dir
}

#[test]
fn default_and_manifest_configs_merge_with_expansion_and_namespacing() {
    let dir = fixture_plugin(
        json!({"lspServers":{"rust":{"command":"${CLAUDE_PLUGIN_ROOT}/bin/rust","extensionToLanguage":{".rs":"rust"},"initializationOptions":{"workspace":"${workspaceFolder}"}}}}),
    );
    std::fs::write(dir.path().join(".lsp.json"), json!({"python":{"command":"pylsp","extensionToLanguage":{".py":"python"}},"rust":{"command":"old","extensionToLanguage":{".rs":"rust"}}}).to_string()).unwrap();
    let root = dir.path().canonicalize().unwrap();
    let servers = load_servers(&root, "market/plugin", Path::new("/workspace")).unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[1].name, "market/plugin:rust");
    assert_eq!(
        servers[1].config.command,
        format!("{}/bin/rust", root.display())
    );
    assert_eq!(
        servers[1].config.initialization_options.as_ref().unwrap()["workspace"],
        "/workspace"
    );
}

#[test]
fn custom_config_paths_and_invalid_shapes() {
    for declared in [
        json!("servers.json"),
        json!(["servers.json"]),
        json!("${CLAUDE_PLUGIN_ROOT}/servers.json"),
        json!(["${CLAUDE_PLUGIN_ROOT}/servers.json"]),
    ] {
        let dir = fixture_plugin(json!({"lspServers":declared}));
        std::fs::write(
            dir.path().join("servers.json"),
            json!({"test":{"command":"test-lsp","extensionToLanguage":{".test":"test"}}})
                .to_string(),
        )
        .unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(
            load_servers(&root, "one/plugin", &root).unwrap()[0].name,
            "one/plugin:test"
        );
        std::fs::write(dir.path().join("servers.json"), json!({"test":{"command":"test-lsp","transport":"socket","extensionToLanguage":{".test":"test"}}}).to_string()).unwrap();
        assert!(load_servers(&root, "one/plugin", &root).is_err());
        std::fs::write(dir.path().join("servers.json"), "[]").unwrap();
        assert!(load_servers(&root, "one/plugin", &root).is_err());
    }
}

#[test]
fn manifest_array_accepts_mixed_paths_and_inline_overrides() {
    let dir = fixture_plugin(
        json!({"lspServers":["servers.json", {"test":{"command":"override","extensionToLanguage":{".test":"test"}}}]}),
    );
    std::fs::write(
        dir.path().join("servers.json"),
        json!({"test":{"command":"original","extensionToLanguage":{".test":"test"}}}).to_string(),
    )
    .unwrap();
    let root = dir.path().canonicalize().unwrap();
    let servers = load_servers(&root, "market/plugin", &root).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].config.command, "override");
}

#[test]
fn config_paths_cannot_escape_root() {
    let dir = fixture_plugin(json!({}));
    let root = dir.path().canonicalize().unwrap();
    assert!(read_config_file(&root, "/etc/passwd").is_err());
    assert!(read_config_file(&root, "${CLAUDE_PLUGIN_ROOT}//etc/passwd").is_err());
    assert!(read_config_file(&root, "../outside.json").is_err());
    let outside = tempfile::NamedTempFile::new().unwrap();
    let traversal = format!(
        "../{}",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    for path in [
        traversal.clone(),
        format!("${{CLAUDE_PLUGIN_ROOT}}/{traversal}"),
    ] {
        assert!(
            read_config_file(&root, &path)
                .unwrap_err()
                .to_string()
                .contains("escapes plugin root")
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), root.join("escape.json")).unwrap();
        for path in ["escape.json", "${CLAUDE_PLUGIN_ROOT}/escape.json"] {
            assert!(
                read_config_file(&root, path)
                    .unwrap_err()
                    .to_string()
                    .contains("escapes plugin root")
            );
        }
    }
}

#[test]
fn expansion_is_literal_and_missing_variables_fail_closed() {
    assert_eq!(
        expand(
            "${CLAUDE_PLUGIN_ROOT}/x; echo nope",
            Path::new("/plugin space"),
            Path::new("/work")
        )
        .unwrap(),
        "/plugin space/x; echo nope"
    );
    assert!(
        expand(
            "${JCODE_LSP_TEST_VARIABLE_THAT_DOES_NOT_EXIST_7029}",
            Path::new("/plugin"),
            Path::new("/work")
        )
        .is_err()
    );
    assert!(expand("${broken", Path::new("/plugin"), Path::new("/work")).is_err());
    let mut data = json!({"known":"${CLAUDE_PLUGIN_ROOT}", "unknown":"${JCODE_LSP_TEST_VARIABLE_THAT_DOES_NOT_EXIST_7029}"});
    expand_json(&mut data, Path::new("/plugin"), Path::new("/work")).unwrap();
    assert_eq!(data["known"], "/plugin");
    assert_eq!(
        data["unknown"],
        "${JCODE_LSP_TEST_VARIABLE_THAT_DOES_NOT_EXIST_7029}"
    );
}

#[test]
fn broken_plugin_does_not_block_good_and_namespace_collisions_are_explicit() {
    let bad = fixture_plugin(json!({"lspServers":true}));
    let good = fixture_plugin(
        json!({"lspServers":{"rust":{"command":"rust-analyzer","extensionToLanguage":{".rs":"rust"}}}}),
    );
    let roots = || {
        vec![
            (bad.path().canonicalize().unwrap(), "market/broken".into()),
            (good.path().canonicalize().unwrap(), "market/good".into()),
        ]
    };
    let workspace = good.path().canonicalize().unwrap();
    assert_eq!(
        matching_server(&workspace, ".rs", None, roots())
            .unwrap()
            .name,
        "market/good:rust"
    );
    assert_eq!(
        matching_server(&workspace, ".rs", Some("market/good:rust"), roots())
            .unwrap()
            .name,
        "market/good:rust"
    );
    assert!(
        matching_server(&workspace, ".py", None, roots())
            .unwrap_err()
            .to_string()
            .contains("market/broken")
    );
    let duplicate = vec![
        (workspace.clone(), "market/good".into()),
        (workspace.clone(), "other/good".into()),
    ];
    assert!(
        matching_server(&workspace, ".rs", None, duplicate)
            .unwrap_err()
            .to_string()
            .contains("Multiple")
    );
    assert!(matching_server(&workspace, ".rs", None, Vec::new()).is_err());
}

#[tokio::test]
async fn malformed_or_oversized_frames_fail_without_unbounded_allocation() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for wire in [
        "Content-Length: 9000000\r\n\r\n",
        "Content-Type: application/json\r\n\r\n",
        "Content-Length: nope\r\n\r\n",
    ] {
        let code = format!("import sys; sys.stdout.write({wire:?}); sys.stdout.flush()");
        let config: ServerConfig = serde_json::from_value(json!({"command":"python3","args":["-u","-c",code],"extensionToLanguage":{".rs":"rust"}})).unwrap();
        let server = PluginServer {
            name: "fixture/bad:test".into(),
            root: dir.path().into(),
            config,
            plugin: None,
        };
        let mut client = Client::spawn(&server, dir.path()).unwrap();
        assert!(client.request("initialize", json!({})).await.is_err());
        client.shutdown().await;
    }
}

#[tokio::test]
async fn stdio_initialize_queries_notifications_error_and_shutdown() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config: ServerConfig = serde_json::from_value(json!({"command":"python3","args":["-u","-c",include_str!("lsp_fixture.py")],"extensionToLanguage":{".rs":"rust"}})).unwrap();
    let server = PluginServer {
        name: "fixture/plugin:test".into(),
        root: dir.path().into(),
        config,
        plugin: None,
    };
    let mut client = Client::spawn(&server, dir.path()).unwrap();
    let result = client.request("initialize", json!({})).await.unwrap();
    assert_eq!(result["capabilities"]["hoverProvider"], true);
    client.notify("initialized", json!({})).await.unwrap();
    let uri = "file:///fixture.rs";
    client.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":"rust","version":1,"text":"fn main() {}"}})).await.unwrap();
    for method in [
        "textDocument/hover",
        "textDocument/definition",
        "textDocument/references",
        "textDocument/documentSymbol",
    ] {
        let result = client
            .request(
                method,
                json!({"textDocument":{"uri":uri},"position":{"line":0,"character":2}}),
            )
            .await
            .unwrap();
        assert_eq!(result["method"], method);
        assert_eq!(result["text"], "café λ");
    }
    assert_eq!(client.diagnostics[uri][0]["message"], "fixture diagnostic");
    assert!(
        client
            .request("fixture/error", json!({}))
            .await
            .unwrap_err()
            .to_string()
            .contains("fixture error")
    );
    client.shutdown().await;
    assert!(client.child.try_wait().unwrap().unwrap().success());
}

#[tokio::test]
async fn full_query_operations_use_current_file_and_one_based_positions() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("fixture.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let config: ServerConfig = serde_json::from_value(json!({"command":"python3","args":["-u","-c",include_str!("lsp_fixture.py")],"extensionToLanguage":{".rs":"rust"}})).unwrap();
    let server = PluginServer {
        name: "fixture/plugin:test".into(),
        root: root.clone(),
        config,
        plugin: None,
    };
    for operation in [
        "hover",
        "definition",
        "references",
        "documentSymbols",
        "diagnostics",
    ] {
        let input: Input = serde_json::from_value(
            json!({"operation":operation,"file_path":"fixture.rs","line":2,"character":3}),
        )
        .unwrap();
        let output = run_server(&server, &root, &file, ".rs", &input)
            .await
            .unwrap();
        let output: Value = serde_json::from_str(&output.output).unwrap();
        assert_eq!(output["operation"], operation);
        if operation == "diagnostics" {
            assert_eq!(
                output["result"]["items"][0]["message"],
                "fixture diagnostic"
            );
        } else if operation == "documentSymbols" {
            assert_eq!(output["result"]["method"], "textDocument/documentSymbol");
        } else {
            assert_eq!(
                output["result"]["params"]["position"],
                json!({"line":1,"character":2})
            );
            if operation == "references" {
                assert_eq!(
                    output["result"]["params"]["context"]["includeDeclaration"],
                    true
                );
            }
        }
    }
    let mut pull_server = server;
    pull_server
        .config
        .env
        .insert("FIXTURE_PULL_DIAGNOSTICS".into(), "1".into());
    let input: Input =
        serde_json::from_value(json!({"operation":"diagnostics","file_path":"fixture.rs"}))
            .unwrap();
    let output = run_server(&pull_server, &root, &file, ".rs", &input)
        .await
        .unwrap();
    let output: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(output["result"]["method"], "textDocument/diagnostic");
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_request_terminates_server_descendants() {
    if !python_available() {
        return;
    }
    struct Cleanup(Vec<u32>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for pid in &self.0 {
                unsafe {
                    libc::kill(*pid as i32, libc::SIGKILL);
                }
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let code = r#"
import json, subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(120)'])
body = json.dumps({'jsonrpc':'2.0','id':1,'result':{'pid':child.pid}}).encode()
sys.stdout.buffer.write(f'Content-Length: {len(body)}\r\n\r\n'.encode() + body)
sys.stdout.buffer.flush()
time.sleep(120)
"#;
    let config: ServerConfig = serde_json::from_value(json!({
        "command":"python3", "args":["-u","-c",code],
        "extensionToLanguage":{".rs":"rust"}
    }))
    .unwrap();
    let server = PluginServer {
        name: "fixture/cancel:server".into(),
        root: dir.path().into(),
        config,
        plugin: None,
    };
    let mut client = Client::spawn(&server, dir.path()).unwrap();
    let parent = client.child.id().unwrap();
    let result = client.request("initialize", json!({})).await.unwrap();
    let descendant = u32::try_from(result["pid"].as_u64().unwrap()).unwrap();
    let cleanup = Cleanup(vec![parent, descendant]);
    let task = tokio::spawn(async move { client.request("fixture/stall", json!({})).await });
    tokio::task::yield_now().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(5), async {
        while cleanup
            .0
            .iter()
            .any(|pid| crate::platform::is_process_running(*pid))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled LSP request left a server process running");
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn installed_plugin_runs_through_registry_and_respects_project_disablement() {
    if !python_available() {
        return;
    }
    use jcode_base::plugin::{PluginManager, PluginScope};
    let _guard = crate::storage::lock_test_env();
    let _restore = Restore(vec![("JCODE_HOME", std::env::var_os("JCODE_HOME"))]);
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let market = temp.path().join("market");
    let plugin = market.join("demo");
    for directory in [
        &project,
        &market.join(".claude-plugin"),
        &plugin.join(".claude-plugin"),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    crate::env::set_var("JCODE_HOME", &home);
    std::fs::write(
        market.join(".claude-plugin/marketplace.json"),
        json!({
            "name":"fixture", "plugins":[{"name":"demo","source":"./demo"}]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        json!({
            "name":"demo", "lspServers":{"rust":{
                "command":"python3", "args":["-u","-c",include_str!("lsp_fixture.py")],
                "extensionToLanguage":{".rs":"rust"}
            }}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(project.join("main.rs"), "fn main() {}\n").unwrap();
    let manager = PluginManager::new(home.clone(), project.clone());
    manager.add_marketplace(market.to_str().unwrap()).unwrap();
    manager.install("demo", PluginScope::Project).unwrap();
    let registry = super::super::Registry::empty();
    *registry.tools.write().await = super::super::Registry::base_tools(&registry.skills);
    assert!(
        registry
            .definitions(None)
            .await
            .iter()
            .any(|tool| tool.name == "lsp")
    );
    assert!(!home.join("plugin-data").exists());
    let context = ToolContext {
        session_id: "plugin-lsp-registry-fixture".into(),
        message_id: "message".into(),
        tool_call_id: "lsp-call".into(),
        working_dir: Some(project.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    };
    let input = json!({"operation":"hover","file_path":"main.rs","line":1,"character":1});
    let output = registry
        .execute("lsp", input.clone(), context.clone())
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(value["server"], "fixture/demo:rust");
    assert_eq!(value["result"]["method"], "textDocument/hover");
    assert!(home.join("plugin-data/demo-fixture").is_dir());
    let outside = temp.path().join("outside.rs");
    std::fs::write(&outside, "fn outside() {}\n").unwrap();
    let escape = json!({"operation":"hover","file_path":outside});
    assert!(
        registry
            .execute("lsp", escape, context.clone())
            .await
            .is_err()
    );
    let other = temp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("main.rs"), "fn other() {}\n").unwrap();
    let mut other_context = context.clone();
    other_context.working_dir = Some(other);
    assert!(
        registry
            .execute("lsp", input.clone(), other_context)
            .await
            .is_err()
    );
    manager.disable("demo", PluginScope::Project).unwrap();
    assert!(registry.execute("lsp", input, context).await.is_err());
}

#[test]
fn invalid_sibling_server_does_not_block_matching_plugin_server() {
    for broken in [
        json!({"command":"${JCODE_LSP_TEST_MISSING_SIBLING_994315}","extensionToLanguage":{".py":"python"}}),
        json!({"command":42,"extensionToLanguage":{".py":"python"}}),
    ] {
        let dir = fixture_plugin(json!({"lspServers":{
            "broken":broken,
            "rust":{"command":"must-not-start-rust-fixture","extensionToLanguage":{".rs":"rust"}}
        }}));
        let root = dir.path().canonicalize().unwrap();
        let plugin = jcode_base::plugin::PluginRoot {
            name: "same".into(),
            marketplace: "market".into(),
            root: root.clone(),
            version: None,
            scope: jcode_base::plugin::PluginScope::User,
        };
        for selected in [None, Some("market/same:rust")] {
            let server = matching_plugin_server(&root, ".rs", selected, [plugin.clone()]).unwrap();
            assert_eq!(server.name, "market/same:rust");
        }
        assert!(
            matching_plugin_server(&root, ".py", Some("market/same:broken"), [plugin]).is_err()
        );
    }
}

#[tokio::test]
async fn transport_retains_only_opened_document_diagnostics() {
    if !python_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config: ServerConfig = serde_json::from_value(json!({
        "command":"python3","args":["-u","-c",include_str!("lsp_fixture.py")],
        "env":{"FIXTURE_DIAGNOSTIC_FLOOD":"1"},"extensionToLanguage":{".rs":"rust"}
    }))
    .unwrap();
    let server = PluginServer {
        name: "fixture/diagnostics:server".into(),
        root: dir.path().into(),
        config,
        plugin: None,
    };
    let mut client = Client::spawn(&server, dir.path()).unwrap();
    client.request("initialize", json!({})).await.unwrap();
    client.notify("initialized", json!({})).await.unwrap();
    for uri in ["file:///first.rs", "file:///second.rs"] {
        client
            .notify("textDocument/didOpen", json!({"textDocument":{"uri":uri}}))
            .await
            .unwrap();
        client
            .request("textDocument/hover", json!({"textDocument":{"uri":uri}}))
            .await
            .unwrap();
        assert_eq!(
            client
                .diagnostics
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![uri]
        );
        assert_eq!(client.diagnostics[uri][0]["message"], "fixture diagnostic");
    }
    client.shutdown().await;
}

#[path = "lsp_special_file_tests.rs"]
mod special_file_tests;
