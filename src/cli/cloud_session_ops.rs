//! Cloud session lifecycle operations. Remote arguments always pass through Target's
//! quoting boundary; the detached supervisor is fixed code with positional data.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{session, storage};

#[derive(Debug)]
pub(super) struct WaitTimeout;
impl std::fmt::Display for WaitTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cloud wait timed out (the remote turn is still running)")
    }
}
impl std::error::Error for WaitTimeout {}

#[derive(Debug, Serialize, Deserialize)]
struct Report {
    session_id: String,
    state: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    messages: Vec<serde_json::Value>,
    #[serde(default)]
    log: String,
    #[serde(default)]
    lease_block: Option<String>,
}

pub(super) fn fork(
    reference: Option<&str>,
    prompt: Option<&str>,
    target: &super::Target,
    json: bool,
) -> Result<()> {
    if prompt.is_some_and(|value| value.trim().is_empty()) {
        bail!("fork prompt must not be empty");
    }
    let parent_id = super::resolve_session_id(reference)?;
    let parent = session::Session::load(&parent_id)?;
    if let Some(block) = parent.migration_lease_block() {
        bail!("cannot fork a non-owning local transcript: {block}");
    }
    let mut child = session::Session::create(Some(parent_id.clone()), None);
    child.replace_messages(parent.messages.clone());
    child.compaction = parent.compaction.clone();
    child.system_prompt = parent.system_prompt.clone();
    child.working_dir = parent.working_dir.clone();
    child.model = parent.model.clone();
    child.provider_key = parent.provider_key.clone();
    child.route_api_method = parent.route_api_method.clone();
    child.reasoning_effort = parent.reasoning_effort.clone();
    child.context_cap = parent.context_cap;
    child.status = session::SessionStatus::Closed;
    child.append_fork_notice(&parent_id, parent.display_name());
    child.save()?;
    crate::session_env::inherit_session_env(&parent_id, &child.id);
    let id = child.id.clone();
    // A failure intentionally retains the new child so its transcript is recoverable.
    let moved = super::run_move(Some(&id), target, false, false)
        .with_context(|| format!("fork {id} was saved locally, but could not be moved"))?;
    if let Some(message) = prompt {
        let receipt = target
            .run_remote(
                &moved.host,
                &["cloud", "run-detached", "--session", &id, "--", message],
                None,
            )
            .with_context(|| {
                format!("fork {id} moved successfully, but its initial task did not start")
            })?;
        validate_started_receipt(&receipt, &id)
            .with_context(|| format!("fork {id} moved, but its initial task was not confirmed"))?;
    }
    let result = serde_json::json!({"session_id": id, "parent_id": parent_id, "host": moved.host, "started": prompt.is_some(), "attach_command": moved.attach_command, "return_command": moved.return_command});
    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("Forked {parent_id} into {id} on {}", moved.host);
    }
    Ok(())
}

fn target() -> super::Target {
    super::Target {
        transport: match std::env::var("JCODE_CLOUD_TRANSPORT") {
            Ok(value) => Some(value).filter(|v| !v.trim().is_empty()),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                crate::logging::info("Ignoring non-Unicode JCODE_CLOUD_TRANSPORT");
                None
            }
        },
        ..Default::default()
    }
}

pub(super) fn send(reference: &str, message: &str, json: bool) -> Result<()> {
    if message.trim().is_empty() {
        bail!("cloud message must not be empty");
    }
    let (id, host) = super::away_host(Some(reference))?;
    let out = target().run_remote(
        &host,
        &["cloud", "run-detached", "--session", &id, "--", message],
        None,
    )?;
    validate_started_receipt(&out, &id)?;
    if json {
        std::io::Write::write_all(&mut std::io::stdout(), &out)?;
    } else {
        println!("Started detached turn for {id} on {host}");
    }
    Ok(())
}

fn validate_started_receipt(bytes: &[u8], id: &str) -> Result<()> {
    let receipt: serde_json::Value =
        serde_json::from_slice(bytes).context("invalid cloud detached-start response")?;
    if receipt["session_id"].as_str() != Some(id)
        || receipt["state"].as_str() != Some("running")
        || receipt["pid"].as_u64().is_none_or(|pid| pid == 0)
    {
        bail!("cloud did not confirm a detached turn for the requested session");
    }
    Ok(())
}

fn remote_report(id: &str, host: &str, messages: usize, lines: usize) -> Result<Report> {
    let out = target().run_remote(
        host,
        &[
            "cloud",
            "inspect",
            "--session",
            id,
            "--messages",
            &messages.to_string(),
            "--log-lines",
            &lines.to_string(),
        ],
        None,
    )?;
    let report: Report = serde_json::from_slice(&out).context("invalid cloud inspect response")?;
    if report.session_id != id {
        bail!("cloud returned status for a different session");
    }
    if !matches!(report.state.as_str(), "running" | "idle" | "failed") {
        bail!("cloud returned unknown session state {:?}", report.state);
    }
    Ok(report)
}

fn print_report(report: &Report, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(report)?);
    } else {
        println!("{}: {}", report.session_id, report.state);
        if let Some(block) = &report.lease_block {
            println!("{block}");
        }
        for message in &report.messages {
            println!("{}", serde_json::to_string(message)?);
        }
        if !report.log.is_empty() {
            println!("Recent run log:\n{}", report.log);
        }
    }
    Ok(())
}

pub(super) fn status(reference: &str, messages: usize, json: bool) -> Result<()> {
    let (id, host) = super::away_host(Some(reference))?;
    print_report(&remote_report(&id, &host, messages, 40)?, json)
}

pub(super) fn wait(reference: &str, timeout: u64, messages: usize, json: bool) -> Result<()> {
    let (id, host) = super::away_host(Some(reference))?;
    let started = Instant::now();
    let limit = Duration::from_secs(timeout);
    loop {
        let report = remote_report(&id, &host, messages, 40)?;
        if report.state != "running" {
            print_report(&report, json)?;
            if report.state == "failed" {
                bail!("cloud turn failed for {id}");
            }
            if report.lease_block.is_some() {
                bail!("cloud session no longer owns its transcript");
            }
            return Ok(());
        }
        if started.elapsed() >= limit {
            print_report(&report, json)?;
            return Err(WaitTimeout.into());
        }
        std::thread::sleep(Duration::from_secs(1).min(limit.saturating_sub(started.elapsed())));
    }
}

pub(super) fn logs(reference: &str, lines: usize) -> Result<()> {
    let (id, host) = super::away_host(Some(reference))?;
    let report = remote_report(&id, &host, 0, lines)?;
    std::io::Write::write_all(&mut std::io::stdout(), report.log.as_bytes())?;
    Ok(())
}

fn checked_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.starts_with('.')
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    {
        bail!("invalid cloud session ID");
    }
    Ok(())
}

fn run_directory(id: &str) -> Result<PathBuf> {
    checked_id(id)?;
    Ok(storage::jcode_dir()?.join("cloud_runs").join(id))
}

fn lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

// Bounded I/O even for very large logs. Keep at most the last MiB.
fn tail(path: &Path, lines: usize) -> Result<String> {
    if lines == 0 {
        return Ok(String::new());
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    let len = file.metadata()?.len();
    let offset = len.saturating_sub(1024 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if offset > 0 {
        text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        &text
    };
    let start = text.lines().count().saturating_sub(lines);
    let mut result = text.lines().skip(start).collect::<Vec<_>>().join("\n");
    if !result.is_empty() && text.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

fn read_report(id: &str, directory: &Path, messages: usize, log_lines: usize) -> Result<Report> {
    let sess = session::Session::load(id)?;
    let mut running = storage::streaming_session_ids().iter().any(|s| s == id);
    if directory.join("owner.lock").exists() {
        let file = lock_file(&directory.join("owner.lock"))?;
        match file.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => running = true,
            Err(error) => return Err(error.into()),
        }
    }
    let exit_code = match std::fs::read_to_string(directory.join("exit")) {
        Ok(text) => Some(
            text.trim()
                .parse::<i32>()
                .context("invalid detached exit status")?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let lease_block = sess.migration_lease_block().map(|b| b.to_string());
    let state = if running {
        "running"
    } else if exit_code.is_some_and(|code| code != 0)
        || (exit_code.is_none() && directory.join("started").exists())
    {
        "failed"
    } else {
        "idle"
    };
    let recent = sess
        .messages
        .iter()
        .skip(sess.messages.len().saturating_sub(messages))
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Report {
        session_id: id.to_string(),
        state: state.to_string(),
        exit_code,
        messages: recent,
        log: tail(&directory.join("run.log"), log_lines)?,
        lease_block,
    })
}

pub(super) fn inspect(reference: &str, messages: usize, log_lines: usize) -> Result<()> {
    let id = super::resolve_session_id(Some(reference))?;
    print_report(
        &read_report(&id, &run_directory(&id)?, messages, log_lines)?,
        true,
    )
}

// No input is interpolated into shell source. Descriptor 8 holds the migration
// lock and 9 the run lock. Only the supervisor retains them, not spawned daemons.
#[cfg(unix)]
const SUPERVISOR: &str = r#"
trap '' HUP
umask 077
"$1" --no-update --no-selfdev --resume "$2" run --json -- "$4" 8>&- 9>&-
code=$?
tmp="$3.tmp.$$"
printf '%s\n' "$code" > "$tmp" && mv -f -- "$tmp" "$3"
exit "$code"
"#;

pub(super) fn run_detached(reference: &str, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        bail!("message must not be empty");
    }
    #[cfg(not(unix))]
    {
        bail!(
            "detached cloud turns require a Unix host (session reference length {}, message length {})",
            reference.len(),
            message.len()
        );
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        use std::os::unix::process::CommandExt;
        let id = super::resolve_session_id(Some(reference))?;
        let directory = run_directory(&id)?;
        let migration = super::group::lock_sessions(std::slice::from_ref(&id))?;
        let sess = session::Session::load(&id)?;
        if let Some(block) = sess.migration_lease_block() {
            bail!("{block}");
        }
        // A standalone run must never take over an agent already served by the daemon.
        if storage::active_session_ids().contains(&id)
            || storage::streaming_session_ids().contains(&id)
        {
            bail!(
                "session {id} already has an active agent; detach/close it before sending a cloud turn"
            );
        }
        std::fs::create_dir_all(&directory)?;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        let owner = lock_file(&directory.join("owner.lock"))?;
        owner
            .try_lock()
            .context("a detached turn already owns this session")?;
        let exit_path = directory.join("exit");
        match std::fs::remove_file(&exit_path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        storage::write_json_secret(
            &directory.join("started"),
            &serde_json::json!({"started_at": chrono::Utc::now().to_rfc3339()}),
        )?;
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(directory.join("run.log"))?;
        log.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(SUPERVISOR)
            .arg("jcode-cloud-run")
            .arg(std::env::current_exe()?)
            .arg(&id)
            .arg(&exit_path)
            .arg(message)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        if let Some(cwd) = &sess.working_dir {
            command.current_dir(cwd);
        }
        // Duplicate above the reserved destinations first to avoid descriptor overlap.
        let duplicate = |file: &File| -> Result<File> {
            use std::os::fd::FromRawFd;
            let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        };
        let migration_fd = duplicate(&migration[0])?;
        let owner_fd = duplicate(&owner)?;
        let mfd = migration_fd.as_raw_fd();
        let ofd = owner_fd.as_raw_fd();
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 || libc::dup2(mfd, 8) == -1 || libc::dup2(ofd, 9) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().context("launch detached cloud turn")?;
        println!(
            "{}",
            serde_json::json!({"session_id": id, "state": "running", "pid": child.id()})
        );
        // Do not wait: the supervisor owns both locks until it records completion.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_detached_start_receipt() {
        assert!(
            validate_started_receipt(br#"{"session_id":"s","state":"running","pid":42}"#, "s")
                .is_ok()
        );
        for bytes in [
            b"not json".as_slice(),
            br#"{}"#,
            br#"{"session_id":"other","state":"running","pid":42}"#,
            br#"{"session_id":"s","state":"idle","pid":42}"#,
            br#"{"session_id":"s","state":"running","pid":0}"#,
        ] {
            assert!(validate_started_receipt(bytes, "s").is_err());
        }
    }

    #[test]
    fn rejects_path_and_shell_ids() {
        for id in [
            "",
            ".",
            "..",
            "../escape",
            "a/b",
            "a b",
            "a;echo",
            "$(id)",
            "a\n",
        ] {
            assert!(checked_id(id).is_err(), "{id:?}");
        }
        assert!(checked_id("session-fox_123.json").is_ok());
    }

    #[test]
    fn log_tail_fixtures() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("run.log");
        assert_eq!(tail(&log, 2).unwrap(), "");
        std::fs::write(&log, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail(&log, 2).unwrap(), "two\nthree\n");
        assert_eq!(tail(&log, 0).unwrap(), "");
        std::fs::write(&log, "one\ntwo").unwrap();
        assert_eq!(tail(&log, 1).unwrap(), "two");
    }

    #[test]
    fn owner_lock_excludes_second_descriptor_and_releases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.lock");
        let first = lock_file(&path).unwrap();
        first.try_lock().unwrap();
        let second = lock_file(&path).unwrap();
        assert!(matches!(
            second.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(first);
        second.try_lock().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn supervisor_preserves_literal_prompt_and_failed_exit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("fake jcode");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s' \"$8\"\nexit 7\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let exit = dir.path().join("exit status");
        let prompt = "$(touch should-not-exist); 'quoted'\nnext line";
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(SUPERVISOR)
            .arg("fixture")
            .arg(&executable)
            .arg("session-fixture")
            .arg(&exit)
            .arg(prompt)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(std::fs::read_to_string(exit).unwrap(), "7\n");
        assert_eq!(String::from_utf8(out.stdout).unwrap(), prompt);
    }

    #[test]
    fn timeout_is_downcastable() {
        let error: anyhow::Error = WaitTimeout.into();
        assert!(error.downcast_ref::<WaitTimeout>().is_some());
    }
}
