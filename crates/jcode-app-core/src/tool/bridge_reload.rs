//! `reload-bridge`: restart the harness API bridge onto a new binary. Shared by
//! `selfdev` (CLI checkout) and `desktop_selfdev` (Desktop checkout).
//!
//! Agents used to do this by hand from `bash` (`kill` the old bridge, then
//! `setsid nohup` a new one). When the agent was itself connected through that
//! bridge, killing it cut the agent's own connection, the turn was interrupted
//! and the second half never ran. The socket file stayed behind with nothing
//! listening and every Desktop panel was stranded.
//!
//! This action runs as a daemon-owned background task instead, so the caller
//! disconnecting mid-restart cannot stop it. The bridge holds a single-instance
//! lock, so old and new cannot overlap on one socket. The sequence is therefore:
//! preflight the new binary, record how the old bridge was launched, stop it,
//! start the new one, verify the socket accepts, and relaunch the old command
//! line if the new bridge does not come up.

use crate::background::{self, TaskResult};
use crate::build;
use crate::tool::ToolOutput;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

const BRIDGE_START_TIMEOUT: Duration = Duration::from_secs(10);
const BRIDGE_STOP_TIMEOUT: Duration = Duration::from_secs(3);

/// How a running bridge was launched, so it can be relaunched on rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BridgeLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl BridgeLaunch {
    fn display(&self) -> String {
        std::iter::once(self.program.display().to_string())
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Parse a NUL-separated `/proc/<pid>/cmdline`, preferring the resolved exe
/// path over argv[0] (argv[0] may be a bare name resolved through PATH).
pub(crate) fn parse_cmdline(raw: &[u8], exe: Option<PathBuf>) -> Option<BridgeLaunch> {
    let mut parts = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned());
    let argv0 = parts.next()?;
    let exe = exe.map(|path| {
        // A rebuilt binary replaced on disk shows up as "<path> (deleted)".
        let text = path.to_string_lossy();
        PathBuf::from(text.strip_suffix(" (deleted)").unwrap_or(&text))
    });
    Some(BridgeLaunch {
        program: exe.unwrap_or_else(|| PathBuf::from(argv0)),
        args: parts.collect(),
    })
}

/// The CLI self-dev bridge: `jcode api-bridge` from the latest local build.
pub(crate) fn cli_bridge_launch() -> Result<BridgeLaunch> {
    let program = build::current_binary_path()
        .ok()
        .filter(|path| path.exists())
        .or_else(|| std::env::current_exe().ok())
        .context("could not resolve a jcode binary for the bridge")?;
    Ok(BridgeLaunch {
        program,
        args: vec!["api-bridge".into()],
    })
}

/// The Desktop self-dev bridge: the checkout's own standalone bridge build,
/// newest profile first, falling back to the CLI bridge.
pub(crate) fn desktop_bridge_launch(desktop_root: &Path) -> Result<BridgeLaunch> {
    let newest = ["release", "debug"]
        .iter()
        .map(|profile| {
            desktop_root
                .join("target")
                .join(profile)
                .join("jcode-harness-api-bridge")
        })
        .filter_map(|path| {
            let modified = path.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified);
    match newest {
        Some((_, program)) => Ok(BridgeLaunch {
            program,
            args: Vec::new(),
        }),
        None => cli_bridge_launch(),
    }
}

fn socket_accepts(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// PID of the process listening on `socket`, from the peer credentials a
/// connecting client sees.
#[cfg(unix)]
fn listener_pid(socket: &Path) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let stream = std::os::unix::net::UnixStream::connect(socket).ok()?;
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        (rc == 0 && cred.pid > 1).then_some(cred.pid)
    }
    #[cfg(target_os = "macos")]
    {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut len,
            )
        };
        (rc == 0 && pid > 1).then_some(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        None
    }
}

#[cfg(target_os = "linux")]
fn running_launch(pid: i32) -> Option<BridgeLaunch> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    parse_cmdline(&raw, exe)
}

#[cfg(not(target_os = "linux"))]
fn running_launch(_pid: i32) -> Option<BridgeLaunch> {
    None
}

fn process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn stop_process(pid: i32) {
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + BRIDGE_STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
    let deadline = Instant::now() + BRIDGE_STOP_TIMEOUT;
    while Instant::now() < deadline && process_alive(pid) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Start a bridge in its own process group so it outlives daemon reloads, and
/// reap it from a thread so it never lingers as a zombie of the daemon.
///
/// A log that cannot be opened must not block a restart (least of all a
/// rollback), so output is discarded in that case.
fn spawn_bridge(launch: &BridgeLaunch, log: &Path) -> Result<SpawnedBridge> {
    use std::os::unix::process::CommandExt;
    let (stdout, stderr) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|file| Ok((file.try_clone()?, file)))
        .map(|(out, err)| {
            (
                std::process::Stdio::from(out),
                std::process::Stdio::from(err),
            )
        })
        .unwrap_or_else(|_| (std::process::Stdio::null(), std::process::Stdio::null()));
    let mut child = std::process::Command::new(&launch.program)
        .args(&launch.args)
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .with_context(|| format!("spawn {}", launch.display()))?;
    let pid = child.id();
    let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let exited_flag = std::sync::Arc::clone(&exited);
    std::thread::Builder::new()
        .name("jcode-api-bridge-reaper".into())
        .spawn(move || {
            let _ = child.wait();
            exited_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .ok();
    Ok(SpawnedBridge { pid, exited })
}

struct SpawnedBridge {
    pid: u32,
    exited: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SpawnedBridge {
    fn has_exited(&self) -> bool {
        self.exited.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Wait until `socket` accepts, giving up early if `bridge` exits first.
async fn wait_for_socket(socket: &Path, timeout: Duration, bridge: Option<&SpawnedBridge>) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if bridge.is_some_and(SpawnedBridge::has_exited) {
            return socket_accepts(socket);
        }
        if socket_accepts(socket) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn preflight(launch: &BridgeLaunch) -> Result<()> {
    // A binary that ignores `--help` may start serving instead of exiting.
    // Bound the probe and kill it rather than hanging the reload forever.
    let probe = tokio::process::Command::new(&launch.program)
        .args(&launch.args)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(10), probe)
        .await
        .map_err(|_| anyhow::anyhow!("{} --help did not exit within 10s", launch.display()))?
        .with_context(|| format!("run {} --help", launch.display()))?;
    if !output.status.success() {
        bail!(
            "{} --help failed ({}): {}",
            launch.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// The restart itself. Writes a progress log to `output` and returns an error
/// describing the final state if the new bridge could not be brought up.
async fn restart_bridge(
    new_launch: BridgeLaunch,
    socket: PathBuf,
    log: PathBuf,
    output: PathBuf,
) -> Result<TaskResult> {
    let mut out = tokio::fs::File::create(&output).await?;
    macro_rules! say {
        ($($arg:tt)*) => {{
            let line = format!($($arg)*);
            let _ = out.write_all(format!("{line}\n").as_bytes()).await;
            let _ = out.flush().await;
        }};
    }

    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    say!("new bridge: {}", new_launch.display());
    if let Err(error) = preflight(&new_launch).await {
        say!("preflight failed, old bridge left running: {error:#}");
        return Ok(TaskResult::failed(Some(1), format!("{error:#}")));
    }

    let old_pid = listener_pid(&socket);
    let old_launch = old_pid.and_then(running_launch);
    match (old_pid, &old_launch) {
        (Some(pid), Some(launch)) => say!("old bridge: pid {pid}: {}", launch.display()),
        (Some(pid), None) => say!("old bridge: pid {pid} (command line unavailable)"),
        (None, _) => say!("no bridge listening on {}", socket.display()),
    }
    if let Some(pid) = old_pid {
        stop_process(pid).await;
        if process_alive(pid) {
            let message = format!("old bridge pid {pid} did not exit; aborting");
            say!("{message}");
            return Ok(TaskResult::failed(Some(1), message));
        }
    }

    let new_bridge = spawn_bridge(&new_launch, &log);
    let new_pid = new_bridge.as_ref().map(|bridge| bridge.pid).unwrap_or(0);
    if let Ok(bridge) = &new_bridge
        && wait_for_socket(&socket, BRIDGE_START_TIMEOUT, Some(bridge)).await
        && listener_pid(&socket).is_none_or(|pid| pid as u32 == new_pid)
    {
        say!("new bridge pid {new_pid} accepting on {}", socket.display());
        return Ok(TaskResult::completed(Some(0)));
    }
    match &new_bridge {
        Err(error) => say!("could not start new bridge: {error:#}"),
        Ok(bridge) if bridge.has_exited() => {
            say!(
                "new bridge pid {new_pid} exited before accepting; see {}",
                log.display()
            )
        }
        Ok(_) => say!(
            "new bridge pid {new_pid} did not accept within {:?}; see {}",
            BRIDGE_START_TIMEOUT,
            log.display()
        ),
    }
    if let Ok(bridge) = &new_bridge
        && !bridge.has_exited()
    {
        stop_process(new_pid as i32).await;
    }

    let Some(old_launch) = old_launch else {
        let message = "new bridge failed and the old command line is unknown; no bridge running";
        say!("{message}");
        return Ok(TaskResult::failed(Some(1), message.to_string()));
    };
    say!("rolling back: {}", old_launch.display());
    match spawn_bridge(&old_launch, &log) {
        Ok(old) if wait_for_socket(&socket, BRIDGE_START_TIMEOUT, Some(&old)).await => {
            let message = format!(
                "new bridge failed; rolled back to old bridge (pid {})",
                old.pid
            );
            say!("{message}");
            Ok(TaskResult::failed(Some(1), message))
        }
        Ok(_) | Err(_) => {
            let message = "new bridge failed and rollback did not come up; no bridge running";
            say!("{message}");
            Ok(TaskResult::failed(Some(1), message.to_string()))
        }
    }
}

/// Queue the restart as a daemon-owned background task and return at once.
pub(crate) async fn spawn_bridge_reload(session_id: &str, new_launch: BridgeLaunch) -> ToolOutput {
    let program = new_launch.display();
    let info = background::global()
        .spawn_with_notify(
            "reload-bridge",
            Some("reload harness API bridge".to_string()),
            session_id,
            true,
            true,
            move |output| {
                let log = crate::storage::jcode_dir()
                    .map(|dir| dir.join("logs").join("api-bridge.log"))
                    .unwrap_or_else(|_| std::env::temp_dir().join("jcode-api-bridge.log"));
                restart_bridge(
                    new_launch,
                    jcode_harness_api::api_socket_path(),
                    log,
                    output,
                )
            },
        )
        .await;
    ToolOutput::new(format!(
        "Restarting the harness API bridge onto `{program}` as daemon background task {}. \
         It runs independently of this turn, so it completes even if this connection drops. \
         Clients going through the bridge (Desktop, SDK) reconnect automatically. You will be \
         woken with the result. Progress: {}. Do not restart the bridge by hand from bash.",
        info.task_id,
        info.output_file.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_prefers_resolved_exe_and_strips_deleted_suffix() {
        let raw = b"jcode\0api-bridge\0--api-socket\0/tmp/a.sock\0";
        let launch = parse_cmdline(raw, Some(PathBuf::from("/opt/jcode (deleted)"))).unwrap();
        assert_eq!(launch.program, PathBuf::from("/opt/jcode"));
        assert_eq!(launch.args, ["api-bridge", "--api-socket", "/tmp/a.sock"]);
    }

    #[test]
    fn cmdline_falls_back_to_argv0() {
        let launch = parse_cmdline(b"/bin/bridge\0", None).unwrap();
        assert_eq!(launch.program, PathBuf::from("/bin/bridge"));
        assert!(launch.args.is_empty());
        assert!(parse_cmdline(b"", None).is_none());
    }

    #[test]
    fn desktop_launch_picks_newest_profile_build() {
        let root = tempfile::tempdir().unwrap();
        for profile in ["debug", "release"] {
            let dir = root.path().join("target").join(profile);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("jcode-harness-api-bridge"), b"").unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let launch = desktop_bridge_launch(root.path()).unwrap();
        assert!(
            launch
                .program
                .ends_with("target/release/jcode-harness-api-bridge")
        );
        assert!(launch.args.is_empty());
    }

    /// End-to-end against real processes: a fake bridge on a private socket is
    /// replaced by a new one, and a broken new binary rolls back to the old.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn restart_replaces_bridge_and_rolls_back_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("api.sock");
        let script = dir.path().join("bridge.py");
        std::fs::write(
            &script,
            "import socket,sys,os\n\
             p=sys.argv[1]\n\
             if '--help' in sys.argv: sys.exit(0)\n\
             if 'broken' in sys.argv: sys.exit(3)\n\
             try: os.unlink(p)\n\
             except OSError: pass\n\
             s=socket.socket(socket.AF_UNIX); s.bind(p); s.listen(8)\n\
             while True: s.accept()[0].close()\n",
        )
        .unwrap();
        let log = dir.path().join("log");
        let python = |args: &[&str]| BridgeLaunch {
            program: PathBuf::from("python3"),
            args: [script.display().to_string(), socket.display().to_string()]
                .into_iter()
                .chain(args.iter().map(|arg| arg.to_string()))
                .collect(),
        };

        let first = spawn_bridge(&python(&[]), &log).unwrap().pid;
        assert!(wait_for_socket(&socket, BRIDGE_START_TIMEOUT, None).await);
        assert_eq!(listener_pid(&socket), Some(first as i32));

        let result = restart_bridge(
            python(&[]),
            socket.clone(),
            log.clone(),
            dir.path().join("out1"),
        )
        .await
        .unwrap();
        assert_eq!(
            result.status,
            Some(crate::bus::BackgroundTaskStatus::Completed)
        );
        let second = listener_pid(&socket).unwrap();
        assert_ne!(second, first as i32);
        assert!(!process_alive(first as i32));

        // `--help` succeeds, so preflight passes, but the bridge exits at once.
        let result = restart_bridge(
            python(&["broken"]),
            socket.clone(),
            log.clone(),
            dir.path().join("out2"),
        )
        .await
        .unwrap();
        assert_eq!(
            result.status,
            Some(crate::bus::BackgroundTaskStatus::Failed)
        );
        let progress = std::fs::read_to_string(dir.path().join("out2")).unwrap();
        assert!(progress.contains("exited before accepting"), "{progress}");
        assert!(progress.contains("rolled back"), "{progress}");
        let restored = listener_pid(&socket).unwrap();
        assert_ne!(restored, second);
        stop_process(restored).await;
    }
}
