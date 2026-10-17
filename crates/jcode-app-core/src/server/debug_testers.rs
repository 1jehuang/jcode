use anyhow::Result;
use std::path::PathBuf;

/// Execute tester commands
pub(super) async fn execute_tester_command(command: &str) -> Result<String> {
    let trimmed = command.trim();

    if trimmed == "list" {
        let testers = load_testers()?;
        if testers.is_empty() {
            return Ok("No active testers.".to_string());
        }
        return Ok(serde_json::to_string_pretty(&testers)?);
    }

    if trimmed == "spawn" || trimmed.starts_with("spawn ") {
        let opts: serde_json::Value = if trimmed == "spawn" {
            serde_json::json!({})
        } else {
            serde_json::from_str(trimmed.strip_prefix("spawn ").unwrap_or("{}"))?
        };
        return spawn_tester(opts).await;
    }

    let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
    if parts.len() >= 2 {
        let tester_id = parts[0];
        let cmd = parts[1];
        let arg = parts.get(2).copied();
        return execute_tester_subcommand(tester_id, cmd, arg).await;
    }

    Err(anyhow::anyhow!(
        "Unknown tester command: {}. Use tester:help for usage.",
        trimmed
    ))
}

fn load_testers() -> Result<Vec<serde_json::Value>> {
    let path = crate::storage::jcode_dir()?.join("testers.json");
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        if content.trim().is_empty() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(vec![])
    }
}

fn save_testers(testers: &[serde_json::Value]) -> Result<()> {
    let path = crate::storage::jcode_dir()?.join("testers.json");
    std::fs::write(&path, serde_json::to_string_pretty(testers)?)?;
    Ok(())
}

async fn spawn_tester(opts: serde_json::Value) -> Result<String> {
    use std::process::Stdio;

    let id = format!("tester_{}", crate::id::new_id("tui"));
    let cwd = opts.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
    let binary = opts.get("binary").and_then(|v| v.as_str());
    let cols = opts.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
    let rows = opts.get("rows").and_then(|v| v.as_u64()).unwrap_or(40) as u16;

    let binary_path = if let Some(b) = binary {
        PathBuf::from(b)
    } else if let Ok(current) = crate::build::current_binary_path() {
        if current.exists() {
            current
        } else if let Ok(canary) = crate::build::canary_binary_path() {
            if canary.exists() {
                canary
            } else {
                std::env::current_exe()?
            }
        } else {
            std::env::current_exe()?
        }
    } else if let Ok(canary) = crate::build::canary_binary_path() {
        if canary.exists() {
            canary
        } else {
            std::env::current_exe()?
        }
    } else {
        std::env::current_exe()?
    };

    if !binary_path.exists() {
        return Err(anyhow::anyhow!(
            "Binary not found: {}",
            binary_path.display()
        ));
    }

    let debug_cmd = std::env::temp_dir().join(format!("jcode_debug_cmd_{}", id));
    let debug_resp = std::env::temp_dir().join(format!("jcode_debug_response_{}", id));
    let stdout_path = std::env::temp_dir().join(format!("jcode_tester_stdout_{}", id));
    let stderr_path = std::env::temp_dir().join(format!("jcode_tester_stderr_{}", id));

    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let _ = crate::platform::set_permissions_owner_only(&stdout_path);
    let _ = crate::platform::set_permissions_owner_only(&stderr_path);
    let _ = std::fs::File::create(&debug_cmd)
        .and_then(|_| crate::platform::set_permissions_owner_only(&debug_cmd));
    let _ = std::fs::File::create(&debug_resp)
        .and_then(|_| crate::platform::set_permissions_owner_only(&debug_resp));

    let mut cmd = tokio::process::Command::new(&binary_path);
    cmd.current_dir(cwd);
    cmd.env(jcode_selfdev_types::CLIENT_SELFDEV_ENV, "1");
    cmd.env(
        "KCODE_DEBUG_CMD_PATH",
        debug_cmd.to_string_lossy().to_string(),
    );
    cmd.env(
        "KCODE_DEBUG_RESPONSE_PATH",
        debug_resp.to_string_lossy().to_string(),
    );
    cmd.arg("--debug-socket");

    // The TUI refuses to start unless stdin/stdout are a TTY, so a headless
    // tester must own a real PTY. Allocate one, hand the slave end to the
    // child, and drain the master into the stdout log so the child never
    // blocks on a full PTY buffer.
    #[cfg(unix)]
    {
        let pty = allocate_pty(cols, rows)
            .map_err(|e| anyhow::anyhow!("Failed to allocate tester PTY: {}", e))?;
        use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

        let slave: OwnedFd = pty.slave;
        let master: OwnedFd = pty.master;

        let stdin_slave = slave.try_clone()?;
        let stdout_slave = slave.try_clone()?;
        cmd.stdin(Stdio::from(stdin_slave));
        cmd.stdout(Stdio::from(stdout_slave));
        cmd.stderr(Stdio::from(stderr_file));
        drop(slave);

        // Drain the master side so TUI output (including image escapes) does
        // not back up. Log it for debugging; the thread ends when the child
        // exits and the slave side closes.
        let mut stdout_log = stdout_file;
        let master_fd = master.into_raw_fd();
        std::thread::Builder::new()
            .name(format!("tester-pty-drain-{}", id))
            .spawn(move || {
                use std::io::{Read, Write};
                // Safety: we own master_fd; this is the only holder now.
                let mut master_file = unsafe { std::fs::File::from_raw_fd(master_fd) };
                let mut buf = [0u8; 8192];
                loop {
                    match master_file.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = stdout_log.write_all(&buf[..n]);
                        }
                        Err(_) => break,
                    }
                }
            })
            .ok();
    }
    #[cfg(not(unix))]
    {
        let _ = (cols, rows);
        cmd.stdout(Stdio::from(stdout_file));
        cmd.stderr(Stdio::from(stderr_file));
    }

    let child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);

    let info = serde_json::json!({
        "id": id,
        "pid": pid,
        "binary": binary_path.to_string_lossy(),
        "cwd": cwd,
        "debug_cmd_path": debug_cmd.to_string_lossy(),
        "debug_response_path": debug_resp.to_string_lossy(),
        "stdout_path": stdout_path.to_string_lossy(),
        "stderr_path": stderr_path.to_string_lossy(),
        "started_at": chrono::Utc::now().to_rfc3339(),
    });

    let mut testers = load_testers()?;
    testers.push(info);
    save_testers(&testers)?;

    Ok(serde_json::json!({
        "id": id,
        "pid": pid,
        "message": format!("Spawned tester {} (pid {})", id, pid)
    })
    .to_string())
}

/// Master/slave ends of a freshly allocated PTY.
#[cfg(unix)]
struct TesterPty {
    master: std::os::fd::OwnedFd,
    slave: std::os::fd::OwnedFd,
}

/// Allocate a PTY sized `cols` x `rows` for a headless tester. The TUI's
/// terminal init requires stdin/stdout to be a TTY, so testers get the slave
/// end as their stdio while the server drains the master end.
#[cfg(unix)]
#[allow(
    clippy::unnecessary_mut_passed,
    reason = "libc::openpty takes mutable termios/winsize pointers on Apple/BSD targets"
)]
fn allocate_pty(cols: u16, rows: u16) -> std::io::Result<TesterPty> {
    use std::os::fd::{FromRawFd, OwnedFd};

    let mut winsize = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut master_fd: libc::c_int = -1;
    let mut slave_fd: libc::c_int = -1;
    // Safety: openpty writes two valid fds on success; name is unused (null).
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Safety: on success both fds are valid and exclusively owned here.
    Ok(unsafe {
        TesterPty {
            master: OwnedFd::from_raw_fd(master_fd),
            slave: OwnedFd::from_raw_fd(slave_fd),
        }
    })
}

async fn execute_tester_subcommand(
    tester_id: &str,
    cmd: &str,
    arg: Option<&str>,
) -> Result<String> {
    let testers = load_testers()?;
    let tester = testers
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tester_id))
        .ok_or_else(|| anyhow::anyhow!("Tester not found: {}", tester_id))?;

    let debug_cmd_path = tester
        .get("debug_cmd_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid tester config"))?;
    let debug_resp_path = tester
        .get("debug_response_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid tester config"))?;

    let file_cmd = match cmd {
        "frame" => "screen-json".to_string(),
        "frame-normalized" => "screen-json-normalized".to_string(),
        "state" => "state".to_string(),
        "history" => "history".to_string(),
        "wait" => "wait".to_string(),
        "input" => "input".to_string(),
        "message" => format!("message:{}", arg.unwrap_or("")),
        "inject" => format!("inject:{}", arg.unwrap_or("")),
        "keys" => format!("keys:{}", arg.unwrap_or("")),
        "set_input" => format!("set_input:{}", arg.unwrap_or("")),
        "scroll" => format!("scroll:{}", arg.unwrap_or("down")),
        "scroll-test" => match arg {
            Some(raw) => format!("scroll-test:{}", raw),
            None => "scroll-test".to_string(),
        },
        "scroll-suite" => match arg {
            Some(raw) => format!("scroll-suite:{}", raw),
            None => "scroll-suite".to_string(),
        },
        "side-panel-latency" => match arg {
            Some(raw) => format!("side-panel-latency:{}", raw),
            None => "side-panel-latency".to_string(),
        },
        "mermaid-ui-bench" => match arg {
            Some(raw) => format!("mermaid:ui-bench:{}", raw),
            None => "mermaid:ui-bench".to_string(),
        },
        "stop" => {
            if let Some(pid) = tester.get("pid").and_then(|v| v.as_u64()) {
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .output();
            }
            let mut testers = load_testers()?;
            testers.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(tester_id));
            save_testers(&testers)?;
            return Ok("Stopped tester.".to_string());
        }
        _ => return Err(anyhow::anyhow!("Unknown tester command: {}", cmd)),
    };

    std::fs::write(debug_cmd_path, &file_cmd)?;

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Timeout waiting for tester response"));
        }
        if let Ok(response) = std::fs::read_to_string(debug_resp_path)
            && !response.is_empty()
        {
            let _ = std::fs::remove_file(debug_resp_path);
            return Ok(response);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
#[path = "debug_testers_tests.rs"]
mod debug_testers_tests;
