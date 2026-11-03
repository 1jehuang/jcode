use super::*;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;

fn sequenced_server(
    steps: Vec<&'static str>,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::thread::JoinHandle<usize>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let worker = std::thread::spawn(move || {
        let count = steps.len();
        for step in steps {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(frame["type"], "upgrade_runtime");
            writeln!(
                stream,
                "{}",
                json!({"type":"reload_progress", "id":1, "step":step, "message":"status"})
            )
            .unwrap();
        }
        count
    });
    (dir, path, worker)
}

#[test]
fn deferred_busy_upgrade_retries_then_stops_when_runtime_is_current() {
    let (_dir, path, worker) = sequenced_server(vec!["busy", "busy", "unchanged"]);
    assert_eq!(
        retry_until_idle(&path, Path::new("/bundle/jcode"), 5, Duration::ZERO).unwrap(),
        Outcome::Unchanged
    );
    assert_eq!(worker.join().unwrap(), 3);
}

#[test]
fn busy_retry_is_bounded_and_reports_reopen_app_after_idle() {
    let (_dir, path, worker) = sequenced_server(vec!["busy", "busy"]);
    let Outcome::Deferred(message) =
        retry_until_idle(&path, Path::new("/bundle/jcode"), 2, Duration::ZERO).unwrap()
    else {
        panic!("expected pending state")
    };
    assert!(message.contains("reopen"));
    assert_eq!(worker.join().unwrap(), 2);
}

#[test]
fn reconnect_confirmation_requires_version_decision_not_socket_connect() {
    let (_dir, path, worker) = sequenced_server(vec!["unchanged"]);
    confirm_reconnect(&path, Path::new("/bundle/jcode"), Duration::from_secs(1)).unwrap();
    assert_eq!(worker.join().unwrap(), 1);
    let (_dir, path, worker) = sequenced_server(vec!["failed"]);
    assert!(confirm_reconnect(&path, Path::new("/bundle/jcode"), Duration::from_secs(1)).is_err());
    worker.join().unwrap();
}

#[test]
fn custom_remote_or_path_fallback_never_qualifies_for_local_upgrade() {
    let runtime = jcode_harness_api::runtime_dir();
    let api = runtime.join("jcode-api.sock");
    let native = runtime.join("jcode.sock");
    assert!(is_local_shared(&api, &native, Path::new("/bundle/jcode")));
    assert!(!is_local_shared(
        Path::new("/forwarded/remote.sock"),
        &native,
        Path::new("/bundle/jcode")
    ));
    assert!(!is_local_shared(
        &api,
        Path::new("/forwarded/native.sock"),
        Path::new("/bundle/jcode")
    ));
    assert!(!is_local_shared(&api, &native, Path::new("jcode")));
}

fn negotiate(replies: Vec<Value>) -> (Outcome, Value) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["type"], "upgrade_runtime");
        assert!(request.get("force").is_none());
        for reply in replies {
            writeln!(stream, "{reply}").unwrap();
        }
        // Legacy fallback must close rather than issue a second Reload request.
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
        request
    });
    let outcome = request(
        &path,
        Path::new("/bundle/bin/jcode"),
        Duration::from_secs(1),
    )
    .unwrap();
    (outcome, server.join().unwrap())
}

#[test]
fn busy_runtime_defers_with_clear_status_without_reload_fallback() {
    let (outcome, sent) = negotiate(vec![
        json!({"type":"reload_progress","id":1,"step":"busy","message":"Upgrade deferred: active tools running"}),
    ]);
    assert_eq!(
        outcome,
        Outcome::Busy("Upgrade deferred: active tools running".into())
    );
    assert_eq!(sent["candidate"], "/bundle/bin/jcode");
}

#[test]
fn old_runtime_unknown_request_is_compatible_and_does_not_force_restart() {
    let (outcome, _) = negotiate(vec![
        json!({"type":"error","id":0,"message":"Invalid request: unknown variant upgrade_runtime"}),
    ]);
    let Outcome::Deferred(message) = outcome else {
        panic!("expected deferred old runtime")
    };
    assert!(message.contains("predates safe bundled upgrades"));
    assert!(message.contains("once all work is idle"));
}

#[test]
fn no_downgrade_or_unchanged_runtime_does_not_restart() {
    let (outcome, _) = negotiate(vec![
        json!({"type":"reload_progress","id":1,"step":"unchanged","message":"Already newer"}),
    ]);
    assert_eq!(outcome, Outcome::Unchanged);
}

#[test]
fn failed_exec_preserves_old_runtime_status() {
    let (outcome, _) = negotiate(vec![
        json!({"type":"reload_progress","id":1,"step":"upgrading"}),
        json!({"type":"reload_progress","id":1,"step":"failed","message":"Old runtime retained"}),
    ]);
    assert_eq!(outcome, Outcome::Deferred("Old runtime retained".into()));
}

#[test]
fn accepted_upgrade_requires_exec_connection_close_before_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        writeln!(
            stream,
            "{}",
            json!({"type":"reload_progress","id":1,"step":"upgrading"})
        )
        .unwrap();
    });
    assert_eq!(
        request(&path, Path::new("/bundle/jcode"), Duration::from_secs(1)).unwrap(),
        Outcome::ReconnectPending
    );
    server.join().unwrap();
}

#[test]
fn upgrade_negotiation_is_bounded_for_unresponsive_old_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        std::thread::sleep(Duration::from_millis(100));
    });
    let start = Instant::now();
    assert!(request(&path, Path::new("/bundle/jcode"), Duration::from_millis(20)).is_err());
    assert!(start.elapsed() < Duration::from_millis(300));
    server.join().unwrap();
}
