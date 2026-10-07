#![allow(clippy::await_holding_lock)]
use super::*;

#[cfg(unix)]
#[tokio::test]
async fn browser_session_reuses_daemon_across_five_tool_calls() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    let previous_session = std::env::var_os("BROWSER_SESSION");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::remove_var("BROWSER_SESSION");
    let bin = temp.path().join("browser/browser");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, r#"#!/usr/bin/env python3
import json, os, socket, sys
runtime = os.environ.get('XDG_RUNTIME_DIR', '/tmp')
if sys.argv[1:4] == ['session', 'start', '--help']:
    print('--bind-window')
elif sys.argv[1:3] == ['session', 'start']:
    name = sys.argv[3]
    path = runtime + '/browser-session-' + name
    with open(os.environ['JCODE_HOME'] + '/starts', 'a') as f:
        f.write(name + '\n')
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(path + '.sock')
    listener.listen()
    with open(path + '.pid', 'w') as f:
        f.write(str(os.getpid()))
    while True:
        conn, _ = listener.accept()
        with conn:
            if conn.recv(4096):
                conn.sendall((json.dumps({'ok': True, 'pid': os.getpid(), 'session': name}) + '\n').encode())
else:
    name = os.environ['BROWSER_SESSION']
    with socket.socket(socket.AF_UNIX) as conn:
        conn.connect(runtime + '/browser-session-' + name + '.sock')
        conn.sendall(b'{}\n')
        print(conn.recv(4096).decode())
"#).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = ToolContext {
        session_id: format!("browser-reuse-test-{}", std::process::id()),
        message_id: "m".into(),
        tool_call_id: "t".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    };
    let started = std::time::Instant::now();
    let mut responses = Vec::new();
    for _ in 0..5 {
        responses
            .push(firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &ctx).await);
    }
    let elapsed = started.elapsed();
    let name = format!("{}-chrome", ctx.session_id);
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into());
    let pid_file = runtime.join(format!("browser-session-{name}.pid"));
    if let Ok(pid) = std::fs::read_to_string(&pid_file)
        && let Ok(pid) = pid.parse::<i32>()
    {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    let _ = std::fs::remove_file(pid_file);
    let _ = std::fs::remove_file(runtime.join(format!("browser-session-{name}.sock")));
    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    match previous_session {
        Some(value) => crate::env::set_var("BROWSER_SESSION", value),
        None => crate::env::remove_var("BROWSER_SESSION"),
    }
    let responses: Vec<_> = responses.into_iter().map(Result::unwrap).collect();
    assert!(
        responses.windows(2).all(|pair| pair[0] == pair[1]),
        "all calls must reach the same daemon"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "healthy calls took {elapsed:?}"
    );
}
