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
    Attached {
        session_id: String,
    },
    /// The provider and model serving the session.
    Model {
        provider: Option<String>,
        model: Option<String>,
    },
    Text(String),
    /// The agent's current phase (a tool intent, or "thinking"), for the
    /// activity line. Streamed so the UI is never silent mid-turn.
    Activity(String),
    TurnDone,
    /// The daemon's current session list, for the session strip.
    Sessions(Vec<crate::strip::Entry>),
}

/// A command from the UI thread to the connection worker.
///
/// Sending a message and switching sessions travel the same channel so they
/// stay ordered with respect to each other: a switch must never overtake a
/// message that was typed into the session being left.
#[derive(Debug)]
pub enum Command {
    Send(String),
    /// Attach to another session; the worker retargets subsequent sends.
    Attach(String),
}

/// The API socket both this app and the bridge agree on. Shared with the
/// bridge via `jcode-harness-api` so the two can never disagree.
pub fn api_socket_path() -> PathBuf {
    jcode_harness_api::api_socket_path()
}

/// Working directory for sessions this app creates.
///
/// Desktop2 is developed on itself, so a session opened from the app should
/// land in the desktop2 crate: the daemon derives self-dev mode and the
/// desktop2 product focus from this directory, and a session rooted anywhere
/// else gets an agent that assumes it is working on the TUI. Overridable so a
/// desktop2 build can be pointed at another project.
fn default_working_dir() -> Option<String> {
    if let Some(raw) = std::env::var_os("JCODE_DESKTOP2_WORKING_DIR") {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            return Some(path.display().to_string());
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .is_dir()
        .then(|| manifest_dir.display().to_string())
}

/// How long to wait for a freshly spawned runtime to publish its socket.
const RUNTIME_START_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the session strip is refreshed.
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(2);

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
pub fn spawn(redraw: impl Fn() + Send + 'static) -> (Receiver<HarnessUpdate>, Sender<Command>) {
    let (update_tx, update_rx) = channel::<HarnessUpdate>();
    let (outgoing_tx, outgoing_rx) = channel::<Command>();
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
    outgoing: Receiver<Command>,
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
    client.send(ApiRequest::CreateSession {
        working_dir: default_working_dir(),
    })?;

    // Writer thread: forwards user messages immediately even while the read
    // loop below is blocked on the stream. Frame ids start high so they never
    // collide with the reader-side HarnessClient's counter.
    let session_id = Arc::new(Mutex::new(String::new()));
    let writer_ids = AtomicU64::new(1_000_000);
    std::thread::spawn({
        let session_id = Arc::clone(&session_id);
        let mut writer_stream = stream.try_clone()?;
        move || {
            while let Ok(command) = outgoing.recv() {
                let request = match command {
                    Command::Send(content) => {
                        let session = session_id.lock().map(|s| s.clone()).unwrap_or_default();
                        if session.is_empty() {
                            continue;
                        }
                        ApiRequest::SendMessage {
                            session_id: session,
                            content,
                            images: vec![],
                        }
                    }
                    // Retarget immediately rather than waiting for the
                    // `Attached` event: a message typed straight after a
                    // switch must land in the session the user is looking at.
                    Command::Attach(target) => {
                        if let Ok(mut guard) = session_id.lock() {
                            *guard = target.clone();
                        }
                        ApiRequest::AttachSession { session_id: target }
                    }
                };
                let frame = ClientFrame::new(writer_ids.fetch_add(1, Ordering::Relaxed), request);
                if write_frame(&mut writer_stream, &frame).is_err() {
                    break;
                }
            }
        }
    });

    // Session-list poller. The API has no push notification for sessions
    // appearing or disappearing, so the strip is refreshed on a slow timer;
    // slow because a strip that is a second stale costs nothing, while a busy
    // poll would tax the daemon for the whole life of the window.
    std::thread::spawn({
        let mut poll_stream = stream.try_clone()?;
        let poll_ids = AtomicU64::new(2_000_000);
        move || {
            loop {
                let frame = ClientFrame::new(
                    poll_ids.fetch_add(1, Ordering::Relaxed),
                    ApiRequest::ListSessions,
                );
                if write_frame(&mut poll_stream, &frame).is_err() {
                    break;
                }
                std::thread::sleep(SESSION_POLL_INTERVAL);
            }
        }
    });

    // Streamed tool arguments, keyed by call id, so a tool's `intent` can be
    // shown while it is still arriving. Cleared as each call finishes: a turn
    // with hundreds of calls must not accumulate their arguments forever.
    let mut tool_input: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // The most recent `tool_start`. The server does not populate `call_id` on
    // `tool_input` deltas, so arguments are attributed to the call that opened
    // last; tool calls stream one at a time, so this is exact today and would
    // degrade to a briefly wrong label rather than a panic if that changed.
    let mut current_call = String::new();
    loop {
        let frame = client.recv()?;
        match frame.event {
            ApiEvent::Attached { session } => {
                if let Ok(mut guard) = session_id.lock() {
                    *guard = session.session_id.clone();
                }
                // The daemon reports the full session set only alongside
                // history, so ask for it once on attach; without this the
                // strip would only ever see the session we are attached to.
                // A write failure here means the connection is gone, which is
                // the read loop's error to report, so surface it rather than
                // continuing to poll a dead socket.
                client.send(ApiRequest::GetHistory {
                    session_id: session.session_id.clone(),
                })?;
                send(HarnessUpdate::Attached {
                    session_id: session.session_id,
                });
            }
            ApiEvent::TextDelta { text, .. } => send(HarnessUpdate::Text(text)),
            // Reasoning is not rendered as transcript text yet, but its
            // arrival is proof the model is working, which is the thing the
            // silent-until-done UI was missing.
            ApiEvent::ReasoningDelta { .. } => {
                send(HarnessUpdate::Activity("thinking".into()));
            }
            ApiEvent::ToolStart { call_id, name, .. } => {
                tool_input.remove(&call_id);
                current_call = call_id;
                send(HarnessUpdate::Activity(name));
            }
            ApiEvent::ToolInputDelta { call_id, delta, .. } => {
                let key = if call_id.is_empty() {
                    current_call.clone()
                } else {
                    call_id
                };
                let buffer = tool_input.entry(key).or_default();
                buffer.push_str(&delta);
                if let Some(intent) = crate::activity::intent_from_partial_json(buffer) {
                    send(HarnessUpdate::Activity(intent));
                }
            }
            ApiEvent::ToolExec { call_id, name, .. } => {
                // Prefer the intent the model wrote over the bare tool name:
                // "check the build" says more than "bash". When the arguments
                // did not carry one, leave the label alone rather than
                // downgrading a good line back to the tool's name.
                match tool_input
                    .get(&call_id)
                    .and_then(|input| crate::activity::intent_from_partial_json(input))
                {
                    Some(intent) => send(HarnessUpdate::Activity(intent)),
                    None if tool_input.contains_key(&call_id) => {}
                    None => send(HarnessUpdate::Activity(name)),
                }
            }
            ApiEvent::ToolDone { call_id, .. } => {
                tool_input.remove(&call_id);
                send(HarnessUpdate::Activity("thinking".into()));
            }
            ApiEvent::Sessions { sessions } => {
                send(HarnessUpdate::Sessions(
                    sessions
                        .into_iter()
                        .map(|session| crate::strip::Entry {
                            session_id: session.session_id,
                            working_dir: session.working_dir,
                            busy: session.status == "busy",
                        })
                        .collect(),
                ));
            }
            ApiEvent::ModelInfo {
                provider, model, ..
            } => send(HarnessUpdate::Model { provider, model }),
            ApiEvent::TurnDone { .. } => send(HarnessUpdate::TurnDone),
            ApiEvent::Error { message, .. } => {
                send(HarnessUpdate::Status(format!("error: {message}")));
            }
            _ => {}
        }
    }
}
