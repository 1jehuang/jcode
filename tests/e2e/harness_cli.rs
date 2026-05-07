use crate::test_support::*;
use serde_json::Value;
use std::process::Stdio;

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

fn harness_command_with_piped_stdout(home: &std::path::Path, cwd: &std::path::Path) -> Command {
    let mut cmd = harness_command(home, cwd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd
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
    for skill in [
        "karpathy-guidelines",
        "clean-code-guardian",
        "optimization",
        "llmwiki-memory",
    ] {
        assert!(
            stdout.contains(&format!("## Skill: {skill}")),
            "missing {skill}. stdout: {stdout}"
        );
    }

    Ok(())
}

#[test]
fn harness_run_json_uses_mock_provider_without_network() -> Result<()> {
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
            "review this diff",
            "--json",
            "--mock-response",
            "mocked harness response",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["provider"], "harness-mock");
    assert_eq!(report["model"], "harness-mock-model");
    assert_eq!(report["text"], "mocked harness response");
    assert_eq!(report["usage"]["input_tokens"], 1);
    assert_eq!(report["usage"]["output_tokens"], 1);

    Ok(())
}

#[test]
fn harness_run_ndjson_uses_mock_provider_without_network() -> Result<()> {
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
            "optimize memory usage",
            "--ndjson",
            "--mock-response",
            "mocked ndjson response",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let lines: Vec<Value> = stdout
        .lines()
        .map(serde_json::from_str)
        .collect::<serde_json::Result<_>>()?;
    assert_eq!(lines.len(), 2, "stdout: {stdout}");
    assert_eq!(lines[0]["type"], "start");
    assert_eq!(lines[0]["provider"], "harness-mock");
    assert_eq!(lines[0]["model"], "harness-mock-model");
    assert_eq!(lines[1]["type"], "done");
    assert_eq!(lines[1]["text"], "mocked ndjson response");
    assert_eq!(lines[1]["usage"]["input_tokens"], 1);
    assert_eq!(lines[1]["usage"]["output_tokens"], 1);

    Ok(())
}

#[test]
fn safe_eval_creates_isolated_profile_files_and_json_contract() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["safe-eval", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["profile"], "safe-eval");
    assert!(
        report["source_command"]
            .as_str()
            .unwrap()
            .contains("safe-eval.env")
    );
    assert!(
        report["powershell_command"]
            .as_str()
            .unwrap()
            .contains("safe-eval.ps1")
    );
    assert!(
        report["disabled_surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "telemetry")
    );

    let env_file = cwd.join(".jcode/safe-eval/safe-eval.env");
    let ps1_file = cwd.join(".jcode/safe-eval/safe-eval.ps1");
    let guide_file = cwd.join(".jcode/safe-eval/README.md");
    let safe_home = cwd.join(".jcode/safe-eval/home");
    assert!(env_file.is_file());
    assert!(ps1_file.is_file());
    assert!(guide_file.is_file());
    assert!(safe_home.is_dir());

    let env_content = std::fs::read_to_string(env_file)?;
    assert!(env_content.contains("export JCODE_SAFE_EVAL='1'"));
    assert!(env_content.contains("export JCODE_NO_TELEMETRY='1'"));
    assert!(env_content.contains("export DO_NOT_TRACK='1'"));
    assert!(env_content.contains("export JCODE_AMBIENT_ENABLED='false'"));
    assert!(env_content.contains("export JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES=''"));

    let guide_content = std::fs::read_to_string(guide_file)?;
    assert!(guide_content.contains("Trust checklist before leaving safe-eval"));
    assert!(guide_content.contains("jcode-harness smoke"));

    let print_env_output = harness_command(&home, &cwd)
        .args(["safe-eval", "--print-env"])
        .output()?;
    let print_env_stdout = stdout_text(&print_env_output);
    assert!(
        print_env_output.status.success(),
        "stderr: {}",
        stderr_text(&print_env_output)
    );
    assert!(print_env_stdout.starts_with("source "));
    assert!(print_env_stdout.contains("safe-eval.env"));

    Ok(())
}

#[test]
fn doctor_json_reports_safe_eval_privacy_skills_and_mcp_without_network() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(cwd.join(".jcode"))?;
    std::fs::write(cwd.join(".jcode/mcp.json"), "{\"servers\":{}}\n")?;

    let output = harness_command(&home, &cwd)
        .args(["doctor", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["offline"], true);
    assert_eq!(report["status"], "warn");
    assert_eq!(report["jcode_home"]["source"], "env");
    assert_eq!(report["safe_eval"]["active"], false);
    assert_eq!(report["privacy"]["telemetry_opted_out"], false);
    assert_eq!(report["skills"]["status"], "ok");
    assert!(report["skills"]["builtins"].as_u64().unwrap() >= 4);
    assert!(
        report["mcp"]["configs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|config| config["scope"] == "project-jcode"
                && config["exists"] == true
                && config["requires_review"] == true)
    );
    assert!(
        report["recommendations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("safe-eval"))
    );

    let human_output = harness_command(&home, &cwd).args(["doctor"]).output()?;
    let human_stdout = stdout_text(&human_output);
    assert!(
        human_output.status.success(),
        "stderr: {}",
        stderr_text(&human_output)
    );
    assert!(human_stdout.contains("jcode-harness doctor: warn"));
    assert!(human_stdout.contains("MCP configs found: 1"));

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
    for skill in [
        "karpathy-guidelines",
        "clean-code-guardian",
        "optimization",
        "llmwiki-memory",
    ] {
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
    for skill in [
        "karpathy-guidelines",
        "clean-code-guardian",
        "optimization",
        "llmwiki-memory",
    ] {
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
fn skills_doctor_exits_cleanly_when_stdout_pipe_closes() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let mut child = harness_command_with_piped_stdout(&home, &cwd)
        .args(["skills", "doctor"])
        .spawn()?;
    drop(child.stdout.take());

    let output = child.wait_with_output()?;
    let stderr = stderr_text(&output);
    assert!(
        output.status.success(),
        "broken stdout pipe should exit cleanly. status: {:?} stderr: {stderr}",
        output.status.code()
    );
    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "broken pipe should not print panic output. stderr: {stderr}"
    );

    Ok(())
}

#[test]
fn skills_json_commands_are_machine_readable() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    write_skill(&cwd, ".claude", "json-shared", "Claude JSON duplicate")?;
    write_skill(&home, "", "json-shared", "Global JSON duplicate")?;

    let list_output = harness_command(&home, &cwd)
        .args(["skills", "list", "--json"])
        .output()?;
    let list_stdout = stdout_text(&list_output);
    assert!(
        list_output.status.success(),
        "list stderr: {}",
        stderr_text(&list_output)
    );
    let list_json: Value = serde_json::from_str(&list_stdout)?;
    let skills = list_json["skills"].as_array().expect("skills array");
    assert!(
        skills
            .iter()
            .any(|skill| skill["name"] == "karpathy-guidelines" && skill["origin"] == "built-in"),
        "stdout: {list_stdout}"
    );
    assert!(
        skills
            .iter()
            .any(|skill| skill["name"] == "llmwiki-memory" && skill["origin"] == "built-in"),
        "stdout: {list_stdout}"
    );
    assert!(
        skills
            .iter()
            .any(|skill| skill["name"] == "json-shared" && skill["origin"] == "global"),
        "global skill should win over claude compat. stdout: {list_stdout}"
    );

    let show_output = harness_command(&home, &cwd)
        .args(["skills", "show", "json-shared", "--json"])
        .output()?;
    let show_stdout = stdout_text(&show_output);
    assert!(
        show_output.status.success(),
        "show stderr: {}",
        stderr_text(&show_output)
    );
    let show_json: Value = serde_json::from_str(&show_stdout)?;
    assert_eq!(show_json["name"], "json-shared");
    assert_eq!(show_json["origin"], "global");
    assert_eq!(show_json["description"], "Global JSON duplicate");
    assert!(
        show_json["content"]
            .as_str()
            .unwrap_or_default()
            .contains("Use json-shared")
    );

    let doctor_output = harness_command(&home, &cwd)
        .args(["skills", "doctor", "--json"])
        .output()?;
    let doctor_stdout = stdout_text(&doctor_output);
    assert!(
        doctor_output.status.success(),
        "doctor stderr: {}",
        stderr_text(&doctor_output)
    );
    let doctor_json: Value = serde_json::from_str(&doctor_stdout)?;
    assert!(doctor_json["skills_loaded"].as_u64().unwrap_or_default() >= 3);
    assert!(
        doctor_json["builtins"]
            .as_array()
            .expect("builtins array")
            .iter()
            .any(|builtin| builtin["name"] == "llmwiki-memory" && builtin["status"] == "ok"),
        "stdout: {doctor_stdout}"
    );
    assert!(
        doctor_json["duplicates"]
            .as_array()
            .expect("duplicates array")
            .iter()
            .any(|duplicate| duplicate["name"] == "json-shared"
                && duplicate["entries"].as_array().map(Vec::len) == Some(2)),
        "stdout: {doctor_stdout}"
    );

    Ok(())
}

#[test]
fn skills_match_json_reports_task_and_repo_scoped_selection() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    write_skill(
        &cwd,
        ".jcode",
        "clean-code-guardian",
        "Project-specific clean code policy",
    )?;
    write_skill(&cwd, ".jcode", "repo-skill", "Repo scoped helper")?;

    let output = harness_command(&home, &cwd)
        .args([
            "skills",
            "match",
            "fix this Rust bug and review the diff",
            "--skill",
            "repo-skill",
            "--json",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    let selected = report["selected"].as_array().expect("selected array");
    assert_eq!(selected[0]["name"], "repo-skill");
    assert_eq!(selected[0]["origin"], "project-local");
    assert!(
        selected
            .iter()
            .any(|skill| skill["name"] == "clean-code-guardian"
                && skill["origin"] == "project-local"
                && skill["description"] == "Project-specific clean code policy"),
        "stdout: {stdout}"
    );

    Ok(())
}

#[test]
fn skills_llmwiki_bridge_prints_permission_reviewed_mcp_mapping() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["skills", "llmwiki-bridge"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    assert!(
        stdout.contains("LLM wiki bridge for skill: llmwiki-memory"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("Offline preview: true"), "stdout: {stdout}");
    assert!(
        stdout.contains("wiki_query -> mcp__llmwiki__wiki_query"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("wiki_sync -> mcp__llmwiki__wiki_sync"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("does not invoke MCP tools"),
        "stdout: {stdout}"
    );

    Ok(())
}

#[test]
fn skills_llmwiki_bridge_json_is_machine_readable_and_offline() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["skills", "llmwiki-bridge", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["skill"], "llmwiki-memory");
    assert_eq!(report["kind"], "local-mcp-bridge-preview");
    assert_eq!(report["offline"], true);
    assert_eq!(report["network_required"], false);
    assert!(
        report["permission_boundary"]["secrets"]
            .as_str()
            .unwrap_or_default()
            .contains("credentials"),
        "stdout: {stdout}"
    );
    let commands = report["commands"].as_array().expect("commands array");
    assert!(
        commands
            .iter()
            .any(|command| command["name"] == "wiki_query"
                && command["mcp_tool"] == "mcp__llmwiki__wiki_query"),
        "stdout: {stdout}"
    );
    assert!(
        commands.iter().any(|command| command["name"] == "wiki_sync"
            && command["example"]["dry_run"] == true
            && command["write_risk"] == "local-files"),
        "stdout: {stdout}"
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

#[test]
fn clean_code_rules_prints_parseable_builtin_yaml() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-clean-code-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["clean-code", "rules"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        output.status.success(),
        "rules should succeed. stderr: {}",
        stderr_text(&output)
    );
    let rules: Value = serde_yaml::from_str(&stdout)?;
    assert_eq!(rules["name"], "clean-code-default");
    assert!(
        rules["rules"]
            .as_array()
            .expect("rules sequence")
            .iter()
            .any(|rule| rule["id"] == "no-silent-error-swallowing")
    );

    Ok(())
}

#[test]
fn clean_code_fail_on_info_fails_for_info_findings() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-clean-code-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    std::fs::write(cwd.join("sample.rs"), format!("// {}\n", "x".repeat(141)))?;

    let default_output = harness_command(&home, &cwd)
        .args(["clean-code", "check", "--json", "sample.rs"])
        .output()?;
    let default_stdout = stdout_text(&default_output);
    assert!(
        default_output.status.success(),
        "default fail-on error should pass on info. stdout: {default_stdout} stderr: {}",
        stderr_text(&default_output)
    );
    let default_report: Value = serde_json::from_str(&default_stdout)?;
    assert_eq!(default_report["findings"][0]["severity"], "info");

    let info_output = harness_command(&home, &cwd)
        .args([
            "clean-code",
            "check",
            "--json",
            "--fail-on",
            "info",
            "sample.rs",
        ])
        .output()?;
    assert!(
        !info_output.status.success(),
        "fail-on info should fail. stdout: {} stderr: {}",
        stdout_text(&info_output),
        stderr_text(&info_output)
    );

    Ok(())
}
