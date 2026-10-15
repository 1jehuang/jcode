use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::process::{Command, Stdio};

use tokio::time::{Duration, timeout};

const CLIENT_TITLE_PREFIXES: &[&str] = &["jcode:d:", "jcode:c:"];

#[derive(Debug, Clone)]
pub struct DictationRun {
    pub text: String,
    pub mode: jcode_base::protocol::TranscriptMode,
}

pub async fn run_configured() -> Result<DictationRun> {
    let cfg = jcode_base::config::config().dictation.clone();
    let command = cfg.command.trim();
    if command.is_empty() {
        anyhow::bail!(
            "Dictation is not configured. Set `[dictation].command` in `~/.jcode/config.toml`."
        );
    }

    let text = run_command(command, cfg.timeout_secs).await?;
    Ok(DictationRun {
        text,
        mode: cfg.mode,
    })
}

pub async fn run_command(command: &str, timeout_secs: u64) -> Result<String> {
    let mut child = shell_command(command);
    child.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = child
        .spawn()
        .with_context(|| format!("failed to start `{}`", command))?;

    let output = if timeout_secs == 0 {
        child
            .wait_with_output()
            .await
            .context("failed to wait for dictation command")?
    } else {
        timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
            .with_context(|| format!("dictation command timed out after {}s", timeout_secs))?
            .context("failed to wait for dictation command")?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            anyhow::bail!("dictation command exited with {}", output.status);
        }
        anyhow::bail!(stderr);
    }

    let transcript = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .trim()
        .to_string();
    if transcript.is_empty() {
        anyhow::bail!("dictation command returned an empty transcript");
    }

    Ok(transcript)
}

pub fn type_text(text: &str) -> Result<()> {
    let status = Command::new("wtype")
        .arg("--")
        .arg(text)
        .status()
        .context("failed to launch `wtype`")?;
    if !status.success() {
        anyhow::bail!("`wtype` exited with {}", status);
    }
    Ok(())
}

pub fn focused_jcode_session() -> Result<Option<String>> {
    let Some(window) = focused_window_niri()? else {
        return Ok(None);
    };
    Ok(resolve_session_for_window(&window))
}

/// Session of the Jcode CLI (TUI) client in the focused window, if the focused
/// window is a terminal running one. Jcode Desktop windows never match: their
/// own chats handle voice in-app.
pub fn focused_cli_session() -> Result<Option<String>> {
    let Some(window) = focused_window_niri()? else {
        return Ok(None);
    };
    if window_is_desktop(&window) {
        return Ok(None);
    }
    Ok(resolve_session_for_window(&window))
}

fn window_is_desktop(window: &NiriFocusedWindow) -> bool {
    window
        ._app_id
        .as_deref()
        .is_some_and(|app_id| app_id.contains("jcode-desktop"))
        || window.pid == std::process::id()
        || process_exe_name(window.pid).is_some_and(|name| name.starts_with("jcode-desktop"))
}

fn process_read<T>(result: std::io::Result<T>, pid: u32, field: &str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        // Processes may exit between enumeration and inspection.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            jcode_base::logging::debug(&format!("Cannot inspect process {pid} {field}: {error}"));
            None
        }
    }
}

fn process_exe_name(pid: u32) -> Option<String> {
    let exe = process_read(
        std::fs::read_link(format!("/proc/{pid}/exe")),
        pid,
        "executable",
    )?;
    let name = exe.file_name()?.to_string_lossy();
    Some(name.trim_end_matches(" (deleted)").to_string())
}

/// Deliver `text` to a live TUI session exactly like `jcode transcript`, over
/// the shared server's debug socket. Blocking, bounded by `timeout`.
#[cfg(unix)]
pub fn send_transcript_blocking(
    text: &str,
    mode: jcode_base::protocol::TranscriptMode,
    session_id: Option<String>,
    timeout: std::time::Duration,
) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    let path = debug_socket_path();
    let mut stream = std::os::unix::net::UnixStream::connect(&path)
        .with_context(|| format!("failed to connect to {}", path.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = jcode_base::protocol::Request::Transcript {
        id: 1,
        text: text.to_string(),
        mode,
        session_id,
    };
    stream.write_all((serde_json::to_string(&request)? + "\n").as_bytes())?;
    let mut reader = BufReader::new(stream);
    let deadline = std::time::Instant::now() + timeout;
    let mut line = String::new();
    while std::time::Instant::now() < deadline {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            anyhow::bail!("jcode server closed the connection");
        }
        match serde_json::from_str::<jcode_base::protocol::ServerEvent>(line.trim()) {
            Ok(jcode_base::protocol::ServerEvent::Done { id: 1 }) => return Ok(()),
            Ok(jcode_base::protocol::ServerEvent::Error { id: 1, message, .. }) => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
    anyhow::bail!("timed out waiting for the jcode server")
}

/// Mirrors the server's socket naming (`JCODE_SOCKET`, else the runtime dir).
#[cfg(unix)]
fn debug_socket_path() -> std::path::PathBuf {
    let main = std::env::var_os("JCODE_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| jcode_base::storage::runtime_dir().join("jcode.sock"));
    let name = main
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("jcode.sock")
        .replace(".sock", "-debug.sock");
    main.with_file_name(name)
}

/// A running `jcode` TUI client, not a server or one-shot subcommand.
fn is_jcode_client_process(pid: u32) -> bool {
    if process_exe_name(pid).as_deref() != Some("jcode") {
        return false;
    }
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let args: Vec<String> = bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .skip(1)
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    // Flags only, or `--resume <id>`. Any subcommand is not an interactive client.
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            return false;
        }
        if matches!(
            arg.as_str(),
            "--resume" | "-r" | "--provider" | "--model" | "-m"
        ) {
            iter.next();
        }
    }
    true
}

#[derive(Debug, Deserialize)]
struct NiriFocusedWindow {
    pid: u32,
    title: Option<String>,
    #[serde(rename = "app_id")]
    _app_id: Option<String>,
}

fn focused_window_niri() -> Result<Option<NiriFocusedWindow>> {
    let output = Command::new("niri")
        .args(["msg", "-j", "focused-window"])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(None);
    }

    let window: NiriFocusedWindow =
        serde_json::from_str(trimmed).context("failed to parse `niri msg -j focused-window`")?;
    Ok(Some(window))
}

fn resolve_session_for_window(window: &NiriFocusedWindow) -> Option<String> {
    let children = match proc_children_map() {
        Ok(children) => Some(children),
        Err(error) => {
            jcode_base::logging::warn(&format!(
                "Cannot inspect foreground process tree, falling back to window title: {error:#}"
            ));
            None
        }
    };
    let mut tree = Vec::new();
    if let Some(children) = children.as_ref() {
        let mut queue = VecDeque::from([window.pid]);
        while let Some(pid) = queue.pop_front() {
            tree.push(pid);
            if let Some(next) = children.get(&pid) {
                queue.extend(next.iter().copied());
            }
        }
    }

    // Exact: a client in this window registered the session it shows.
    let registered: Vec<String> = tree
        .iter()
        .filter_map(|&pid| jcode_base::client_focus::registered_client_session(pid))
        .collect();
    if registered.len() == 1 {
        return registered.into_iter().next();
    }

    if let Some(title) = window.title.as_deref()
        && let Some(session_id) = resolve_session_from_window_title(title)
    {
        return Some(session_id);
    }

    let candidates: Vec<ClientCandidate> = tree
        .iter()
        .filter_map(|&pid| inspect_client_process(pid))
        .collect();
    if !candidates.is_empty() {
        let selected = select_candidate(&candidates, window.title.as_deref())?;
        return resolve_candidate_session_id(&selected);
    }

    // A generic `jcode` client (no session in its title or argv) is running in
    // this window. The TUI records its session on focus, so the last-focused
    // session belongs to it.
    if tree.iter().any(|&pid| is_jcode_client_process(pid)) {
        return match jcode_base::client_focus::last_focused_session() {
            Ok(session) => session,
            Err(error) => {
                jcode_base::logging::warn(&format!(
                    "Cannot read last-focused CLI session: {error:#}"
                ));
                None
            }
        };
    }
    None
}

fn resolve_session_from_window_title(title: &str) -> Option<String> {
    let short_name = extract_session_short_name_from_window_title(title)?;
    let mut matching: Vec<String> = jcode_base::storage::active_session_ids()
        .into_iter()
        .filter(|session_id| {
            jcode_base::id::extract_session_name(session_id)
                .map(|name| name.eq_ignore_ascii_case(&short_name))
                .unwrap_or(false)
        })
        .collect();
    matching.sort();
    matching.pop()
}

fn extract_session_short_name_from_window_title(title: &str) -> Option<String> {
    let (_, rest) = title
        .split_once("jcode/")
        .or_else(|| title.split_once("jcode "))?;
    // The TUI appends live metrics after " · " (for example "· last ~18s").
    let rest = rest.split(" · ").next().unwrap_or(rest);
    let candidate = rest.split('[').next().unwrap_or(rest).trim();
    let token = candidate.split_whitespace().next_back()?;
    normalize_session_short_name(token)
}

fn normalize_session_short_name(token: &str) -> Option<String> {
    let normalized = token
        .trim()
        .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientCandidate {
    pid: u32,
    short_name: String,
    session_id: Option<String>,
}

fn inspect_client_process(pid: u32) -> Option<ClientCandidate> {
    if let Some(session_id) = read_resumed_session_id(pid) {
        let short_name = jcode_base::id::extract_session_name(&session_id)
            .unwrap_or(session_id.as_str())
            .to_string();
        return Some(ClientCandidate {
            pid,
            short_name,
            session_id: Some(session_id),
        });
    }

    let comm = process_read(
        std::fs::read_to_string(format!("/proc/{pid}/comm")),
        pid,
        "command name",
    )?;
    let comm = comm.trim();
    let short_name = CLIENT_TITLE_PREFIXES
        .iter()
        .find_map(|prefix| comm.strip_prefix(prefix))?
        .trim()
        .to_string();
    if short_name.is_empty() {
        return None;
    }

    Some(ClientCandidate {
        pid,
        short_name,
        session_id: read_resumed_session_id(pid),
    })
}

fn read_resumed_session_id(pid: u32) -> Option<String> {
    let bytes = process_read(
        std::fs::read(format!("/proc/{pid}/cmdline")),
        pid,
        "command arguments",
    )?;
    let args: Vec<String> = bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    for pair in args.windows(2) {
        if pair[0] == "--resume" && pair[1].starts_with("session_") {
            return Some(pair[1].clone());
        }
    }
    None
}

fn select_candidate(
    candidates: &[ClientCandidate],
    title: Option<&str>,
) -> Option<ClientCandidate> {
    if candidates.len() == 1 {
        return candidates.first().cloned();
    }

    let title = title?.to_ascii_lowercase();
    candidates
        .iter()
        .find(|candidate| title.contains(&candidate.short_name.to_ascii_lowercase()))
        .cloned()
        .or_else(|| candidates.first().cloned())
}

fn resolve_candidate_session_id(candidate: &ClientCandidate) -> Option<String> {
    if let Some(session_id) = &candidate.session_id {
        return Some(session_id.clone());
    }

    let mut matching: Vec<String> = jcode_base::storage::active_session_ids()
        .into_iter()
        .filter(|session_id| {
            jcode_base::id::extract_session_name(session_id)
                .map(|name| name.eq_ignore_ascii_case(&candidate.short_name))
                .unwrap_or(false)
        })
        .collect();

    matching.sort();
    matching.pop()
}

fn proc_children_map() -> Result<HashMap<u32, Vec<u32>>> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let proc_dir = std::fs::read_dir("/proc").context("failed to read /proc")?;

    for entry in proc_dir {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name
            .to_str()
            .filter(|name| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()))
        else {
            continue;
        };
        let pid = match name.parse::<u32>() {
            Ok(pid) => pid,
            Err(error) => {
                jcode_base::logging::debug(&format!(
                    "Ignoring out-of-range process directory: {error}"
                ));
                continue;
            }
        };

        let status_path = entry.path().join("status");
        let Ok(status) = std::fs::read_to_string(status_path) else {
            continue;
        };
        let Some(ppid) = parse_ppid(&status) else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
    }

    Ok(children)
}

fn parse_ppid(status: &str) -> Option<u32> {
    status.lines().find_map(|line| {
        let value = line.strip_prefix("PPid:")?;
        match value.trim().parse::<u32>() {
            Ok(ppid) => Some(ppid),
            Err(error) => {
                jcode_base::logging::debug(&format!("Cannot parse process parent PID: {error}"));
                None
            }
        }
    })
}

fn shell_command(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }

    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-lc").arg(command);
        cmd
    }
}

#[cfg(test)]
#[path = "dictation_tests.rs"]
mod dictation_tests;
