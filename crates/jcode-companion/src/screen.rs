//! Explicitly opted-in, metadata-only local Niri observation.
//! Wire schema: https://docs.rs/niri-ipc/latest/niri_ipc/enum.Request.html
//! No screenshots, OCR, input monitoring, focus changes, or compositor actions.
use anyhow::{Context, Result, ensure};
use jcode_base::protocol::{ScreenInteraction, ScreenOutput, ScreenSnapshot, ScreenWindow};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::watch;

const DEBOUNCE: Duration = Duration::from_millis(500);
const RATE: Duration = Duration::from_millis(2_050);
const HEARTBEAT: Duration = Duration::from_secs(15);
const MAX_IPC_BYTES: usize = 1024 * 1024;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn text(value: &Value, max: usize) -> Option<String> {
    let mut result = String::new();
    for ch in value.as_str()?.chars().filter(|ch| !ch.is_control()) {
        if result.len() + ch.len_utf8() > max {
            break;
        }
        result.push(ch);
    }
    Some(result)
}
fn geometry(value: &Value, fields: &[&str]) -> Option<Value> {
    let source = value.as_object()?;
    let mut result = serde_json::Map::new();
    for field in fields {
        if let Some(value) = source.get(*field) {
            // Numeric geometry only. Do not relay arbitrary compositor strings.
            let valid = value.is_number()
                || value.is_null()
                || value
                    .as_array()
                    .is_some_and(|v| v.len() == 2 && v.iter().all(Value::is_number));
            if valid {
                result.insert((*field).into(), value.clone());
            }
        }
    }
    result.insert(
        "coordinate_space".into(),
        json!("niri_logical_pixels_not_global_window_rect"),
    );
    Some(Value::Object(result))
}

fn snapshot(
    windows: &Value,
    outputs: &Value,
    sequence: u64,
    observed_at_ms: u64,
) -> Result<ScreenSnapshot> {
    let source_windows = windows
        .as_array()
        .context("Niri Windows response is not an array")?;
    let source_outputs = outputs
        .as_object()
        .context("Niri Outputs response is not an object")?;
    // Focus first so truncation cannot drop the foreground window.
    let mut ordered: Vec<_> = source_windows.iter().collect();
    ordered.sort_by_key(|w| {
        (
            w.get("is_focused").and_then(Value::as_bool) != Some(true),
            w.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX),
        )
    });
    let mut result = ScreenSnapshot {
        sequence,
        observed_at_ms,
        source: "niri".into(),
        interaction: ScreenInteraction::Unknown,
        focused_window: None,
        windows: vec![],
        outputs: vec![],
    };
    for window in ordered
        .into_iter()
        .take(jcode_base::protocol::SCREEN_MAX_WINDOWS)
    {
        let id = window
            .get("id")
            .and_then(Value::as_u64)
            .context("Niri window missing id")?;
        if window.get("is_focused").and_then(Value::as_bool) == Some(true) {
            result.focused_window = Some(id);
        }
        result.windows.push(ScreenWindow {
            id,
            app_id: text(&window["app_id"], 128),
            title: text(&window["title"], 512),
            workspace_id: window.get("workspace_id").and_then(Value::as_u64),
            // Active workspace and focus alone do not establish visibility or occlusion.
            visible: None,
            layout: geometry(
                &window["layout"],
                &[
                    "pos_in_scrolling_layout",
                    "tile_size",
                    "window_size",
                    "tile_pos_in_workspace_view",
                    "window_offset_in_tile",
                ],
            ),
        });
    }
    let mut ordered_outputs: Vec<_> = source_outputs.iter().collect();
    ordered_outputs.sort_by_key(|(name, _)| *name);
    for (name, output) in ordered_outputs
        .into_iter()
        .take(jcode_base::protocol::SCREEN_MAX_OUTPUTS)
    {
        let logical = &output["logical"];
        result.outputs.push(ScreenOutput {
            name: text(&json!(name), 128).unwrap_or_default(),
            logical: geometry(logical, &["x", "y", "width", "height"]),
            scale: logical
                .get("scale")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n > 0.0),
            transform: text(&logical["transform"], 64),
        });
    }
    // Total bound takes precedence over count. Preserve focus at index zero.
    while serde_json::to_vec(&result)?.len() > jcode_base::protocol::SCREEN_MAX_BYTES {
        if result.windows.len() > 1 {
            result.windows.pop();
        } else if !result.outputs.is_empty() {
            result.outputs.pop();
        } else {
            anyhow::bail!("Niri snapshot cannot fit protocol bound");
        }
    }
    result.validate().map_err(anyhow::Error::msg)?;
    Ok(result)
}

#[derive(Default)]
struct Schedule {
    first_event: Option<Instant>,
    last_event: Option<Instant>,
    last_publish: Option<Instant>,
}
impl Schedule {
    fn event(&mut self, now: Instant) {
        self.first_event.get_or_insert(now);
        self.last_event = Some(now);
    }
    fn deadline(&self, now: Instant) -> Instant {
        let heartbeat = self.last_publish.map_or(now, |last| last + HEARTBEAT);
        let Some(first) = self.first_event else {
            return heartbeat;
        };
        let debounced = (self.last_event.unwrap_or(first) + DEBOUNCE).min(first + RATE);
        let throttled = self
            .last_publish
            .map_or(debounced, |last| debounced.max(last + RATE));
        heartbeat.min(throttled)
    }
    fn published(&mut self, now: Instant) {
        self.last_publish = Some(now);
        self.first_event = None;
        self.last_event = None;
    }
}
fn relevant(event: &Value) -> bool {
    event.as_object().is_some_and(|object| {
        object.keys().any(|key| {
            matches!(
                key.as_str(),
                "WindowsChanged"
                    | "WindowOpenedOrChanged"
                    | "WindowClosed"
                    | "WindowFocusChanged"
                    | "WindowLayoutsChanged"
                    | "WorkspacesChanged"
                    | "WorkspaceActivated"
                    | "WorkspaceActiveWindowChanged"
                    | "ConfigLoaded"
                    | "OverviewOpenedOrClosed"
            )
        })
    })
}
fn same_metadata(a: &ScreenSnapshot, b: &ScreenSnapshot) -> bool {
    a.source == b.source
        && a.interaction == b.interaction
        && a.focused_window == b.focused_window
        && a.windows == b.windows
        && a.outputs == b.outputs
}

#[cfg(unix)]
async fn line_bounded<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> Result<()> {
    use tokio::io::AsyncBufReadExt;
    loop {
        let buf = reader.fill_buf().await?;
        ensure!(!buf.is_empty(), "Niri IPC disconnected");
        let end = buf.iter().position(|byte| *byte == b'\n');
        let count = end.map_or(buf.len(), |i| i + 1);
        ensure!(
            line.len() + count <= MAX_IPC_BYTES,
            "Niri IPC line exceeds 1 MiB"
        );
        line.extend_from_slice(&buf[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(());
        }
    }
}
#[cfg(unix)]
async fn query(path: &std::path::Path, kind: &str) -> Result<Value> {
    use tokio::io::{AsyncWriteExt, BufReader};
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut socket = tokio::net::UnixStream::connect(path).await?;
        socket.write_all(format!("\"{kind}\"\n").as_bytes()).await?;
        let mut line = vec![];
        line_bounded(&mut BufReader::new(socket), &mut line).await?;
        let reply: Value = serde_json::from_slice(&line)?;
        reply
            .get("Ok")
            .and_then(|ok| ok.get(kind))
            .cloned()
            .context("Unexpected Niri reply")
    })
    .await?
}

/// One replaceable watch slot, no event history or normal user messages.
#[cfg(unix)]
pub async fn run(sender: watch::Sender<Option<ScreenSnapshot>>) -> Result<()> {
    let path =
        std::env::var_os("NIRI_SOCKET").context("Screen context requires a local NIRI_SOCKET")?;
    run_at(sender, std::path::PathBuf::from(path)).await
}

#[cfg(unix)]
async fn run_at(
    sender: watch::Sender<Option<ScreenSnapshot>>,
    path: std::path::PathBuf,
) -> Result<()> {
    use tokio::io::{AsyncWriteExt, BufReader};
    let mut stream = tokio::net::UnixStream::connect(&path)
        .await
        .context("Connect local Niri metadata stream")?;
    stream.write_all(b"\"EventStream\"\n").await?;
    let mut stream = BufReader::new(stream);
    let mut line = vec![];
    tokio::time::timeout(Duration::from_secs(2), line_bounded(&mut stream, &mut line)).await??;
    let reply: Value = serde_json::from_slice(&line)?;
    ensure!(
        reply.get("Ok") == Some(&json!("Handled")),
        "Niri did not accept metadata event subscription"
    );
    line.clear();
    let mut schedule = Schedule::default();
    let mut sequence = 0u64;
    let mut previous: Option<ScreenSnapshot> = None;
    let mut last_sent: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let heartbeat_deadline = last_sent.map_or(now, |last| last + HEARTBEAT);
        let deadline = schedule.deadline(now).min(heartbeat_deadline);
        tokio::select! {
            _ = sender.closed() => return Ok(()),
            result = line_bounded(&mut stream, &mut line) => {
                result?;
                let event: Value = serde_json::from_slice(&line)?;
                line.clear();
                if relevant(&event) { schedule.event(Instant::now()); }
            }
            _ = tokio::time::sleep_until(deadline.into()) => {
                // Refresh authoritative snapshots, not a guessed reduction of
                // event payloads. Separate IPC queries are not an atomic view.
                let windows = query(&path, "Windows").await?;
                let outputs = query(&path, "Outputs").await?;
                sequence = sequence.checked_add(1).context("Screen sequence exhausted")?;
                let next = snapshot(&windows, &outputs, sequence, now_ms())?;
                let now = Instant::now();
                let heartbeat = last_sent.is_none_or(|last| now.duration_since(last) >= HEARTBEAT);
                if heartbeat || previous.as_ref().is_none_or(|prev| !same_metadata(prev, &next)) {
                    sender.send_replace(Some(next.clone()));
                    previous = Some(next);
                    last_sent = Some(now);
                }
                schedule.published(now);
            }
        }
    }
}
#[cfg(not(unix))]
pub async fn run(_sender: watch::Sender<Option<ScreenSnapshot>>) -> Result<()> {
    anyhow::bail!("Niri metadata observation requires a local Unix compositor")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screen_burst_scheduler_is_bounded_debounced_and_trailing() {
        let start = Instant::now();
        let mut schedule = Schedule::default();
        schedule.published(start);
        for i in 0..100 {
            schedule.event(start + Duration::from_millis(i * 5));
        }
        assert_eq!(schedule.deadline(start), start + RATE);
        schedule.published(start + RATE);
        assert_eq!(schedule.deadline(start + RATE), start + RATE + HEARTBEAT);
        schedule.event(start + RATE + Duration::from_secs(3));
        assert_eq!(
            schedule.deadline(start),
            start + RATE + Duration::from_millis(3_500)
        );
    }
    #[test]
    fn screen_niri_schema_bounds_and_unknowns() {
        let windows = json!([{"id":5,"title":"x".repeat(900),"app_id":"org.browser","workspace_id":9,"is_focused":true,"layout":{"tile_size":[1000.0,800.0],"pos_in_scrolling_layout":[1,2]}}, {"id":6}]);
        let outputs = json!({"DP-1":{"logical":{"x":0,"y":0,"width":1920,"height":1080,"scale":1.25,"transform":"Normal"}}});
        let result = snapshot(&windows, &outputs, 1, 100).unwrap();
        assert_eq!(result.focused_window, Some(5));
        assert_eq!(result.windows[0].title.as_ref().unwrap().len(), 512);
        assert_eq!(result.windows[0].visible, None);
        assert_eq!(result.interaction, ScreenInteraction::Unknown);
        assert_eq!(result.outputs[0].scale, Some(1.25));
        assert_eq!(result.outputs[0].transform.as_deref(), Some("Normal"));
        assert!(result.windows[1].layout.is_none());
        assert!(!relevant(&json!({"ScreenshotCaptured":{"path":"private"}})));
        assert!(!relevant(
            &json!({"WindowFocusTimestampChanged":{"id":5,"focus_timestamp":100}})
        ));
        assert!(relevant(&json!({"WindowFocusChanged":{"id":5}})));
    }
    #[test]
    fn screen_latest_total_bound_and_dedupe() {
        let windows = Value::Array((0..100).map(|id| json!({"id":id,"title":"界".repeat(300),"app_id":"a".repeat(200),"is_focused":id == 99,"layout":{"tile_size":[100.0,100.0]}})).collect());
        let first = snapshot(&windows, &json!({}), 1, 100).unwrap();
        let second = snapshot(&windows, &json!({}), 2, 200).unwrap();
        assert!(same_metadata(&first, &second));
        assert_eq!(first.focused_window, Some(99));
        assert!(first.windows.len() <= 32);
        assert!(
            serde_json::to_vec(&first).unwrap().len() <= jcode_base::protocol::SCREEN_MAX_BYTES
        );
        assert_eq!(text(&json!("hello\n\u{1b}[31m"), 128).unwrap(), "hello[31m");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn screen_niri_socket_feed_is_readonly_and_stops_with_receiver() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("niri.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert_eq!(line, "\"EventStream\"\n");
            stream
                .get_mut()
                .write_all(b"{\"Ok\":\"Handled\"}\n")
                .await
                .unwrap();
            for _ in 0..100 {
                stream
                    .get_mut()
                    .write_all(b"{\"WindowFocusChanged\":{\"id\":42}}\n")
                    .await
                    .unwrap();
            }
            // Authoritative snapshot requests must be strictly read-only.
            for expected in ["Windows", "Outputs"] {
                let (query, _) = listener.accept().await.unwrap();
                let mut query = BufReader::new(query);
                let mut line = String::new();
                query.read_line(&mut line).await.unwrap();
                assert_eq!(line.trim(), format!("\"{expected}\""));
                let reply = if expected == "Windows" {
                    json!({"Ok":{"Windows":[{"id":42,"title":"final metadata","is_focused":true}]}})
                } else {
                    json!({"Ok":{"Outputs":{}}})
                };
                query
                    .get_mut()
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
            }
            // Keep compositor subscription live until the client drops it.
            let mut rest = String::new();
            assert_eq!(stream.read_line(&mut rest).await.unwrap(), 0);
        });
        let (tx, mut rx) = watch::channel(None);
        let collector = tokio::spawn(run_at(tx, path));
        tokio::time::timeout(Duration::from_secs(3), rx.changed())
            .await
            .unwrap()
            .unwrap();
        let observation = rx.borrow_and_update().clone().unwrap();
        assert_eq!(observation.focused_window, Some(42));
        assert_eq!(
            observation.windows[0].title.as_deref(),
            Some("final metadata")
        );
        assert_eq!(observation.interaction, ScreenInteraction::Unknown);
        drop(rx);
        tokio::time::timeout(Duration::from_secs(1), collector)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn screen_line_reader_bounds_local_ipc() {
        use tokio::io::BufReader;
        let bytes = vec![b'x'; MAX_IPC_BYTES + 1];
        let mut reader = BufReader::new(bytes.as_slice());
        let mut line = vec![];
        assert!(line_bounded(&mut reader, &mut line).await.is_err());
        assert!(line.len() <= MAX_IPC_BYTES);
    }
}
