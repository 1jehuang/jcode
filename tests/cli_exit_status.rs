//! Process-level regressions for clap's CLI exit status and output streams.
use std::process::{Command, Output};

fn invoke(args: &[&str]) -> Output {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_jcode"))
        .current_dir(root.path())
        .env_clear()
        .env("HOME", root.path())
        .env("JCODE_HOME", root.path().join("jcode"))
        .env("JCODE_NO_TELEMETRY", "1")
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("run jcode CLI");
    assert!(
        !root.path().join("jcode").exists(),
        "parsing must exit before creating application state"
    );
    output
}

#[test]
fn unknown_subcommand_exits_with_usage_error() {
    let output = invoke(&["definitely-not-a-command"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unrecognized subcommand 'definitely-not-a-command'"));
    assert!(stderr.contains("Usage:"));
}

#[test]
fn unknown_option_exits_with_usage_error() {
    let output = invoke(&["--definitely-not-an-option"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unexpected argument '--definitely-not-an-option'"));
    assert!(stderr.contains("Usage:"));
}

#[test]
fn help_exits_successfully_on_stdout() {
    let output = invoke(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("Commands:"));
}

#[test]
fn version_exits_successfully_on_stdout() {
    let output = invoke(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("jcode ")
    );
}
