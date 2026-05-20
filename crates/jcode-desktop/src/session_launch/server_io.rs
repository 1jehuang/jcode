use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, mpsc::Receiver};
use std::time::{Duration, Instant};

use super::events::{
    desktop_event_from_server_value, history_reasoning_effort_from_server_value,
    model_catalog_event_from_server_value,
};
use super::terminal::jcode_bin;
use super::{
    DesktopSessionCommand, DesktopSessionEvent, DesktopSessionEventSender, DesktopSessionStatus,
    SERVER_CONNECT_RETRY_DELAY, SERVER_START_TIMEOUT, send_desktop_event_ref, socket_path,
};

const CANCEL_COMPLETION_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn ensure_server_running() -> Result<()> {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Ok(());
    }

    static SERVER_START_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = SERVER_START_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("desktop server startup lock is poisoned"))?;

    if UnixStream::connect(&path).is_ok() {
        return Ok(());
    }

    spawn_jcode_server_with_diagnostics()?;
    connect_server_with_retry_path(&path, SERVER_START_TIMEOUT).map(|_| ())
}

fn spawn_jcode_server_with_diagnostics() -> Result<()> {
    let mut command = Command::new(jcode_bin());
    command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(working_dir) = super::default_desktop_working_dir() {
        command.current_dir(working_dir);
    }

    let mut child = command.spawn().context("failed to spawn jcode serve")?;

    let pid = child.id();
    if let Some(stdout) = child.stdout.take() {
        spawn_child_output_logger(pid, "stdout", stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_child_output_logger(pid, "stderr", stderr);
    }

    std::thread::Builder::new()
        .name("jcode-desktop-serve-wait".to_string())
        .spawn(move || match child.wait() {
            Ok(status) if status.success() => crate::desktop_log::info(format_args!(
                "jcode-desktop: jcode serve child pid={pid} exited with {status}"
            )),
            Ok(status) => crate::desktop_log::error(format_args!(
                "jcode-desktop: jcode serve child pid={pid} exited with {status}"
            )),
            Err(error) => crate::desktop_log::warn(format_args!(
                "jcode-desktop: failed to wait for jcode serve child pid={pid}: {error}"
            )),
        })
        .context("failed to spawn jcode serve wait logger")?;

    Ok(())
}

fn spawn_child_output_logger<R>(pid: u32, stream_name: &'static str, pipe: R)
where
    R: Read + Send + 'static,
{
    if let Err(error) = std::thread::Builder::new()
        .name(format!("jcode-desktop-serve-{stream_name}"))
        .spawn(move || {
            let reader = BufReader::new(pipe);
            for line in reader.lines() {
                match line {
                    Ok(line) => crate::desktop_log::info(format_args!(
                        "jcode-desktop: jcode serve pid={pid} {stream_name}: {}",
                        crate::desktop_log::truncate_for_log(&line, 4096)
                    )),
                    Err(error) => {
                        crate::desktop_log::warn(format_args!(
                            "jcode-desktop: failed reading jcode serve pid={pid} {stream_name}: {error}"
                        ));
                        break;
                    }
                }
            }
        })
    {
        crate::desktop_log::warn(format_args!(
            "jcode-desktop: failed to spawn jcode serve {stream_name} logger: {error}"
        ));
    }
}

#[cfg(unix)]
pub(super) fn connect_server_with_retry(timeout: Duration) -> Result<UnixStream> {
    connect_server_with_retry_path(&socket_path(), timeout)
}

#[cfg(unix)]
pub(super) fn connect_server_with_retry_path(
    socket_path: &Path,
    timeout: Duration,
) -> Result<UnixStream> {
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
pub(super) fn validate_reload_socket_path(
    current_socket_path: &Path,
    raw_new_socket: &str,
) -> Result<PathBuf> {
    let trimmed = raw_new_socket.trim();
    if trimmed.is_empty() {
        anyhow::bail!("jcode server advertised an empty reload socket path");
    }

    let new_socket_path = PathBuf::from(trimmed);
    if !new_socket_path.is_absolute() {
        anyhow::bail!(
            "jcode server advertised non-absolute reload socket path {}",
            new_socket_path.display()
        );
    }

    let metadata = std::fs::symlink_metadata(&new_socket_path).with_context(|| {
        format!(
            "failed to inspect advertised reload socket {}",
            new_socket_path.display()
        )
    })?;
    if !metadata.file_type().is_socket() {
        anyhow::bail!(
            "jcode server advertised reload path that is not a socket: {}",
            new_socket_path.display()
        );
    }

    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        anyhow::bail!(
            "jcode server advertised reload socket owned by uid {}, expected uid {}: {}",
            metadata.uid(),
            effective_uid,
            new_socket_path.display()
        );
    }

    let current_parent = current_socket_path.parent().with_context(|| {
        format!(
            "current jcode socket path has no parent: {}",
            current_socket_path.display()
        )
    })?;
    let new_parent = new_socket_path.parent().with_context(|| {
        format!(
            "advertised reload socket path has no parent: {}",
            new_socket_path.display()
        )
    })?;
    let current_parent = current_parent.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize current jcode socket directory {}",
            current_parent.display()
        )
    })?;
    let new_parent = new_parent.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize advertised reload socket directory {}",
            new_parent.display()
        )
    })?;
    if current_parent != new_parent {
        anyhow::bail!(
            "jcode server advertised reload socket outside current socket directory: {} (current directory {})",
            new_socket_path.display(),
            current_parent.display()
        );
    }

    Ok(new_socket_path)
}

#[cfg(unix)]
pub(super) fn subscribe_to_server(
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
pub(super) fn establish_session_id(
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
pub(super) fn subscribe_and_establish_session(
    reader: &mut BufReader<UnixStream>,
    writer: &mut UnixStream,
    next_request_id: &mut u64,
    target_session_id: Option<&str>,
    event_tx: Option<&DesktopSessionEventSender>,
) -> Result<String> {
    let subscribe_request_id = *next_request_id;
    subscribe_to_server(writer, subscribe_request_id, target_session_id)?;
    *next_request_id += 1;
    establish_session_id(
        reader,
        writer,
        next_request_id,
        subscribe_request_id,
        event_tx,
    )
}

#[cfg(unix)]
pub(super) fn read_session_id_from_events(
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
                let value = parse_server_event_line(&line, "waiting for session id")?;
                if value.get("type").and_then(Value::as_str) == Some("session") {
                    let Some(session_id) = value.get("session_id").and_then(Value::as_str) else {
                        anyhow::bail!("jcode server sent malformed session event");
                    };
                    return Ok(Some(session_id.to_string()));
                }
                forward_non_done_server_event(event_tx, &value);
                if value.get("type").and_then(Value::as_str) == Some("error") {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected fresh session: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
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
pub(super) fn read_session_id_from_state(
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
                let value = parse_server_event_line(&line, "waiting for server state")?;
                if value.get("type").and_then(Value::as_str) == Some("state")
                    && value.get("id").and_then(Value::as_u64) == Some(state_request_id)
                {
                    let Some(session_id) = value.get("session_id").and_then(Value::as_str) else {
                        anyhow::bail!("jcode server sent malformed state event");
                    };
                    return Ok(session_id.to_string());
                }
                forward_non_done_server_event(event_tx, &value);
                if value.get("type").and_then(Value::as_str) == Some("error")
                    && value.get("id").and_then(Value::as_u64) == Some(state_request_id)
                {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected state request id={state_request_id}: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
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
pub(super) fn read_model_changed(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    request_id: u64,
) -> Result<()> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let started = Instant::now();
    let mut line = String::new();
    while started.elapsed() < timeout {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => anyhow::bail!("jcode server disconnected before switching model"),
            Ok(_) => {
                let value = parse_server_event_line(&line, "waiting for model switch")?;
                if value.get("type").and_then(Value::as_str) == Some("model_changed")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    if let Some(event) = desktop_event_from_server_value(&value) {
                        send_desktop_event_ref(event_tx, event);
                    }
                    return Ok(());
                }
                forward_non_done_server_event(event_tx, &value);
                if value.get("type").and_then(Value::as_str) == Some("error")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected model switch id={request_id}: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
                    anyhow::bail!("jcode server rejected model switch: {message}");
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

    anyhow::bail!("timed out waiting for jcode server model switch")
}

#[cfg(unix)]
pub(super) fn read_history_reasoning_effort(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    request_id: u64,
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
            Ok(0) => anyhow::bail!("jcode server disconnected before loading history"),
            Ok(_) => {
                let value = parse_server_event_line(&line, "waiting for history")?;
                if value.get("type").and_then(Value::as_str) == Some("history")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    return Ok(history_reasoning_effort_from_server_value(&value));
                }
                forward_non_done_server_event(event_tx, &value);
                if value.get("type").and_then(Value::as_str) == Some("error")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected history request id={request_id}: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
                    anyhow::bail!("jcode server rejected history request: {message}");
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

    anyhow::bail!("timed out waiting for jcode server history")
}

#[cfg(unix)]
pub(super) fn read_reasoning_effort_changed(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    request_id: u64,
) -> Result<()> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let started = Instant::now();
    let mut line = String::new();
    while started.elapsed() < timeout {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => anyhow::bail!("jcode server disconnected before switching reasoning effort"),
            Ok(_) => {
                let value = parse_server_event_line(&line, "waiting for reasoning effort switch")?;
                if value.get("type").and_then(Value::as_str) == Some("reasoning_effort_changed")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    if let Some(event) = desktop_event_from_server_value(&value) {
                        send_desktop_event_ref(event_tx, event);
                    }
                    return Ok(());
                }
                forward_non_done_server_event(event_tx, &value);
                if value.get("type").and_then(Value::as_str) == Some("error")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected reasoning effort switch id={request_id}: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
                    anyhow::bail!("jcode server rejected reasoning effort switch: {message}");
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

    anyhow::bail!("timed out waiting for jcode server reasoning effort switch")
}

#[cfg(unix)]
pub(super) fn read_model_catalog(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    request_id: u64,
) -> Result<()> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let started = Instant::now();
    let mut line = String::new();
    while started.elapsed() < timeout {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => anyhow::bail!("jcode server disconnected before loading model catalog"),
            Ok(_) => {
                let value = parse_server_event_line(&line, "waiting for model catalog")?;
                if value.get("type").and_then(Value::as_str) == Some("history")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    if let Some(event) = model_catalog_event_from_server_value(&value) {
                        send_desktop_event_ref(event_tx, event);
                        return Ok(());
                    }
                    anyhow::bail!("jcode server returned malformed model catalog");
                }
                forward_non_done_server_event(event_tx, &value);
                if value.get("type").and_then(Value::as_str) == Some("error")
                    && value.get("id").and_then(Value::as_u64) == Some(request_id)
                {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected model catalog request id={request_id}: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
                    anyhow::bail!("jcode server rejected model catalog request: {message}");
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

    anyhow::bail!("timed out waiting for jcode server model catalog")
}

#[cfg(unix)]
pub(super) fn read_control_response(
    reader: &mut BufReader<UnixStream>,
    timeout: Duration,
    event_tx: Option<&DesktopSessionEventSender>,
    request_id: u64,
    expected_event_types: &[&str],
    action_label: &str,
) -> Result<()> {
    reader
        .get_ref()
        .set_read_timeout(Some(SERVER_CONNECT_RETRY_DELAY))
        .context("failed to configure server socket timeout")?;
    let started = Instant::now();
    let mut line = String::new();
    while started.elapsed() < timeout {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => anyhow::bail!("jcode server disconnected while {action_label}"),
            Ok(_) => {
                let value = parse_server_event_line(&line, action_label)?;
                let event_type = value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let event_id = value.get("id").and_then(Value::as_u64);
                let id_matches = event_id == Some(request_id);

                if expected_event_types.contains(&event_type) && (id_matches || event_id.is_none())
                {
                    if let Some(event) = desktop_event_from_server_value(&value) {
                        send_desktop_event_ref(event_tx, event);
                    }
                    return Ok(());
                }

                forward_non_done_server_event(event_tx, &value);
                if event_type == "error" && id_matches {
                    let message = value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error");
                    crate::desktop_log::error(format_args!(
                        "jcode-desktop: jcode server rejected {action_label} id={request_id}: {}",
                        crate::desktop_log::truncate_for_log(message, 2048)
                    ));
                    anyhow::bail!("jcode server rejected {action_label}: {message}");
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

    anyhow::bail!("timed out waiting for jcode server while {action_label}")
}

#[cfg(unix)]
pub(super) fn write_json_line(writer: &mut UnixStream, value: Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, &value).context("failed to encode server request")?;
    writer
        .write_all(b"\n")
        .context("failed to send server request")?;
    writer.flush().context("failed to flush server request")
}

#[cfg(unix)]
pub(super) enum DrainOutcome {
    Terminal,
    Disconnected,
    Reloading { new_socket: Option<String> },
}

#[cfg(unix)]
pub(super) fn drain_session_events(
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
    let mut pending_cancel_requests = Vec::<PendingCancelRequest>::new();
    loop {
        let now = Instant::now();
        pending_cancel_requests.extend(
            drain_worker_commands(writer, next_request_id, event_tx, command_rx)?
                .into_iter()
                .map(|request_id| PendingCancelRequest {
                    request_id,
                    requested_at: now,
                }),
        );
        if let Some(expired) = pending_cancel_requests
            .iter()
            .find(|request| request.requested_at.elapsed() >= CANCEL_COMPLETION_TIMEOUT)
        {
            anyhow::bail!(
                "timed out waiting for cancel request {} to complete",
                expired.request_id
            );
        }
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
                let value = parse_server_event_line(&line, "draining session events")?;
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

                let event_type = value.get("type").and_then(Value::as_str);
                let event_id = value.get("id").and_then(Value::as_u64);
                if let Some(cancel_request_id) = event_id.filter(|event_id| {
                    pending_cancel_requests
                        .iter()
                        .any(|request| request.request_id == *event_id)
                }) {
                    match event_type {
                        Some("done") => {
                            send_desktop_event_ref(
                                event_tx,
                                DesktopSessionEvent::Status(DesktopSessionStatus::Cancelled),
                            );
                            send_desktop_event_ref(event_tx, DesktopSessionEvent::Done);
                            return Ok(DrainOutcome::Terminal);
                        }
                        Some("error") => {
                            let message = value
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown server error");
                            anyhow::bail!(
                                "jcode server rejected cancel request {cancel_request_id}: {message}"
                            );
                        }
                        _ => {}
                    }
                }

                let is_terminal = match event_type {
                    Some("done") => {
                        value.get("id").and_then(Value::as_u64) == Some(terminal_request_id)
                    }
                    Some("error") => value
                        .get("id")
                        .and_then(Value::as_u64)
                        .is_none_or(|id| id == terminal_request_id),
                    _ => false,
                };
                if let Some(event) = desktop_event_from_server_value(&value)
                    && (!matches!(event, DesktopSessionEvent::Done) || is_terminal)
                {
                    send_desktop_event_ref(event_tx, event);
                }
                if is_terminal {
                    return Ok(DrainOutcome::Terminal);
                }
            }
        }
    }
}

fn parse_server_event_line(line: &str, context: &str) -> Result<Value> {
    match serde_json::from_str::<Value>(line.trim()) {
        Ok(value) => {
            validate_server_event_value(&value, context)?;
            Ok(value)
        }
        Err(error) => {
            crate::desktop_log::error(format_args!(
                "jcode-desktop: failed to parse jcode server event while {context}: {error}; line={}",
                crate::desktop_log::truncate_for_log(line.trim(), 512)
            ));
            Err(error).context("failed to parse jcode server event")
        }
    }
}

fn validate_server_event_value(value: &Value, context: &str) -> Result<()> {
    let Some(object) = value.as_object() else {
        anyhow::bail!("jcode server sent non-object event while {context}");
    };
    let Some(event_type) = object.get("type").and_then(Value::as_str) else {
        anyhow::bail!("jcode server sent event without string type while {context}");
    };

    match event_type {
        "stdin_request" => {
            require_non_empty_event_string(value, "request_id", event_type, context)?;
            require_non_empty_event_string(value, "tool_call_id", event_type, context)?;
            if value
                .get("prompt")
                .is_some_and(|prompt| !prompt.is_string())
            {
                anyhow::bail!(
                    "jcode server sent stdin_request with non-string prompt while {context}"
                );
            }
            if value
                .get("is_password")
                .is_some_and(|is_password| !is_password.is_boolean())
            {
                anyhow::bail!(
                    "jcode server sent stdin_request with non-boolean is_password while {context}"
                );
            }
        }
        "reloading" => {
            if value
                .get("new_socket")
                .is_some_and(|new_socket| !new_socket.is_string())
            {
                anyhow::bail!(
                    "jcode server sent reloading event with non-string new_socket while {context}"
                );
            }
        }
        _ => {}
    }

    Ok(())
}

fn require_non_empty_event_string(
    value: &Value,
    field: &str,
    event_type: &str,
    context: &str,
) -> Result<()> {
    if value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        Ok(())
    } else {
        anyhow::bail!(
            "jcode server sent {event_type} event without non-empty string {field} while {context}"
        );
    }
}

struct PendingCancelRequest {
    request_id: u64,
    requested_at: Instant,
}

fn forward_non_done_server_event(event_tx: Option<&DesktopSessionEventSender>, value: &Value) {
    if let Some(event) = desktop_event_from_server_value(value)
        && !matches!(event, DesktopSessionEvent::Done)
    {
        send_desktop_event_ref(event_tx, event);
    }
}

#[cfg(unix)]
pub(super) fn drain_worker_commands(
    writer: &mut UnixStream,
    next_request_id: &mut u64,
    event_tx: Option<&DesktopSessionEventSender>,
    command_rx: &Receiver<DesktopSessionCommand>,
) -> Result<Vec<u64>> {
    let mut cancel_request_ids = Vec::new();
    while let Ok(command) = command_rx.try_recv() {
        match command {
            DesktopSessionCommand::Cancel => {
                send_desktop_event_ref(
                    event_tx,
                    DesktopSessionEvent::Status(DesktopSessionStatus::Cancelling),
                );
                let request_id = *next_request_id;
                let write_start = Instant::now();
                crate::desktop_log::info(format_args!(
                    "DESKTOP_INTERRUPT_SEND_START kind=cancel id={}",
                    request_id
                ));
                write_json_line(
                    writer,
                    json!({
                        "type": "cancel",
                        "id": request_id,
                    }),
                )?;
                crate::desktop_log::info(format_args!(
                    "DESKTOP_INTERRUPT_SEND_OK kind=cancel id={} write_ms={}",
                    request_id,
                    write_start.elapsed().as_millis()
                ));
                *next_request_id += 1;
                cancel_request_ids.push(request_id);
            }
            DesktopSessionCommand::StdinResponse { request_id, input } => {
                send_desktop_event_ref(
                    event_tx,
                    DesktopSessionEvent::Status(DesktopSessionStatus::SendingInteractiveInput),
                );
                write_json_line(
                    writer,
                    json!({
                        "type": "stdin_response",
                        "id": *next_request_id,
                        "request_id": request_id,
                        "input": input,
                    }),
                )?;
                *next_request_id += 1;
            }
        }
    }
    Ok(cancel_request_ids)
}
