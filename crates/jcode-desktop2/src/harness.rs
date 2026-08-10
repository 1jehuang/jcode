//! Harness wiring for desktop2, on top of the Rust SDK.
//!
//! This is the app's half of the conversation: it maps SDK events onto the
//! `HarnessUpdate`s the UI draws, and UI `Command`s onto SDK calls. Everything
//! that is not desktop2-specific (starting the runtime, correlating replies,
//! explaining a lost connection) lives in `jcode-sdk`, so this file is about
//! the app rather than about the protocol.
//!
//! Desktop2 is built on the SDK deliberately: it is the SDK's only large,
//! shipping consumer, which is what keeps the SDK's shape honest instead of
//! validated only by its own examples.

use jcode_sdk::{ApiEvent, ConnectOptions, JcodeClient, LaunchOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Failure wording and connection-stage reporting are the SDK's, so the app
/// and any other client describe the same failure the same way.
pub use jcode_sdk::{SocketState, Stage, describe_disconnect, explain};

/// Use the same concise lifecycle vocabulary as the TUI. The API intentionally
/// carries the daemon's stable wire strings, so this remains tolerant of a new
/// phase added by a newer daemon.
fn connection_phase_label(phase: String) -> String {
    match phase.as_str() {
        "authenticating" => "refreshing auth".to_string(),
        "connecting" => "connecting".to_string(),
        "sending request" => "sending context".to_string(),
        "waiting for response" => "waiting for response".to_string(),
        "streaming" => "streaming".to_string(),
        _ if phase.starts_with("retrying (") && phase.ends_with(')') => {
            format!("retrying {}", &phase[10..phase.len() - 1])
        }
        _ => phase,
    }
}

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
    /// One SDK `list_models` result for the caption menu.
    Models {
        models: Vec<String>,
        current: Option<String>,
    },
    /// Confirmation that SDK `set_model` accepted the selected route.
    ModelSelected(String),
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
    /// The newest structured plan snapshot produced by the `todo` tool.
    Todo(crate::todos::TodoCard),
    TurnDone,
    /// A background task this session is waiting on: how far along it is, or
    /// that it finished. Forwarded so a long wait shows a moving bar instead of
    /// a spinner that only says "still working".
    Progress {
        task_id: String,
        label: String,
        summary: String,
        percent: Option<f32>,
        done: bool,
    },
    /// The agent took delivery of the user's message. The proof a send landed,
    /// separate from the reply: a turn can think for minutes before its first
    /// token, and until this arrives the app only knows it *wrote* to a socket.
    MessageAccepted,
    /// Something failed: a turn that could not run, a provider that could not
    /// be reached, the runtime going away. Distinct from `Status` because a
    /// status line is hidden once a session is attached, which is exactly when
    /// a failure matters most.
    Failed(String),
    /// The runtime transport disappeared and the worker is reconnecting.
    /// Unlike `Failed`, this is not a failed model turn and must not leave an
    /// error card in the conversation.
    ConnectionLost(String),
    /// The daemon's current session list, for the session strip.
    Sessions(Vec<crate::strip::Panel>),
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
    Send {
        content: String,
        /// (media_type, base64_data) pairs, attached to this message only.
        images: Vec<(String, String)>,
    },
    /// Attach to another session; the worker retargets subsequent sends.
    Attach(String),
    /// Fetch the tail of another session without attaching to it.
    Peek(String),
    /// Start a fresh session and attach to it. Travels the same channel as
    /// `Send` and `Attach` so a message typed just before it still lands in
    /// the session the user was looking at when they typed it.
    New,
    /// Fetch the current session's SDK model catalog.
    ListModels,
    /// Select one exact id returned by `list_models`.
    SetModel(String),
}

/// UI handle for commands sent to the harness worker.
///
/// New-session is a lifecycle interrupt, not an ordinary ordered request. If it
/// sits behind a slow attach/model request in the command queue, the UI clears
/// its page and then waits forever for a command the worker cannot reach. Keep
/// that one signal out-of-band; ordinary sends and attaches retain FIFO order.
#[derive(Clone)]
pub struct CommandSender {
    tx: Sender<Command>,
    new_requested: Arc<AtomicBool>,
}

impl CommandSender {
    #[cfg(test)]
    pub(crate) fn for_test(tx: Sender<Command>) -> Self {
        Self {
            tx,
            new_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_requested_for_test(&self) -> bool {
        self.new_requested.load(Ordering::Acquire)
    }

    pub fn send(&self, command: Command) -> Result<(), std::sync::mpsc::SendError<Command>> {
        if matches!(command, Command::New) {
            self.new_requested.store(true, Ordering::Release);
            Ok(())
        } else {
            self.tx.send(command)
        }
    }
}

/// A handle every worker thread can use to reach the UI.
///
/// The connection worker, the command thread and the session poller all
/// produce updates, so the sink is a cloneable handle rather than a closure
/// borrowed from one of their frames.
#[derive(Clone)]
struct Ui {
    updates: Sender<HarnessUpdate>,
    redraw: Arc<dyn Fn() + Send + Sync>,
}

impl Ui {
    fn send(&self, update: HarnessUpdate) {
        let _ = self.updates.send(update);
        (self.redraw)();
    }
}

/// The API socket both this app and the bridge agree on.
pub fn api_socket_path() -> PathBuf {
    jcode_sdk::api_socket_path()
}

/// Working directory for sessions this app creates.
///
/// Desktop2 is developed on itself, so a session opened from the app should
/// land in the desktop2 crate: the daemon derives self-dev mode and the
/// desktop2 product focus from this directory, and a session rooted anywhere
/// else gets an agent that assumes it is working on the TUI. Overridable so a
/// desktop2 build can be pointed at another project.
pub(crate) fn default_working_dir() -> Option<String> {
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

/// How often the session strip is refreshed.
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(2);

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
pub fn spawn(
    redraw: impl Fn() + Send + Sync + 'static,
) -> (Receiver<HarnessUpdate>, CommandSender) {
    let (update_tx, update_rx) = channel::<HarnessUpdate>();
    let (outgoing_tx, outgoing_rx) = channel::<Command>();
    let new_requested = Arc::new(AtomicBool::new(false));
    let commands = CommandSender {
        tx: outgoing_tx,
        new_requested: Arc::clone(&new_requested),
    };
    let ui = Ui {
        updates: update_tx,
        redraw: Arc::new(redraw),
    };
    std::thread::spawn(move || {
        // Shared across attempts: the command queue must survive a reconnect,
        // and the session to re-attach to has to be remembered.
        let outgoing = Arc::new(Mutex::new(outgoing_rx));
        let resume = Arc::new(Mutex::new(String::new()));
        let mut backoff = RECONNECT_BACKOFF;
        loop {
            // Where the attempt got to, and when the stream came up: both are
            // needed to say why it ended and are only known inside `run`.
            let stage = Arc::new(Mutex::new(Stage::Starting));
            let connected_at = Arc::new(Mutex::new(None::<Instant>));
            let error = match run(
                &ui,
                Arc::clone(&outgoing),
                Arc::clone(&resume),
                Arc::clone(&stage),
                Arc::clone(&connected_at),
                Arc::clone(&new_requested),
            ) {
                // `run` only returns on failure; `Ok` would mean the stream
                // ended cleanly, which is still a lost connection.
                Ok(()) if new_requested.swap(false, Ordering::AcqRel) => {
                    // A fresh session requires a fresh legacy connection. The
                    // daemon binds one session id to a connection for life, so
                    // a second subscribe without a target cannot retarget it.
                    // Re-enter immediately with an empty resume id; `run` then
                    // creates the new session on a clean connection.
                    continue;
                }
                Ok(()) => "the harness closed the connection".to_string(),
                Err(error) => error.to_string(),
            };
            let path = api_socket_path();
            let stage = stage.lock().map(|s| *s).unwrap_or(Stage::Starting);
            let uptime = connected_at
                .lock()
                .ok()
                .and_then(|guard| *guard)
                .map(|at| at.elapsed());
            // Reaching the daemon proves the retry loop recovered. Without
            // resetting here, one old outage permanently leaves every later
            // disconnect at the 10-second ceiling.
            if uptime.is_some() {
                backoff = RECONNECT_BACKOFF;
            }
            ui.send(HarnessUpdate::ConnectionLost(describe_disconnect(
                stage,
                &error,
                uptime,
                &path,
                SocketState::probe(&path),
            )));
            ui.send(HarnessUpdate::Status(format!(
                "reconnecting in {}s...",
                backoff.as_secs_f64().round().max(1.0)
            )));
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
        }
    });
    (update_rx, commands)
}

/// One connection attempt: connect, attach, then stream until it dies.
fn run(
    ui: &Ui,
    outgoing: Arc<Mutex<Receiver<Command>>>,
    resume: Arc<Mutex<String>>,
    stage: Arc<Mutex<Stage>>,
    connected_at: Arc<Mutex<Option<Instant>>>,
    new_requested: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let set_stage = |next: Stage| {
        if let Ok(mut guard) = stage.lock() {
            *guard = next;
        }
    };
    let path = api_socket_path();
    ui.send(HarnessUpdate::Status(format!(
        "connecting to {}...",
        path.display()
    )));

    set_stage(Stage::Starting);
    jcode_sdk::ensure_runtime(&LaunchOptions::default(), &|status| {
        ui.send(HarnessUpdate::Status(status.to_string()))
    })?;

    set_stage(Stage::Connecting);
    let client = JcodeClient::connect(ConnectOptions {
        client_name: concat!("jcode-desktop2/", env!("CARGO_PKG_VERSION")).to_string(),
        // The runtime is already up; a second check would only cost a dial.
        ensure_runtime: false,
        ..Default::default()
    })?;
    if let Ok(mut guard) = connected_at.lock() {
        *guard = Some(Instant::now());
    }

    // Subscribe before attaching, so events the daemon pushes as part of the
    // attach (the model it chose, the first status) are not missed.
    let events = client.events(None);

    set_stage(Stage::Attaching);
    ui.send(HarnessUpdate::Status("connected, attaching...".into()));
    // Re-attach after a reconnect, so the conversation the user was reading
    // comes back instead of being replaced by a fresh empty session.
    let previous = resume.lock().map(|guard| guard.clone()).unwrap_or_default();
    let attached = match previous.is_empty() {
        true => client.create_session(default_working_dir())?,
        false => client.attach_session(&previous)?,
    };
    let session_id = Arc::new(Mutex::new(attached.session_id.clone()));
    if let Ok(mut guard) = resume.lock() {
        *guard = attached.session_id.clone();
    }
    set_stage(Stage::Streaming);
    ui.send(HarnessUpdate::Attached {
        session_id: attached.session_id,
        working_dir: attached.working_dir,
    });

    // Command thread: forwards user messages immediately even while the event
    // loop below is blocked on the stream. It owns the requests whose replies
    // are values rather than stream events (peeks, attaches), so the event
    // loop never has to guess which request a `history` frame answers.
    std::thread::spawn({
        let client = client.clone();
        let session_id = Arc::clone(&session_id);
        let resume = Arc::clone(&resume);
        let ui = ui.clone();
        let new_requested = Arc::clone(&new_requested);
        move || {
            // `recv_timeout` rather than `recv`: a blocking receive would hold
            // the queue past this connection's death and swallow the first
            // command the *next* connection should have sent.
            while !client.is_closed() && !new_requested.load(Ordering::Acquire) {
                let command = {
                    let Ok(queue) = outgoing.lock() else { break };
                    match queue.recv_timeout(Duration::from_millis(100)) {
                        Ok(command) => command,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                };
                let outcome = match command {
                    Command::Send { content, images } => {
                        let session = session_id.lock().map(|s| s.clone()).unwrap_or_default();
                        if session.is_empty() {
                            continue;
                        }
                        // The acknowledgement is rendered from the stream, so
                        // there is nothing to wait for here.
                        client.send_message(&session, &content, images, None)
                    }
                    // Retarget immediately rather than waiting for the attach
                    // to complete: a message typed straight after a switch must
                    // land in the session the user is looking at.
                    Command::Attach(target) => {
                        if let Ok(mut guard) = session_id.lock() {
                            *guard = target.clone();
                        }
                        if let Ok(mut guard) = resume.lock() {
                            *guard = target.clone();
                        }
                        client.attach_session(&target).map(|session| {
                            ui.send(HarnessUpdate::Attached {
                                session_id: session.session_id,
                                working_dir: session.working_dir,
                            })
                        })
                    }
                    // A peek must not retarget anything: it is a read of
                    // another session, and the one we are attached to has to
                    // stay the one a message would land in. It is also
                    // background decoration for a neighboring column, so it
                    // must never hold the command queue in front of a user's
                    // message. A missing archive can take the full request
                    // timeout; run it independently and leave the live session
                    // usable while that happens.
                    Command::Peek(target) => {
                        let ui = ui.clone();
                        std::thread::spawn(move || {
                            // The bridge serves one connection's requests in
                            // order. A stored-history read can block on another
                            // process writing that archive, so using a clone of
                            // the live client would still stall message sends
                            // and stream events behind the peek. Give previews
                            // their own connection instead.
                            let preview = JcodeClient::connect(ConnectOptions {
                                client_name: concat!(
                                    "jcode-desktop2-preview/",
                                    env!("CARGO_PKG_VERSION")
                                )
                                .to_string(),
                                ensure_runtime: false,
                                ..Default::default()
                            });
                            if let Ok(client) = preview
                                && let Ok(messages) = client.peek_session(&target, None)
                            {
                                ui.send(HarnessUpdate::Peek {
                                    session_id: target,
                                    transcript: to_transcript(messages),
                                });
                            }
                        });
                        Ok(())
                    }
                    // A daemon connection is permanently bound to the session
                    // from its first subscribe. Signal the stream loop to drop
                    // this connection and let the outer worker create on a new
                    // one instead of issuing a second subscribe that can never
                    // produce a different id.
                    Command::New => {
                        if let Ok(mut guard) = session_id.lock() {
                            guard.clear();
                        }
                        if let Ok(mut guard) = resume.lock() {
                            guard.clear();
                        }
                        new_requested.store(true, Ordering::Release);
                        Ok(())
                    }
                    Command::ListModels => {
                        let session = session_id.lock().map(|s| s.clone()).unwrap_or_default();
                        if session.is_empty() {
                            continue;
                        }
                        client.list_models(&session).map(|(models, current)| {
                            ui.send(HarnessUpdate::Models { models, current })
                        })
                    }
                    Command::SetModel(model) => {
                        let session = session_id.lock().map(|s| s.clone()).unwrap_or_default();
                        if session.is_empty() {
                            continue;
                        }
                        client.set_model(&session, &model).map(|()| {
                            ui.send(HarnessUpdate::ModelSelected(model));
                        })
                    }
                };
                // A failed command is the user's action not happening, so it is
                // reported rather than swallowed. The connection dying is the
                // event loop's error to report, so this thread just retires.
                if let Err(error) = outcome {
                    if error.code() == "disconnected" {
                        break;
                    }
                    ui.send(HarnessUpdate::Failed(explain(&error.message)));
                }
            }
        }
    });

    // Session-list polling is independent of the command worker. Give it its
    // own connection as well as its own thread: the bridge serves requests on
    // one connection in order, so a list read on a clone of the live client can
    // otherwise sit in front of a user message (or its streamed events). This
    // is the same isolation used for transcript previews above.
    std::thread::spawn({
        let ui = ui.clone();
        let poll_new_requested = Arc::clone(&new_requested);
        move || {
            let Ok(client) = JcodeClient::connect(ConnectOptions {
                client_name: concat!("jcode-desktop2-sessions/", env!("CARGO_PKG_VERSION"))
                    .to_string(),
                ensure_runtime: false,
                ..Default::default()
            }) else {
                return;
            };
            while !client.is_closed() && !poll_new_requested.load(Ordering::Acquire) {
                match client.list_sessions() {
                    Ok(sessions) => ui.send(HarnessUpdate::Sessions(
                        sessions.into_iter().map(to_entry).collect(),
                    )),
                    Err(_) => break,
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
        if new_requested.load(Ordering::Acquire) {
            if let Ok(mut guard) = resume.lock() {
                guard.clear();
            }
            return Ok(());
        }
        let Some(event) = events.next_timeout(Duration::from_millis(25)) else {
            if client.is_closed() {
                break;
            }
            continue;
        };
        match event {
            ApiEvent::TextDelta { text, .. } => ui.send(HarnessUpdate::Text(text)),
            // Reasoning is not rendered as transcript text yet, but its
            // arrival is proof the model is working, which is the thing the
            // silent-until-done UI was missing.
            ApiEvent::ReasoningDelta { text, .. } => {
                ui.send(HarnessUpdate::Activity("thinking".into()));
                ui.send(HarnessUpdate::Reasoning(text));
            }
            ApiEvent::ConnectionPhase { phase, .. } => {
                // Match the TUI's user-facing vocabulary rather than exposing
                // the provider protocol's `sending request` wording. These
                // events arrive before reasoning/text, which is precisely when
                // a generic "thinking" label otherwise looks hung.
                ui.send(HarnessUpdate::Activity(connection_phase_label(phase)));
            }
            ApiEvent::ToolStart { call_id, name, .. } => {
                tool_input.remove(&call_id);
                current_call = call_id.clone();
                // The call opens under its tool name; the streamed arguments
                // usually carry a better line (the `intent`), which replaces
                // this one in place as it arrives.
                ui.send(HarnessUpdate::Tool {
                    call_id,
                    label: name.clone(),
                });
                ui.send(HarnessUpdate::Activity(name));
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
                    ui.send(HarnessUpdate::Tool {
                        call_id: key,
                        label: intent.clone(),
                    });
                    ui.send(HarnessUpdate::Activity(intent));
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
                        ui.send(HarnessUpdate::Tool {
                            call_id,
                            label: intent.clone(),
                        });
                        ui.send(HarnessUpdate::Activity(intent));
                    }
                    None if tool_input.contains_key(&call_id) => {}
                    None => ui.send(HarnessUpdate::Activity(name)),
                }
            }
            ApiEvent::ToolDone {
                call_id,
                name,
                output,
                error,
                ..
            } => {
                if error.is_none()
                    && name == "todo"
                    && let Some(card) =
                        crate::todos::parse(tool_input.get(&call_id).map(String::as_str))
                {
                    ui.send(HarnessUpdate::Todo(card));
                }
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
                    ui.send(HarnessUpdate::Edit(card));
                }
                tool_input.remove(&call_id);
                ui.send(HarnessUpdate::Activity("thinking".into()));
            }
            ApiEvent::ModelInfo {
                provider, model, ..
            } => ui.send(HarnessUpdate::Model { provider, model }),
            ApiEvent::MessageAccepted { .. } => ui.send(HarnessUpdate::MessageAccepted),
            ApiEvent::TurnDone { .. } => ui.send(HarnessUpdate::TurnDone),
            ApiEvent::BackgroundProgress {
                task_id,
                label,
                summary,
                percent,
                done,
                ..
            } => ui.send(HarnessUpdate::Progress {
                task_id,
                label,
                summary,
                percent,
                done,
            }),
            ApiEvent::Error { message, .. }
                if message.eq_ignore_ascii_case("daemon connection closed") =>
            {
                // The bridge sends this immediately before closing the API
                // stream. Let the outer retry loop report it once as a
                // recoverable transport interruption, rather than first
                // recording a failed turn and then recording a disconnect.
                return Err(
                    std::io::Error::new(std::io::ErrorKind::ConnectionReset, message).into(),
                );
            }
            ApiEvent::Error { message, .. } => {
                // A failed request is also the end of the turn it belonged to:
                // the daemon sends `error` *instead of* `done`, so without this
                // the UI would spin its activity indicator forever.
                ui.send(HarnessUpdate::Failed(explain(&message)));
                ui.send(HarnessUpdate::TurnDone);
            }
            _ => {}
        }
    }
    // The event stream only ends when the connection does.
    Ok(())
}

#[cfg(test)]
mod command_sender_tests {
    use super::*;

    #[test]
    fn provider_phases_use_tui_status_labels() {
        assert_eq!(
            connection_phase_label("authenticating".into()),
            "refreshing auth"
        );
        assert_eq!(connection_phase_label("connecting".into()), "connecting");
        assert_eq!(
            connection_phase_label("sending request".into()),
            "sending context"
        );
        assert_eq!(
            connection_phase_label("waiting for response".into()),
            "waiting for response"
        );
        assert_eq!(
            connection_phase_label("retrying (2/4)".into()),
            "retrying 2/4"
        );
        assert_eq!(connection_phase_label("streaming".into()), "streaming");
        assert_eq!(
            connection_phase_label("negotiating proxy".into()),
            "negotiating proxy"
        );
    }

    #[test]
    fn new_session_bypasses_an_occupied_command_queue() {
        let (tx, rx) = channel();
        let new_requested = Arc::new(AtomicBool::new(false));
        let commands = CommandSender {
            tx,
            new_requested: Arc::clone(&new_requested),
        };

        commands.send(Command::ListModels).unwrap();
        commands.send(Command::New).unwrap();

        assert!(new_requested.load(Ordering::Acquire));
        assert!(matches!(rx.try_recv(), Ok(Command::ListModels)));
        assert!(
            rx.try_recv().is_err(),
            "New must not wait in the FIFO queue"
        );
    }
}

/// A session-list entry, sized for the overview.
fn to_entry(session: jcode_sdk::SessionInfo) -> crate::strip::Panel {
    crate::strip::Panel {
        session_id: session.session_id,
        title: session.title,
        working_dir: session.working_dir,
        busy: session.status == "busy",
        // The overview sizes a blob by how much conversation the session
        // holds; a session the server could not measure is drawn at the floor
        // rather than dropped.
        weight: session.transcript_bytes.unwrap_or(0) as f64,
    }
}

/// Stored history as a transcript the overview can preview.
fn to_transcript(messages: Vec<jcode_sdk::HistoryMessage>) -> crate::transcript::Transcript {
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
    transcript
}
