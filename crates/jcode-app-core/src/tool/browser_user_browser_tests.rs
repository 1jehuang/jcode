//! Regressions for #1743: browser control of the user's own browser.
#![allow(clippy::await_holding_lock)]
use super::*;
use std::os::unix::fs::PermissionsExt;

/// Fake bridge CLI. Session daemons answer like the real host. A daemon whose
/// name has a `stale` marker file reports its bound tab as closed, the way the
/// host does after the user closes the agent's window. Calls without a session
/// record their params, as the CLI's direct mode would see them.
const FAKE_CLI: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys, time
runtime = os.environ.get('XDG_RUNTIME_DIR', '/tmp')
home = os.environ['JCODE_HOME']
def log(entry):
    with open(home + '/calls', 'a') as f:
        f.write(json.dumps(entry) + '\n')
if sys.argv[1:4] == ['session', 'start', '--help']:
    print('--bind-window')
elif sys.argv[1:3] == ['session', 'start']:
    name = sys.argv[3]
    path = runtime + '/browser-session-' + name
    starts = home + '/starts'
    with open(starts, 'a') as f:
        f.write(json.dumps([name, os.getpid()]) + '\n')
    generation = sum(1 for _ in open(starts))
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(path + '.sock')
    listener.listen()
    with open(path + '.pid', 'w') as f:
        f.write(str(os.getpid()))
    while True:
        conn, _ = listener.accept()
        with conn:
            raw = conn.recv(65536)
            if not raw:
                continue
            if generation == 1 and os.path.exists(home + '/stale'):
                reply = {'ok': False, 'error': 'No tab with id: 41.'}
            else:
                reply = {'ok': True, 'result': {'tabId': 100 + generation, 'windowId': 900 + generation}}
            conn.sendall((json.dumps(reply) + '\n').encode())
else:
    action, params = sys.argv[1], json.loads(sys.argv[2])
    name = os.environ.get('BROWSER_SESSION')
    log({'action': action, 'params': params, 'session': name})
    if name is None:
        if action == 'uploadFile' and 'path' not in params:
            print("Error: uploadFile requires 'path' param", file=sys.stderr)
            sys.exit(1)
        print(json.dumps({'windows': [{'windowId': 1, 'tabs': [{'tabId': 7, 'url': 'https://app.example.com/'}]}], 'direct': True}))
        sys.exit(0)
    with socket.socket(socket.AF_UNIX) as conn:
        conn.connect(runtime + '/browser-session-' + name + '.sock')
        conn.sendall((json.dumps({'action': action, 'params': params}) + '\n').encode())
        reply = json.loads(conn.makefile().readline())
    if not reply['ok']:
        print('Error: ' + reply['error'], file=sys.stderr)
        sys.exit(1)
    print(json.dumps(reply['result']))
"#;

struct Fixture {
    _env_guard: std::sync::MutexGuard<'static, ()>,
    temp: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_session: Option<std::ffi::OsString>,
}

impl Fixture {
    fn new() -> Self {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        let previous_session = std::env::var_os("BROWSER_SESSION");
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::remove_var("BROWSER_SESSION");
        let bin = temp.path().join("browser/browser");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, FAKE_CLI).unwrap();
        std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _env_guard: guard,
            temp,
            previous_home,
            previous_session,
        }
    }

    fn lines(&self, file: &str) -> Vec<Value> {
        std::fs::read_to_string(self.temp.path().join(file))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn context(&self, name: &str) -> ToolContext {
        ToolContext {
            session_id: format!("br1743-{}-{name}", std::process::id()),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: None,
            stdin_request_tx: None,
            decision_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/tmp".into());
        for start in self.lines("starts") {
            let (name, pid) = (start[0].as_str().unwrap(), start[1].as_i64().unwrap());
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            for extension in ["pid", "sock"] {
                let _ = std::fs::remove_file(
                    runtime.join(format!("browser-session-{name}.{extension}")),
                );
            }
        }
        match &self.previous_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        match &self.previous_session {
            Some(value) => crate::env::set_var("BROWSER_SESSION", value),
            None => crate::env::remove_var("BROWSER_SESSION"),
        }
    }
}

#[tokio::test]
async fn closed_agent_window_restarts_the_session_and_retries_once() {
    let fixture = Fixture::new();
    let ctx = fixture.context("stale");
    std::fs::write(fixture.temp.path().join("stale"), "").unwrap();

    let result = firefox_run_bridge_command("getActiveTab", json!({}), Some("chrome"), &ctx)
        .await
        .expect("a closed agent window must not fail every later call");

    assert_eq!(result["windowId"], 902, "retried on a fresh daemon");
    assert_eq!(fixture.lines("starts").len(), 2);
    // Later calls stay on the new daemon.
    let again = firefox_run_bridge_command("getActiveTab", json!({}), Some("chrome"), &ctx)
        .await
        .unwrap();
    assert_eq!(again["windowId"], 902);
    assert_eq!(fixture.lines("starts").len(), 2);
}

#[tokio::test]
async fn explicit_tab_targets_are_not_retried_on_a_new_session() {
    let fixture = Fixture::new();
    let ctx = fixture.context("explicit");
    std::fs::write(fixture.temp.path().join("stale"), "").unwrap();

    let error = firefox_run_bridge_command("click", json!({"tabId": 41}), Some("chrome"), &ctx)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("No tab with id"), "{error:#}");
    assert_eq!(
        fixture.lines("starts").len(),
        1,
        "a closed user tab is not a session problem"
    );
}

#[tokio::test]
async fn list_tabs_shows_every_window_without_opening_an_agent_window() {
    let fixture = Fixture::new();
    let ctx = fixture.context("list");

    let listing = firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &ctx)
        .await
        .unwrap();
    assert_eq!(
        listing["direct"], true,
        "listing bypasses the window-bound session"
    );
    assert!(
        fixture.lines("starts").is_empty(),
        "listing alone must not open a window"
    );
    assert!(listing.get("agentWindowId").is_none());

    // Once the agent has its own window, the listing marks it.
    firefox_run_bridge_command("getActiveTab", json!({}), Some("chrome"), &ctx)
        .await
        .unwrap();
    let listing = firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &ctx)
        .await
        .unwrap();
    assert_eq!(listing["direct"], true);
    assert_eq!(listing["agentWindowId"], 901);

    // An explicit window still goes through the session.
    let scoped =
        firefox_run_bridge_command("listTabs", json!({"windowId": 901}), Some("chrome"), &ctx)
            .await
            .unwrap();
    assert!(scoped.get("direct").is_none());
}

#[tokio::test]
async fn upload_works_without_a_session_daemon() {
    let fixture = Fixture::new();
    let input = BrowserInput {
        action: "upload".into(),
        selector: Some("input[type=file]".into()),
        path: Some("/tmp/report.pdf".into()),
        ..Default::default()
    };
    let (action, params, _) = bridge_request("upload", &input).unwrap();
    assert_eq!(
        params["filePath"], "/tmp/report.pdf",
        "read by the native host"
    );
    assert_eq!(
        params["path"], "/tmp/report.pdf",
        "read by the CLI direct mode"
    );
    // Direct mode (no session daemon), which used to fail with
    // "uploadFile requires 'path' param".
    let output = tokio::process::Command::new(fixture.temp.path().join("browser/browser"))
        .arg(&action)
        .arg(params.to_string())
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn bridge_errors_are_explained() {
    let timeout = explain_bridge_error("navigate", "Timed out waiting for page load");
    assert!(timeout.contains("snapshot"), "{timeout}");
    let internal = explain_bridge_error("scroll", "Cannot access a chrome:// URL");
    assert!(internal.contains("internal pages"), "{internal}");
    let missing = explain_bridge_error(
        "getContent",
        "Could not establish connection. Receiving end does not exist.",
    );
    assert!(missing.contains("content script"), "{missing}");
    assert_eq!(
        explain_bridge_error("click", "Element not found: #x"),
        "Element not found: #x"
    );
}
