//! Real stdio coverage of the public MCP manager/proxy API. No configured user
//! servers are loaded. The test re-execs itself with an empty home/environment
//! before constructing a runtime, keeping config/provenance globals isolated.

use jcode_base::mcp::{
    McpConfig, McpManager, McpServerConfig, McpToolDef, create_mcp_tools,
    create_mcp_tools_from_cached_many, dispatch_name,
};
use jcode_tool_core::{Tool, ToolContext, ToolExecutionMode};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::RwLock;

const CHILD_MARKER: &str = "JCODE_MCP_STDIO_TEST_CHILD";
const TEST_NAME: &str = "real_stdio_collision_aliases_preserve_original_targets";
const SERVERS: [(&str, [&str; 2]); 2] = [
    ("server-a", ["query-docs", "only-a"]),
    ("server_a", ["query_docs", "only_b"]),
];

// Each server rejects unknown original tool names, and reports its own identity
// and PID. This catches accidentally forwarding a generated alias to the wire,
// routing both aliases to one process, and losing arguments during dispatch.
const SERVER: &str = r#"
import json, os, sys
server, names = sys.argv[1], json.loads(sys.argv[2])
for line in sys.stdin:
    req = json.loads(line)
    method = req.get('method')
    if method == 'shutdown':
        break
    if 'id' not in req:
        continue
    reply = {'jsonrpc': '2.0', 'id': req['id']}
    if method == 'initialize':
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}},
                  'serverInfo': {'name': server, 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': name, 'description': server + ':' + name,
                            'inputSchema': {'type': 'object', 'properties': {
                                'token': {'type': 'string'}}}} for name in names]}
    elif method == 'tools/call' and req['params']['name'] in names:
        value = {'server': server, 'tool': req['params']['name'],
                 'arguments': req['params']['arguments'], 'pid': os.getpid()}
        result = {'content': [{'type': 'text', 'text': json.dumps(value)}], 'isError': False}
    else:
        reply['error'] = {'code': -32602, 'message': 'unknown method or original tool name'}
        print(json.dumps(reply), flush=True)
        continue
    reply['result'] = result
    print(json.dumps(reply), flush=True)
"#;

#[test]
fn real_stdio_collision_aliases_preserve_original_targets() {
    if std::env::var_os(CHILD_MARKER).is_none() {
        if !Command::new("python3")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            eprintln!("SKIP {TEST_NAME}: python3 is unavailable for real stdio fixtures");
            return;
        }
        let sandbox = tempfile::tempdir().expect("isolated MCP test home");
        let mut child = Command::new(std::env::current_exe().expect("test executable"));
        child.env_clear();
        // Only executable/dynamic-library search and Windows process setup are
        // inherited. In particular, no provider credentials or JCODE_* config.
        for key in ["PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(key) {
                child.env(key, value);
            }
        }
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
        ] {
            child.env(key, sandbox.path());
        }
        let status = child
            .env(CHILD_MARKER, "1")
            .env("JCODE_HOME", sandbox.path().join("jcode"))
            .env("JCODE_RUNTIME_DIR", sandbox.path().join("runtime"))
            .current_dir(sandbox.path())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .status()
            .expect("run isolated MCP integration test");
        assert!(status.success(), "isolated MCP test failed: {status}");
        return;
    }
    tokio::runtime::Runtime::new()
        .expect("test runtime")
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(30), exercise_proxies())
                .await
                .expect("real MCP fixture test exceeded 30 seconds");
        });
}

fn config() -> McpConfig {
    McpConfig {
        servers: SERVERS
            .iter()
            .map(|(server, names)| {
                let config: McpServerConfig = serde_json::from_value(json!({
                    "command": "python3",
                    "args": ["-I", "-S", "-u", "-c", SERVER, server,
                             serde_json::to_string(names).unwrap()],
                    "shared": false,
                    "timeout_secs": 5,
                }))
                .unwrap();
                (server.to_string(), config)
            })
            .collect(),
    }
}

fn cached_definitions() -> Vec<(String, McpToolDef)> {
    SERVERS
        .iter()
        .flat_map(|(server, names)| {
            names.iter().map(move |name| {
                (
                    server.to_string(),
                    McpToolDef {
                        name: name.to_string(),
                        description: Some(format!("{server}:{name}")),
                        input_schema: json!({"type": "object", "properties": {
                            "token": {"type": "string"}
                        }}),
                    },
                )
            })
        })
        .collect()
}

type Proxies = Vec<(String, Arc<dyn Tool>)>;
type Targets = BTreeMap<(String, String), String>;

async fn call(tool: &Arc<dyn Tool>) -> Value {
    let output = tool
        .execute(
            json!({"token": "stdio-sentinel", "intent": "test original target"}),
            ToolContext {
                session_id: "mcp-stdio-test".into(),
                message_id: "message".into(),
                tool_call_id: "call".into(),
                working_dir: None,
                stdin_request_tx: None,
                graceful_shutdown_signal: None,
                execution_mode: ToolExecutionMode::Direct,
            },
        )
        .await
        .expect("execute public MCP proxy through real stdio");
    let reply: Value = serde_json::from_str(&output.output).expect("fixture identity reply");
    assert_eq!(reply["arguments"], json!({"token": "stdio-sentinel"}));
    reply
}

async fn targets(proxies: &Proxies) -> Targets {
    assert_eq!(proxies.len(), 4);
    let mut aliases = HashSet::new();
    let mut pids = HashSet::new();
    let mut targets = BTreeMap::new();
    for (alias, tool) in proxies {
        assert!(aliases.insert(alias), "duplicate exposed alias {alias}");
        let reply = call(tool).await;
        pids.insert(reply["pid"].as_u64().expect("child process ID"));
        let server = reply["server"].as_str().unwrap();
        let name = reply["tool"].as_str().unwrap();
        assert_eq!(tool.name(), name, "wire name must remain original");
        assert!(
            targets
                .insert((server.into(), name.into()), alias.clone())
                .is_none()
        );
    }
    assert_eq!(
        pids.len(),
        2,
        "both real server processes must receive calls"
    );
    for (server, names) in SERVERS {
        for name in names {
            let alias = &targets[&(server.to_string(), name.to_string())];
            let historical = dispatch_name(server, name);
            if name.starts_with("only") {
                assert_eq!(alias, &historical, "noncollision spelling changed");
            } else {
                assert!(alias.starts_with(&format!("{historical}__")));
            }
        }
    }
    targets
}

async fn exercise_proxies() {
    let manager = Arc::new(RwLock::new(McpManager::with_config(config())));
    let (connected, failures) = manager.read().await.connect_all().await.unwrap();
    assert_eq!(connected, 2);
    assert!(
        failures.is_empty(),
        "fixture connections failed: {failures:?}"
    );
    let eager = create_mcp_tools(manager.clone()).await;
    let eager_targets = targets(&eager).await;
    manager.read().await.disconnect_all().await;

    // Cached proxies are built with neither server connected. Executing them
    // exercises the actual connect-on-first-call path, not a warmed manager.
    let manager = Arc::new(RwLock::new(McpManager::with_config(config())));
    let mut definitions = cached_definitions();
    let cached = create_mcp_tools_from_cached_many(&definitions, manager.clone());
    assert!(!manager.read().await.has_connections().await);
    assert_eq!(targets(&cached).await, eager_targets);
    definitions.reverse();
    let reversed = create_mcp_tools_from_cached_many(&definitions, manager.clone());
    assert_eq!(
        targets(&reversed).await,
        eager_targets,
        "enumeration order changed aliases"
    );
    assert_eq!(
        targets(&create_mcp_tools(manager.clone()).await).await,
        eager_targets
    );
    manager.read().await.disconnect_all().await;

    // A partial live surface is intentionally a different naming set. Document
    // that adding a colliding server renames the old key, but never retargets
    // its existing proxy. Registry owners must reconcile obsolete aliases.
    let configs = config();
    let manager = Arc::new(RwLock::new(McpManager::with_config(configs.clone())));
    let partial_definitions: Vec<_> = cached_definitions()
        .into_iter()
        .filter(|(server, _)| server == "server-a")
        .collect();
    let partial_cached = create_mcp_tools_from_cached_many(&partial_definitions, manager.clone());
    assert!(!manager.read().await.has_connections().await);
    let historical = dispatch_name("server-a", "query-docs");
    let cached_proxy = &partial_cached
        .iter()
        .find(|(name, _)| name == &historical)
        .unwrap()
        .1;
    assert_eq!(call(cached_proxy).await["server"], "server-a");
    let partial = create_mcp_tools(manager.clone()).await;
    let old_proxy = &partial
        .iter()
        .find(|(name, _)| name == &historical)
        .unwrap()
        .1;
    assert_eq!(call(old_proxy).await["server"], "server-a");
    manager
        .read()
        .await
        .connect("server_a", &configs.servers["server_a"])
        .await
        .unwrap();
    let complete = create_mcp_tools(manager.clone()).await;
    assert_eq!(targets(&complete).await, eager_targets);
    assert!(!complete.iter().any(|(name, _)| name == &historical));
    assert_eq!(
        call(old_proxy).await["server"],
        "server-a",
        "old proxy was retargeted"
    );
    assert_eq!(call(cached_proxy).await["server"], "server-a");
    manager.read().await.disconnect_all().await;
    assert!(!manager.read().await.has_connections().await);
}
