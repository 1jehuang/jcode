//! Launching jcode sessions in new terminal windows.
//!
//! These helpers spawn a fresh `jcode` process (resume or self-dev) inside a
//! new terminal window. They are pure process/terminal orchestration built on
//! the low-level `terminal_launch` facade and depend only on core modules
//! (`id`, `process_title`, `registry`, `server::socket_path`, `platform`), so
//! they live in the core layer rather than the CLI command layer. This lets
//! lower layers like `server`, `restart_snapshot`, and `tool` relaunch
//! sessions without depending on `cli`.

use anyhow::Result;

use crate::{id, server};

/// Compute the window/terminal title used when (re)launching a session.
pub fn resumed_window_title(session_id: &str) -> String {
    let session_name = crate::process_title::session_name(session_id);
    let icon = id::session_icon(&session_name);
    let session_label = crate::process_title::terminal_session_label_for_id(session_id);
    if let Some(server_info) = crate::registry::find_server_by_socket_sync(&server::socket_path()) {
        format!("{} jcode/{} {}", icon, server_info.name, session_label)
    } else {
        format!("{} jcode {}", icon, session_label)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn focus_title_best_effort(title: &str) {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(
            "sleep 0.4; \
             if command -v wmctrl >/dev/null 2>&1; then wmctrl -a \"$JCODE_WINDOW_TITLE\" >/dev/null 2>&1 && exit 0; fi; \
             if command -v xdotool >/dev/null 2>&1; then xdotool search --name \"$JCODE_WINDOW_TITLE\" windowactivate >/dev/null 2>&1 && exit 0; fi; \
             exit 0",
        )
        .env("JCODE_WINDOW_TITLE", title)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let _ = crate::platform::spawn_detached(&mut cmd);
}

#[cfg(any(not(unix), target_os = "macos"))]
fn focus_title_best_effort(_title: &str) {}

#[cfg(unix)]
pub fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_resume_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(unix)]
pub fn spawn_resume_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    let title = resumed_window_title(session_id);
    let mut args = vec!["--fresh-spawn".to_string()];
    if let Some(provider_key) = provider_key.filter(|value| !value.trim().is_empty()) {
        args.push("--provider".to_string());
        args.push(provider_key.to_string());
    }
    args.extend(["--resume".to_string(), session_id.to_string()]);
    let command = crate::terminal_launch::TerminalCommand::new(exe, args)
        .title(title)
        .fresh_spawn();
    crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)
}

#[cfg(unix)]
pub fn spawn_selfdev_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_selfdev_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(unix)]
pub fn spawn_selfdev_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    let selfdev_title = format!("{} [self-dev]", resumed_window_title(session_id));
    let mut args = vec!["--fresh-spawn".to_string()];
    if let Some(provider_key) = provider_key.filter(|value| !value.trim().is_empty()) {
        args.push("--provider".to_string());
        args.push(provider_key.to_string());
    }
    args.extend([
        "--resume".to_string(),
        session_id.to_string(),
        "self-dev".to_string(),
    ]);
    let command = crate::terminal_launch::TerminalCommand::new(exe, args)
        .title(selfdev_title.clone())
        .fresh_spawn();
    let spawned = crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)?;
    if spawned {
        focus_title_best_effort(&selfdev_title);
    }
    Ok(spawned)
}

#[cfg(not(unix))]
fn find_wezterm_gui_binary() -> Option<String> {
    use std::process::{Command, Stdio};

    if let Ok(exe) = std::env::var("WEZTERM_EXECUTABLE") {
        let p = std::path::Path::new(&exe);
        let gui = p.with_file_name("wezterm-gui.exe");
        if gui.exists() {
            return Some(gui.to_string_lossy().into_owned());
        }
        return Some(exe);
    }

    let candidates = [
        r"C:\Program Files\WezTerm\wezterm-gui.exe",
        r"C:\Program Files (x86)\WezTerm\wezterm-gui.exe",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }

    for bin in &["wezterm-gui", "wezterm"] {
        if let Ok(output) = Command::new("where")
            .arg(bin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.lines().next() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        if *bin == "wezterm" {
                            let p = std::path::Path::new(trimmed);
                            let gui = p.with_file_name("wezterm-gui.exe");
                            if gui.exists() {
                                return Some(gui.to_string_lossy().into_owned());
                            }
                        }
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(not(unix))]
fn resume_terminal_candidates_windows() -> Vec<String> {
    std::env::var("JCODE_RESUME_TERMINAL")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|candidates| !candidates.is_empty())
        .unwrap_or_else(|| {
            vec![
                "wezterm".to_string(),
                "wt".to_string(),
                "alacritty".to_string(),
            ]
        })
}

#[cfg(not(unix))]
pub fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_resume_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(not(unix))]
pub fn spawn_resume_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let mut jcode_args: Vec<String> = Vec::new();
    if let Some(provider_key) = provider_key.filter(|value| !value.trim().is_empty()) {
        jcode_args.push("--provider".to_string());
        jcode_args.push(provider_key.to_string());
    }
    jcode_args.push("--resume".to_string());
    jcode_args.push(session_id.to_string());

    let wezterm_gui = find_wezterm_gui_binary();
    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    for term in resume_terminal_candidates_windows() {
        let status = match term.as_str() {
            "wezterm" => {
                let Some(ref wezterm_bin) = wezterm_gui else {
                    continue;
                };
                let mut cmd = Command::new(wezterm_bin);
                cmd.args(["start", "--always-new-process", "--"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "wt" | "windows-terminal" => {
                if !wt_available {
                    continue;
                }
                let mut cmd = Command::new("wt.exe");
                cmd.args(["-p", "Command Prompt"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "alacritty" => {
                if !alacritty_available {
                    continue;
                }
                let mut cmd = Command::new("alacritty");
                cmd.args(["-e"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            _ => continue,
        };

        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
pub fn spawn_selfdev_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_selfdev_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(not(unix))]
pub fn spawn_selfdev_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let mut jcode_args: Vec<String> = Vec::new();
    if let Some(provider_key) = provider_key.filter(|value| !value.trim().is_empty()) {
        jcode_args.push("--provider".to_string());
        jcode_args.push(provider_key.to_string());
    }
    jcode_args.extend([
        "--resume".to_string(),
        session_id.to_string(),
        "self-dev".to_string(),
    ]);

    let wezterm_gui = find_wezterm_gui_binary();
    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    for term in resume_terminal_candidates_windows() {
        let status = match term.as_str() {
            "wezterm" => {
                let Some(ref wezterm_bin) = wezterm_gui else {
                    continue;
                };
                let mut cmd = Command::new(wezterm_bin);
                cmd.args(["start", "--always-new-process", "--"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "wt" | "windows-terminal" => {
                if !wt_available {
                    continue;
                }
                let mut cmd = Command::new("wt.exe");
                cmd.args(["-p", "Command Prompt"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "alacritty" => {
                if !alacritty_available {
                    continue;
                }
                let mut cmd = Command::new("alacritty");
                cmd.args(["-e"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            _ => continue,
        };

        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}
