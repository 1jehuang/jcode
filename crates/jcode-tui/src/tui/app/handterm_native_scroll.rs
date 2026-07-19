use super::{App, MouseScrollTarget};
#[cfg(unix)]
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedReceiver;
#[cfg(unix)]
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(unix)]
use std::thread::{self, JoinHandle};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
const ENV_SOCKET: &str = "HANDTERM_NATIVE_SCROLL_SOCKET";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PaneKind {
    Chat,
    SidePanel,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PaneState {
    pub kind: PaneKind,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub position: usize,
    pub content_length: usize,
    pub viewport_length: usize,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct PaneSnapshot {
    panes: Vec<PaneState>,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AppToHost {
    PaneSnapshot { panes: Vec<PaneState> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum HostToApp {
    Scroll { pane: PaneKind, delta: i32 },
}

pub(super) struct HandtermNativeScrollClient {
    #[cfg(unix)]
    updates_tx: Sender<AppToHost>,
    commands_rx: UnboundedReceiver<HostToApp>,
    #[cfg(unix)]
    last_sent: Option<PaneSnapshot>,
    #[cfg(unix)]
    stop: Arc<AtomicBool>,
    #[cfg(unix)]
    thread: Option<JoinHandle<()>>,
}

impl HandtermNativeScrollClient {
    pub(super) fn connect_from_env() -> Option<Self> {
        #[cfg(not(unix))]
        {
            None
        }

        #[cfg(unix)]
        {
            let socket_path = std::env::var_os(ENV_SOCKET).map(PathBuf::from)?;
            Self::connect(socket_path)
        }
    }

    #[cfg(unix)]
    fn connect(socket_path: PathBuf) -> Option<Self> {
        let (updates_tx, updates_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = spawn_bridge_thread(socket_path, updates_rx, commands_tx, stop.clone())?;
        Some(Self {
            updates_tx,
            commands_rx,
            last_sent: None,
            stop,
            thread: Some(thread),
        })
    }

    pub(super) fn sync_from_app(&mut self, app: &App) {
        #[cfg(not(unix))]
        {
            let _ = app;
            return;
        }

        #[cfg(unix)]
        {
            let snapshot = app.current_native_scroll_snapshot();
            if self.last_sent.as_ref() == Some(&snapshot) {
                return;
            }
            if self
                .updates_tx
                .send(AppToHost::PaneSnapshot {
                    panes: snapshot.panes.clone(),
                })
                .is_ok()
            {
                self.last_sent = Some(snapshot);
            }
        }
    }

    pub(super) async fn recv(&mut self) -> Option<HostToApp> {
        self.commands_rx.recv().await
    }
}

#[cfg(unix)]
impl Drop for HandtermNativeScrollClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl App {
    #[cfg(unix)]
    fn current_native_scroll_snapshot(&self) -> PaneSnapshot {
        let mut panes = Vec::new();
        if let Some(layout) = crate::tui::ui::last_layout_snapshot() {
            if self.chat_native_scrollbar {
                let viewport = layout.messages_area.height as usize;
                let max_scroll = crate::tui::ui::last_max_scroll();
                let position = if self.auto_scroll_paused {
                    self.scroll_offset.min(max_scroll)
                } else {
                    max_scroll
                };
                panes.push(PaneState {
                    kind: PaneKind::Chat,
                    x: layout.messages_area.x,
                    y: layout.messages_area.y,
                    width: layout.messages_area.width,
                    height: layout.messages_area.height,
                    position,
                    content_length: max_scroll.saturating_add(viewport),
                    viewport_length: viewport,
                });
            }

            if self.side_panel_native_scrollbar
                && let Some(area) = layout.diff_pane_area
            {
                let viewport = area.height as usize;
                let content_length = crate::tui::ui::pinned_pane_total_lines().max(viewport);
                panes.push(PaneState {
                    kind: PaneKind::SidePanel,
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: area.height,
                    position: crate::tui::ui::last_diff_pane_effective_scroll(),
                    content_length,
                    viewport_length: viewport,
                });
            }
        }
        PaneSnapshot { panes }
    }

    pub(super) fn apply_handterm_native_scroll(&mut self, command: HostToApp) {
        let HostToApp::Scroll { pane, delta } = command;
        self.enqueue_native_scroll(
            match pane {
                PaneKind::Chat => MouseScrollTarget::Chat,
                PaneKind::SidePanel => MouseScrollTarget::SidePane,
            },
            delta,
        );
    }
}

#[cfg(unix)]
fn spawn_bridge_thread(
    socket_path: PathBuf,
    updates_rx: Receiver<AppToHost>,
    commands_tx: UnboundedSender<HostToApp>,
    stop: Arc<AtomicBool>,
) -> Option<JoinHandle<()>> {
    match thread::Builder::new()
        .name("jcode-handterm-scroll".to_string())
        .spawn(move || bridge_thread(socket_path, updates_rx, commands_tx, stop))
    {
        Ok(thread) => Some(thread),
        Err(err) => {
            crate::logging::warn(&format!(
                "Failed to spawn handterm native scroll bridge thread: {}",
                err
            ));
            None
        }
    }
}

#[cfg(unix)]
fn bridge_thread(
    socket_path: PathBuf,
    updates_rx: Receiver<AppToHost>,
    commands_tx: UnboundedSender<HostToApp>,
    stop: Arc<AtomicBool>,
) {
    let mut latest_update = None::<AppToHost>;
    while !stop.load(Ordering::Relaxed) {
        while let Ok(update) = updates_rx.try_recv() {
            latest_update = Some(update);
        }
        let Some(mut stream) = connect_with_retry(&socket_path, &stop) else {
            break;
        };
        if stream.set_nonblocking(true).is_err() {
            continue;
        }
        while let Ok(update) = updates_rx.try_recv() {
            latest_update = Some(update);
        }
        if let Some(update) = latest_update.as_ref()
            && write_line(&mut stream, update).is_err()
        {
            continue;
        }
        let mut read_buf = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let mut disconnected = false;
            let mut update_pending = false;
            while let Ok(update) = updates_rx.try_recv() {
                latest_update = Some(update);
                update_pending = true;
            }
            if update_pending
                && let Some(update) = latest_update.as_ref()
                && write_line(&mut stream, update).is_err()
            {
                disconnected = true;
            }
            if disconnected {
                break;
            }
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => {
                        disconnected = true;
                        break;
                    }
                    Ok(n) => read_buf.extend_from_slice(&chunk[..n]),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if disconnected {
                break;
            }
            while let Some(pos) = read_buf.iter().position(|&b| b == b'\n') {
                let line = read_buf.drain(..=pos).collect::<Vec<_>>();
                let line = &line[..line.len().saturating_sub(1)];
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_slice::<HostToApp>(line) {
                    Ok(command) => {
                        let _ = commands_tx.send(command);
                    }
                    Err(_) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if disconnected {
                break;
            }
            thread::sleep(Duration::from_millis(8));
        }
    }
}

#[cfg(unix)]
fn connect_with_retry(socket_path: &PathBuf, stop: &AtomicBool) -> Option<UnixStream> {
    while !stop.load(Ordering::Relaxed) {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Some(stream),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
    None
}

#[cfg(unix)]
fn write_line<T: Serialize>(stream: &mut UnixStream, message: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec(message).context("failed encoding native scroll state")?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .context("failed writing native scroll state")
}

#[cfg(all(test, unix))]
mod bridge_tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn unique_socket_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "jcode-handterm-{label}-{}-{nonce:x}.sock",
            std::process::id()
        ))
    }

    fn accept_with_timeout(listener: &UnixListener) -> UnixStream {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "native scroll client did not connect"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("failed accepting native scroll client: {err}"),
            }
        }
    }

    fn snapshot(position: usize) -> AppToHost {
        AppToHost::PaneSnapshot {
            panes: vec![PaneState {
                kind: PaneKind::Chat,
                x: 1,
                y: 2,
                width: 30,
                height: 12,
                position,
                content_length: 100,
                viewport_length: 12,
            }],
        }
    }

    fn read_snapshot(stream: &UnixStream) -> AppToHost {
        stream
            .set_nonblocking(false)
            .expect("accepted stream should become blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout should set");
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .expect("snapshot line should arrive");
        serde_json::from_str(line.trim_end()).expect("snapshot should decode")
    }

    #[test]
    fn bridge_reconnects_and_replays_latest_snapshot() {
        let socket_path = unique_socket_path("reconnect");
        let listener = UnixListener::bind(&socket_path).expect("test socket should bind");
        listener
            .set_nonblocking(true)
            .expect("listener should become nonblocking");
        let (updates_tx, updates_rx) = mpsc::channel();
        let (commands_tx, _commands_rx) = unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread =
            spawn_bridge_thread(socket_path.clone(), updates_rx, commands_tx, stop.clone())
                .expect("bridge thread should spawn");

        let first = accept_with_timeout(&listener);
        let latest = snapshot(7);
        updates_tx
            .send(latest.clone())
            .expect("snapshot should queue");
        assert_eq!(read_snapshot(&first), latest);
        drop(first);

        let second = accept_with_timeout(&listener);
        assert_eq!(read_snapshot(&second), latest);

        stop.store(true, Ordering::Relaxed);
        thread.join().expect("bridge thread should stop cleanly");
        let _ = std::fs::remove_file(socket_path);
    }

    #[test]
    fn bridge_coalesces_updates_while_waiting_for_host() {
        let socket_path = unique_socket_path("coalesce");
        let (updates_tx, updates_rx) = mpsc::channel();
        let (commands_tx, _commands_rx) = unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread =
            spawn_bridge_thread(socket_path.clone(), updates_rx, commands_tx, stop.clone())
                .expect("bridge thread should spawn");

        updates_tx
            .send(snapshot(1))
            .expect("old snapshot should queue");
        let latest = snapshot(9);
        updates_tx
            .send(latest.clone())
            .expect("latest snapshot should queue");
        let listener = UnixListener::bind(&socket_path).expect("test socket should bind");
        listener
            .set_nonblocking(true)
            .expect("listener should become nonblocking");
        let stream = accept_with_timeout(&listener);
        assert_eq!(read_snapshot(&stream), latest);

        stop.store(true, Ordering::Relaxed);
        thread.join().expect("bridge thread should stop cleanly");
        let _ = std::fs::remove_file(socket_path);
    }
}
