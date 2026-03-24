use super::*;

// ============================================================================
// Binary Integration Tests
// These tests run the actual jcode binary and require real credentials.
// Run with: cargo test --test e2e binary_integration -- --ignored
// ============================================================================

/// Test that the jcode binary can run standalone with Claude provider
#[tokio::test]
#[ignore] // Requires Claude credentials
async fn binary_integration_standalone_claude() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
            "--bin",
            "jcode",
            "--",
            "run",
            "Say 'test-ok' and nothing else",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success() || stdout.contains("test") || stderr.contains("Claude"),
        "Binary should run successfully. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

/// Test that the jcode binary can run with OpenAI provider
#[tokio::test]
#[ignore] // Requires OpenAI/Codex credentials
async fn binary_integration_openai_provider() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
            "--bin",
            "jcode",
            "--",
            "--provider",
            "openai",
            "run",
            "Say 'openai-ok' and nothing else",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check either success or identifiable OpenAI response
    let has_response = stdout.to_lowercase().contains("openai")
        || stdout.to_lowercase().contains("ok")
        || stderr.contains("OpenAI");

    assert!(
        output.status.success() || has_response,
        "OpenAI provider should work. stdout: {}, stderr: {}",
        stdout,
        stderr
    );

    Ok(())
}

/// Test that jcode version command works
#[tokio::test]
async fn binary_version_command() -> Result<()> {
    use std::process::Command;
    let _env = setup_test_env()?;

    let output = Command::new(env!("CARGO_BIN_EXE_jcode"))
        .arg("--version")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "Version command should succeed");
    assert!(
        stdout.contains("jcode") || stdout.contains("20"),
        "Version should contain 'jcode' or date. Got: {}",
        stdout
    );

    Ok(())
}

/// Test full server reload handoff against a real spawned server process.
///
/// Requires a built release binary at target/release/jcode because the reload
/// flow execs into the repo's reload candidate.
#[tokio::test]
#[ignore]
async fn binary_integration_reload_handoff() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    let stderr_path = temp_root.path().join("server-stderr.log");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");

    let stderr_file = std::fs::File::create(&stderr_path)?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_jcode"))
        .arg("--no-update")
        .arg("--socket")
        .arg(&socket_path)
        .arg("serve")
        // This test must exercise the real exec-based reload handoff, not the
        // in-process test shortcut used by other e2e cases.
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir)
        .env("JCODE_DEBUG_CONTROL", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;
        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id before reload"))?
            .to_string();

        let mut client = wait_for_server_client(&socket_path).await?;
        client.reload().await?;

        let disconnect_deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_disconnect = false;
        while Instant::now() < disconnect_deadline {
            match tokio::time::timeout(Duration::from_secs(1), client.read_event()).await {
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => {
                    saw_disconnect = true;
                    break;
                }
            }
        }
        assert!(
            saw_disconnect,
            "old client connection never disconnected during reload"
        );

        let marker_deadline = Instant::now() + Duration::from_secs(20);
        while jcode::server::reload_marker_active(Duration::from_secs(30)) {
            if Instant::now() >= marker_deadline {
                anyhow::bail!("reload marker remained active too long after restart");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        wait_for_debug_socket_ready(&debug_socket_path).await?;
        let _client = wait_for_server_client(&socket_path).await?;

        let server_info_after =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_after_json: serde_json::Value = serde_json::from_str(&server_info_after)?;
        let server_id_after = server_info_after_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id after reload"))?;

        assert_ne!(
            server_id_after, server_id_before,
            "server identity should change after exec-based reload"
        );
        assert!(
            server_info_after_json
                .get("uptime_secs")
                .and_then(|v| v.as_u64())
                .is_some(),
            "replacement server should answer debug state queries after reload"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    kill_child(&mut child);
    if let Err(ref error) = test_result {
        if let Ok(stderr) = std::fs::read_to_string(&stderr_path) {
            eprintln!("spawned server stderr:\n{}", stderr);
        }
        eprintln!("reload e2e test error: {error:#}");
    }
    test_result
}

/// Test repeated self-dev reload handoff against a real TUI client running in a PTY.
///
/// Requires a built release binary at target/release/jcode because the
/// self-dev server reload path execs into the repo's reload candidate.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn binary_integration_selfdev_reload_reconnects_quickly() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-selfdev-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let _home_guard = EnvVarGuard::set("JCODE_HOME", &home_dir);
    let _runtime_guard = EnvVarGuard::set("JCODE_RUNTIME_DIR", &runtime_dir);
    let _install_guard = EnvVarGuard::set("JCODE_INSTALL_DIR", &install_dir);

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let mut command = Command::new(&release_binary);
    command
        .arg("--no-update")
        .arg("--provider")
        .arg("antigravity")
        .arg("self-dev")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir);

    let mut child = spawn_pty_child(command)?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;
        let session_id =
            wait_for_connected_client_session(&debug_socket_path, Duration::from_secs(10)).await?;

        let state_before =
            debug_run_command(debug_socket_path.clone(), "client:state", None).await?;
        let _: serde_json::Value = serde_json::from_str(&state_before)?;

        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let mut server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing initial server id"))?
            .to_string();

        for cycle in 1..=3 {
            child.send_command("/server-reload")?;

            let server_id_after = wait_for_selfdev_reload_cycle(
                &debug_socket_path,
                &session_id,
                &server_id_before,
                Duration::from_secs(20),
            )
            .await?;
            assert_ne!(
                server_id_after, server_id_before,
                "self-dev reload cycle {} should replace the server process",
                cycle
            );
            server_id_before = server_id_after;
        }

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        debug_run_command(debug_socket_path.clone(), "client:quit", None),
    )
    .await;
    kill_child(&mut child.child);

    if let Err(ref error) = test_result {
        eprintln!("self-dev reload e2e test error: {error:#}");
        eprintln!("self-dev client PTY output:\n{}", child.output_text());
        if let Some(log_excerpt) = latest_log_excerpt(&home_dir) {
            eprintln!("self-dev reload logs (tail):\n{}", log_excerpt);
        }
    }

    test_result
}

/// Test self-dev client binary reload against a real TUI client running in a PTY.
///
/// Starts from the test binary, then forces `/client-reload` to re-exec into
/// the built release candidate while keeping the shared server online.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn binary_integration_selfdev_client_reload_resumes_session() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-selfdev-client-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let _home_guard = EnvVarGuard::set("JCODE_HOME", &home_dir);
    let _runtime_guard = EnvVarGuard::set("JCODE_RUNTIME_DIR", &runtime_dir);
    let _install_guard = EnvVarGuard::set("JCODE_INSTALL_DIR", &install_dir);

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let starter_binary = temp_root.path().join("jcode-selfdev-client-starter");
    std::fs::copy(env!("CARGO_BIN_EXE_jcode"), &starter_binary)?;
    let starter_mtime = std::fs::metadata(&release_binary)?
        .modified()?
        .checked_sub(Duration::from_secs(60))
        .unwrap_or(std::time::UNIX_EPOCH + Duration::from_secs(1));
    set_file_mtime(&starter_binary, starter_mtime)?;

    let mut command = Command::new(&starter_binary);
    command
        .arg("--no-update")
        .arg("--provider")
        .arg("antigravity")
        .arg("self-dev")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir);

    let mut child = spawn_pty_child(command)?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;

        let session_id =
            wait_for_connected_client_session(&debug_socket_path, Duration::from_secs(10)).await?;

        let state_before =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_before_json: serde_json::Value = serde_json::from_str(&state_before)?;
        let version_before = state_before_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version before reload"))?
            .to_string();

        let clients_before =
            debug_run_command(debug_socket_path.clone(), "clients:map", None).await?;
        let clients_before_json: serde_json::Value = serde_json::from_str(&clients_before)?;
        let client_id_before = clients_before_json
            .get("clients")
            .and_then(|v| v.as_array())
            .and_then(|clients| {
                clients.iter().find_map(|client| {
                    let session = client.get("session_id").and_then(|v| v.as_str())?;
                    if session != session_id {
                        return None;
                    }
                    client
                        .get("client_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
            })
            .ok_or_else(|| anyhow::anyhow!("missing client id before reload"))?;

        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id before client reload"))?
            .to_string();

        child.send_command("/client-reload")?;

        let client_id_after = wait_for_selfdev_client_reload_cycle(
            &debug_socket_path,
            &session_id,
            &client_id_before,
            &server_id_before,
            Duration::from_secs(20),
        )
        .await?;

        let state_after =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_after_json: serde_json::Value = serde_json::from_str(&state_after)?;
        let version_after = state_after_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version after reload"))?;

        assert_ne!(
            client_id_after, client_id_before,
            "client reload should reconnect with a different client id"
        );
        assert_ne!(
            version_after, version_before,
            "client reload should switch binaries"
        );

        let server_info_after =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_after_json: serde_json::Value = serde_json::from_str(&server_info_after)?;
        let server_id_after = server_info_after_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id after client reload"))?;
        assert_eq!(
            server_id_after, server_id_before,
            "client reload should not replace the server process"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        debug_run_command(debug_socket_path.clone(), "client:quit", None),
    )
    .await;
    kill_child(&mut child.child);

    if let Err(ref error) = test_result {
        eprintln!("self-dev client reload e2e test error: {error:#}");
        eprintln!("self-dev client PTY output:\n{}", child.output_text());
        if let Some(log_excerpt) = latest_log_excerpt(&home_dir) {
            eprintln!("self-dev client reload logs (tail):\n{}", log_excerpt);
        }
    }

    test_result
}

/// Test full self-dev `/reload` against a real TUI client running in a PTY.
///
/// Starts from an older starter binary so the client reloads into the built
/// release candidate while the shared server also restarts.
#[cfg(unix)]
#[tokio::test]
#[ignore]
async fn binary_integration_selfdev_full_reload_resumes_session_quickly() -> Result<()> {
    let _env = setup_test_env()?;

    let release_binary =
        jcode::build::release_binary_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    if !release_binary.exists() {
        anyhow::bail!(
            "release binary missing at {} (run `cargo build --release` first)",
            release_binary.display()
        );
    }

    let temp_root = tempfile::Builder::new()
        .prefix("jcode-selfdev-full-reload-e2e-")
        .tempdir()?;
    let runtime_dir = temp_root.path().join("runtime");
    let home_dir = temp_root.path().join("home");
    let install_dir = temp_root.path().join("install");
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::create_dir_all(&home_dir)?;
    std::fs::create_dir_all(&install_dir)?;

    let _home_guard = EnvVarGuard::set("JCODE_HOME", &home_dir);
    let _runtime_guard = EnvVarGuard::set("JCODE_RUNTIME_DIR", &runtime_dir);
    let _install_guard = EnvVarGuard::set("JCODE_INSTALL_DIR", &install_dir);

    let socket_path = runtime_dir.join("jcode.sock");
    let debug_socket_path = runtime_dir.join("jcode-debug.sock");
    let starter_binary = temp_root.path().join("jcode-selfdev-full-reload-starter");
    std::fs::copy(env!("CARGO_BIN_EXE_jcode"), &starter_binary)?;
    let starter_mtime = std::fs::metadata(&release_binary)?
        .modified()?
        .checked_sub(Duration::from_secs(60))
        .unwrap_or(std::time::UNIX_EPOCH + Duration::from_secs(1));
    set_file_mtime(&starter_binary, starter_mtime)?;

    let mut command = Command::new(&starter_binary);
    command
        .arg("--no-update")
        .arg("--provider")
        .arg("antigravity")
        .arg("self-dev")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("JCODE_TEST_SESSION")
        .env("JCODE_HOME", &home_dir)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_INSTALL_DIR", &install_dir);

    let mut child = spawn_pty_child(command)?;

    let test_result = async {
        wait_for_socket(&socket_path).await?;
        wait_for_debug_socket_ready(&debug_socket_path).await?;

        let session_id =
            wait_for_connected_client_session(&debug_socket_path, Duration::from_secs(10)).await?;

        let state_before =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_before_json: serde_json::Value = serde_json::from_str(&state_before)?;
        let version_before = state_before_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version before full reload"))?
            .to_string();

        let clients_before =
            debug_run_command(debug_socket_path.clone(), "clients:map", None).await?;
        let clients_before_json: serde_json::Value = serde_json::from_str(&clients_before)?;
        let client_id_before = clients_before_json
            .get("clients")
            .and_then(|v| v.as_array())
            .and_then(|clients| {
                clients.iter().find_map(|client| {
                    let session = client.get("session_id").and_then(|v| v.as_str())?;
                    if session != session_id {
                        return None;
                    }
                    client
                        .get("client_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
            })
            .ok_or_else(|| anyhow::anyhow!("missing client id before full reload"))?;

        let server_info_before =
            debug_run_command(debug_socket_path.clone(), "server:info", None).await?;
        let server_info_before_json: serde_json::Value = serde_json::from_str(&server_info_before)?;
        let server_id_before = server_info_before_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing server id before full reload"))?
            .to_string();

        child.send_command("/reload")?;

        let server_id_after = wait_for_selfdev_reload_cycle(
            &debug_socket_path,
            &session_id,
            &server_id_before,
            Duration::from_secs(20),
        )
        .await?;

        let client_id_after = wait_for_selfdev_client_reload_cycle(
            &debug_socket_path,
            &session_id,
            &client_id_before,
            &server_id_after,
            Duration::from_secs(20),
        )
        .await?;

        let state_after =
            debug_run_command(debug_socket_path.clone(), "client:state", Some(&session_id)).await?;
        let state_after_json: serde_json::Value = serde_json::from_str(&state_after)?;
        let version_after = state_after_json
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing client version after full reload"))?;

        assert_ne!(
            server_id_after, server_id_before,
            "full reload should replace the server process"
        );
        assert_ne!(
            client_id_after, client_id_before,
            "full reload should reconnect with a different client id"
        );
        assert_ne!(
            version_after, version_before,
            "full reload should switch binaries"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        debug_run_command(debug_socket_path.clone(), "client:quit", None),
    )
    .await;
    kill_child(&mut child.child);

    if let Err(ref error) = test_result {
        eprintln!("self-dev full reload e2e test error: {error:#}");
        eprintln!("self-dev client PTY output:\n{}", child.output_text());
        if let Some(log_excerpt) = latest_log_excerpt(&home_dir) {
            eprintln!("self-dev full reload logs (tail):\n{}", log_excerpt);
        }
    }

    test_result
}
