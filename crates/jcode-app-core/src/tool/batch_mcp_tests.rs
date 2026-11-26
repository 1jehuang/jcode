use super::*;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

const SERVER: &str = r#"
import json,sys
server,tool=sys.argv[1:]
for line in sys.stdin:
 r=json.loads(line); m=r.get('method')
 if m=='shutdown': break
 if 'id' not in r: continue
 out={'jsonrpc':'2.0','id':r['id']}
 if m=='initialize': out['result']={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':server,'version':'1'}}
 elif m=='tools/list': out['result']={'tools':[{'name':tool,'description':'fixture','inputSchema':{'type':'object','properties':{'id':{'type':'string'},'extra':{'type':'string'},'server':{'type':'string'},'arguments':{'type':'object','properties':{'id':{'type':'string'},'extra':{'type':'string'}}}}}}]}
 elif m=='tools/call' and r['params']['name']==tool:
  v={'server':server,'tool':r['params']['name'],'arguments':r['params']['arguments']}; out['result']={'content':[{'type':'text','text':json.dumps(v)}],'isError':False}
 else: out['error']={'code':-32602,'message':'unexpected request'}
 print(json.dumps(out),flush=True)
"#;

fn context() -> ToolContext {
    ToolContext {
        session_id: "issue1811-transport".into(),
        message_id: "message".into(),
        tool_call_id: "batch-call".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

fn run_case(test_name: &str, input: Value, expected_arguments: Value) {
    const MARKER: &str = "JCODE_ISSUE1811_MCP_CHILD";
    if std::env::var_os(MARKER).is_none() {
        let home = tempfile::tempdir().expect("isolated home");
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.env_clear();
        for key in ["PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
        ] {
            cmd.env(key, home.path());
        }
        let status = cmd
            .env(MARKER, "1")
            .env("JCODE_CI", "1")
            .env("JCODE_HOME", home.path().join("jcode"))
            .env("JCODE_RUNTIME_DIR", home.path().join("runtime"))
            .current_dir(home.path())
            .args(["--exact", test_name, "--nocapture"])
            .status()
            .expect("run isolated child test");
        assert!(
            status.success(),
            "isolated MCP transport test failed: {status}"
        );
        return;
    }
    assert!(
        std::process::Command::new("python3")
            .arg("--version")
            .status()
            .expect("python3 is required for MCP transport acceptance tests")
            .success(),
        "python3 is required for MCP transport acceptance tests"
    );
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let manager=Arc::new(tokio::sync::RwLock::new(crate::mcp::McpManager::with_config(crate::mcp::McpConfig::default())));
        let work=tokio::time::timeout(Duration::from_secs(35), async {
            let registry=Registry::empty();
            let config=crate::mcp::McpServerConfig { command:"python3".into(), args:vec!["-I".into(),"-S".into(),"-u".into(),"-c".into(),SERVER.into(),"example_server".into(),"example_lookup".into()], env:HashMap::new(), shared:false, transport:None, url:None, headers:HashMap::new(), enabled:None, disabled:None, timeout_secs:Some(5) };
            manager.read().await.connect("example_server",&config).await?;
            async {
                let proxies=crate::mcp::create_mcp_tools(Arc::clone(&manager)).await;
                let mut tools=registry.tools.write().await;
                tools.insert("batch".into(),Arc::new(BatchTool::new(registry.downgrade())));
                tools.insert("mcp_call".into(),Arc::new(crate::tool::mcp::McpCallTool::new(Arc::clone(&manager)).with_registry(registry.clone())));
                drop(tools);
                registry.reconcile_mcp_tools(proxies).await;
                let output=registry.execute("batch",input,context()).await?;
                anyhow::ensure!(output.output.contains("Completed: 1 succeeded, 0 failed"),"unexpected batch completion: {}",output.output);
                let echoed: Value=output.output.lines().find_map(|line| serde_json::from_str(line).ok()).ok_or_else(|| anyhow::anyhow!("fixture response JSON missing from batch result: {}",output.output))?;
                anyhow::ensure!(echoed["server"]=="example_server","wrong backend server: {echoed}");
                anyhow::ensure!(echoed["tool"]=="example_lookup","wrong backend tool: {echoed}");
                anyhow::ensure!(echoed["arguments"]==expected_arguments,"backend arguments differ: {echoed}");
                println!("MCP transport acceptance observed: case={test_name} server={} tool={} arguments={}",echoed["server"],echoed["tool"],echoed["arguments"]);
                anyhow::Ok(())
            }.await
        }).await;
        let disconnected=manager.read().await.disconnect("example_server").await;
        work.expect("MCP stdio acceptance fixture exceeded 35 seconds").expect("batch transport assertion/execution");
        disconnected.expect("disconnect local MCP fixture");
    });
}

#[test]
fn issue1811_flat_legacy_mcp_call_reaches_stdio_backend() {
    run_case(
        "tool::batch::batch_mcp_tests::issue1811_flat_legacy_mcp_call_reaches_stdio_backend",
        json!({"tool_calls":[{"name":"mcp_call","tool":"example_lookup","server":"example_server","arguments":{"id":"synthetic","extra":"preserve-me"},"intent":"lookup"}]}),
        json!({"id":"synthetic","extra":"preserve-me"}),
    );
}

#[test]
fn issue1811_nested_mcp_call_control_reaches_stdio_backend() {
    run_case(
        "tool::batch::batch_mcp_tests::issue1811_nested_mcp_call_control_reaches_stdio_backend",
        json!({"tool_calls":[{"tool":"mcp_call","intent":"lookup","parameters":{"server":"example_server","tool":"example_lookup","arguments":{"id":"synthetic","extra":"preserve-me"}}}]}),
        json!({"id":"synthetic","extra":"preserve-me"}),
    );
}

#[test]
fn issue1811_direct_mcp_tool_preserves_native_server_and_arguments_fields() {
    run_case(
        "tool::batch::batch_mcp_tests::issue1811_direct_mcp_tool_preserves_native_server_and_arguments_fields",
        json!({"tool_calls":[{"tool":"mcp__example_server__example_lookup","server":"child-context","arguments":{"id":"synthetic"},"intent":"lookup"}]}),
        json!({"server":"child-context","arguments":{"id":"synthetic"}}),
    );
}
