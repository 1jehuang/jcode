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

fn write_skill(root: &std::path::Path, scope: &str, name: &str, description: &str) -> Result<()> {
    let dir = root.join(scope).join("skills").join(name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nUse {name}.\n"),
    )?;
    Ok(())
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
fn skills_doctor_reports_duplicate_names_across_origins() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    write_skill(&cwd, ".claude", "shared-skill", "Claude compat duplicate")?;
    write_skill(&home, "", "shared-skill", "Global duplicate")?;
    write_skill(&cwd, ".jcode", "shared-skill", "Project duplicate")?;

    let output = harness_command(&home, &cwd)
        .args(["skills", "doctor"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        output.status.success(),
        "skills doctor should succeed. stderr: {}",
        stderr_text(&output)
    );
    assert!(stdout.contains("duplicates: 1 name(s)"), "stdout: {stdout}");
    assert!(
        stdout.contains("duplicate shared-skill:"),
        "stdout: {stdout}"
    );
    for origin in ["claude-compat", "global", "project-local"] {
        assert!(
            stdout.contains(origin),
            "missing {origin}. stdout: {stdout}"
        );
    }

    Ok(())
}

#[test]
fn global_jcode_skill_overrides_claude_compat_but_not_project_local() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    write_skill(&cwd, ".claude", "precedence-skill", "Claude compat version")?;
    write_skill(&home, "", "precedence-skill", "Global version")?;

    let global_output = harness_command(&home, &cwd)
        .args(["skills", "show", "precedence-skill"])
        .output()?;
    let global_stdout = stdout_text(&global_output);
    assert!(
        global_output.status.success(),
        "global show should succeed. stderr: {}",
        stderr_text(&global_output)
    );
    assert!(
        global_stdout.contains("origin: global"),
        "stdout: {global_stdout}"
    );
    assert!(
        global_stdout.contains("description: Global version"),
        "stdout: {global_stdout}"
    );

    write_skill(&cwd, ".jcode", "precedence-skill", "Project version")?;
    let project_output = harness_command(&home, &cwd)
        .args(["skills", "show", "precedence-skill"])
        .output()?;
    let project_stdout = stdout_text(&project_output);
    assert!(
        project_output.status.success(),
        "project show should succeed. stderr: {}",
        stderr_text(&project_output)
    );
    assert!(
        project_stdout.contains("origin: project-local"),
        "stdout: {project_stdout}"
    );
    assert!(
        project_stdout.contains("description: Project version"),
        "stdout: {project_stdout}"
    );

    Ok(())
}

#[test]
fn skills_list_and_sync_expose_builtin_harness_skills() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let list_output = harness_command(&home, &cwd)
        .args(["skills", "list"])
        .output()?;
    let list_stdout = stdout_text(&list_output);
    assert!(
        list_output.status.success(),
        "skills list should succeed. stderr: {}",
        stderr_text(&list_output)
    );
    for skill in ["karpathy-guidelines", "clean-code-guardian", "optimization"] {
        assert!(
            list_stdout.contains(&format!("{skill}\tbuilt-in")),
            "missing built-in {skill}. stdout: {list_stdout}"
        );
    }

    let sync_output = harness_command(&home, &cwd)
        .args(["skills", "sync"])
        .output()?;
    let sync_stdout = stdout_text(&sync_output);
    assert!(
        sync_output.status.success(),
        "skills sync should succeed. stderr: {}",
        stderr_text(&sync_output)
    );
    for skill in ["karpathy-guidelines", "clean-code-guardian", "optimization"] {
        let synced = home.join("skills").join(skill).join("SKILL.md");
        assert!(synced.exists(), "missing synced file: {}", synced.display());
        assert!(
            sync_stdout.contains(&synced.display().to_string()),
            "sync output should mention {}. stdout: {sync_stdout}",
            synced.display()
        );
    }

    let second_sync = harness_command(&home, &cwd)
        .args(["skills", "sync"])
        .output()?;
    let second_stdout = stdout_text(&second_sync);
    assert!(
        second_sync.status.success(),
        "second sync stderr: {}",
        stderr_text(&second_sync)
    );
    assert!(
        second_stdout.contains("No built-in skills copied"),
        "second sync should not overwrite by default. stdout: {second_stdout}"
    );

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
