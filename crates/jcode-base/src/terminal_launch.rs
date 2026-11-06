use anyhow::{Result, bail};
pub use jcode_terminal_launch::{
    SpawnAttempt, TerminalCommand, apply_client_terminal_env, build_hook_spawn_command,
    detected_resume_terminal, expand_home, parse_hook_command, resume_terminal_candidates,
    sh_escape, shell_command, snapshot_client_terminal_env, spawn_command_in_new_terminal_with,
};
use std::path::Path;

/// The configured external spawn hook, if any.
///
/// Resolution order: `JCODE_SPAWN_HOOK` env (handled by config env overrides;
/// empty value disables) then `[terminal] spawn_hook` in config.toml.
pub fn configured_spawn_hook() -> Option<String> {
    crate::config::config()
        .terminal
        .spawn_hook
        .as_deref()
        .map(str::trim)
        .filter(|hook| !hook.is_empty())
        .map(str::to_string)
}

/// Spawn `command` in a new terminal window/pane.
///
/// When a spawn hook is configured (`[terminal] spawn_hook` / `JCODE_SPAWN_HOOK`),
/// the hook takes over the spawn: jcode runs `<hook> <program> <args...>` with
/// `JCODE_SPAWN_*` metadata env vars so external programs (tmux, kitty remote,
/// herd, window managers) control where and how the session appears. If the
/// hook cannot be started, jcode falls back to its built-in terminal detection.
pub fn spawn_command_in_new_terminal(command: &TerminalCommand, cwd: &Path) -> Result<bool> {
    // The user's desktop session owns terminal windows. A running `jcode
    // companion` spawns there, which is right even when this process is a
    // daemon started from another session. Without one, spawn locally.
    if try_spawn_via_companion(command, cwd) {
        return Ok(true);
    }
    if try_spawn_via_configured_hook(command, cwd) {
        return Ok(true);
    }
    jcode_terminal_launch::spawn_command_in_new_terminal_with(command, cwd, |cmd| {
        crate::platform::spawn_detached(cmd).map(|_| ())
    })
}

/// Ask `jcode companion` to open the terminal. `false` when no companion is
/// running (or it is this process, see `JCODE_COMPANION`), so callers fall back.
fn try_spawn_via_companion(command: &TerminalCommand, cwd: &Path) -> bool {
    // Tests must never reach a real companion on the developer's desktop.
    if std::env::var_os("JCODE_COMPANION").is_some() || crate::auth::running_in_test_harness() {
        return false;
    }
    let request = crate::companion_client::CompanionRequest::SpawnTerminal {
        program: command.program.clone(),
        args: command.args.clone(),
        cwd: cwd.to_path_buf(),
        title: command.title.clone(),
        kind: command.kind.clone(),
        session_id: command.session_id.clone(),
        env: command.extra_env.clone(),
        client_terminal_env: command.client_terminal_env.clone(),
    };
    match crate::companion_client::request(&request, std::time::Duration::from_secs(10)) {
        Ok(_) => true,
        Err(crate::companion_client::CompanionError::Unavailable) => false,
        Err(error) => {
            crate::logging::warn(&format!(
                "jcode companion could not spawn a terminal ({error}); spawning locally"
            ));
            false
        }
    }
}

/// Try launching via the configured spawn hook.
///
/// Returns `true` when the hook process started; `false` when no hook is
/// configured or it failed to start (callers should fall back to the built-in
/// terminal spawning for their platform).
pub fn try_spawn_via_configured_hook(command: &TerminalCommand, cwd: &Path) -> bool {
    let Some(hook) = configured_spawn_hook() else {
        return false;
    };
    match spawn_via_hook(&hook, command, cwd) {
        Ok(()) => true,
        Err(error) => {
            crate::logging::warn(&format!(
                "Spawn hook '{hook}' failed ({error}); falling back to built-in terminal spawn"
            ));
            false
        }
    }
}

fn spawn_via_hook(hook: &str, command: &TerminalCommand, cwd: &Path) -> Result<()> {
    let mut cmd = build_hook_spawn_command(hook, command, cwd)?;
    let mut child = cmd.spawn()?;
    // How long to wait for the hook to either exit or prove it is alive.
    // Long-running hooks are reaped asynchronously after the window closes;
    // the window must tolerate the *caller* being descheduled, or a
    // fast-failing hook gets misread as long-running and its error swallowed.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("hook exited with {status}");
            }
            break;
        }
        if std::time::Instant::now() >= deadline {
            // Long-running hooks may intentionally own their terminal process.
            // Reap them asynchronously while treating survival past the startup
            // window as successful launch admission.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    crate::logging::info(&format!(
        "Spawn hook '{hook}' launched terminal spawn (kind={:?} session={:?})",
        command.kind, command.session_id
    ));
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn spawn_via_hook_runs_hook_with_metadata_env() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().expect("temp dir");
        let record = temp.path().join("record.txt");
        let hook_path = temp.path().join("hook.sh");
        std::fs::write(
            &hook_path,
            format!(
                "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$JCODE_SPAWN_KIND\" \"$JCODE_SPAWN_SESSION_ID\" \"$JCODE_SPAWN_SWARM_ID\" \"$*\" > {}\n",
                sh_escape(&record.to_string_lossy())
            ),
        )
        .expect("write hook");
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod hook");

        let command = TerminalCommand::new(
            "/usr/local/bin/jcode",
            vec!["--resume".to_string(), "ses_hooked".to_string()],
        )
        .kind("swarm-agent")
        .session_id("ses_hooked")
        .spawn_env("JCODE_SPAWN_SWARM_ID", "swarm-7");

        spawn_via_hook(&hook_path.to_string_lossy(), &command, temp.path())
            .expect("hook should spawn");

        // The hook runs detached; poll briefly for its output.
        let mut recorded = String::new();
        for _ in 0..100 {
            if let Ok(data) = std::fs::read_to_string(&record)
                && !data.is_empty()
            {
                recorded = data;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            recorded, "swarm-agent|ses_hooked|swarm-7|/usr/local/bin/jcode --resume ses_hooked",
            "hook should receive metadata env and the jcode command as argv"
        );
    }

    #[test]
    fn spawn_via_hook_reports_immediate_nonzero_exit() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().expect("temp dir");
        let hook_path = temp.path().join("reject-outside-tmux.sh");
        std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").expect("write hook");
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod hook");

        let command = TerminalCommand::new(
            "/usr/local/bin/jcode",
            vec!["--resume".to_string(), "ses_fallback".to_string()],
        )
        .kind("swarm-agent")
        .session_id("ses_fallback");

        let error = spawn_via_hook(&hook_path.to_string_lossy(), &command, temp.path())
            .expect_err("non-zero hook exit must trigger built-in/headless fallback");
        assert!(error.to_string().contains("hook exited with"));
    }
}
