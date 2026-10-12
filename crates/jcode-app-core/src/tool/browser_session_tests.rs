#![allow(clippy::await_holding_lock)]
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

struct BrowserFixture {
    _env_guard: std::sync::MutexGuard<'static, ()>,
    temp: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_session: Option<std::ffi::OsString>,
}

impl BrowserFixture {
    fn new() -> Self {
        let guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        let previous_session = std::env::var_os("BROWSER_SESSION");
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::remove_var("BROWSER_SESSION");
        let bin = temp.path().join("browser/browser");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, r#"#!/usr/bin/env python3
import json, os, socket, sys, time
runtime = os.environ.get('XDG_RUNTIME_DIR', '/tmp')
home = os.environ['JCODE_HOME']
if sys.argv[1:4] == ['session', 'start', '--help']:
    print('--bind-window')
elif sys.argv[1:3] == ['session', 'start']:
    name = sys.argv[3]
    path = runtime + '/browser-session-' + name
    with open(home + '/starts', 'a') as f:
        f.write(json.dumps([name, os.getpid()]) + '\n')
    if 'blocked' in name:
        while not os.path.exists(home + '/release'):
            time.sleep(0.01)
    else:
        time.sleep(0.2)
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
        std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _env_guard: guard,
            temp,
            previous_home,
            previous_session,
        }
    }

    fn starts(&self) -> Vec<(String, i32)> {
        let path = self.temp.path().join("starts");
        if !path.exists() {
            return Vec::new();
        }
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn context(&self, name: &str) -> ToolContext {
        ToolContext {
            session_id: format!("brtest-{}-{name}", std::process::id()),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }
}

impl Drop for BrowserFixture {
    fn drop(&mut self) {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/tmp".into());
        for (name, pid) in self.starts() {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
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
async fn browser_session_reuses_daemon_across_five_tool_calls() {
    let fixture = BrowserFixture::new();
    let ctx = fixture.context("reuse");
    let started = Instant::now();
    let mut responses = Vec::new();
    for _ in 0..5 {
        responses.push(
            firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &ctx)
                .await
                .unwrap(),
        );
    }
    assert!(
        responses.windows(2).all(|pair| pair[0] == pair[1]),
        "all calls must reach the same daemon"
    );
    assert_eq!(fixture.starts().len(), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_session_concurrent_calls_start_one_daemon() {
    let fixture = BrowserFixture::new();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(6));
    let mut calls = Vec::new();
    for _ in 0..5 {
        let ctx = fixture.context("concurrent");
        let barrier = barrier.clone();
        calls.push(tokio::spawn(async move {
            barrier.wait().await;
            firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &ctx).await
        }));
    }
    barrier.wait().await;
    let mut responses = Vec::new();
    for call in calls {
        responses.push(call.await.unwrap());
    }
    assert_eq!(
        fixture.starts().len(),
        1,
        "overlapping calls must not create extra daemons/windows"
    );
    let responses: Vec<_> = responses.into_iter().map(Result::unwrap).collect();
    assert!(responses.windows(2).all(|pair| pair[0] == pair[1]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_session_stalled_startup_keeps_healthy_calls_and_workers_responsive() {
    let fixture = BrowserFixture::new();
    let healthy_ctx = fixture.context("healthy");
    firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &healthy_ctx)
        .await
        .unwrap();

    // The external watchdog bounds failures even if all Tokio workers block.
    let release_path = fixture.temp.path().join("release");
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        let _ = release_rx.recv_timeout(Duration::from_secs(3));
        std::fs::write(release_path, "ready").unwrap();
    });
    let started = Instant::now();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(5));
    let mut browser_calls = Vec::new();
    let mut bash_calls = Vec::new();
    for _ in 0..2 {
        let ctx = fixture.context("blocked-browser");
        let browser_barrier = barrier.clone();
        browser_calls.push(tokio::spawn(async move {
            browser_barrier.wait().await;
            firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &ctx).await
        }));
        let ctx = fixture.context("blocked-bash");
        let bash_barrier = barrier.clone();
        bash_calls.push(tokio::spawn(async move {
            bash_barrier.wait().await;
            crate::tool::bash::BashTool::new()
                .execute(json!({"command":"browser ping"}), ctx)
                .await
        }));
    }
    barrier.wait().await;
    while !fixture
        .starts()
        .iter()
        .any(|(name, _)| name.contains("blocked"))
        && started.elapsed() < Duration::from_secs(2)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let healthy = tokio::spawn(async move {
        firefox_run_bridge_command("listTabs", json!({}), Some("chrome"), &healthy_ctx).await
    });
    let heartbeat = tokio::spawn(async {
        tokio::task::yield_now().await;
    });
    let healthy_result = healthy.await.unwrap();
    heartbeat.await.unwrap();
    let elapsed = started.elapsed();
    release_tx.send(()).unwrap_or_default();
    watchdog.join().unwrap();
    for call in browser_calls {
        call.await.unwrap().unwrap();
    }
    for call in bash_calls {
        call.await.unwrap().unwrap();
    }
    healthy_result.unwrap();
    assert!(
        elapsed < Duration::from_secs(1),
        "healthy calls and unrelated tasks stalled for {elapsed:?}"
    );
    assert_eq!(fixture.starts().len(), 3, "one daemon per session/browser");
}
