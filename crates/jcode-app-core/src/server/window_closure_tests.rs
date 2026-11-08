#![allow(clippy::await_holding_lock)]
use super::*;

struct Runtime {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}
impl Runtime {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", dir.path());
        Self {
            _dir: dir,
            previous,
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => crate::env::set_var("JCODE_RUNTIME_DIR", value),
            None => crate::env::remove_var("JCODE_RUNTIME_DIR"),
        }
    }
}
fn connection(id: &str) -> ClientConnectionInfo {
    ClientConnectionInfo {
        client_id: id.into(),
        session_id: "session_closure_test".into(),
        client_instance_id: None,
        debug_client_id: None,
        connected_at: std::time::Instant::now(),
        last_seen: std::time::Instant::now(),
        is_processing: false,
        current_tool_name: None,
        terminal_env: vec![],
        disconnect_tx: mpsc::unbounded_channel().0,
    }
}
fn closures() -> HashMap<String, chrono::DateTime<chrono::Utc>> {
    crate::session::recent_window_closures(Duration::from_secs(30))
}

#[tokio::test]
async fn only_final_transport_close_records_time_and_cleanup_does_not_refresh_it() {
    let _lock = crate::storage::lock_test_env();
    let _runtime = Runtime::new();
    let connections = Arc::new(RwLock::new(HashMap::from([
        ("first".into(), connection("first")),
        ("last".into(), connection("last")),
    ])));
    record_transport_closed("session_closure_test", "first", &connections).await;
    assert!(closures().is_empty(), "a second window is still attached");
    record_transport_closed("session_closure_test", "last", &connections).await;
    let first = closures();
    assert!(first.contains_key("session_closure_test"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    record_transport_closed("session_closure_test", "last", &connections).await;
    assert_eq!(
        closures(),
        first,
        "slow repeated cleanup must not renew the event"
    );
    crate::session::clear_window_closed("session_closure_test");
    assert!(
        closures().is_empty(),
        "reattach clears the old close without sending a message"
    );
}

#[tokio::test]
async fn explicit_detach_then_socket_close_is_not_a_window_close() {
    let _lock = crate::storage::lock_test_env();
    let _runtime = Runtime::new();
    let connections = Arc::new(RwLock::new(HashMap::from([(
        "first".into(),
        connection("first"),
    )])));
    detach_client_attachment(
        "session_closure_test",
        "first",
        "debug",
        &connections,
        &Arc::new(RwLock::new(ClientDebugState::default())),
        &Arc::new(RwLock::new(HashMap::new())),
    )
    .await;
    record_transport_closed("session_closure_test", "first", &connections).await;
    assert!(closures().is_empty());
}

#[tokio::test]
async fn server_reload_does_not_manufacture_window_close() {
    let _lock = crate::storage::lock_test_env();
    let _runtime = Runtime::new();
    crate::server::write_reload_state(
        "closure-test",
        "hash",
        crate::server::ReloadPhase::Starting,
        None,
    );
    let connections = Arc::new(RwLock::new(HashMap::from([(
        "first".into(),
        connection("first"),
    )])));
    record_transport_closed("session_closure_test", "first", &connections).await;
    assert!(closures().is_empty());
}
