use super::App;
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
use std::thread;
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
            Self::connect(socket_path).ok()
        }
    }

    #[cfg(unix)]
    fn connect(socket_path: PathBuf) -> Result<Self> {
        let (updates_tx, updates_rx) = mpsc::channel();
        let (commands_tx, commands_rx) = unbounded_channel();
        spawn_bridge_thread(socket_path, updates_rx, commands_tx);
        Ok(Self {
            updates_tx,
            commands_rx,
            last_sent: None,
        })
    }

    pub(super) fn sync_from_app(&mut self, app: &App) {
        #[cfg(not(unix))]
        {
            let _ = app;
        }

        #[cfg(unix)]
        {
            let snapshot = app.current_native_scroll_snapshot();
            if self.last_sent.as_ref() == Some(&snapshot) {
                return;
            }
            self.last_sent = Some(snapshot.clone());
            let _ = self.updates_tx.send(AppToHost::PaneSnapshot {
                panes: snapshot.panes,
            });
        }
    }

    pub(super) async fn recv(&mut self) -> Option<HostToApp> {
        self.commands_rx.recv().await
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
        match command {
            HostToApp::Scroll { pane, delta } if delta < 0 => {
                let amount = delta.unsigned_abs() as usize;
                match pane {
                    PaneKind::Chat => self.scroll_up(amount),
                    PaneKind::SidePanel => {
                        let current = if self.diff_pane_scroll == usize::MAX {
                            crate::tui::ui::last_diff_pane_effective_scroll()
                        } else {
                            self.diff_pane_scroll
                        };
                        self.diff_pane_scroll = current.saturating_sub(amount);
                        self.diff_pane_auto_scroll = false;
                    }
                }
            }
            HostToApp::Scroll { pane, delta } if delta > 0 => {
                let amount = delta as usize;
                match pane {
                    PaneKind::Chat => self.scroll_down(amount),
                    PaneKind::SidePanel => {
                        if self.diff_pane_scroll == usize::MAX {
                            self.diff_pane_scroll =
                                crate::tui::ui::last_diff_pane_effective_scroll();
                        }
                        self.diff_pane_scroll = self.diff_pane_scroll.saturating_add(amount);
                        self.diff_pane_auto_scroll = false;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(unix)]
fn spawn_bridge_thread(
    socket_path: PathBuf,
    updates_rx: Receiver<AppToHost>,
    commands_tx: UnboundedSender<HostToApp>,
) {
    if let Err(err) = thread::Builder::new()
        .name("jcode-handterm-scroll".to_string())
        .spawn(move || {
            let _ = bridge_thread(socket_path, updates_rx, commands_tx);
        })
    {
        crate::logging::warn(&format!(
            "Failed to spawn handterm native scroll bridge thread: {}",
            err
        ));
    }
}

#[cfg(unix)]
fn bridge_thread(
    socket_path: PathBuf,
    updates_rx: Receiver<AppToHost>,
    commands_tx: UnboundedSender<HostToApp>,
) -> Result<()> {
    let mut stream = connect_with_retry(&socket_path)?;
    stream
        .set_nonblocking(true)
        .context("failed setting native scroll socket nonblocking")?;
    let mut read_buf = Vec::new();

    loop {
        while let Ok(update) = updates_rx.try_recv() {
            write_line(&mut stream, &update)?;
        }

        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => return Ok(()),
                Ok(n) => read_buf.extend_from_slice(&chunk[..n]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(err).context("failed reading native scroll command"),
            }
        }

        while let Some(pos) = read_buf.iter().position(|&b| b == b'\n') {
            let line = read_buf.drain(..=pos).collect::<Vec<_>>();
            let line = &line[..line.len().saturating_sub(1)];
            if line.is_empty() {
                continue;
            }
            let command = serde_json::from_slice::<HostToApp>(line)
                .context("failed decoding native scroll command")?;
            let _ = commands_tx.send(command);
        }

        thread::sleep(Duration::from_millis(8));
    }
}

#[cfg(unix)]
fn connect_with_retry(socket_path: &PathBuf) -> Result<UnixStream> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(err) if std::time::Instant::now() < deadline => {
                if err.kind() != std::io::ErrorKind::NotFound
                    && err.kind() != std::io::ErrorKind::ConnectionRefused
                {
                    return Err(err).with_context(|| {
                        format!(
                            "failed connecting handterm native scroll socket {}",
                            socket_path.display()
                        )
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed connecting handterm native scroll socket {}",
                        socket_path.display()
                    )
                });
            }
        }
    }
}

#[cfg(unix)]
fn write_line<T: Serialize>(stream: &mut UnixStream, message: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec(message).context("failed encoding native scroll state")?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .context("failed writing native scroll state")
}
