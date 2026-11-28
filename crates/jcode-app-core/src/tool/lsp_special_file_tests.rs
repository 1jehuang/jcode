use super::*;

fn enabled_fixture(root: &Path) -> (PathBuf, PathBuf, ToolContext) {
    use jcode_base::plugin::{PluginManager, PluginScope};
    let home = root.join("home");
    let project = root.join("project");
    let market = root.join("market");
    let plugin = market.join("demo");
    for directory in [
        &project,
        &market.join(".claude-plugin"),
        &plugin.join(".claude-plugin"),
    ] {
        std::fs::create_dir_all(directory).unwrap();
    }
    crate::env::set_var("JCODE_HOME", &home);
    std::fs::write(
        market.join(".claude-plugin/marketplace.json"),
        json!({"name":"fixture", "plugins":[{"name":"demo","source":"./demo"}]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        json!({"name":"demo", "lspServers":{"rust":{
            "command":"lsp-special-file-fixture-must-not-start",
            "extensionToLanguage":{".rs":"rust"}
        }}})
        .to_string(),
    )
    .unwrap();
    std::fs::write(project.join("input.rs"), "fn fixture() {}\n").unwrap();
    let manager = PluginManager::new(home, project.clone());
    manager.add_marketplace(market.to_str().unwrap()).unwrap();
    manager.install("demo", PluginScope::Project).unwrap();
    let enabled = jcode_base::plugin::enabled_plugins_for_dir(&project);
    assert_eq!(enabled.len(), 1, "the .rs language server must be enabled");
    let installed = enabled[0].root.clone();
    let context = ToolContext {
        session_id: "lsp-special-file".into(),
        message_id: "message".into(),
        tool_call_id: "call".into(),
        working_dir: Some(project.clone()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::super::ToolExecutionMode::Direct,
    };
    (project, installed, context)
}

fn query_error(context: ToolContext) -> String {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime
        .block_on(
            LspTool::new().execute(json!({"operation":"hover","file_path":"input.rs"}), context),
        )
        .unwrap_err();
    format!("{error:#}")
}

#[cfg(unix)]
fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::mkfifo(path.as_ptr(), 0o600) },
        0,
        "mkfifo: {}",
        std::io::Error::last_os_error()
    );
}

#[cfg(unix)]
struct ReapChild(Option<std::process::Child>);

#[cfg(unix)]
impl Drop for ReapChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if let Err(error) = child.kill() {
                eprintln!("FIFO child kill during cleanup: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("FIFO child join during cleanup: {error}");
            }
        }
    }
}

#[cfg(unix)]
fn fifo_case_in_bounded_child(test: &str, config: bool) {
    const CHILD_CASE: &str = "JCODE_LSP_SPECIAL_FIFO_CASE";
    const CHILD_ROOT: &str = "JCODE_LSP_SPECIAL_FIFO_ROOT";
    if std::env::var(CHILD_CASE).as_deref() == Ok(test) {
        let _guard = crate::storage::lock_test_env();
        let _restore = Restore(vec![("JCODE_HOME", std::env::var_os("JCODE_HOME"))]);
        let root = PathBuf::from(std::env::var_os(CHILD_ROOT).unwrap());
        let (project, plugin, context) = enabled_fixture(&root);
        let path = if config {
            plugin.join(".lsp.json")
        } else {
            project.join("input.rs")
        };
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        make_fifo(&path);
        std::fs::write(root.join("ready"), "opening FIFO").unwrap();
        let error = query_error(context);
        assert!(
            error.contains("regular file"),
            "special-file rejection missing: {error}"
        );
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let output_path = root.path().join("child.log");
    let output = std::fs::File::create(&output_path).unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            format!("{module}::{test}"),
            "--exact".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
        ])
        .env(CHILD_CASE, test)
        .env(CHILD_ROOT, root.path())
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    let mut child = ReapChild(Some(command.spawn().unwrap()));
    let pid = child.0.as_ref().unwrap().id();
    let started = std::time::Instant::now();
    let mut ready_at = None;
    loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            child.0.as_mut().unwrap().wait().unwrap();
            child.0.take();
            eprintln!("LSP special-file child pid={pid} reaped=true status={status}");
            assert!(
                root.path().join("ready").exists(),
                "child did not reach the actual FIFO open"
            );
            assert!(
                status.success(),
                "FIFO child failed: {}",
                std::fs::read_to_string(output_path).unwrap()
            );
            return;
        }
        if ready_at.is_none() && root.path().join("ready").exists() {
            ready_at = Some(std::time::Instant::now());
        }
        let blocked = ready_at
            .is_some_and(|ready: std::time::Instant| ready.elapsed() > Duration::from_secs(2));
        if blocked || started.elapsed() > Duration::from_secs(10) {
            child.0.as_mut().unwrap().kill().unwrap();
            let status = child.0.as_mut().unwrap().wait().unwrap();
            child.0.take();
            eprintln!("LSP special-file child pid={pid} killed=true reaped=true status={status}");
            panic!(
                "LSP FIFO open did not return; isolated child was killed and joined (ready={})",
                ready_at.is_some()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn workspace_fifo_is_rejected_without_a_blocked_worker() {
    fifo_case_in_bounded_child("workspace_fifo_is_rejected_without_a_blocked_worker", false);
}

#[cfg(unix)]
#[test]
fn plugin_config_fifo_is_rejected_without_a_blocked_worker() {
    fifo_case_in_bounded_child(
        "plugin_config_fifo_is_rejected_without_a_blocked_worker",
        true,
    );
}

#[cfg(unix)]
fn bind_short_socket(path: &Path) -> std::os::unix::net::UnixListener {
    struct RestoreCwd(PathBuf);
    impl Drop for RestoreCwd {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.0).unwrap();
        }
    }
    let _restore = RestoreCwd(std::env::current_dir().unwrap());
    std::env::set_current_dir(path.parent().unwrap()).unwrap();
    // Unix socket names have a short length limit; the isolated proof root
    // can be much longer even though ordinary filesystem paths are valid.
    std::os::unix::net::UnixListener::bind(path.file_name().unwrap()).unwrap()
}

#[test]
fn workspace_directories_and_sockets_are_rejected_before_server_spawn() {
    let _guard = crate::storage::lock_test_env();
    let _restore = Restore(vec![("JCODE_HOME", std::env::var_os("JCODE_HOME"))]);
    let directory = tempfile::tempdir().unwrap();
    let (project, _, context) = enabled_fixture(directory.path());
    let file = project.join("input.rs");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(query_error(context.clone()).contains("regular file"));
    #[cfg(unix)]
    {
        std::fs::remove_dir(&file).unwrap();
        let _socket = bind_short_socket(&file);
        assert!(query_error(context).contains("regular file"));
    }
}

#[test]
fn config_directories_and_sockets_are_rejected_before_reading() {
    let _guard = crate::storage::lock_test_env();
    let directory = fixture_plugin(json!({"name":"fixture"}));
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("special.json");
    std::fs::create_dir(&path).unwrap();
    assert!(
        read_config_file(&root, "special.json")
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
    #[cfg(unix)]
    {
        std::fs::remove_dir(&path).unwrap();
        let _socket = bind_short_socket(&path);
        assert!(
            read_config_file(&root, "special.json")
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
    }
}

#[test]
fn regular_input_and_config_size_limits_remain_bounded() {
    let _guard = crate::storage::lock_test_env();
    let _restore = Restore(vec![("JCODE_HOME", std::env::var_os("JCODE_HOME"))]);
    let directory = tempfile::tempdir().unwrap();
    let (project, plugin, context) = enabled_fixture(directory.path());
    std::fs::write(project.join("input.rs"), vec![b'x'; MAX_MESSAGE / 2 + 1]).unwrap();
    assert!(query_error(context).contains("LSP file too large"));
    std::fs::write(plugin.join("large.json"), vec![b' '; MAX_MESSAGE + 1]).unwrap();
    assert!(
        read_config_file(&plugin, "large.json")
            .unwrap_err()
            .to_string()
            .contains("config too large")
    );
}
