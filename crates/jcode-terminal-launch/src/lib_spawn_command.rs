use super::*;

pub(super) fn build_spawn_command(
    term: &str,
    command: &TerminalCommand,
    cwd: &Path,
) -> Option<Command> {
    let title = command.title.as_deref().unwrap_or("jcode");
    let mut cmd = Command::new(term);
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if command.fresh_spawn {
        cmd.env("JCODE_FRESH_SPAWN", "1");
    }

    match term {
        #[cfg(unix)]
        "herdr" => {
            // `pane split` deliberately creates a shell and returns its pane id;
            // `pane run` is the atomic, bracketed-paste-aware way to start a
            // command in that shell. Keep the small composition here rather
            // than requiring every user to configure an equivalent spawn hook.
            let herdr = terminal_env_value(&command.client_terminal_env, "HERDR_BIN_PATH")
                .unwrap_or_else(|| "herdr".to_string());
            // Unlike tmux/zellij, herdr runs the command inside an interactive
            // shell, which would outlive jcode and leave an empty pane behind
            // (e.g. after `swarm stop`). Exit that shell when jcode exits
            // cleanly so the pane closes, but keep it open after a failure so
            // the error stays readable.
            // The server may start a clean shell that does not inherit this
            // process's env, so serialize runtime allowlist values only when a
            // runtime context was explicitly supplied.
            let command_line = format!("{} && exit", shell_command(&command_parts(command)));
            let script = concat!(
                "set -eu; ",
                "response=\"$(\"$1\" pane split --current --direction right --cwd \"$2\" --focus)\"; ",
                "pane_id=\"$(printf '%s\\n' \"$response\" | sed -n '",
                "s/.*\\\"pane_id\\\"[[:space:]]*:[[:space:]]*\\\"\\([^\\\"]*\\)\\\".*/\\1/p' | head -n 1)\"; ",
                "test -n \"$pane_id\"; ",
                "exec \"$1\" pane run \"$pane_id\" \"$3\""
            );
            cmd = Command::new("sh");
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args(["-c", script, "jcode-herdr-spawn", &herdr])
                .arg(cwd)
                .arg(command_line);
            if command.fresh_spawn {
                cmd.env("JCODE_FRESH_SPAWN", "1");
            }
        }
        #[cfg(unix)]
        "tmux" => {
            cmd.args(["split-window", "-h"]);
            if let Some(pane) = terminal_env_value(&command.client_terminal_env, "TMUX_PANE") {
                cmd.args(["-t", &pane]);
            }
            cmd.arg("-c").arg(cwd).args(command_parts(command));
        }
        #[cfg(unix)]
        "zellij" => {
            cmd.args(["action", "new-pane", "--direction", "right", "--cwd"])
                .arg(cwd)
                .arg("--")
                .args(command_parts(command));
        }
        #[cfg(unix)]
        "screen" => {
            let inner = format!(
                "cd {} && exec {}",
                sh_escape(&cwd.to_string_lossy()),
                shell_command(&command_parts(command))
            );
            if let Some(sty) = terminal_env_value(&command.client_terminal_env, "STY") {
                cmd.args(["-S", &sty]);
            }
            cmd.args(["-X", "screen", "sh", "-lc"]).arg(inner);
        }
        #[cfg(unix)]
        "handterm" => {
            let shell = shell_command(&command_parts(command));
            cmd.args(["--backend", "gpu", "--exec", &shell]);
        }
        #[cfg(target_os = "macos")]
        "ghostty" => {
            // Ghostty 1.3+ exposes a native AppleScript API. Use it instead of
            // `open -na`, which always creates a separate app instance/window
            // and therefore made `/fork` ignore Ghostty's tabbed workflow.
            // Run AppleScript synchronously inside a detached helper so errors
            // (Ghostty <1.3, disabled AppleScript, denied Automation access)
            // can fall back to the older, universally supported new-window
            // launch. Spawning `osascript` directly reports success before the
            // script runs and would silently lose the fork on those versions.
            cmd = Command::new("sh");
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args([
                    "-c",
                    macos_ghostty_spawn_wrapper(),
                    "jcode-ghostty-spawn",
                    &macos_ghostty_applescript(command, cwd),
                    &macos_terminal_inner_script(command, cwd),
                ]);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        "ghostty" => {
            cmd.arg(format!("--working-directory={}", cwd.to_string_lossy()))
                .arg("-e")
                .args(command_parts(command));
        }
        "kitty" => {
            cmd.args(["--title", title, "-e"])
                .args(command_parts(command));
        }
        "wezterm" => {
            cmd.args(["start", "--always-new-process", "--"])
                .args(command_parts(command));
        }
        "alacritty" => {
            cmd.args(["--title", title, "-e"])
                .args(command_parts(command));
        }
        "gnome-terminal" => {
            cmd.arg("--title").arg(title);
            cmd.arg("--").args(command_parts(command));
        }
        "konsole" | "xterm" | "foot" => {
            cmd.args(["-e"]).args(command_parts(command));
        }
        #[cfg(target_os = "macos")]
        "iterm2" => {
            let shell = shell_command(&command_parts(command));
            cmd = Command::new("osascript");
            cmd.args([
                "-e",
                &format!(
                    r#"tell application "iTerm2"
                        create window with default profile command "{}"
                    end tell"#,
                    shell.replace('"', "\\\"")
                ),
            ]);
        }
        #[cfg(target_os = "macos")]
        "terminal" => {
            // `open -a Terminal <binary> --args ...` does NOT execute the binary with
            // arguments; it asks Terminal to open the file as a document. On a default
            // macOS install (where Apple Terminal is the only available terminal), that
            // means split/resume spawns silently fail to launch jcode. Use AppleScript's
            // `do script` so the command actually runs in a new Terminal window.
            cmd = Command::new("osascript");
            cmd.args(["-e", &macos_terminal_applescript(command, cwd)]);
        }
        #[cfg(any(not(unix), test))]
        "wt" => {
            cmd.args(["new-tab", "--title", title]);
            cmd.arg(&command.program).args(&command.args);
            apply_runtime_env(&mut cmd, &command.runtime_env);
        }
        #[cfg(any(not(unix), test))]
        "cmd" => {
            cmd.args(["/C", "start", title, "cmd.exe", "/K"]);
            cmd.arg(windows_command_line(&command_parts(command)));
        }
        _ => return None,
    }

    // Export spawn metadata to the terminal process so programs running
    // inside (shells, multiplexers) can also see why the window was opened.
    // Note: terminals launched indirectly (macOS `open`/`osascript` paths) do
    // not inherit this env, matching the existing JCODE_FRESH_SPAWN caveat.
    for (key, value) in spawn_metadata_env(command, cwd) {
        cmd.env(key, value);
    }
    apply_runtime_env(&mut cmd, &command.runtime_env);

    Some(cmd)
}
