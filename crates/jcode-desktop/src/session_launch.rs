use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(10);
const SERVER_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DesktopSessionEvent {
    Status(String),
    SessionStarted { session_id: String },
    TextDelta(String),
    TextReplace(String),
    Reloading { new_socket: Option<String> },
    Done,
    Error(String),
}

pub type DesktopSessionEventSender = Sender<DesktopSessionEvent>;

#[derive(Clone, Debug)]
pub struct DesktopSessionHandle {
    command_tx: Sender<DesktopSessionCommand>,
}

impl DesktopSessionHandle {
    pub fn cancel(&self) -> Result<()> {
        self.command_tx
            .send(DesktopSessionCommand::Cancel)
            .context("failed to send cancel to desktop session worker")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DesktopSessionCommand {
    Cancel,
}

pub fn launch_resume_session(session_id: &str, title: &str) -> Result<()> {
    let title = format!("jcode · {}", compact_title(title));
    let candidates = terminal_candidates(&title, &["--resume", session_id]);
    launch_first_available_terminal(candidates, &format!("jcode --resume {session_id}"))
}

pub fn launch_new_session() -> Result<()> {
    let candidates = terminal_candidates("jcode · new session", &["--fresh-spawn"]);
    launch_first_available_terminal(candidates, "jcode")
}

pub fn send_message_to_session(session_id: &str, _title: &str, message: &str) -> Result<()> {
    validate_resume_session_id(session_id).context("refusing to send to invalid session id")?;
    if message.trim().is_empty() {
        anyhow::bail!("empty draft message");
    }

    Command::new(jcode_bin())
        .arg("--resume")
        .arg(session_id)
        .arg("run")
        .arg(message)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to spawn jcode run for {session_id}"))?;

    Ok(())
}

pub fn spawn_fresh_server_session(
    message: String,
    event_tx: DesktopSessionEventSender,
) -> Result<DesktopSessionHandle> {
    if message.trim().is_empty() {
        anyhow::bail!("empty draft message");
    }

    let (command_tx, command_rx) = mpsc::channel();
    let handle = DesktopSessionHandle { command_tx };
    std::thread::Builder::new()
        .name("jcode-desktop-fresh-session".to_string())
        .spawn(move || {
            if let Err(error) =
                run_server_session(None, &message, Some(event_tx.clone()), command_rx)
            {
                let _ = event_tx.send(DesktopSessionEvent::Error(format!("{error:#}")));
            }
        })
        .context("failed to spawn desktop session worker")?;
    Ok(handle)
}

pub fn spawn_message_to_session(
    session_id: String,
    message: String,
    event_tx: DesktopSessionEventSender,
) -> Result<DesktopSessionHandle> {
    validate_resume_session_id(&session_id).context("refusing to send to invalid session id")?;
    if message.trim().is_empty() {
        anyhow::bail!("empty draft message");
    }

    let (command_tx, command_rx) = mpsc::channel();
    let handle = DesktopSessionHandle { command_tx };
    std::thread::Builder::new()
        .name("jcode-desktop-session-message".to_string())
        .spawn(move || {
            if let Err(error) = run_server_session(
                Some(&session_id),
                &message,
                Some(event_tx.clone()),
                command_rx,
            ) {
                let _ = event_tx.send(DesktopSessionEvent::Error(format!("{error:#}")));
            }
        })
        .context("failed to spawn desktop session worker")?;
    Ok(handle)
}

#[cfg(unix)]
fn run_server_session(
    target_session_id: Option<&str>,
    message: &str,
    event_tx: Option<DesktopSessionEventSender>,
    command_rx: Receiver<DesktopSessionCommand>,
) -> Result<String> {
    send_desktop_status(&event_tx, "starting shared server");
    ensure_server_running()?;
    send_desktop_status(&event_tx, "connecting to shared server");
    let stream = connect_server_with_retry(SERVER_START_TIMEOUT)?;
    let mut writer = stream
        .try_clone()
        .context("failed to clone server socket writer")?;
    let mut reader = BufReader::new(stream);
    let mut next_request_id = 1_u64;

    let subscribe_request_id = next_request_id;
    subscribe_to_server(&mut writer, subscribe_request_id, target_session_id)?;
    next_request_id += 1;

    let session_id = establish_session_id(
        &mut reader,
        &mut writer,
        &mut next_request_id,
        subscribe_request_id,
        event_tx.as_ref(),
    )?;
    send_desktop_event(
        &event_tx,
        DesktopSessionEvent::SessionStarted {
            session_id: session_id.clone(),
        },
    );

    send_desktop_status(&event_tx, "sending message");
    let message_request_id = next_request_id;
    write_json_line(
        &mut writer,
        json!({
            "type": "message",
            "id": message_request_id,
            "content": message,
            "images": [],
        }),
    )?;
    next_request_id += 1;

    let mut current_socket_path = socket_path();
    loop {
        match drain_session_events(
            reader,
            &mut writer,
            &mut next_request_id,
            event_tx.as_ref(),
            &command_rx,
            message_request_id,
        )? {
            DrainOutcome::Terminal => break,
            DrainOutcome::Disconnected => {
                send_desktop_status(&event_tx, "server disconnected, reconnecting");
            }
            DrainOutcome::Reloading { new_socket } => {
                if let Some(path) = new_socket {
                    current_socket_path = PathBuf::from(path);
                }
                send_desktop_status(&event_tx, "server reloading, reconnecting");
            }
        }

        let stream = connect_server_with_retry_path(&current_socket_path, SERVER_START_TIMEOUT)?;
        writer = stream
            .try_clone()
            .context("failed to clone reconnected server socket writer")?;
        reader = BufReader::new(stream);
        let subscribe_request_id = next_request_id;
        subscribe_to_server(&mut writer, subscribe_request_id, Some(&session_id))?;
        next_request_id += 1;
        let _ = establish_session_id(
            &mut reader,
            &mut writer,
            &mut next_request_id,
            subscribe_request_id,
            event_tx.as_ref(),
        )?;
    }
    Ok(session_id)
}

#[cfg(not(unix))]
fn run_server_session(
    _target_session_id: Option<&str>,
    _message: &str,
    _event_tx: Option<DesktopSessionEventSender>,
    _command_rx: Receiver<DesktopSessionCommand>,
) -> Result<String> {
    anyhow::bail!("desktop server sessions are not implemented on this platform yet")
}

#[cfg(unix)]
fn ensure_server_running() -> Result<()> {
    if UnixStream::connect(socket_path()).is_ok() {
        return Ok(());
    }

    Command::new(jcode_bin())
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn jcode serve")?;

    connect_server_with_retry(SERVER_START_TIMEOUT).map(|_| ())
}

#[cfg(unix)]
fn connect_server_with_retry(timeout: Duration) -> Result<UnixStream> {
    connect_server_with_retry_path(&socket_path(), timeout)
}

#[cfg(unix)]
fn connect_server_with_retry_path(socket_path: &PathBuf, timeout: Duration) -> Result<UnixStream> {
    let started = Instant::now();
    let mut last_error = None;
    while started.elapsed() < timeout {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(SERVER_CONNECT_RETRY_DELAY);
    }

    match last_error {
        Some(error) => Err(error).with_context(|| {
            format!(
                "timed out connecting to jcode server at {}",
                socket_path.display()
            )
        }),
        None => anyhow::bail!("timed out connecting to jcode server"),
    }
}

#[cfg(unix)]
fn subscribe_to_server(
    writer: &mut UnixStream,
    id: u64,
    target_session_id: Option<&str>,
) -> Result<()> {
    write_json_line(
        writer,
        json!({
            "type": "subscribe",
            "id": id,
            "target_session_id": target_session_id,
            "client_has_local_history": false,
            "allow_session_takeover": false,
        }),
    )
}

#[cfg(unix)]
fn establish_session_id(
    reader: &mut BufReader<UnixStream>,
    writer: &mut UnixStream,
    next_request_id: &mut u64,
    subscribe_request_id: u64,
    event_tx: Option<&DesktopSessionEventSender>,
) -> Result<String> {
    if let Some(session_id) = read_session_id_from_events(
        reader,
        SERVER_START_TIMEOUT,
        event_tx,
        Some(subscribe_request_id),
    )? {
        return Ok(session_id);
    }

    let state_request_id = *next_request_id;
    write_json_line(
        writer,
        json!({
            "type": "state",
            "id": state_request_id,
        }),
    )?;
    *next_request_id += 1;
    read_session_id_from_state(reader, SERVER_START_TIMEOUT, event_tx, state_request_id)
}

#[cfg(unix)]
fn read_session_id_from_events(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    complete_request_id: Option<u64>,
) -> Result<Option<String>> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let started = Instant::now();
    let mut line = String::new();
    while started.elapsed() < timeout {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => anyhow::bail!("jcode server disconnected before assigning a session"),
            Ok(_) => {
                let value: Value = serde_json::from_str(line.trim())
                    .context("failed to parse jcode server event")?;
                if value.get("type").and_then(Value::as_str) == Some("session") {
                    let Some(session_id) = value.get("session_id").and_then(Value::as_str) else {
                        anyhow::bail!("jcode server sent malformed session event");
                    };
                    return Ok(Some(session_id.to_string()));
                }
                if let Some(event) = desktop_event_from_server_value(&value) {
                    if !matches!(event, DesktopSessionEvent::Done) {
                        send_desktop_event_ref(event_tx, event);
                    }
                }
                if value.get("type").and_then(Value::as_str) == Some("error") {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    anyhow::bail!("jcode server rejected fresh session: {message}");
                }
                if value.get("type").and_then(Value::as_str) == Some("done")
                    && complete_request_id
                        .is_some_and(|id| value.get("id").and_then(Value::as_u64) == Some(id))
                {
                    return Ok(None);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error).context("failed to read jcode server event"),
        }
    }

    anyhow::bail!("timed out waiting for jcode server session id")
}

#[cfg(unix)]
fn read_session_id_from_state(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    state_request_id: u64,
) -> Result<String> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let started = Instant::now();
    let mut line = String::new();
    while started.elapsed() < timeout {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => anyhow::bail!("jcode server disconnected before returning state"),
            Ok(_) => {
                let value: Value = serde_json::from_str(line.trim())
                    .context("failed to parse jcode server event")?;
                if value.get("type").and_then(Value::as_str) == Some("state")
                    && value.get("id").and_then(Value::as_u64) == Some(state_request_id)
                {
                    let Some(session_id) = value.get("session_id").and_then(Value::as_str) else {
                        anyhow::bail!("jcode server sent malformed state event");
                    };
                    return Ok(session_id.to_string());
                }
                if let Some(event) = desktop_event_from_server_value(&value) {
                    if !matches!(event, DesktopSessionEvent::Done) {
                        send_desktop_event_ref(event_tx, event);
                    }
                }
                if value.get("type").and_then(Value::as_str) == Some("error")
                    && value.get("id").and_then(Value::as_u64) == Some(state_request_id)
                {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    anyhow::bail!("jcode server rejected state request: {message}");
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error).context("failed to read jcode server event"),
        }
    }

    anyhow::bail!("timed out waiting for jcode server state")
}

#[cfg(unix)]
fn write_json_line(writer: &mut UnixStream, value: Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, &value).context("failed to encode server request")?;
    writer
        .write_all(b"\n")
        .context("failed to send server request")?;
    writer.flush().context("failed to flush server request")
}

#[cfg(unix)]
enum DrainOutcome {
    Terminal,
    Disconnected,
    Reloading { new_socket: Option<String> },
}

#[cfg(unix)]
fn drain_session_events(
    mut reader: BufReader<UnixStream>,
    writer: &mut UnixStream,
    next_request_id: &mut u64,
    event_tx: Option<&DesktopSessionEventSender>,
    command_rx: &Receiver<DesktopSessionCommand>,
    terminal_request_id: u64,
) -> Result<DrainOutcome> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let mut line = String::new();
    loop {
        drain_worker_commands(writer, next_request_id, event_tx, command_rx)?;
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(DrainOutcome::Disconnected),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("failed to read jcode server event"),
            Ok(_) => {
                if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
                    if value.get("type").and_then(Value::as_str) == Some("reloading") {
                        let new_socket = value
                            .get("new_socket")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        send_desktop_event_ref(
                            event_tx,
                            DesktopSessionEvent::Reloading {
                                new_socket: new_socket.clone(),
                            },
                        );
                        return Ok(DrainOutcome::Reloading { new_socket });
                    }
                    let is_terminal = match value.get("type").and_then(Value::as_str) {
                        Some("done") => {
                            value.get("id").and_then(Value::as_u64) == Some(terminal_request_id)
                        }
                        Some("error") => value
                            .get("id")
                            .and_then(Value::as_u64)
                            .is_none_or(|id| id == terminal_request_id),
                        _ => false,
                    };
                    if let Some(event) = desktop_event_from_server_value(&value) {
                        send_desktop_event_ref(event_tx, event);
                    }
                    if is_terminal {
                        return Ok(DrainOutcome::Terminal);
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
fn drain_worker_commands(
    writer: &mut UnixStream,
    next_request_id: &mut u64,
    event_tx: Option<&DesktopSessionEventSender>,
    command_rx: &Receiver<DesktopSessionCommand>,
) -> Result<()> {
    while let Ok(command) = command_rx.try_recv() {
        match command {
            DesktopSessionCommand::Cancel => {
                send_desktop_event_ref(
                    event_tx,
                    DesktopSessionEvent::Status("cancelling".to_string()),
                );
                write_json_line(
                    writer,
                    json!({
                        "type": "cancel",
                        "id": *next_request_id,
                    }),
                )?;
                *next_request_id += 1;
            }
        }
    }
    Ok(())
}

fn desktop_event_from_server_value(value: &Value) -> Option<DesktopSessionEvent> {
    match value.get("type").and_then(Value::as_str)? {
        "session" => value
            .get("session_id")
            .and_then(Value::as_str)
            .map(|session_id| DesktopSessionEvent::SessionStarted {
                session_id: session_id.to_string(),
            }),
        "text_delta" => value
            .get("text")
            .and_then(Value::as_str)
            .map(|text| DesktopSessionEvent::TextDelta(text.to_string())),
        "text_replace" => value
            .get("text")
            .and_then(Value::as_str)
            .map(|text| DesktopSessionEvent::TextReplace(text.to_string())),
        "connection_phase" => value
            .get("phase")
            .and_then(Value::as_str)
            .map(|phase| DesktopSessionEvent::Status(phase.to_string())),
        "status_detail" => value
            .get("detail")
            .and_then(Value::as_str)
            .map(|detail| DesktopSessionEvent::Status(detail.to_string())),
        "tool_start" | "tool_exec" => value
            .get("name")
            .and_then(Value::as_str)
            .map(|name| DesktopSessionEvent::Status(format!("using tool {name}"))),
        "interrupted" => Some(DesktopSessionEvent::Status("interrupted".to_string())),
        "reloading" => Some(DesktopSessionEvent::Reloading {
            new_socket: value
                .get("new_socket")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        }),
        "done" => Some(DesktopSessionEvent::Done),
        "error" => Some(DesktopSessionEvent::Error(
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown server error")
                .to_string(),
        )),
        _ => None,
    }
}

fn send_desktop_status(event_tx: &Option<DesktopSessionEventSender>, status: &str) {
    send_desktop_event(event_tx, DesktopSessionEvent::Status(status.to_string()));
}

fn send_desktop_event(event_tx: &Option<DesktopSessionEventSender>, event: DesktopSessionEvent) {
    send_desktop_event_ref(event_tx.as_ref(), event);
}

fn send_desktop_event_ref(
    event_tx: Option<&DesktopSessionEventSender>,
    event: DesktopSessionEvent,
) {
    if let Some(event_tx) = event_tx {
        let _ = event_tx.send(event);
    }
}

fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_SOCKET") {
        return PathBuf::from(custom);
    }
    if let Ok(dir) = std::env::var("JCODE_RUNTIME_DIR") {
        return PathBuf::from(dir).join("jcode.sock");
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("jcode.sock");
    }
    std::env::temp_dir()
        .join(format!("jcode-{}", runtime_user_discriminator()))
        .join("jcode.sock")
}

#[cfg(unix)]
fn runtime_user_discriminator() -> String {
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(not(unix))]
fn runtime_user_discriminator() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".to_string())
}

fn launch_first_available_terminal(candidates: Vec<Command>, description: &str) -> Result<()> {
    let mut failures = Vec::new();

    for mut candidate in candidates {
        match candidate.spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                failures.push(format!(
                    "{} not found",
                    candidate.get_program().to_string_lossy()
                ));
            }
            Err(error) => {
                failures.push(format!(
                    "{}: {error}",
                    candidate.get_program().to_string_lossy()
                ));
            }
        }
    }

    anyhow::bail!(
        "failed to launch a terminal for {description}: {}",
        failures.join("; ")
    )
}

fn terminal_candidates(title: &str, jcode_args: &[&str]) -> Vec<Command> {
    let mut candidates = Vec::new();

    if let Ok(program) = std::env::var("JCODE_DESKTOP_TERMINAL") {
        candidates.push(terminal_command(program, &[], jcode_args));
    }

    candidates.push(terminal_command(
        "footclient",
        &["-T", title, "--"],
        jcode_args,
    ));
    candidates.push(terminal_command("foot", &["-T", title, "--"], jcode_args));
    candidates.push(terminal_command("kitty", &["--title", title], jcode_args));
    candidates.push(terminal_command(
        "alacritty",
        &["-t", title, "-e"],
        jcode_args,
    ));
    candidates.push(terminal_command("wezterm", &["start", "--"], jcode_args));
    candidates.push(terminal_command(
        "x-terminal-emulator",
        &["-T", title, "-e"],
        jcode_args,
    ));

    candidates
}

fn terminal_command(
    program: impl AsRef<str>,
    prefix_args: &[&str],
    jcode_args: &[&str],
) -> Command {
    let mut command = Command::new(program.as_ref());
    command
        .args(prefix_args)
        .arg(jcode_bin())
        .args(jcode_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn jcode_bin() -> String {
    std::env::var("JCODE_BIN").unwrap_or_else(|_| "jcode".to_string())
}

fn compact_title(title: &str) -> String {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "session".to_string();
    }

    let mut chars = normalized.chars();
    let compact = chars.by_ref().take(48).collect::<String>();
    if chars.next().is_some() {
        format!("{compact}…")
    } else {
        compact
    }
}

pub fn validate_resume_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() {
        anyhow::bail!("empty session id");
    }
    if !session_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        anyhow::bail!("session id contains unsupported characters");
    }
    Ok(())
}

pub fn launch_validated_resume_session(session_id: &str, title: &str) -> Result<()> {
    validate_resume_session_id(session_id).context("refusing to launch invalid session id")?;
    launch_resume_session(session_id, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::net::UnixListener;
    #[cfg(unix)]
    use std::sync::Mutex;

    #[test]
    fn validates_safe_session_ids() -> Result<()> {
        validate_resume_session_id("session_cow_123-abc.def")?;
        assert!(validate_resume_session_id("bad/id").is_err());
        assert!(validate_resume_session_id("bad id").is_err());
        Ok(())
    }

    #[test]
    fn compact_title_shortens_long_titles() {
        let title =
            compact_title("this is a very long title that should become shorter for terminals");
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= 49);
    }

    #[test]
    fn desktop_event_parser_maps_streaming_server_events() {
        assert_eq!(
            desktop_event_from_server_value(&json!({"type": "text_delta", "text": "hello"})),
            Some(DesktopSessionEvent::TextDelta("hello".to_string()))
        );
        assert_eq!(
            desktop_event_from_server_value(&json!({"type": "done", "id": 2})),
            Some(DesktopSessionEvent::Done)
        );
        assert_eq!(
            desktop_event_from_server_value(&json!({"type": "tool_exec", "name": "bash"})),
            Some(DesktopSessionEvent::Status("using tool bash".to_string()))
        );
        assert_eq!(
            desktop_event_from_server_value(&json!({
                "type": "reloading",
                "new_socket": "/tmp/jcode-new.sock"
            })),
            Some(DesktopSessionEvent::Reloading {
                new_socket: Some("/tmp/jcode-new.sock".to_string())
            })
        );
    }

    #[test]
    fn desktop_session_handle_sends_cancel_command() {
        let (command_tx, command_rx) = mpsc::channel();
        let handle = DesktopSessionHandle { command_tx };

        handle.cancel().unwrap();

        assert_eq!(command_rx.try_recv(), Ok(DesktopSessionCommand::Cancel));
    }

    #[cfg(unix)]
    #[test]
    fn desktop_worker_roundtrips_message_with_fake_server() -> Result<()> {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let socket_path = std::env::temp_dir().join(format!(
            "jcode-desktop-worker-smoke-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path)?;
        let previous_socket = std::env::var_os("JCODE_SOCKET");
        unsafe {
            std::env::set_var("JCODE_SOCKET", &socket_path);
        }

        let server = std::thread::spawn(move || fake_desktop_server_roundtrip(listener));
        let (event_tx, event_rx) = mpsc::channel();
        let (_command_tx, command_rx) = mpsc::channel();

        let result = run_server_session(None, "hello desktop", Some(event_tx), command_rx);

        restore_env_var("JCODE_SOCKET", previous_socket);
        let _ = std::fs::remove_file(&socket_path);

        assert_eq!(result?, "session_desktop_fake");
        let requests = server.join().unwrap()?;
        assert_eq!(requests[0]["type"], "subscribe");
        assert_eq!(requests[1]["type"], "state");
        assert_eq!(requests[2]["type"], "message");
        assert_eq!(requests[2]["content"], "hello desktop");
        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(events.contains(&DesktopSessionEvent::SessionStarted {
            session_id: "session_desktop_fake".to_string()
        }));
        assert!(events.contains(&DesktopSessionEvent::TextDelta(
            "fake assistant response".to_string()
        )));
        assert!(events.contains(&DesktopSessionEvent::Done));
        Ok(())
    }

    #[cfg(unix)]
    fn fake_desktop_server_roundtrip(listener: UnixListener) -> Result<Vec<Value>> {
        let (mut reader, mut writer, subscribe) = accept_first_requesting_client(&listener)?;
        write_json_line(&mut writer, json!({"type": "ack", "id": subscribe["id"]}))?;
        write_json_line(&mut writer, json!({"type": "mcp_status", "servers": []}))?;
        write_json_line(&mut writer, json!({"type": "done", "id": subscribe["id"]}))?;

        let state = read_fake_server_request(&mut reader)?;
        write_json_line(
            &mut writer,
            json!({
                "type": "state",
                "id": state["id"],
                "session_id": "session_desktop_fake",
                "message_count": 0,
                "is_processing": false,
            }),
        )?;

        let message = read_fake_server_request(&mut reader)?;
        write_json_line(&mut writer, json!({"type": "ack", "id": message["id"]}))?;
        write_json_line(
            &mut writer,
            json!({"type": "text_delta", "text": "fake assistant response"}),
        )?;
        write_json_line(&mut writer, json!({"type": "done", "id": message["id"]}))?;
        Ok(vec![subscribe, state, message])
    }

    #[cfg(unix)]
    fn accept_first_requesting_client(
        listener: &UnixListener,
    ) -> Result<(BufReader<UnixStream>, UnixStream, Value)> {
        loop {
            let (stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut first_line = String::new();
            match reader.read_line(&mut first_line) {
                Ok(0) => continue,
                Ok(_) => {
                    let first_request = serde_json::from_str(first_line.trim())?;
                    return Ok((reader, stream, first_request));
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    #[cfg(unix)]
    fn read_fake_server_request(reader: &mut BufReader<UnixStream>) -> Result<Value> {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        Ok(serde_json::from_str(line.trim())?)
    }

    fn restore_env_var(key: &str, value: Option<std::ffi::OsString>) {
        unsafe {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}
