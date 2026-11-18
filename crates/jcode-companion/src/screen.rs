//! Explicitly opted-in, metadata-only local Niri observation.
//! No screenshots, input monitoring, focus changes, or model requests.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub const MAX_WINDOWS: usize = 32;
pub const MAX_TITLE_BYTES: usize = 256;
const MAX_REPLY_BYTES: u64 = 1024 * 1024;

fn bounded_text(value: &Value, max: usize) -> Value {
    match value.as_str() {
        Some(text) => {
            let mut end = text.len().min(max);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(text[..end].chars().filter(|c| !c.is_control()).collect())
        }
        None => Value::Null,
    }
}

/// Geometry is explicitly in Niri's scrolling-layout coordinate space.
/// Workspace activity does not prove a tile is visible on the output.
pub fn normalize(windows: &Value, workspaces: &Value, outputs: &Value) -> Value {
    let mut ordered: Vec<&Value> = windows.as_array().into_iter().flatten().collect();
    ordered.sort_by_key(|w| {
        (
            !w["is_focused"].as_bool().unwrap_or(false),
            w["id"].as_u64(),
        )
    });
    let windows: Vec<Value> = ordered.into_iter().take(MAX_WINDOWS).map(|w| {
        let layout = &w["layout"];
        json!({"id":w["id"].as_u64(), "app_id":bounded_text(&w["app_id"],128),
            "title":bounded_text(&w["title"],MAX_TITLE_BYTES), "workspace_id":w["workspace_id"].as_u64(),
            "is_focused":w["is_focused"].as_bool(), "visibility":"unknown",
            "layout":{"coordinate_space":"niri_scrolling_layout", "tile_size":layout["tile_size"],
                "window_size":layout["window_size"], "pos_in_scrolling_layout":layout["pos_in_scrolling_layout"],
                "tile_pos_in_workspace_view":layout["tile_pos_in_workspace_view"]}})
    }).collect();
    let workspaces: Vec<Value> = workspaces.as_array().into_iter().flatten().take(32).map(|w|
        json!({"id":w["id"], "name":bounded_text(&w["name"],128), "output":bounded_text(&w["output"],128),
            "is_active":w["is_active"], "is_focused":w["is_focused"]})).collect();
    let outputs: Vec<Value> = outputs
        .as_object()
        .into_iter()
        .flat_map(|o| o.iter())
        .take(8)
        .map(|(name, o)| {
            json!({"name":bounded_text(&Value::String(name.clone()),128),"logical":o["logical"],
            "coordinate_space":"niri_logical_output"})
        })
        .collect();
    let mut snapshot = json!({"source":"niri", "interaction":"unknown", "visible_window_ids":null,
        "windows":windows,"workspaces":workspaces,"outputs":outputs});
    // Leave room for the wire envelope. Focused windows sort first and survive
    // truncation. Never grow a pending history to preserve every observed tile.
    while serde_json::to_vec(&snapshot).map_or(true, |v| v.len() > 15 * 1024) {
        let windows = snapshot["windows"].as_array_mut().unwrap();
        if windows.len() > 1 {
            windows.pop();
            continue;
        }
        let workspaces = snapshot["workspaces"].as_array_mut().unwrap();
        if !workspaces.is_empty() {
            workspaces.pop();
            continue;
        }
        let outputs = snapshot["outputs"].as_array_mut().unwrap();
        if !outputs.is_empty() {
            outputs.pop();
            continue;
        }
        // A malformed oversized layout is not useful desktop context.
        if let Some(window) = snapshot["windows"].as_array_mut().unwrap().first_mut() {
            window["layout"] = Value::Null;
        } else {
            break;
        }
    }
    snapshot
}

#[cfg(unix)]
mod local {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::sync::watch;
    use tokio::task::JoinHandle;
    use tokio::time::{Duration, Instant, sleep, timeout};

    async fn buffered_line(
        reader: &mut BufReader<UnixStream>,
        bytes: &mut Vec<u8>,
    ) -> Result<Value> {
        let remaining = MAX_REPLY_BYTES.saturating_sub(bytes.len() as u64);
        let count = reader.take(remaining).read_until(b'\n', bytes).await?;
        if count == 0 || !bytes.ends_with(b"\n") {
            bail!("Niri reply missing or oversized");
        }
        let result = serde_json::from_slice(bytes);
        bytes.clear();
        Ok(result?)
    }

    async fn line(reader: &mut BufReader<UnixStream>) -> Result<Value> {
        buffered_line(reader, &mut Vec::new()).await
    }

    async fn request(path: &std::path::Path, name: &str) -> Result<Value> {
        timeout(Duration::from_secs(2), async {
            let mut socket = UnixStream::connect(path).await?;
            socket.write_all(format!("\"{name}\"\n").as_bytes()).await?;
            let reply = line(&mut BufReader::new(socket)).await?;
            reply
                .get("Ok")
                .and_then(|ok| ok.get(name))
                .cloned()
                .context("Unexpected Niri response")
        })
        .await
        .context("Niri snapshot timed out")?
    }

    /// A watch channel retains exactly one latest observation. Slow consumers
    /// never accumulate historical window titles. Dropping this handle closes
    /// both IPC sockets by aborting the owning task.
    pub struct ScreenFeed {
        latest: watch::Receiver<Option<Value>>,
        task: JoinHandle<()>,
    }
    impl Drop for ScreenFeed {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl ScreenFeed {
        pub async fn next(&mut self) -> Option<Value> {
            self.latest.changed().await.ok()?;
            self.latest.borrow_and_update().clone()
        }
    }

    pub async fn subscribe() -> Result<ScreenFeed> {
        let path = std::path::PathBuf::from(
            std::env::var_os("NIRI_SOCKET").context("NIRI_SOCKET is not set")?,
        );
        subscribe_path(path).await
    }

    async fn subscribe_path(path: std::path::PathBuf) -> Result<ScreenFeed> {
        let mut stream = UnixStream::connect(&path).await?;
        stream.write_all(b"\"EventStream\"\n").await?;
        let mut events = BufReader::new(stream);
        let reply = timeout(Duration::from_secs(2), line(&mut events)).await??;
        if reply.get("Ok").is_none() {
            bail!("Niri rejected event subscription");
        }
        let (tx, latest) = watch::channel(None);
        let task = tokio::spawn(async move {
            let mut previous = Value::Null;
            // Keep partial lines across debounce cancellation boundaries.
            let mut event_bytes = Vec::new();
            let mut next_publish = Instant::now();
            loop {
                // Refresh authoritative snapshots, never interpret individual
                // focus events as user activity or model instructions.
                let snapshot = async {
                    let windows = request(&path, "Windows").await?;
                    let workspaces = request(&path, "Workspaces").await?;
                    let outputs = request(&path, "Outputs").await?;
                    anyhow::Ok(normalize(&windows, &workspaces, &outputs))
                }
                .await;
                let Ok(snapshot) = snapshot else {
                    break;
                };
                if serde_json::to_vec(&snapshot).map_or(true, |bytes| bytes.len() > 16 * 1024) {
                    break;
                }
                if snapshot != previous {
                    previous = snapshot.clone();
                    if tx.send(Some(snapshot)).is_err() {
                        break;
                    }
                    next_publish = Instant::now() + Duration::from_secs(2);
                }
                if buffered_line(&mut events, &mut event_bytes).await.is_err() {
                    break;
                }
                // Drain bursts while waiting for the later of debounce and
                // rate limit. No unbounded event queue or pending snapshots.
                let deadline = next_publish.max(Instant::now() + Duration::from_millis(500));
                loop {
                    tokio::select! {
                        _ = sleep(deadline.saturating_duration_since(Instant::now())) => break,
                        event = buffered_line(&mut events, &mut event_bytes) => { if event.is_err() { return; } }
                    }
                }
            }
        });
        Ok(ScreenFeed { latest, task })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::net::UnixListener;

        #[tokio::test]
        async fn burst_is_coalesced_latest_only_and_drop_closes_subscription() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("niri.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let revision = Arc::new(AtomicUsize::new(0));
            let reads = Arc::new(AtomicUsize::new(0));
            let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<()>(1);
            let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
            let revision_server = revision.clone();
            let reads_server = reads.clone();
            let server = tokio::spawn(async move {
                let mut event_socket: Option<UnixStream> = None;
                let mut closed_tx = Some(closed_tx);
                loop {
                    tokio::select! {
                        socket = listener.accept() => {
                            let (socket, _) = socket.unwrap();
                            let mut reader = BufReader::new(socket);
                            let request = line(&mut reader).await.unwrap();
                            let name = request.as_str().unwrap();
                            if name == "EventStream" {
                                reader.get_mut().write_all(b"{\"Ok\":\"Handled\"}\n").await.unwrap();
                                event_socket = Some(reader.into_inner());
                            } else {
                                reads_server.fetch_add(1, Ordering::SeqCst);
                                let payload = match name {
                                    "Windows" => json!([{"id":1,"title":revision_server.load(Ordering::SeqCst).to_string()}]),
                                    "Workspaces" => json!([]),
                                    _ => json!({}),
                                };
                                let response = json!({"Ok":{name:payload}}).to_string()+"\n";
                                reader.get_mut().write_all(response.as_bytes()).await.unwrap();
                            }
                        }
                        _ = event_rx.recv() => {
                            if let Some(socket) = &mut event_socket {
                                for _ in 0..100 { socket.write_all(b"{\"WindowFocusChanged\":{}}\n").await.unwrap(); }
                            }
                        }
                        result = async { match &mut event_socket {
                            Some(socket) => { let mut byte=[0]; socket.read(&mut byte).await },
                            None => std::future::pending().await,
                        }} => {
                            if result.unwrap() == 0 { let _ = closed_tx.take().unwrap().send(()); return; }
                        }
                    }
                }
            });
            let mut feed = subscribe_path(path).await.unwrap();
            let first = timeout(Duration::from_secs(1), feed.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(first["windows"][0]["title"], "0");
            revision.store(100, Ordering::SeqCst);
            event_tx.send(()).await.unwrap();
            assert!(
                timeout(Duration::from_millis(300), feed.next())
                    .await
                    .is_err()
            );
            let latest = timeout(Duration::from_secs(3), feed.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(latest["windows"][0]["title"], "100");
            assert_eq!(reads.load(Ordering::SeqCst), 6);
            assert!(
                timeout(Duration::from_millis(100), feed.next())
                    .await
                    .is_err()
            );
            drop(feed);
            timeout(Duration::from_secs(1), closed_rx)
                .await
                .unwrap()
                .unwrap();
            server.await.unwrap();
        }

        #[tokio::test]
        #[ignore = "requires the local graphical Niri session"]
        async fn local_niri_snapshot_smoke() {
            let mut feed = subscribe().await.unwrap();
            let snapshot = timeout(Duration::from_secs(5), feed.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(snapshot["source"], "niri");
            assert!(snapshot["windows"].is_array());
            assert_eq!(snapshot["interaction"], "unknown");
        }
    }
}
#[cfg(unix)]
pub use local::{ScreenFeed, subscribe};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_is_bounded_and_never_claims_visibility_or_activity() {
        let windows = Value::Array(
            (0..100)
                .map(|id| {
                    json!({"id":id,"title":"界".repeat(500),
            "app_id":"app", "is_focused":id==99, "workspace_id":1,
            "layout":{"tile_size":[900,1105]}})
                })
                .collect(),
        );
        let snapshot = normalize(&windows, &json!([]), &json!({}));
        let windows = snapshot["windows"].as_array().unwrap();
        assert!(windows.len() <= MAX_WINDOWS && !windows.is_empty());
        assert_eq!(windows[0]["id"], 99);
        assert!(
            windows
                .iter()
                .all(|w| w["title"].as_str().unwrap().len() <= MAX_TITLE_BYTES)
        );
        assert_eq!(snapshot["interaction"], "unknown");
        assert!(snapshot["visible_window_ids"].is_null());
        assert_eq!(windows[0]["visibility"], "unknown");
        assert!(windows[0]["layout"]["tile_pos_in_workspace_view"].is_null());
        assert!(serde_json::to_vec(&snapshot).unwrap().len() <= 15 * 1024);
    }
    #[test]
    fn snapshot_is_deterministic_across_compositor_window_order() {
        let a = json!({"id":1,"title":"a"});
        let b = json!({"id":2,"title":"b"});
        assert_eq!(
            normalize(&json!([a, b]), &json!([]), &json!({})),
            normalize(&json!([b, a]), &json!([]), &json!({}))
        );
    }
}
