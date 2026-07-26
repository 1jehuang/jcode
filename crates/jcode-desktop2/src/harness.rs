//! Harness API wiring for desktop2.
//!
//! Connects to the harness API socket (served by `jcode-harness-api-bridge`)
//! on a background thread, attaches a session, and forwards streamed events
//! to the UI thread over a channel.
//!
//! The app starts the runtime it needs rather than telling the user to. A
//! desktop app that only works when you have already launched two daemons by
//! hand is indistinguishable from a broken one, so `ensure_runtime` boots the
//! jcode daemon and the bridge on demand and waits for the socket.

use jcode_harness_api::{ApiEvent, ApiRequest, ClientFrame, HarnessClient, write_frame};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// UI-facing updates produced by the connection worker.
#[derive(Debug)]
pub enum HarnessUpdate {
    Status(String),
    Attached { session_id: String },
    Text(String),
    TurnDone,
}

/// The API socket both this app and the bridge agree on. Shared with the
/// bridge via `jcode-harness-api` so the two can never disagree.
pub fn api_socket_path() -> PathBuf {
    jcode_harness_api::api_socket_path()
}

/// How long to wait for a freshly spawned runtime to publish its socket.
const RUNTIME_START_TIMEOUT: Duration = Duration::from_secs(30);

fn socket_accepts(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Locate a sibling executable next to our own, falling back to `$PATH`.
///
/// Self-dev and release builds both keep the binaries side by side, so a
/// sibling lookup starts the *matching* build instead of whatever stale copy
/// happens to be first on `$PATH`.
fn sibling_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Start the daemon and the API bridge if they are not already listening.
///
/// Idempotent and safe to race: both the daemon and the bridge refuse to
/// replace a live socket, so a duplicate spawn simply exits.
fn ensure_runtime(send: &impl Fn(HarnessUpdate)) -> Result<(), Box<dyn std::error::Error>> {
    let api = api_socket_path();
    if socket_accepts(&api) {
        return Ok(());
    }

    let legacy = jcode_harness_api::legacy_socket_path();
    if !socket_accepts(&legacy) {
        send(HarnessUpdate::Status("starting jcode runtime...".into()));
        std::process::Command::new(sibling_exe("jcode"))
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start the jcode runtime: {error}"))?;
        wait_for_socket(&legacy, "jcode runtime")?;
    }

    send(HarnessUpdate::Status(
        "starting harness API bridge...".into(),
    ));
    std::process::Command::new(sibling_exe("jcode-harness-api-bridge"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start jcode-harness-api-bridge: {error}"))?;
    wait_for_socket(&api, "harness API bridge")?;
    Ok(())
}

fn wait_for_socket(path: &Path, what: &str) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + RUNTIME_START_TIMEOUT;
    while Instant::now() < deadline {
        if socket_accepts(path) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("timed out waiting for the {what} at {}", path.display()).into())
}

/// Spawn the connection worker. Returns the receiving side for the UI and a
/// sender for outgoing user messages.
pub fn spawn(redraw: impl Fn() + Send + 'static) -> (Receiver<HarnessUpdate>, Sender<String>) {
    let (update_tx, update_rx) = channel::<HarnessUpdate>();
    let (outgoing_tx, outgoing_rx) = channel::<String>();
    std::thread::spawn(move || {
        let send = move |update: HarnessUpdate| {
            let _ = update_tx.send(update);
            redraw();
        };
        if let Err(error) = run(&send, outgoing_rx) {
            send(HarnessUpdate::Status(format!("disconnected: {error}")));
        }
    });
    (update_rx, outgoing_tx)
}

fn run(
    send: &impl Fn(HarnessUpdate),
    outgoing: Receiver<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = api_socket_path();
    send(HarnessUpdate::Status(format!(
        "connecting to {}...",
        path.display()
    )));
    ensure_runtime(send)?;
    let stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|error| format!("{error} (socket {})", path.display()))?;
    let reader = BufReader::new(stream.try_clone()?);
    let mut client = HarnessClient::new(reader, stream.try_clone()?);
    client.hello(concat!("jcode-desktop2/", env!("CARGO_PKG_VERSION")))?;
    send(HarnessUpdate::Status("connected, attaching...".into()));
    client.send(ApiRequest::CreateSession { working_dir: None })?;

    // Writer thread: forwards user messages immediately even while the read
    // loop below is blocked on the stream. Frame ids start high so they never
    // collide with the reader-side HarnessClient's counter.
    let session_id = Arc::new(Mutex::new(String::new()));
    let writer_ids = AtomicU64::new(1_000_000);
    std::thread::spawn({
        let session_id = Arc::clone(&session_id);
        let mut writer_stream = stream.try_clone()?;
        move || {
            while let Ok(content) = outgoing.recv() {
                let session = session_id.lock().map(|s| s.clone()).unwrap_or_default();
                if session.is_empty() {
                    continue;
                }
                let frame = ClientFrame::new(
                    writer_ids.fetch_add(1, Ordering::Relaxed),
                    ApiRequest::SendMessage {
                        session_id: session,
                        content,
                        images: vec![],
                    },
                );
                if write_frame(&mut writer_stream, &frame).is_err() {
                    break;
                }
            }
        }
    });

    loop {
        let frame = client.recv()?;
        match frame.event {
            ApiEvent::Attached { session } => {
                if let Ok(mut guard) = session_id.lock() {
                    *guard = session.session_id.clone();
                }
                send(HarnessUpdate::Attached {
                    session_id: session.session_id,
                });
            }
            ApiEvent::TextDelta { text, .. } => send(HarnessUpdate::Text(text)),
            ApiEvent::TurnDone { .. } => send(HarnessUpdate::TurnDone),
            ApiEvent::Error { message, .. } => {
                send(HarnessUpdate::Status(format!("error: {message}")));
            }
            _ => {}
        }
    }
}
