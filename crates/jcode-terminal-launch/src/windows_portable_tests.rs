//! Windows spawn checks that run on every platform.
//!
//! The Windows tests in `lib.rs` are all `cfg(not(unix))`, so none of them run
//! in this repo's Linux CI or on a Linux dev box. That blind spot is what let
//! the in-app spawn stub sit unnoticed off Unix (#715). Anything checkable
//! without a Windows host belongs here instead.

use super::{
    TerminalCommand, build_spawn_command, spawn_command_with_candidates, windows_command_line,
    windows_resume_terminal_candidates_with,
};
use std::path::Path;

/// #715: the in-app resume spawn was a no-op off Unix, so the Windows
/// command line it *would* build was never exercised anywhere that runs.
///
/// The existing Windows coverage is `cfg(not(unix))`, so it never runs in
/// this repo's Linux CI or on a maintainer's Linux box. `windows_command_line`
/// is compiled under `test` on every platform, so pin the resume invocation
/// here where it will actually be checked.
#[test]
fn windows_command_line_quotes_a_resume_invocation_on_every_platform() {
    let line = windows_command_line(&[
        r"C:\Program Files\jcode\jcode.exe".to_string(),
        "--resume".to_string(),
        "session_penguin_123".to_string(),
    ]);

    // The exe path contains a space, so it must be quoted or cmd.exe would
    // treat "C:\Program" as the program.
    assert!(
        line.contains(r#""C:\Program Files\jcode\jcode.exe""#),
        "unquoted program path would break on a default Windows install: {line}"
    );
    assert!(line.contains("--resume"), "{line}");
    assert!(line.contains("session_penguin_123"), "{line}");
    // Session ids are safe, so they must not be needlessly quoted.
    assert!(
        !line.contains(r#""--resume""#),
        "quoting flags changes cmd.exe parsing: {line}"
    );
}

#[test]
fn configured_windows_terminal_precedes_shared_fallback_candidates() {
    let candidates = windows_resume_terminal_candidates_with(Some("cmd".into()), None);
    assert_eq!(candidates.first().map(String::as_str), Some("cmd"));
    assert!(candidates.contains(&"cmd".to_string()));
}

#[test]
fn cmd_only_fallback_builds_the_windows_start_command_with_runtime_identity() {
    let command = TerminalCommand::new(
        r"C:\Program Files\jcode\jcode.exe",
        vec!["--fresh-spawn".into(), "--resume".into(), "ses_cmd".into()],
    )
    .title("cmd fallback")
    .runtime_env(vec![("JCODE_SOCKET".into(), r"C:\connected.sock".into())]);
    let cmd = build_spawn_command("cmd", &command, Path::new(r"C:\work"))
        .expect("cmd fallback invocation");
    assert_eq!(cmd.get_program().to_string_lossy(), "cmd");
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(&args[..5], ["/C", "start", "cmd fallback", "cmd.exe", "/K"]);
    assert!(args[5].contains(r#""C:\Program Files\jcode\jcode.exe""#));
    assert!(args[5].contains("--resume ses_cmd"));
    assert_eq!(
        cmd.get_envs()
            .find(|(key, _)| *key == "JCODE_SOCKET")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
            .as_deref(),
        Some(r"C:\connected.sock")
    );
}

#[test]
fn production_shared_launcher_honors_configured_terminal_before_cmd_fallback() {
    let candidates = windows_resume_terminal_candidates_with(Some("wt".into()), None);
    assert_eq!(candidates.first().map(String::as_str), Some("wt"));
    let expected_attempts = candidates.clone();
    let mut attempted = Vec::new();
    let command = TerminalCommand::new("jcode.exe", vec!["--resume".into()]);
    let result =
        spawn_command_with_candidates(&command, Path::new(r"C:\work"), candidates, |candidate| {
            attempted.push(candidate.get_program().to_string_lossy().into_owned());
            if candidate.get_program() == "cmd" {
                Ok(())
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            }
        })
        .unwrap();
    assert!(result);
    assert_eq!(attempted, expected_attempts);
}
