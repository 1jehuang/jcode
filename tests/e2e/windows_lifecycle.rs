use crate::test_support::*;

struct SpawnedWindowsServer {
    _temp_root: tempfile::TempDir,
    home_dir: std::path::PathBuf,
    runtime_dir: std::path::PathBuf,
    install_dir: std::path::PathBuf,
    socket_path: std::path::PathBuf,
    debug_socket_path: std::path::PathBuf,
    stdout_path: std::path::PathBuf,
    stderr_path: std::path::PathBuf,
    child: Child,
}

impl SpawnedWindowsServer {
    fn spawn(prefix: &str) -> Result<Self> {
        let temp_root = tempfile::Builder::new().prefix(prefix).tempdir()?;
        let home_dir = temp_root.path().join("home");
        let runtime_dir = temp_root.path().join("runtime");
        let install_dir = temp_root.path().join("install");
        let stdout_path = temp_root.path().join("server-stdout.log");
        let stderr_path = temp_root.path().join("server-stderr.log");
        std::fs::create_dir_all(&home_dir)?;
        std::fs::create_dir_all(&runtime_dir)?;
        std::fs::create_dir_all(&install_dir)?;

        let socket_path = runtime_dir.join("jcode-windows-lifecycle.sock");
        let debug_socket_path = runtime_dir.join("jcode-windows-lifecycle-debug.sock");

        let stdout_file = std::fs::File::create(&stdout_path)?;
        let stderr_file = std::fs::File::create(&stderr_path)?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_jcode"));
        command
            .arg("--no-update")
            .arg("--socket")
            .arg(&socket_path)
            .arg("--provider")
            .arg("openai-compatible")
            .arg("--model")
            .arg("windows-e2e-model")
            .arg("serve")
            .env_remove("JCODE_TEST_SESSION")
            .env("JCODE_HOME", &home_dir)
            .env("JCODE_RUNTIME_DIR", &runtime_dir)
            .env("JCODE_INSTALL_DIR", &install_dir)
            .env("JCODE_OPENAI_COMPAT_API_BASE", "http://127.0.0.1:9/v1")
            .env("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "windows-e2e-model")
            .env("JCODE_OPENAI_COMPAT_LOCAL_ENABLED", "1")
            .env("JCODE_DEBUG_CONTROL", "1")
            .env("RUST_BACKTRACE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        let child = command.spawn()?;

        Ok(Self {
            _temp_root: temp_root,
            home_dir,
            runtime_dir,
            install_dir,
            socket_path,
            debug_socket_path,
            stdout_path,
            stderr_path,
            child,
        })
    }

    async fn wait_ready(&self) -> Result<()> {
        wait_for_server_ready(&self.socket_path, &self.debug_socket_path).await
    }

    fn apply_env<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        command
            .env_remove("JCODE_TEST_SESSION")
            .env("JCODE_HOME", &self.home_dir)
            .env("JCODE_RUNTIME_DIR", &self.runtime_dir)
            .env("JCODE_INSTALL_DIR", &self.install_dir)
            .env("JCODE_OPENAI_COMPAT_API_BASE", "http://127.0.0.1:9/v1")
            .env("JCODE_OPENAI_COMPAT_DEFAULT_MODEL", "windows-e2e-model")
            .env("JCODE_OPENAI_COMPAT_LOCAL_ENABLED", "1")
            .env("JCODE_DEBUG_CONTROL", "1")
            .env("RUST_BACKTRACE", "1")
    }

    fn jcode_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jcode"));
        self.apply_env(&mut command);
        command
    }

    fn spawn_same_socket_child(
        &self,
        label: &str,
    ) -> Result<(Child, std::path::PathBuf, std::path::PathBuf)> {
        let stdout_path = self._temp_root.path().join(format!("{label}-stdout.log"));
        let stderr_path = self._temp_root.path().join(format!("{label}-stderr.log"));
        let stdout_file = std::fs::File::create(&stdout_path)?;
        let stderr_file = std::fs::File::create(&stderr_path)?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_jcode"));
        self.apply_env(&mut command)
            .arg("--no-update")
            .arg("--socket")
            .arg(&self.socket_path)
            .arg("--provider")
            .arg("openai-compatible")
            .arg("--model")
            .arg("windows-e2e-model")
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));
        let child = command.spawn()?;
        Ok((child, stdout_path, stderr_path))
    }

    fn dump_extra_logs(
        &self,
        label: &str,
        stdout_path: &std::path::Path,
        stderr_path: &std::path::Path,
    ) {
        eprintln!("=== {label}: extra process diagnostics ===");
        for (name, path) in [
            ("server stdout", stdout_path),
            ("server stderr", stderr_path),
        ] {
            match std::fs::read_to_string(path) {
                Ok(contents) if !contents.trim().is_empty() => {
                    eprintln!("--- {name} ({}) ---\n{contents}", path.display());
                }
                Ok(_) => eprintln!("--- {name} ({}) was empty ---", path.display()),
                Err(err) => eprintln!("--- could not read {name} at {}: {err} ---", path.display()),
            }
        }
    }

    fn dump_logs(&self, label: &str) {
        eprintln!("=== {label}: windows lifecycle server diagnostics ===");
        for (name, path) in [
            ("server stdout", &self.stdout_path),
            ("server stderr", &self.stderr_path),
        ] {
            match std::fs::read_to_string(path) {
                Ok(contents) if !contents.trim().is_empty() => {
                    eprintln!("--- {name} ({}) ---\n{contents}", path.display());
                }
                Ok(_) => eprintln!("--- {name} ({}) was empty ---", path.display()),
                Err(err) => eprintln!("--- could not read {name} at {}: {err} ---", path.display()),
            }
        }

        if let Ok(artifact_root) = std::env::var("JCODE_E2E_ARTIFACT_DIR") {
            let safe_label: String = label
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
                .collect();
            let artifact_dir = std::path::PathBuf::from(artifact_root).join(safe_label);
            let _ = std::fs::create_dir_all(&artifact_dir);
            let _ = std::fs::copy(&self.stdout_path, artifact_dir.join("server-stdout.log"));
            let _ = std::fs::copy(&self.stderr_path, artifact_dir.join("server-stderr.log"));
            let logs_dir = self.home_dir.join("logs");
            if let Ok(entries) = std::fs::read_dir(logs_dir) {
                let copied_logs_dir = artifact_dir.join("jcode-logs");
                let _ = std::fs::create_dir_all(&copied_logs_dir);
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let _ = std::fs::copy(path, copied_logs_dir.join(entry.file_name()));
                    }
                }
            }
        }
    }
}

impl Drop for SpawnedWindowsServer {
    fn drop(&mut self) {
        kill_child(&mut self.child);
    }
}

async fn wait_for_server_unreachable(socket_path: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let connect_result = tokio::time::timeout(
            Duration::from_secs(2),
            server::Client::connect_with_path(socket_path.to_path_buf()),
        )
        .await;
        match connect_result {
            Err(_) if Instant::now() >= deadline => {
                anyhow::bail!(
                    "timed out waiting for server at {} to become unreachable after process exit",
                    socket_path.display()
                );
            }
            Err(_) => {}
            Ok(Err(_)) => return Ok(()),
            Ok(Ok(mut client)) => {
                if client.ping().await.unwrap_or(false) {
                    if Instant::now() >= deadline {
                        anyhow::bail!(
                            "server at {} remained reachable after process exit",
                            socket_path.display()
                        );
                    }
                } else {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn windows_binary_server_accepts_clients_and_debug_cli() -> Result<()> {
    let _env = setup_test_env()?;
    let server = SpawnedWindowsServer::spawn("jcode-windows-lifecycle-")?;

    let result = async {
        server.wait_ready().await?;

        let mut client_a = wait_for_server_client(&server.socket_path).await?;
        anyhow::ensure!(client_a.ping().await?, "first protocol client should ping");

        let mut client_b = wait_for_server_client(&server.socket_path).await?;
        anyhow::ensure!(client_b.ping().await?, "second protocol client should ping");

        let info = debug_run_command_json(server.debug_socket_path.clone(), "server:info", None).await?;
        anyhow::ensure!(
            info.get("id").and_then(|value| value.as_str()).is_some(),
            "server:info should include an id: {info}"
        );
        anyhow::ensure!(
            info.get("debug_control_enabled")
                .and_then(|value| value.as_bool())
                == Some(true),
            "server should honor JCODE_DEBUG_CONTROL in Windows e2e"
        );

        let output = server
            .jcode_command()
            .arg("--no-update")
            .arg("--socket")
            .arg(&server.socket_path)
            .arg("debug")
            .arg("server:info")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::ensure!(
            output.status.success(),
            "debug CLI should round-trip to spawned Windows server. stdout: {stdout}; stderr: {stderr}"
        );
        let cli_info: serde_json::Value = serde_json::from_str(stdout.trim())?;
        anyhow::ensure!(
            cli_info.get("id") == info.get("id"),
            "debug CLI should target the same server as the protocol client"
        );

        Ok::<_, anyhow::Error>(())
    }
    .await;

    if result.is_err() {
        server.dump_logs("binary-server-accepts-clients-and-debug-cli");
    }
    result
}

#[tokio::test]
async fn windows_binary_server_rebinds_named_pipe_after_exit() -> Result<()> {
    let _env = setup_test_env()?;
    let mut first = SpawnedWindowsServer::spawn("jcode-windows-rebind-")?;

    let result = async {
        first.wait_ready().await?;
        let socket_path = first.socket_path.clone();
        let debug_socket_path = first.debug_socket_path.clone();
        kill_child(&mut first.child);
        wait_for_server_unreachable(&socket_path).await?;

        let (mut second_child, second_stdout, second_stderr) =
            first.spawn_same_socket_child("second-server")?;
        let second_result = async {
            wait_for_server_ready(&socket_path, &debug_socket_path).await?;
            let mut client = wait_for_server_client(&socket_path).await?;
            anyhow::ensure!(client.ping().await?, "rebound server should answer ping");
            Ok::<_, anyhow::Error>(())
        }
        .await;
        kill_child(&mut second_child);
        if second_result.is_err() {
            first.dump_extra_logs(
                "binary-server-rebind-second-server",
                &second_stdout,
                &second_stderr,
            );
        }
        second_result
    }
    .await;

    if result.is_err() {
        first.dump_logs("binary-server-rebind-first-server");
    }
    result
}
