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
fn harness_init_json_reports_scaffold_and_detected_stack_offline() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-init-json-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    std::fs::write(
        cwd.join("Cargo.toml"),
        "[package]\nname = \"schema-smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;

    let output = harness_command(&home, &cwd)
        .args(["init", "--cwd"])
        .arg(&cwd)
        .args(["--yes", "--no-memory-wiki", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(
        report["root"].as_str(),
        Some(cwd.canonicalize()?.to_string_lossy().as_ref())
    );

    let files_written = report["files_written"].as_array().expect("files_written");
    for expected_suffix in [
        "AGENTS.md",
        ".jcode/INIT_REPORT.md",
        ".jcode/init/SWARM_ANALYSIS_PLAN.md",
        ".jcode/mcp.json",
    ] {
        assert!(
            files_written.iter().any(|path| path
                .as_str()
                .is_some_and(|path| path.ends_with(expected_suffix))),
            "missing {expected_suffix}. stdout: {stdout}"
        );
    }
    assert_eq!(report["files_skipped"].as_array().map(Vec::len), Some(0));
    assert!(
        report["detected_stack"]
            .as_array()
            .expect("detected_stack")
            .iter()
            .any(|stack| stack == "Rust"),
        "stdout: {stdout}"
    );
    assert!(
        report["next_steps"]
            .as_array()
            .expect("next_steps")
            .iter()
            .any(|step| step
                .as_str()
                .is_some_and(|step| step.contains("jcode-harness skills doctor"))),
        "stdout: {stdout}"
    );
    assert!(cwd.join("AGENTS.md").exists());
    assert!(cwd.join(".jcode/mcp.json").exists());
    assert!(!cwd.join(".jcode/memory_wiki").exists());

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
fn harness_demo_json_lists_offline_claim_demos_without_credentials() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-demo-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["demo", "--cwd"])
        .arg(&cwd)
        .arg("--json")
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["offline"], true);
    assert_eq!(report["network_required"], false);
    assert_eq!(report["credentials_required"], false);

    let demos = report["demos"].as_array().expect("demos");
    for surface in [
        "safe-eval",
        "mock-provider",
        "memory",
        "plan",
        "swarm",
        "browser",
        "skills",
        "release-gates",
    ] {
        assert!(
            demos.iter().any(|demo| demo["surface"] == surface),
            "missing surface {surface}. stdout: {stdout}"
        );
    }
    for demo in demos {
        assert_eq!(demo["offline"], true, "demo: {demo:?}");
        assert_eq!(demo["network_required"], false, "demo: {demo:?}");
        assert_eq!(demo["credentials_required"], false, "demo: {demo:?}");
        assert!(demo["argv"].as_array().is_some_and(|argv| !argv.is_empty()));
        assert!(
            demo["expected_evidence"]
                .as_array()
                .is_some_and(|evidence| !evidence.is_empty())
        );
    }
    assert!(demos.iter().any(|demo| {
        demo["id"] == "mock-provider-run-json"
            && demo["command"]
                .as_str()
                .is_some_and(|command| command.contains("--mock-response"))
    }));
    assert!(
        demos
            .iter()
            .any(|demo| demo["id"] == "release-gate-smoke" && demo["project_writes"] == true)
    );

    let human_output = harness_command(&home, &cwd)
        .args(["demo", "--cwd"])
        .arg(&cwd)
        .output()?;
    let human_stdout = stdout_text(&human_output);
    assert!(
        human_output.status.success(),
        "stderr: {}",
        stderr_text(&human_output)
    );
    assert!(human_stdout.contains("Reproducible demos:"));
    assert!(human_stdout.contains("memory-llmwiki-bridge"));
    assert!(human_stdout.contains("jcode-harness smoke"));

    Ok(())
}

#[test]
fn harness_demo_run_executes_non_writing_demo_and_blocks_project_writes() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-demo-run-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["demo", "run", "mock-provider-run-json", "--cwd"])
        .arg(&cwd)
        .arg("--json")
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["offline"], true);
    assert_eq!(report["requested"], "mock-provider-run-json");
    let result = &report["results"].as_array().expect("results")[0];
    assert_eq!(result["status"], "pass");
    assert_eq!(result["project_writes"], false);
    assert_eq!(result["json_parseable"], true);
    assert!(
        result["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("harness-mock"))
    );

    let blocked = harness_command(&home, &cwd)
        .args(["demo", "run", "release-gate-smoke", "--cwd"])
        .arg(&cwd)
        .arg("--json")
        .output()?;
    let blocked_stdout = stdout_text(&blocked);
    assert!(
        !blocked.status.success(),
        "project-writing demo should be blocked without --allow-writes"
    );
    let blocked_report: Value = serde_json::from_str(&blocked_stdout)?;
    assert_eq!(blocked_report["status"], "blocked");
    let blocked_result = &blocked_report["results"].as_array().expect("results")[0];
    assert_eq!(blocked_result["status"], "blocked");
    assert_eq!(blocked_result["project_writes"], true);
    assert!(
        blocked_result["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("--allow-writes"))
    );
    assert!(!cwd.join(".jcode/demo/smoke/sample.txt").exists());

    Ok(())
}

#[test]
fn harness_demo_run_sandbox_executes_project_writes_without_mutating_cwd() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-demo-sandbox-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command(&home, &cwd)
        .args(["demo", "run", "all", "--cwd"])
        .arg(&cwd)
        .args(["--sandbox", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["sandbox"]["enabled"], true);
    assert_eq!(report["sandbox"]["retained"], false);
    assert_eq!(report["sandbox"]["cleanup"], "removed_after_run");
    let sandbox_path = report["sandbox"]["path"].as_str().expect("sandbox path");
    assert!(
        !std::path::Path::new(sandbox_path).exists(),
        "sandbox should be removed by default: {sandbox_path}"
    );
    assert_eq!(report["results"].as_array().map(Vec::len), Some(8));
    for result in report["results"].as_array().expect("results") {
        assert_eq!(result["status"], "pass", "result: {result:?}");
        assert_eq!(result["executed_root"], sandbox_path);
    }
    assert!(report["results"].as_array().unwrap().iter().any(|result| {
        result["id"] == "release-gate-smoke" && result["project_writes"] == true
    }));
    assert!(
        !cwd.join(".jcode").exists(),
        "sandboxed demo run must not write into requested cwd"
    );

    Ok(())
}

#[test]
fn harness_session_list_json_reports_local_sessions_without_tui() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-session-list-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    let sessions_dir = home.join("sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&cwd)?;

    std::fs::write(
        sessions_dir.join("session_visible.json"),
        serde_json::json!({
            "id": "session_visible",
            "title": "Visible local session",
            "created_at": "2026-05-07T20:00:00Z",
            "updated_at": "2026-05-07T20:05:00Z",
            "last_active_at": "2026-05-07T20:06:00Z",
            "working_dir": cwd,
            "short_name": "visible",
            "provider_key": "openai",
            "model": "gpt-test",
            "saved": true,
            "save_label": "fixture",
            "status": "Closed",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}
            ]
        })
        .to_string(),
    )?;
    std::fs::write(
        sessions_dir.join("session_debug.json"),
        serde_json::json!({
            "id": "session_debug",
            "title": "Debug local session",
            "created_at": "2026-05-07T19:00:00Z",
            "updated_at": "2026-05-07T19:05:00Z",
            "short_name": "debug",
            "is_debug": true,
            "status": "Closed",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hidden"}]}
            ]
        })
        .to_string(),
    )?;

    let output = harness_command(&home, &cwd)
        .args(["session", "list", "--source", "jcode", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["command"], "session list");
    assert_eq!(report["offline"], true);
    assert_eq!(report["read_only"], true);
    assert_eq!(report["source"], "jcode");
    assert_eq!(report["discovered_count"], 2);
    assert_eq!(report["session_count"], 1);
    assert_eq!(report["hidden_test_count"], 1);
    let sessions = report["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1, "stdout: {stdout}");
    let session = &sessions[0];
    assert_eq!(session["id"], "session_visible");
    assert_eq!(session["source"], "jcode");
    assert_eq!(session["short_name"], "visible");
    assert_eq!(session["title"], "Visible local session");
    assert_eq!(session["status"], "closed");
    assert_eq!(session["message_count"], 2);
    assert_eq!(session["user_message_count"], 1);
    assert_eq!(session["assistant_message_count"], 1);
    assert_eq!(session["saved"], true);
    assert_eq!(session["save_label"], "fixture");
    assert_eq!(session["resume_target"]["kind"], "jcode_session");
    assert_eq!(session["resume_target"]["id"], "session_visible");

    let include_test = harness_command(&home, &cwd)
        .args([
            "session",
            "list",
            "--source",
            "jcode",
            "--include-test",
            "--json",
        ])
        .output()?;
    let include_stdout = stdout_text(&include_test);
    assert!(
        include_test.status.success(),
        "stderr: {}",
        stderr_text(&include_test)
    );
    let include_report: Value = serde_json::from_str(&include_stdout)?;
    assert_eq!(include_report["session_count"], 2);
    assert!(
        include_report["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|session| session["id"] == "session_debug" && session["is_debug"] == true),
        "stdout: {include_stdout}"
    );

    Ok(())
}

#[test]
fn harness_session_show_json_reports_metadata_and_opt_in_preview() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-session-show-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    let sessions_dir = home.join("sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    std::fs::create_dir_all(&cwd)?;

    std::fs::write(
        sessions_dir.join("session_show.json"),
        serde_json::json!({
            "id": "session_show",
            "title": "Show local session",
            "created_at": "2026-05-07T20:10:00Z",
            "updated_at": "2026-05-07T20:20:00Z",
            "last_active_at": "2026-05-07T20:21:00Z",
            "working_dir": cwd,
            "short_name": "showcase",
            "provider_key": "openai",
            "provider_session_id": "provider-fixture",
            "model": "gpt-test",
            "reasoning_effort": "medium",
            "saved": true,
            "save_label": "show-fixture",
            "status": "Closed",
            "messages": [
                {
                    "id": "m0",
                    "role": "user",
                    "display_role": "system",
                    "content": [{"type": "text", "text": "<system-reminder>hidden default transcript secret</system-reminder>"}]
                },
                {"id": "m1", "role": "user", "content": [{"type": "text", "text": "first visible prompt"}]},
                {"id": "m2", "role": "assistant", "content": [{"type": "text", "text": "second visible answer"}]},
                {"id": "m3", "role": "user", "content": [{"type": "text", "text": "third visible follow-up"}]}
            ],
            "env_snapshots": [],
            "memory_injections": [],
            "replay_events": []
        })
        .to_string(),
    )?;

    let output = harness_command(&home, &cwd)
        .args(["session", "show", "session_show", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    assert!(
        !stdout.contains("first visible prompt")
            && !stdout.contains("second visible answer")
            && !stdout.contains("hidden default transcript secret"),
        "default show output must not include transcript content. stdout: {stdout}"
    );
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["command"], "session show");
    assert_eq!(report["offline"], true);
    assert_eq!(report["read_only"], true);
    assert_eq!(report["source"], "jcode");
    assert_eq!(report["id"], "session_show");
    assert_eq!(report["metadata"]["display_name"], "showcase");
    assert_eq!(report["metadata"]["status"], "closed");
    assert_eq!(report["metadata"]["stored_message_count"], 4);
    assert_eq!(report["metadata"]["user_message_count"], 2);
    assert_eq!(report["metadata"]["assistant_message_count"], 1);
    assert_eq!(
        report["metadata"]["provider_session_id"],
        "provider-fixture"
    );
    assert_eq!(report["metadata"]["reasoning_effort"], "medium");
    assert_eq!(report["preview"]["requested"], 0);
    assert_eq!(report["preview"]["returned"], 0);
    assert_eq!(
        report["preview"]["messages"].as_array().map(Vec::len),
        Some(0)
    );

    let preview_output = harness_command(&home, &cwd)
        .args([
            "session",
            "show",
            "session_show",
            "--preview",
            "2",
            "--json",
        ])
        .output()?;
    let preview_stdout = stdout_text(&preview_output);
    assert!(
        preview_output.status.success(),
        "stderr: {}",
        stderr_text(&preview_output)
    );
    let preview_report: Value = serde_json::from_str(&preview_stdout)?;
    assert_eq!(preview_report["preview"]["requested"], 2);
    assert_eq!(preview_report["preview"]["returned"], 2);
    let messages = preview_report["preview"]["messages"]
        .as_array()
        .expect("preview messages");
    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["content"], "second visible answer");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "third visible follow-up");
    assert!(
        !preview_stdout.contains("hidden default transcript secret"),
        "system reminder should remain hidden. stdout: {preview_stdout}"
    );

    Ok(())
}

#[test]
fn harness_smoke_runs_offline_tool_cases_with_deterministic_artifacts() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-smoke-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let output = harness_command_with_piped_stdout(&home, &cwd)
        .args(["smoke", "--cwd"])
        .arg(&cwd)
        .output()?;
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);

    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains(&format!("Harness workspace: {}", cwd.display())),
        "stderr: {stderr}"
    );

    for expected in [
        "== write (write sample.txt) ==",
        "== read (read sample.txt) ==",
        "== edit (edit sample.txt (alpha -> alpha1)) ==",
        "== multiedit (multiedit sample.txt) ==",
        "== patch (patch sample.txt) ==",
        "== apply_patch (apply_patch add file) ==",
        "== ls (ls .) ==",
        "== glob (glob *.txt) ==",
        "== grep (grep gamma) ==",
        "== bash (bash pwd) ==",
        "== invalid (invalid tool call) ==",
        "== todo (todo write) ==",
        "== todo (todo read) ==",
        "== batch (batch ls + read) ==",
        "Completed: 2 succeeded, 0 failed",
    ] {
        assert!(
            stdout.contains(expected),
            "missing {expected}. stdout: {stdout}"
        );
    }

    for network_case in ["== webfetch", "== websearch", "== codesearch"] {
        assert!(
            !stdout.contains(network_case),
            "default smoke should not run network-backed case {network_case}. stdout: {stdout}"
        );
    }

    assert_eq!(
        std::fs::read_to_string(cwd.join("sample.txt"))?,
        "alpha2\nbeta1\ngamma\n"
    );
    assert_eq!(std::fs::read_to_string(cwd.join("added.txt"))?, "added\n");

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
fn skills_validate_json_reports_effective_project_skill() -> Result<()> {
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
        "repo-reviewer",
        "Project review rules for this repository",
    )?;

    let output = harness_command(&home, &cwd)
        .args([
            "skills",
            "validate",
            "--cwd",
            cwd.to_str().unwrap(),
            "--json",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["offline"], true);
    assert_eq!(report["errors"], 0);
    assert!(report["checked"].as_u64().unwrap() >= 5);
    assert!(
        report["origins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|origin| origin["origin"] == "project-local"
                && origin["exists"] == true
                && origin["checked"] == 1)
    );
    let repo_skill = report["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["name"] == "repo-reviewer")
        .expect("repo-reviewer skill should be present");
    assert_eq!(repo_skill["origin"], "project-local");
    assert_eq!(repo_skill["valid"], true);
    assert_eq!(repo_skill["effective"], true);

    Ok(())
}

#[test]
fn skills_validate_json_flags_invalid_skill_and_exits_nonzero() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    let bad_dir = cwd.join(".jcode/skills/bad-skill");
    std::fs::create_dir_all(&bad_dir)?;
    std::fs::write(
        bad_dir.join("SKILL.md"),
        "---\nname: bad-skill\nallowed-tools:\n  bash: true\n---\n\n",
    )?;

    let output = harness_command(&home, &cwd)
        .args(["skills", "validate", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(
        !output.status.success(),
        "invalid skill should fail. stdout: {stdout}"
    );
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "error");
    assert!(report["errors"].as_u64().unwrap() >= 2);
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "missing-description")
    );
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "invalid-allowed-tools")
    );

    Ok(())
}

#[test]
fn skills_import_json_previews_agents_skill_without_writing() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    write_skill(
        &cwd,
        ".agents",
        "agent-reviewer",
        "Agent ecosystem review skill",
    )?;

    let output = harness_command(&home, &cwd)
        .args(["skills", "import", "--json"])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["offline"], true);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["target"]["scope"], "project");
    assert_eq!(report["planned"], 1);
    assert_eq!(report["copied"], 0);
    let action = report["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["name"] == "agent-reviewer")
        .expect("agent-reviewer import action");
    assert_eq!(action["source_origin"], "agents");
    assert_eq!(action["action"], "copy");
    assert_eq!(action["applied"], false);
    assert!(!cwd.join(".jcode/skills/agent-reviewer/SKILL.md").exists());

    Ok(())
}

#[test]
fn skills_import_apply_copies_claude_skill_into_project_scope() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;
    write_skill(
        &cwd,
        ".claude",
        "claude-reviewer",
        "Claude-compatible review skill",
    )?;
    std::fs::write(
        cwd.join(".claude/skills/claude-reviewer/notes.md"),
        "extra skill material\n",
    )?;

    let output = harness_command(&home, &cwd)
        .args([
            "skills",
            "import",
            "--from",
            ".claude/skills",
            "--apply",
            "--json",
        ])
        .output()?;
    let stdout = stdout_text(&output);

    assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    let report: Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["status"], "ok");
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["planned"], 1);
    assert_eq!(report["copied"], 1);
    let action = &report["actions"].as_array().unwrap()[0];
    assert_eq!(action["name"], "claude-reviewer");
    assert_eq!(action["source_origin"], "claude-compat");
    assert_eq!(action["applied"], true);
    assert!(cwd.join(".jcode/skills/claude-reviewer/SKILL.md").is_file());
    assert!(cwd.join(".jcode/skills/claude-reviewer/notes.md").is_file());

    let validate_output = harness_command(&home, &cwd)
        .args(["skills", "validate", "--json"])
        .output()?;
    assert!(
        validate_output.status.success(),
        "stderr: {}",
        stderr_text(&validate_output)
    );

    Ok(())
}

#[test]
fn skills_scope_init_set_and_list_json_policy_file() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let init_output = harness_command(&home, &cwd)
        .args(["skills", "scope", "init", "--json"])
        .output()?;
    let init_stdout = stdout_text(&init_output);
    assert!(
        init_output.status.success(),
        "stderr: {}",
        stderr_text(&init_output)
    );
    let init_report: Value = serde_json::from_str(&init_stdout)?;
    assert_eq!(init_report["created"], true);
    assert_eq!(init_report["policy"]["default_state"], "visible");
    assert!(cwd.join(".jcode/skills.scope.json").is_file());

    let set_output = harness_command(&home, &cwd)
        .args([
            "skills",
            "scope",
            "set",
            "optimization",
            "--state",
            "blocked",
            "--reason",
            "benchmark-only in this repo",
            "--json",
        ])
        .output()?;
    let set_stdout = stdout_text(&set_output);
    assert!(
        set_output.status.success(),
        "stderr: {}",
        stderr_text(&set_output)
    );
    let set_report: Value = serde_json::from_str(&set_stdout)?;
    assert_eq!(set_report["updated"], true);
    let entry = set_report["policy"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "optimization")
        .expect("optimization scope entry");
    assert_eq!(entry["state"], "blocked");
    assert_eq!(entry["reason"], "benchmark-only in this repo");

    let list_output = harness_command(&home, &cwd)
        .args(["skills", "scope", "list", "--json"])
        .output()?;
    let list_stdout = stdout_text(&list_output);
    assert!(
        list_output.status.success(),
        "stderr: {}",
        stderr_text(&list_output)
    );
    let list_report: Value = serde_json::from_str(&list_stdout)?;
    assert_eq!(list_report["exists"], true);
    assert_eq!(list_report["policy"]["skills"][0]["name"], "optimization");

    Ok(())
}

#[test]
fn skills_match_and_run_dry_run_respect_scope_policy() -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("jcode-harness-cli-")
        .tempdir()?;
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    for (name, state) in [
        ("optimization", "blocked"),
        ("clean-code-guardian", "discoverable"),
    ] {
        let output = harness_command(&home, &cwd)
            .args(["skills", "scope", "set", name, "--state", state])
            .output()?;
        assert!(output.status.success(), "stderr: {}", stderr_text(&output));
    }

    let match_output = harness_command(&home, &cwd)
        .args([
            "skills",
            "match",
            "optimize this code path and review the diff",
            "--json",
        ])
        .output()?;
    let match_stdout = stdout_text(&match_output);
    assert!(
        match_output.status.success(),
        "stderr: {}",
        stderr_text(&match_output)
    );
    let report: Value = serde_json::from_str(&match_stdout)?;
    let selected = report["selected"].as_array().unwrap();
    assert!(
        selected
            .iter()
            .any(|entry| entry["name"] == "karpathy-guidelines")
    );
    assert!(!selected.iter().any(|entry| entry["name"] == "optimization"));
    assert!(
        !selected
            .iter()
            .any(|entry| entry["name"] == "clean-code-guardian")
    );
    let skipped = report["policy"]["skipped"].as_array().unwrap();
    assert!(
        skipped
            .iter()
            .any(|entry| entry["name"] == "optimization" && entry["state"] == "blocked")
    );
    assert!(skipped.iter().any(|entry| entry["name"] == "clean-code-guardian"
        && entry["state"] == "discoverable"));

    let explicit_output = harness_command(&home, &cwd)
        .args([
            "skills",
            "match",
            "review this diff",
            "--skill",
            "clean-code-guardian",
            "--json",
        ])
        .output()?;
    let explicit_report: Value = serde_json::from_str(&stdout_text(&explicit_output))?;
    assert!(
        explicit_output.status.success(),
        "stderr: {}",
        stderr_text(&explicit_output)
    );
    assert!(
        explicit_report["selected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "clean-code-guardian")
    );

    let dry_run_output = harness_command(&home, &cwd)
        .args(["run", "optimize memory usage", "--dry-run"])
        .output()?;
    let dry_run_stdout = stdout_text(&dry_run_output);
    assert!(
        dry_run_output.status.success(),
        "stderr: {}",
        stderr_text(&dry_run_output)
    );
    assert!(
        !dry_run_stdout.contains("## Skill: optimization"),
        "blocked optimization should not be injected. stdout: {dry_run_stdout}"
    );
    assert_eq!(dry_run_stdout.trim(), "optimize memory usage");

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
