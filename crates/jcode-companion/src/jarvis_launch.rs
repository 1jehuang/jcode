//! Open an always-on Jarvis in a new terminal window (Linux).
//!
//! Used when the Fn push-to-talk key is held and no always-on Jarvis is
//! running. The terminal is started in its own transient systemd user scope
//! when possible, so restarting `jcode-companion.service` (whose cgroup is
//! killed as a whole) never takes the new Jarvis window down with it.

use jcode_base::terminal_launch::TerminalCommand;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The command an Fn launch runs: `jcode --jarvis-always-on <extra...>`.
pub(crate) fn jarvis_command(program: PathBuf, extra: &[String]) -> TerminalCommand {
    let mut args = vec!["--jarvis-always-on".to_string()];
    args.extend(
        extra
            .iter()
            .map(|arg| arg.trim())
            .filter(|arg| !arg.is_empty() && *arg != "--jarvis-always-on")
            .map(str::to_string),
    );
    TerminalCommand::new(program, args)
        .title("Jarvis")
        .kind("jarvis")
}

/// Open the Jarvis window. `Ok(false)` when no terminal emulator was found.
pub(crate) fn launch(extra: &[String]) -> anyhow::Result<bool> {
    let program = jcode_build_support::launcher_binary_path()
        .ok()
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("jcode"));
    let command = jarvis_command(program, extra);
    let cwd = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"));
    // A configured spawn hook owns window placement. The companion never
    // routes to itself here (`JCODE_COMPANION` is set in this process).
    if jcode_base::terminal_launch::configured_spawn_hook().is_some() {
        return jcode_base::terminal_launch::spawn_command_in_new_terminal(&command, &cwd);
    }
    jcode_base::terminal_launch::spawn_command_in_new_terminal_with(&command, &cwd, |cmd| {
        // `systemd-run` itself always starts, so a missing terminal must be
        // caught here for the next candidate to be tried.
        if !cmd_exists(cmd) {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        match jcode_base::platform::spawn_detached(&mut scoped(cmd)) {
            Ok(_) => Ok(()),
            // No systemd: start the terminal directly instead.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                jcode_base::platform::spawn_detached(cmd).map(|_| ())
            }
            Err(error) => Err(error),
        }
    })
}

/// Whether the terminal itself resolves on `PATH`.
fn cmd_exists(cmd: &Command) -> bool {
    let program = Path::new(cmd.get_program());
    if program.components().count() > 1 {
        return program.exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// `cmd` wrapped as `systemd-run --user --scope --collect --quiet -- <cmd>`,
/// keeping its working directory and environment.
pub(crate) fn scoped(cmd: &Command) -> Command {
    let mut scoped = Command::new("systemd-run");
    scoped
        .args(["--user", "--scope", "--collect", "--quiet", "--"])
        .arg(cmd.get_program())
        .args(cmd.get_args());
    if let Some(dir) = cmd.get_current_dir() {
        scoped.current_dir(dir);
    }
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => scoped.env(key, value),
            None => scoped.env_remove(key),
        };
    }
    scoped
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    scoped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_terminals_are_detected_before_wrapping() {
        assert!(!cmd_exists(&Command::new("jcode-no-such-terminal-xyz")));
        assert!(!cmd_exists(&Command::new("/nonexistent/kitty")));
        assert!(cmd_exists(&Command::new("sh")));
    }

    #[test]
    fn jarvis_command_is_always_on_plus_extra_flags() {
        let command = jarvis_command(
            PathBuf::from("/home/u/.local/bin/jcode"),
            &[
                "--jarvis-screen".into(),
                " ".into(),
                "--jarvis-always-on".into(),
                "--no-update".into(),
            ],
        );
        assert_eq!(command.program, PathBuf::from("/home/u/.local/bin/jcode"));
        assert_eq!(
            command.args,
            ["--jarvis-always-on", "--jarvis-screen", "--no-update"]
        );
        assert_eq!(command.title.as_deref(), Some("Jarvis"));
    }

    #[test]
    fn scoped_command_wraps_program_args_cwd_and_env() {
        let mut cmd = Command::new("kitty");
        cmd.args(["--title", "Jarvis", "-e", "jcode", "--jarvis-always-on"])
            .current_dir("/tmp")
            .env("JCODE_SPAWN_KIND", "jarvis")
            .env_remove("JCODE_SOCKET");
        let scoped = scoped(&cmd);
        assert_eq!(scoped.get_program(), "systemd-run");
        let args: Vec<_> = scoped.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(
            args,
            [
                "--user",
                "--scope",
                "--collect",
                "--quiet",
                "--",
                "kitty",
                "--title",
                "Jarvis",
                "-e",
                "jcode",
                "--jarvis-always-on"
            ]
        );
        assert_eq!(scoped.get_current_dir(), Some(Path::new("/tmp")));
        let envs: Vec<_> = scoped.get_envs().collect();
        assert!(envs.contains(&("JCODE_SPAWN_KIND".as_ref(), Some("jarvis".as_ref()))));
        assert!(envs.contains(&("JCODE_SOCKET".as_ref(), None)));
    }
}
