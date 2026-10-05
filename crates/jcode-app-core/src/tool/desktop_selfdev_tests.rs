use super::*;

fn input(action: &str) -> Input {
    serde_json::from_value(json!({"action":action})).unwrap()
}

fn context(working_dir: Option<PathBuf>) -> ToolContext {
    ToolContext {
        session_id: "desktop-test".into(),
        message_id: "test".into(),
        tool_call_id: "test".into(),
        working_dir,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::AgentTurn,
    }
}

fn checkout() -> tempfile::TempDir {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    std::fs::create_dir_all(root.path().join("crates/jcode-desktop-ui/src")).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='jcode-desktop'\nversion='0.1.0'\n",
    )
    .unwrap();
    root
}

#[tokio::test]
async fn every_action_rejects_non_desktop_and_absent_context() {
    let root = tempfile::tempdir().unwrap();
    for action in [
        "status",
        "build",
        "reload",
        "build-reload",
        "test",
        "screenshot",
        "inspect",
    ] {
        for cwd in [None, Some(root.path().to_path_buf())] {
            let error = DesktopSelfDevTool::new()
                .execute(json!({"action":action}), context(cwd))
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("only inside a Jcode Desktop"),
                "{action}: {error}"
            );
        }
    }
}

#[test]
fn builds_both_desktop_packages_with_matching_profile() {
    for (profile, suffix) in [
        ("debug", vec![]),
        ("release", vec!["--release"]),
        ("selfdev", vec!["--profile", "selfdev"]),
    ] {
        let spec = command_spec(Path::new("/desktop"), &input("build"), profile, None).unwrap();
        assert_eq!(spec.program, "cargo");
        let mut expected = vec!["build", "-p", "jcode-desktop", "-p", "jcode-desktop-ui"];
        expected.extend(suffix);
        assert_eq!(spec.args, expected);
        assert!(!spec.args.iter().any(|a| a == "jcode" || a == "self-dev"));
    }
}

#[test]
fn reload_actions_never_route_to_cli_or_subprocess() {
    for action in ["reload", "build-reload"] {
        assert!(command_spec(Path::new("/desktop"), &input(action), "debug", None).is_err());
    }
}

#[test]
fn test_and_inspection_commands_are_scoped_and_read_only() {
    let root = Path::new("/desktop");
    let default = command_spec(root, &input("test"), "debug", None).unwrap();
    assert_eq!(default.program, "cargo");
    assert_eq!(default.args, ["test"]);
    let mut custom = input("test");
    custom.command = Some("cargo test -p jcode-desktop-ui".into());
    let spec = command_spec(root, &custom, "debug", None).unwrap();
    // The caller's command is passed verbatim: cmd.exe does not decode its
    // command line the way the C runtime does, and quoting the command would
    // corrupt any quotes inside it.
    #[cfg(windows)]
    {
        assert_eq!(spec.program, "cmd.exe");
        assert_eq!(spec.args, ["/D", "/S", "/C"]);
    }
    #[cfg(not(windows))]
    {
        assert_eq!(spec.program, "bash");
        assert_eq!(spec.args, ["-c"]);
    }
    assert_eq!(
        spec.raw.as_deref(),
        Some("cargo test -p jcode-desktop-ui"),
        "caller command must survive verbatim"
    );
    // An argv-style command needs no raw argument.
    assert_eq!(default.raw, None);
    let inspect = command_spec(root, &input("inspect"), "debug", Some(42)).unwrap();
    assert_eq!(
        inspect.args,
        ["scripts/preview-state.py", "--list", "--pid", "42"]
    );
    assert!(command_spec(root, &input("inspect"), "debug", None).is_err());
}

#[test]
fn screenshot_uses_private_script_with_fresh_build_and_target_output() {
    let root = checkout();
    let spec = command_spec(root.path(), &input("screenshot"), "release", None).unwrap();
    assert_eq!(spec.program, "python3");
    assert_eq!(
        spec.args,
        vec![
            "scripts/screenshot.py".to_string(),
            root.path()
                .join("target/desktop-selfdev.png")
                .display()
                .to_string()
        ]
    );
    assert!(!spec.args.contains(&"--no-build".into()));
    for path in ["/escape.png", "../escape.png", "x/../../escape.png", ""] {
        let mut params = input("screenshot");
        params.output = Some(path.into());
        assert!(command_spec(root.path(), &params, "debug", None).is_err());
    }
}

#[cfg(unix)]
#[test]
fn screenshot_rejects_symlink_destination() {
    let root = checkout();
    std::os::unix::fs::symlink(root.path(), root.path().join("target")).unwrap();
    assert!(command_spec(root.path(), &input("screenshot"), "debug", None).is_err());
}

// The symlink test above is Unix-only, so nothing covered the walk on Windows,
// where it failed outright. `checkout()` canonicalizes its tempdir, and on
// Windows that yields a verbatim "\?\C:\..." path. Pushing a Prefix
// component on its own yields the bare drive "\?\C:", which
// `symlink_metadata` rejects with ERROR_INVALID_FUNCTION ("Funcao incorreta.
// (os error 1)"), so every screenshot and custom test failed there.
#[test]
fn repo_walk_accepts_a_canonicalized_repo_root() {
    let root = checkout();
    assert!(
        reject_symlink_components(root.path()).is_ok(),
        "canonicalized repo root must be walkable"
    );
    // ...and a path whose leaf does not exist yet, as for a screenshot target.
    assert!(
        reject_symlink_components(&root.path().join("target/desktop.png")).is_ok(),
        "missing leaf must stay tolerated"
    );
    // The exemption is only for the prefix: every real component is still
    // checked. This is the property the walk exists to enforce.
    if let Ok(link) = symlink_dir(&root) {
        let error = reject_symlink_components(&link).unwrap_err().to_string();
        assert!(error.contains("symlink"), "{error}");
    }
}

/// Creates `root/target` as a symlink to `root` and returns the link path.
#[cfg(unix)]
fn symlink_dir(root: &tempfile::TempDir) -> std::io::Result<PathBuf> {
    let link = root.path().join("target");
    std::os::unix::fs::symlink(root.path(), &link)?;
    Ok(link)
}

/// Windows needs the symlink privilege, so "cannot create" means skipped.
#[cfg(windows)]
fn symlink_dir(root: &tempfile::TempDir) -> std::io::Result<PathBuf> {
    let link = root.path().join("target");
    std::os::windows::fs::symlink_dir(root.path(), &link)?;
    Ok(link)
}

#[test]
fn ambiguous_instances_and_foreign_executables_are_rejected() {
    let error = choose_instance(vec![
        ("main", "main.sock".into()),
        ("single-panel", "single.sock".into()),
    ])
    .unwrap_err()
    .to_string();
    assert!(error.contains("main, single-panel"), "{error}");
    assert_eq!(choose_instance(vec![]).unwrap(), None);
    assert_eq!(
        choose_instance(vec![("main", "main.sock".into())]).unwrap(),
        Some("main.sock".into())
    );
    assert!(select_instance(Some("../../arbitrary")).is_err());
    assert!(select_instance(Some("single-panel-123")).is_err());
    // The shared single-panel host is reachable by its fixed socket name.
    assert_eq!(
        instance_socket_name("single-panel"),
        "jcode-desktop-single-panel.sock"
    );
    assert_eq!(instance_socket_name("main"), "jcode-desktop.sock");
    assert_eq!(
        instance_socket_name("no-sidebar"),
        "jcode-desktop-no-sidebar.sock"
    );
    let root = Path::new("/desktop");
    assert_eq!(
        host_profile(root, Path::new("/desktop/target/release/jcode-desktop")).unwrap(),
        "release"
    );
    for exe in [
        "/other/target/debug/jcode-desktop",
        "/desktop/target/debug/jcode",
        "/desktop/target/debug/deps/jcode-desktop",
        "/desktop/target/../jcode-desktop",
    ] {
        assert!(host_profile(root, Path::new(exe)).is_err(), "{exe}");
    }
}

#[tokio::test]
async fn custom_test_executes_from_detected_repo_root() {
    let root = checkout();
    let output = DesktopSelfDevTool::new()
        .execute(
            json!({"action":"test", "command":"cd", "timeout_seconds":5}),
            context(Some(root.path().join("crates/jcode-desktop-ui/src"))),
        )
        .await
        .unwrap();
    let data: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(data["success"], true);
    // `cd` prints the cwd in both shells, unlike `pwd`, which is a bash builtin
    // only. The command runs in the Desktop repository, not in the directory
    // the tool was called from -- that is what this test pins.
    let expected = root.path().canonicalize().unwrap();
    let reported = data["stdout"].as_str().unwrap().trim();
    // Windows canonicalize yields a verbatim "\\?\C:\..." path, which
    // `cd` reports without the prefix, and drive letters are not
    // case-preserved. Compare the canonicalized pair.
    let reported = std::path::Path::new(reported);
    assert_eq!(
        reported.canonicalize().unwrap().to_str().unwrap(),
        expected.to_str().unwrap()
    );
}

#[tokio::test]
async fn invalid_timeout_and_action_fail_before_execution() {
    let root = checkout();
    for value in [
        json!({"action":"test", "timeout_seconds":0}),
        json!({"action":"test", "timeout_seconds":601}),
        json!({"action":"bogus"}),
        json!({"action":"build", "command":"echo should-not-run"}),
    ] {
        assert!(
            DesktopSelfDevTool::new()
                .execute(value, context(Some(root.path().to_path_buf())))
                .await
                .is_err()
        );
    }
}

// The caller's command is a shell command, and its own quoting has to reach
// the child intact. Measured: passing it through ordinary argument escaping
// turns `echo hi "quoted world"` into `hi \"quoted world\"`, because cmd.exe
// does not decode its command line the way the C runtime does. This is why
// CommandSpec has a raw argument. Every other test here sends a command
// without quotes, so without this one the raw argument could be dropped and
// nothing would fail.
#[tokio::test]
async fn custom_test_command_keeps_its_own_quoting() {
    let root = checkout();
    let output = DesktopSelfDevTool::new()
        .execute(
            json!({"action":"test", "command": "echo hi \"quoted world\"", "timeout_seconds":5}),
            context(Some(root.path().to_path_buf())),
        )
        .await
        .unwrap();
    let data: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(data["success"], true);
    assert_eq!(
        data["stdout"].as_str().unwrap().trim(),
        "hi \"quoted world\"",
        "the command's own quotes must survive the shell hop"
    );
}

#[tokio::test]
async fn command_timeout_is_bounded_and_failures_are_reported() {
    let root = checkout();
    let mut params = input("test");
    // `printf ... >&2` and `sleep` are bash-only spellings. Measured in
    // cmd.exe: `echo failure 1>&2 & exit 7` yields rc=7 with stderr
    // "failure", and `ping -n 3 127.0.0.1 > nul` blocks about two seconds.
    #[cfg(windows)]
    {
        params.command = Some("echo failure 1>&2 & exit 7".into());
    }
    #[cfg(not(windows))]
    {
        params.command = Some("printf failure >&2; exit 7".into());
    }
    let spec = command_spec(root.path(), &params, "debug", None).unwrap();
    let output = run_command(root.path(), spec, 5).await.unwrap();
    let data: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(data["success"], false);
    assert_eq!(data["exit_code"], 7);
    // `echo` in cmd.exe pads the redirected line ("failure  "), while bash's
    // `printf` does not, so compare on content rather than byte-exact bytes.
    assert_eq!(
        data["stderr"].as_str().unwrap().trim(),
        "failure",
        "stderr must carry the command's own output"
    );
    #[cfg(windows)]
    {
        params.command = Some("ping -n 3 127.0.0.1 > nul".into());
    }
    #[cfg(not(windows))]
    {
        params.command = Some("sleep 30".into());
    }
    let spec = command_spec(root.path(), &params, "debug", None).unwrap();
    let start = std::time::Instant::now();
    assert!(
        run_command(root.path(), spec, 1)
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn process_output_is_bounded() {
    let data = vec![b'x'; OUTPUT_LIMIT * 2];
    let output = bounded_output(data.as_slice()).await.unwrap();
    assert!(output.len() < OUTPUT_LIMIT + 100);
    assert!(output.ends_with("[output truncated after 64 KiB]"));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn reload_sends_only_r_and_requires_real_acknowledgement() {
    use tokio::io::AsyncWriteExt;
    for reply in [b"ok\n", b"bad"] {
        let (client, mut server) = tokio::net::UnixStream::pair().unwrap();
        let mut host = Host {
            path: "test.sock".into(),
            pid: 1,
            profile: "debug".into(),
            stream: client,
        };
        let receiver = tokio::spawn(async move {
            let mut request = [0];
            server.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"R");
            server.write_all(reply).await.unwrap();
        });
        assert_eq!(host.reload().await.is_ok(), reply == b"ok\n");
        receiver.await.unwrap();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn unsafe_sockets_are_rejected_before_connecting() {
    use std::os::unix::fs::PermissionsExt;
    let root = checkout();
    let path = root.path().join("host.sock");
    std::fs::write(&path, "not a socket").unwrap();
    assert!(connect_host(root.path(), &path).await.is_err());
    std::fs::remove_file(&path).unwrap();
    let _listener = tokio::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(connect_host(root.path(), &path).await.is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    // Even a private socket is insufficient: this test process is not the Desktop host.
    assert!(connect_host(root.path(), &path).await.is_err());
}

#[tokio::test]
async fn screenshot_execution_never_discovers_live_instances() {
    let root = checkout();
    std::fs::create_dir(root.path().join("scripts")).unwrap();
    std::fs::write(
        root.path().join("scripts/screenshot.py"),
        "import json,sys\nprint(json.dumps(sys.argv[1:]))\n",
    )
    .unwrap();
    // An invalid instance would fail discovery. Offline screenshot must not consult it.
    let output = DesktopSelfDevTool::new()
        .execute(
            json!({"action":"screenshot", "instance":"not-a-live-instance", "timeout_seconds":5}),
            context(Some(root.path().to_path_buf())),
        )
        .await
        .unwrap();
    let data: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(data["success"], true);
    let argv: Vec<String> = serde_json::from_str(data["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(
        argv,
        [root
            .path()
            .join("target/desktop-selfdev.png")
            .display()
            .to_string()]
    );
}
