use anyhow::Result;
use std::path::PathBuf;

/// Execute tester commands
pub(super) async fn execute_tester_command(command: &str) -> Result<String> {
    let trimmed = command.trim();

    if trimmed == "list" {
        let testers = load_testers()?;
        if testers.is_empty() {
            return Ok("No active testers.".to_string());
        }
        return Ok(serde_json::to_string_pretty(&testers)?);
    }

    if trimmed == "spawn" || trimmed.starts_with("spawn ") {
        let opts: serde_json::Value = if trimmed == "spawn" {
            serde_json::json!({})
        } else {
            serde_json::from_str(trimmed.strip_prefix("spawn ").unwrap_or("{}"))?
        };
        return spawn_tester(opts).await;
    }

    let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
    if parts.len() >= 2 {
        let tester_id = parts[0];
        let cmd = parts[1];
        let arg = parts.get(2).copied();
        return execute_tester_subcommand(tester_id, cmd, arg).await;
    }

    Err(anyhow::anyhow!(
        "Unknown tester command: {}. Use tester:help for usage.",
        trimmed
    ))
}

fn load_testers() -> Result<Vec<serde_json::Value>> {
    let path = crate::storage::jcode_dir()?.join("testers.json");
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        if content.trim().is_empty() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(vec![])
    }
}

fn save_testers(testers: &[serde_json::Value]) -> Result<()> {
    let path = crate::storage::jcode_dir()?.join("testers.json");
    std::fs::write(&path, serde_json::to_string_pretty(testers)?)?;
    Ok(())
}

async fn spawn_tester(opts: serde_json::Value) -> Result<String> {
    use std::process::Stdio;

    let id = format!("tester_{}", crate::id::new_id("tui"));
    let cwd = opts.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
    let binary = opts.get("binary").and_then(|v| v.as_str());

    let binary_path = if let Some(b) = binary {
        PathBuf::from(b)
    } else if let Ok(canary) = crate::build::canary_binary_path() {
        if canary.exists() {
            canary
        } else {
            std::env::current_exe()?
        }
    } else {
        std::env::current_exe()?
    };

    if !binary_path.exists() {
        return Err(anyhow::anyhow!(
            "Binary not found: {}",
            binary_path.display()
        ));
    }

    let debug_cmd = std::env::temp_dir().join(format!("jcode_debug_cmd_{}", id));
    let debug_resp = std::env::temp_dir().join(format!("jcode_debug_response_{}", id));
    let stdout_path = std::env::temp_dir().join(format!("jcode_tester_stdout_{}", id));
    let stderr_path = std::env::temp_dir().join(format!("jcode_tester_stderr_{}", id));

    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let _ = crate::platform::set_permissions_owner_only(&stdout_path);
    let _ = crate::platform::set_permissions_owner_only(&stderr_path);
    let _ = std::fs::File::create(&debug_cmd)
        .and_then(|_| crate::platform::set_permissions_owner_only(&debug_cmd));
    let _ = std::fs::File::create(&debug_resp)
        .and_then(|_| crate::platform::set_permissions_owner_only(&debug_resp));

    let mut cmd = tokio::process::Command::new(&binary_path);
    cmd.current_dir(cwd);
    cmd.env("JCODE_SELFDEV_MODE", "1");
    cmd.env(
        "JCODE_DEBUG_CMD_PATH",
        debug_cmd.to_string_lossy().to_string(),
    );
    cmd.env(
        "JCODE_DEBUG_RESPONSE_PATH",
        debug_resp.to_string_lossy().to_string(),
    );
    cmd.arg("--debug-socket");
    cmd.stdout(Stdio::from(stdout_file));
    cmd.stderr(Stdio::from(stderr_file));

    let child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);

    let info = serde_json::json!({
        "id": id,
        "pid": pid,
        "binary": binary_path.to_string_lossy(),
        "cwd": cwd,
        "debug_cmd_path": debug_cmd.to_string_lossy(),
        "debug_response_path": debug_resp.to_string_lossy(),
        "stdout_path": stdout_path.to_string_lossy(),
        "stderr_path": stderr_path.to_string_lossy(),
        "started_at": chrono::Utc::now().to_rfc3339(),
    });

    let mut testers = load_testers()?;
    testers.push(info);
    save_testers(&testers)?;

    Ok(serde_json::json!({
        "id": id,
        "pid": pid,
        "message": format!("Spawned tester {} (pid {})", id, pid)
    })
    .to_string())
}

async fn execute_tester_subcommand(
    tester_id: &str,
    cmd: &str,
    arg: Option<&str>,
) -> Result<String> {
    let testers = load_testers()?;
    let tester = testers
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tester_id))
        .ok_or_else(|| anyhow::anyhow!("Tester not found: {}", tester_id))?;

    let debug_cmd_path = tester
        .get("debug_cmd_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid tester config"))?;
    let debug_resp_path = tester
        .get("debug_response_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid tester config"))?;

    let file_cmd = match cmd {
        "frame" => "screen-json".to_string(),
        "frame-normalized" => "screen-json-normalized".to_string(),
        "state" => "state".to_string(),
        "history" => "history".to_string(),
        "wait" => "wait".to_string(),
        "input" => "input".to_string(),
        "message" => format!("message:{}", arg.unwrap_or("")),
        "inject" => format!("inject:{}", arg.unwrap_or("")),
        "keys" => format!("keys:{}", arg.unwrap_or("")),
        "set_input" => format!("set_input:{}", arg.unwrap_or("")),
        "scroll" => format!("scroll:{}", arg.unwrap_or("down")),
        "scroll-test" => match arg {
            Some(raw) => format!("scroll-test:{}", raw),
            None => "scroll-test".to_string(),
        },
        "scroll-suite" => match arg {
            Some(raw) => format!("scroll-suite:{}", raw),
            None => "scroll-suite".to_string(),
        },
        "stop" => {
            if let Some(pid) = tester.get("pid").and_then(|v| v.as_u64()) {
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .output();
            }
            let mut testers = load_testers()?;
            testers.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(tester_id));
            save_testers(&testers)?;
            return Ok("Stopped tester.".to_string());
        }
        _ => return Err(anyhow::anyhow!("Unknown tester command: {}", cmd)),
    };

    std::fs::write(debug_cmd_path, &file_cmd)?;

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Timeout waiting for tester response"));
        }
        if let Ok(response) = std::fs::read_to_string(debug_resp_path) {
            if !response.is_empty() {
                let _ = std::fs::remove_file(debug_resp_path);
                return Ok(response);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{load_testers, save_testers};
    use std::ffi::OsString;

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct TestHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_home: Option<OsString>,
        _temp_home: tempfile::TempDir,
    }

    impl TestHomeGuard {
        fn new() -> Self {
            let lock = lock_env();
            let temp_home = tempfile::Builder::new()
                .prefix("jcode-server-debug-testers-home-")
                .tempdir()
                .expect("create temp home");
            let prev_home = std::env::var_os("JCODE_HOME");
            std::env::set_var("JCODE_HOME", temp_home.path());
            Self {
                _lock: lock,
                prev_home,
                _temp_home: temp_home,
            }
        }
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            if let Some(prev_home) = &self.prev_home {
                std::env::set_var("JCODE_HOME", prev_home);
            } else {
                std::env::remove_var("JCODE_HOME");
            }
        }
    }

    #[test]
    fn load_and_save_testers_roundtrip_manifest() {
        let _guard = TestHomeGuard::new();
        let testers = vec![serde_json::json!({
            "id": "tester_1",
            "pid": 1234,
            "cwd": ".",
        })];

        save_testers(&testers).expect("save testers");
        let loaded = load_testers().expect("load testers");
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].get("id").and_then(|v| v.as_str()),
            Some("tester_1")
        );
    }

    #[test]
    fn load_testers_returns_empty_for_missing_or_empty_manifest() {
        let _guard = TestHomeGuard::new();
        assert!(load_testers()
            .expect("missing manifest returns empty")
            .is_empty());

        let manifest_path = crate::storage::jcode_dir()
            .expect("jcode dir")
            .join("testers.json");
        std::fs::write(&manifest_path, "").expect("write empty manifest");
        assert!(load_testers()
            .expect("empty manifest returns empty")
            .is_empty());
    }
}
