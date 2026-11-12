//! Exercises production telemetry HTTP delivery, not the cfg(test) payload sink.
//! The subprocess proxy accepts CONNECT and never answers. No telemetry leaves
//! loopback, and process-global clients/environment cannot leak between tests.
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn session_creation_does_not_wait_for_unreachable_telemetry() {
    const CHILD: &str = "JCODE_TEST_SESSION_LATENCY_CHILD";
    if std::env::var_os(CHILD).is_some() {
        jcode_telemetry_core::begin_session("test-provider", "test-model");
        jcode_telemetry_core::record_turn();
        // record_turn queues session_start even when the endpoint is offline.
        // Replacing an active session must preserve its end events without
        // waiting for their network delivery on the new session's critical path.
        let start = Instant::now();
        jcode_telemetry_core::begin_session("test-provider", "test-model");
        let elapsed = start.elapsed();
        println!(
            "superseded_session_creation_ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "session creation waited for telemetry: {elapsed:?}"
        );
        return;
    }

    let home = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "session_creation_does_not_wait_for_unreachable_telemetry",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("JCODE_HOME", home.path())
        .env_remove("JCODE_NO_TELEMETRY")
        .env_remove("DO_NOT_TRACK")
        .env("HTTPS_PROXY", &proxy)
        .env("https_proxy", &proxy)
        .env("ALL_PROXY", &proxy)
        .env("all_proxy", &proxy)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .spawn()
        .unwrap();
    let start = Instant::now();
    let mut connections = Vec::new();
    loop {
        while let Ok((stream, _)) = listener.accept() {
            connections.push(stream);
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                !connections.is_empty(),
                "test must exercise the actual HTTP transport"
            );
            assert!(
                status.success(),
                "telemetry latency subprocess failed: {status}"
            );
            break;
        }
        if start.elapsed() > Duration::from_secs(15) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("telemetry latency subprocess hung");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
