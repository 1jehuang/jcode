use crate::test_support::*;
use serde_json::Value;

fn harness_command(home: &std::path::Path, cwd: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jcode-harness"));
    cmd.env("JCODE_HOME", home)
        .env("JCODE_RUNTIME_DIR", home.join("runtime"))
        .env("JCODE_TEST_SESSION", "1")
        .current_dir(cwd)
        .stdin(Stdio::null());
    cmd
}

fn stdout_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn harness_run_dry_run_auto_routes_optimization_only_for_perf_task() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["run", "optimize memory usage", "--dry-run"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        output.status.success(),
        "dry-run should succeed. stderr: {}",
        stderr_text(&output)
    );
    assert!(
        stdout.contains("## Skill: optimization"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("## Skill: karpathy-guidelines"),
        "pure perf task should not inject coding guardrails. stdout: {stdout}"
    );
    assert!(
        !stdout.contains("## Skill: clean-code-guardian"),
        "pure perf task should not inject clean-code guardrails. stdout: {stdout}"
    );

    Ok(())
}

#[test]
fn harness_run_dry_run_off_keeps_only_explicit_skill() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args([
            "run",
            "fix this bug and reduce memory usage",
            "--skills",
            "off",
            "--skill",
            "optimization",
            "--dry-run",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        output.status.success(),
        "dry-run should succeed. stderr: {}",
        stderr_text(&output)
    );
    assert!(
        stdout.contains("## Skill: optimization"),
        "stdout: {stdout}"
    );
    assert!(
        !stdout.contains("## Skill: karpathy-guidelines"),
        "--skills off should suppress automatic skills. stdout: {stdout}"
    );
    assert!(
        !stdout.contains("## Skill: clean-code-guardian"),
        "--skills off should suppress automatic skills. stdout: {stdout}"
    );

    Ok(())
}

#[test]
fn harness_run_dry_run_always_includes_all_builtin_harness_skills() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args([
            "run",
            "write release notes",
            "--skills",
            "always",
            "--dry-run",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        output.status.success(),
        "dry-run should succeed. stderr: {}",
        stderr_text(&output)
    );
    for skill in ["karpathy-guidelines", "clean-code-guardian", "optimization"] {
        assert!(
            stdout.contains(&format!("## Skill: {skill}")),
            "missing {skill}. stdout: {stdout}"
        );
    }

    Ok(())
}

#[test]
fn clean_code_check_json_reports_findings_without_failing_below_threshold() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-clean-code-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    std::fs::write(
        cwd.join("sample.rs"),
        "fn ignore() {\n    let _ = std::fs::read_to_string(\"missing\");\n}\n",
    )?;

    let output = harness_command(&home, &cwd)
        .args([
            "clean-code",
            "check",
            "--json",
            "--fail-on",
            "warning",
            "sample.rs",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        !output.status.success(),
        "warning threshold should fail on error findings. stdout: {stdout} stderr: {}",
        stderr_text(&output)
    );
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["files_scanned"], 1);
    assert_eq!(
        report["findings"][0]["rule_id"],
        "no-silent-error-swallowing"
    );
    assert_eq!(report["findings"][0]["severity"], "error");

    Ok(())
}

#[test]
fn clean_code_check_json_passes_for_clean_file() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-clean-code-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    std::fs::write(
        cwd.join("sample.rs"),
        "fn ok() {\n    println!(\"ok\");\n}\n",
    )?;

    let output = harness_command(&home, &cwd)
        .args(["clean-code", "check", "--json", "sample.rs"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        output.status.success(),
        "clean file should pass. stdout: {stdout} stderr: {}",
        stderr_text(&output)
    );
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["files_scanned"], 1);
    assert_eq!(report["findings"].as_array().map(Vec::len), Some(0));

    Ok(())
}
