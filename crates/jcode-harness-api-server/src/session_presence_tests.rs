//! Transport-level regression tests for globally connected user metadata.
use super::*;
use serde_json::json;
use tokio::net::UnixListener;

struct SocketDir(std::path::PathBuf);
impl SocketDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "jcode-presence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for SocketDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn snapshot(reply: Value) -> std::collections::HashSet<String> {
    let dir = SocketDir::new();
    let path = dir.path().join("presence.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        read_frame(&mut reader, &mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"type":"ping", "id":0})
        );
        write_json_line(reader.get_mut(), &reply).await.unwrap();
    });
    let result = daemon_user_connections(&path).await;
    server.await.unwrap();
    result
}

#[tokio::test]
async fn presence_probe_is_read_only_correlated_and_deduplicated() {
    assert_eq!(
        snapshot(
            json!({"type":"pong","id":0,"user_connected_session_ids":["ram","idle","ram",""]})
        )
        .await,
        ["ram".into(), "idle".into()].into_iter().collect()
    );
    assert!(
        snapshot(json!({"type":"pong","id":1,"user_connected_session_ids":["wrong"]}))
            .await
            .is_empty()
    );
    assert!(
        snapshot(json!({"type":"state","id":0,"user_connected_session_ids":["wrong"]}))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn old_malformed_and_disconnected_sources_never_claim_connected_users() {
    assert!(snapshot(json!({"type":"pong","id":0})).await.is_empty());
    assert!(
        snapshot(json!({"type":"pong","id":0,"user_connected_session_ids":[7]}))
            .await
            .is_empty()
    );
    let dir = SocketDir::new();
    assert!(
        daemon_user_connections(&dir.path().join("missing.sock"))
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn stalled_presence_probe_is_bounded() {
    let dir = SocketDir::new();
    let path = dir.path().join("stalled.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let start = std::time::Instant::now();
    assert!(daemon_user_connections(&path).await.is_empty());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    drop(listener);
}
