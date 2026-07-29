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
        /// The session's working directory, as the daemon reports it.
        working_dir: Option<String>,
    },
    /// The provider and model serving the session.
    Model {
        provider: Option<String>,
        model: Option<String>,
    },
    Text(String),
    /// Streamed reasoning. Kept a separate variant from `Text` so the UI can
    /// place it in its own subordinate block instead of splicing a thought
    /// into the middle of the answer.
    Reasoning(String),
    /// The agent's current phase (a tool intent, or "thinking"), for the
    /// activity line. Streamed so the UI is never silent mid-turn.
    Activity(String),
    /// A tool call's current label, keyed by call id so a streamed `intent`
    /// refines the same transcript line the call opened with.
    Tool {
        call_id: String,
        label: String,
    },
    /// A finished file edit: its intent, the file, and the diff. Sent only for
    /// tools that write to disk, and kept separate from `Tool` because an edit
    /// earns a permanent transcript card while a call's status line does not.
    Edit(crate::edits::EditCard),
    TurnDone,
    /// The agent took delivery of the user's message. The proof a send landed,
    /// separate from the reply: a turn can think for minutes before its first
    /// token, and until this arrives the app only knows it *wrote* to a socket.
    MessageAccepted,
    /// Something failed: a turn that could not run, a provider that could not
    /// be reached, the runtime going away. Distinct from `Status` because a
    /// status line is hidden once a session is attached, which is exactly when
    /// a failure matters most.
    Failed(String),
    /// The daemon's current session list, for the session strip.
    Sessions(Vec<crate::strip::Entry>),
    /// The tail of another session's conversation, for the overview's preview.
    Peek {
        session_id: String,
        transcript: crate::transcript::Transcript,
    },
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
    /// Fetch the tail of another session without attaching to it.
    Peek(String),
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

/// Backoff between reconnection attempts, and its ceiling.
///
/// A dropped runtime is usually back within a second (a rebuild, a restart), so
/// the first retry is quick; the ceiling exists so a window left open against a
/// runtime that is gone for good does not spin.
const RECONNECT_BACKOFF: Duration = Duration::from_millis(500);
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Spawn the connection worker. Returns the receiving side for the UI and a
/// sender for outgoing user messages.
///
/// The worker reconnects on its own. A desktop app whose connection dies once
/// and then silently accepts input forever is the failure this exists to
/// prevent: every attempt reports why it failed, and the next attempt
/// re-attaches the session the user was looking at rather than starting a new
/// one behind their back.
pub fn spawn(redraw: impl Fn() + Send + 'static) -> (Receiver<HarnessUpdate>, Sender<Command>) {
    let (update_tx, update_rx) = channel::<HarnessUpdate>();
    let (outgoing_tx, outgoing_rx) = channel::<Command>();
    std::thread::spawn(move || {
        let send = move |update: HarnessUpdate| {
            let _ = update_tx.send(update);
            redraw();
        };
        // Shared across attempts: the command queue must survive a reconnect,
        // and the session to re-attach to has to be remembered.
        let outgoing = Arc::new(Mutex::new(outgoing_rx));
        let resume = Arc::new(Mutex::new(String::new()));
        let mut backoff = RECONNECT_BACKOFF;
        loop {
            // Each attempt gets its own generation, so the previous attempt's
            // writer and poller threads retire instead of writing into a dead
            // socket (or stealing a command from the live one).
            let generation = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let error = match run(
                &send,
                Arc::clone(&outgoing),
                Arc::clone(&resume),
                Arc::clone(&generation),
            ) {
                // `run` only returns on failure; `Ok` would mean the stream
                // ended cleanly, which is still a lost connection.
                Ok(()) => "the harness closed the connection".to_string(),
                Err(error) => error.to_string(),
            };
            generation.store(false, Ordering::Relaxed);
            send(HarnessUpdate::Failed(format!("disconnected: {error}")));
            send(HarnessUpdate::Status(format!(
                "reconnecting in {}s...",
                backoff.as_secs_f64().round().max(1.0)
            )));
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
        }
    });
    (update_rx, outgoing_tx)
}

/// Human wording for a failure, when the cause is one the user can act on.
///
/// Provider errors arrive as whatever the HTTP stack said, and "error sending
/// request for url (...): dns error: failed to lookup address information" does
/// not tell a user their wifi is off. Everything unrecognised is passed through
/// unchanged: a wrong guess would be worse than the raw text.
pub fn explain(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    const OFFLINE: [&str; 6] = [
        "dns error",
        "failed to lookup address information",
        "temporary failure in name resolution",
        "network is unreachable",
        "no route to host",
        "name or service not known",
    ];
    if OFFLINE.iter().any(|needle| lower.contains(needle)) {
        return format!("no network connection: {message}");
    }
    message.to_string()
}

fn run(
    send: &impl Fn(HarnessUpdate),
    outgoing: Arc<Mutex<Receiver<Command>>>,
    resume: Arc<Mutex<String>>,
    generation: Arc<std::sync::atomic::AtomicBool>,
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
    // Re-attach after a reconnect, so the conversation the user was reading
    // comes back instead of being replaced by a fresh empty session.
    let previous = resume.lock().map(|guard| guard.clone()).unwrap_or_default();
    match previous.is_empty() {
        true => client.send(ApiRequest::CreateSession {
            working_dir: default_working_dir(),
        })?,
        false => client.send(ApiRequest::AttachSession {
            session_id: previous,
        })?,
    };

    // Writer thread: forwards user messages immediately even while the read
    // loop below is blocked on the stream. Frame ids start high so they never
    // collide with the reader-side HarnessClient's counter.
    let session_id = Arc::new(Mutex::new(String::new()));
    let writer_ids = AtomicU64::new(1_000_000);
    std::thread::spawn({
        let session_id = Arc::clone(&session_id);
        let resume = Arc::clone(&resume);
        let generation = Arc::clone(&generation);
        let mut writer_stream = stream.try_clone()?;
        move || {
            // `recv_timeout` rather than `recv`: a blocking receive would hold
            // the queue past this connection's death and swallow the first
            // command the *next* connection should have sent.
            while generation.load(Ordering::Relaxed) {
                let command = {
                    let Ok(queue) = outgoing.lock() else { break };
                    match queue.recv_timeout(Duration::from_millis(100)) {
                        Ok(command) => command,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                };
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
                        if let Ok(mut guard) = resume.lock() {
                            *guard = target.clone();
                        }
                        ApiRequest::AttachSession { session_id: target }
                    }
                    // A peek must not retarget anything: it is a read of
                    // another session, and the one we are attached to has to
                    // stay the one a message would land in.
                    Command::Peek(target) => ApiRequest::PeekSession {
                        session_id: target,
                        limit: None,
                    },
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
        let generation = Arc::clone(&generation);
        move || {
            while generation.load(Ordering::Relaxed) {
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
                // Remember it for a reconnect: coming back to a different
                // session than the one on screen would look like the app lost
                // the conversation.
                if let Ok(mut guard) = resume.lock() {
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
                    working_dir: session.working_dir,
                });
            }
            ApiEvent::TextDelta { text, .. } => send(HarnessUpdate::Text(text)),
            // Reasoning is not rendered as transcript text yet, but its
            // arrival is proof the model is working, which is the thing the
            // silent-until-done UI was missing.
            ApiEvent::ReasoningDelta { text, .. } => {
                send(HarnessUpdate::Activity("thinking".into()));
                send(HarnessUpdate::Reasoning(text));
            }
            ApiEvent::ToolStart { call_id, name, .. } => {
                tool_input.remove(&call_id);
                current_call = call_id.clone();
                // The call opens under its tool name; the streamed arguments
                // usually carry a better line (the `intent`), which replaces
                // this one in place as it arrives.
                send(HarnessUpdate::Tool {
                    call_id,
                    label: name.clone(),
                });
                send(HarnessUpdate::Activity(name));
            }
            ApiEvent::ToolInputDelta { call_id, delta, .. } => {
                let key = if call_id.is_empty() {
                    current_call.clone()
                } else {
                    call_id
                };
                let buffer = tool_input.entry(key.clone()).or_default();
                buffer.push_str(&delta);
                if let Some(intent) = crate::activity::intent_from_partial_json(buffer) {
                    send(HarnessUpdate::Tool {
                        call_id: key,
                        label: intent.clone(),
                    });
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
                    Some(intent) => {
                        send(HarnessUpdate::Tool {
                            call_id,
                            label: intent.clone(),
                        });
                        send(HarnessUpdate::Activity(intent));
                    }
                    None if tool_input.contains_key(&call_id) => {}
                    None => send(HarnessUpdate::Activity(name)),
                }
            }
            ApiEvent::ToolDone {
                call_id,
                name,
                output,
                error,
                ..
            } => {
                // An edit that changed lines becomes a permanent card in the
                // transcript: the intent that motivated it and the lines it
                // added and removed. Read from the call's own arguments and
                // output, so what is shown is what the agent actually did.
                // A failed call is skipped: it changed nothing.
                if error.is_none()
                    && let Some(card) = crate::edits::parse(
                        &name,
                        tool_input.get(&call_id).map(String::as_str),
                        &output,
                    )
                {
                    send(HarnessUpdate::Edit(card));
                }
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
                            // The overview sizes a blob by how much
                            // conversation the session holds; a session the
                            // server could not measure is drawn at the floor
                            // rather than dropped.
                            weight: session.transcript_bytes.unwrap_or(0) as f64,
                        })
                        .collect(),
                ));
            }
            ApiEvent::ModelInfo {
                provider, model, ..
            } => send(HarnessUpdate::Model { provider, model }),
            // A peek's reply. History for the *attached* session arrives on
            // this event too, but the desktop asks for that only to learn the
            // session set, so treating every one as a peek is correct: the
            // preview cache is keyed by id and the attached session's own
            // transcript is built from the stream, not from here.
            ApiEvent::History {
                session_id,
                messages,
            } => {
                let mut transcript = crate::transcript::Transcript::default();
                for message in messages {
                    let text = message.content.trim();
                    if text.is_empty() {
                        continue;
                    }
                    transcript.push(match message.role.as_str() {
                        "user" => crate::transcript::Message::user(text),
                        _ => crate::transcript::Message::assistant(text),
                    });
                }
                send(HarnessUpdate::Peek {
                    session_id,
                    transcript,
                });
            }
            ApiEvent::MessageAccepted { .. } => send(HarnessUpdate::MessageAccepted),
            ApiEvent::TurnDone { .. } => send(HarnessUpdate::TurnDone),
            ApiEvent::Error { message, .. } => {
                // A failed request is also the end of the turn it belonged to:
                // the daemon sends `error` *instead of* `done`, so without this
                // the UI would spin its activity indicator forever.
                send(HarnessUpdate::Failed(explain(&message)));
                send(HarnessUpdate::TurnDone);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure a user is most likely to hit, and the one that motivated
    /// this: the machine is offline, so the provider's DNS lookup fails. The
    /// raw text names a URL and a resolver; the user needs to be told their
    /// network is down.
    #[test]
    fn an_offline_failure_is_explained_in_the_users_terms() {
        let raw = "error sending request for url (https://api.example.com/v1/messages): \
                   dns error: failed to lookup address information: Name or service not known";
        let explained = explain(raw);
        assert!(
            explained.starts_with("no network connection"),
            "offline was not named: {explained}"
        );
        assert!(
            explained.contains("dns error"),
            "the underlying cause must survive: {explained}"
        );
    }

    /// Anything unrecognised is passed through untouched. A wrong guess about a
    /// cause is worse than the provider's own words.
    #[test]
    fn an_unrecognised_failure_is_passed_through() {
        assert_eq!(
            explain("overloaded_error: try again"),
            "overloaded_error: try again"
        );
    }
}
