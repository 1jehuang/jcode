//! Tests for `#[path]`-attributed module `utf8_truncation_tests` of `bash.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

#[cfg(any(windows, unix))]
use super::build_shell_command;
use super::format_command_output;

#[test]
fn format_command_output_truncates_on_utf8_boundary() {
    let input = format!("{}é", "a".repeat(29_999));
    let output = format_command_output(input, None);
    assert!(output.ends_with("\n... (output truncated)"));
    assert!(output.starts_with(&"a".repeat(29_999)));
}

#[cfg(windows)]
#[tokio::test]
async fn build_shell_command_uses_cmd_and_executes_command() {
    let output = build_shell_command("echo hello-from-cmd")
        .output()
        .await
        .expect("run cmd command");
    assert!(output.status.success(), "cmd command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.to_ascii_lowercase().contains("hello-from-cmd"),
        "unexpected stdout: {}",
        stdout
    );

    let probe_path = std::env::temp_dir().join(format!(
        "jcode-cmd-quoting-probe-{}.cmd",
        std::process::id()
    ));
    std::fs::write(
        &probe_path,
        concat!(
            "@echo off\r\n",
            "if \"%~1\"==\"text with spaces\" if \"%~2\"==\"\" (\r\n",
            "  echo quoted-argument-ok\r\n",
            "  exit /b 0\r\n",
            ")\r\n",
            "echo first=[%~1] second=[%~2]\r\n",
            "exit /b 1\r\n",
        ),
    )
    .expect("write cmd quoting probe");

    let quoted_command = format!("call \"{}\" \"text with spaces\"", probe_path.display());
    let quoted_output = build_shell_command(&quoted_command)
        .output()
        .await
        .expect("run cmd quoting probe");
    let _ = std::fs::remove_file(&probe_path);
    let quoted_stdout = String::from_utf8_lossy(&quoted_output.stdout);
    let quoted_stderr = String::from_utf8_lossy(&quoted_output.stderr);
    assert!(
        quoted_output.status.success(),
        "quoted argument should remain one child-process argument; stdout={quoted_stdout:?} stderr={quoted_stderr:?}"
    );
    assert!(
        quoted_stdout.contains("quoted-argument-ok"),
        "unexpected quoted-command stdout: {quoted_stdout}"
    );
}

#[cfg(unix)]
#[test]
fn build_shell_command_uses_disk_backed_scratch_directory() {
    // Keep JCODE_HOME stable until the final directory assertion. Running
    // synchronously avoids holding the environment MutexGuard across await.
    let _env_lock = crate::storage::lock_test_env();
    let expected = super::tool_scratch_dir().expect("jcode scratch directory");
    let output = build_shell_command("printf '%s\\n%s\\n' \"$TMPDIR\" \"$JCODE_SCRATCH_DIR\"")
        .as_std_mut()
        .output()
        .expect("run bash command");
    assert!(output.status.success(), "bash command should succeed");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 scratch paths");
    let paths = stdout.lines().collect::<Vec<_>>();
    let expected = expected.to_string_lossy().into_owned();
    assert_eq!(paths, vec![expected.as_str(), expected.as_str()]);
    assert!(std::path::Path::new(&expected).is_dir());
}
