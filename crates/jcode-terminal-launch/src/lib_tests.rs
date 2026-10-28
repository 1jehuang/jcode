use super::*;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn client_terminal_env_replaces_inherited_identity_and_exports_aliases() {
    let mut command = Command::new("hook");
    command.env("HERDR_PANE_ID", "stale-pane");
    command.env("TMUX_PANE", "stale-tmux");
    apply_client_terminal_env(
        &mut command,
        &[
            ("HERDR_PANE_ID".to_string(), "client-pane".to_string()),
            ("UNTRUSTED_CLIENT_VAR".to_string(), "ignored".to_string()),
        ],
    );
    let env = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(env["HERDR_PANE_ID"].as_deref(), Some("client-pane"));
    assert_eq!(
        env["JCODE_CLIENT_HERDR_PANE_ID"].as_deref(),
        Some("client-pane")
    );
    assert_eq!(env["TMUX_PANE"], None);
    assert!(!env.contains_key("UNTRUSTED_CLIENT_VAR"));
    assert!(!env.contains_key("JCODE_CLIENT_UNTRUSTED_CLIENT_VAR"));
}

#[test]
fn spawn_metadata_env_reexports_client_terminal_env_with_native_and_client_keys() {
    // A spawn carrying the requesting client's terminal env (#405) should
    // export each var both natively (overriding the spawned process's
    // inherited/stale value) and under a `JCODE_CLIENT_*` alias so hooks can
    // distinguish the client's terminal from the server's.
    let command = TerminalCommand::new("/usr/local/bin/jcode", vec!["--resume".to_string()])
        .kind("swarm-agent")
        .session_id("ses_405")
        .client_terminal_env(vec![
            ("ZELLIJ_SESSION_NAME".to_string(), "sessionB".to_string()),
            ("DISPLAY".to_string(), ":1".to_string()),
        ]);
    let env = spawn_metadata_env(&command, Path::new("/tmp/work"));

    let lookup = |key: &str| {
        env.iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .next_back()
    };

    assert_eq!(lookup("ZELLIJ_SESSION_NAME").as_deref(), Some("sessionB"));
    assert_eq!(
        lookup("JCODE_CLIENT_ZELLIJ_SESSION_NAME").as_deref(),
        Some("sessionB")
    );
    assert_eq!(lookup("DISPLAY").as_deref(), Some(":1"));
    assert_eq!(lookup("JCODE_CLIENT_DISPLAY").as_deref(), Some(":1"));
    // The first-class spawn metadata still flows through.
    assert_eq!(lookup("JCODE_SPAWN_KIND").as_deref(), Some("swarm-agent"));
}

#[test]
fn spawn_metadata_env_without_client_env_has_no_client_keys() {
    let command = TerminalCommand::new("/usr/local/bin/jcode", vec!["--resume".to_string()])
        .kind("resume")
        .session_id("ses_plain");
    let env = spawn_metadata_env(&command, Path::new("/tmp/work"));
    assert!(
        !env.iter().any(|(k, _)| k.starts_with("JCODE_CLIENT_")),
        "no client terminal env should produce no JCODE_CLIENT_* keys"
    );
}

#[test]
#[cfg(unix)]
fn snapshot_client_terminal_env_captures_set_vars_only() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("ZELLIJ_SESSION_NAME", "snapshot-test");
        std::env::remove_var("TMUX");
    }
    let snapshot = snapshot_client_terminal_env();
    assert!(
        snapshot
            .iter()
            .any(|(k, v)| k == "ZELLIJ_SESSION_NAME" && v == "snapshot-test")
    );
    assert!(!snapshot.iter().any(|(k, _)| k == "TMUX"));
    unsafe {
        std::env::remove_var("ZELLIJ_SESSION_NAME");
    }
}

#[test]
#[cfg(unix)]
fn detected_resume_terminal_recognizes_ghostty_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var("HANDTERM_SESSION");
        std::env::remove_var("HANDTERM_PID");
        std::env::remove_var("KITTY_PID");
        std::env::remove_var("WEZTERM_EXECUTABLE");
        std::env::remove_var("WEZTERM_PANE");
        std::env::remove_var("ALACRITTY_WINDOW_ID");
        std::env::set_var("GHOSTTY_RESOURCES_DIR", "/tmp/ghostty");
    }
    #[cfg(target_os = "macos")]
    assert_eq!(detected_resume_terminal().as_deref(), Some("ghostty"));
    unsafe {
        std::env::remove_var("GHOSTTY_RESOURCES_DIR");
    }
}

#[test]
#[cfg(unix)]
fn tmux_client_context_is_preferred_over_terminal_emulator() {
    let client_env = vec![
        (
            "TMUX".to_string(),
            "/tmp/tmux-1000/default,123,0".to_string(),
        ),
        ("TMUX_PANE".to_string(), "%42".to_string()),
        ("KITTY_PID".to_string(), "1234".to_string()),
    ];

    let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
    assert_eq!(candidates.first().map(String::as_str), Some("tmux"));
    assert_eq!(candidates.get(1).map(String::as_str), Some("kitty"));
}

#[test]
#[cfg(unix)]
fn inner_multiplexers_are_preferred_over_outer_tmux() {
    for (marker, value, expected) in [
        ("ZELLIJ", "0", "zellij"),
        ("STY", "1234.pts-1.host", "screen"),
    ] {
        let client_env = vec![
            (marker.to_string(), value.to_string()),
            ("ZELLIJ_PANE_ID".to_string(), "7".to_string()),
            ("TMUX".to_string(), "/tmp/tmux,1,0".to_string()),
            ("TMUX_PANE".to_string(), "%2".to_string()),
        ];
        let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
        assert_eq!(candidates.first().map(String::as_str), Some(expected));
        assert!(!candidates.iter().any(|candidate| candidate == "tmux"));
    }
}

#[test]
#[cfg(unix)]
fn term_program_detects_supported_emulators_cross_platform() {
    for (value, expected) in [
        ("ghostty", "ghostty"),
        ("kitty", "kitty"),
        ("WezTerm", "wezterm"),
        ("Alacritty", "alacritty"),
    ] {
        let env = vec![("TERM_PROGRAM".to_string(), value.to_string())];
        assert_eq!(
            detected_resume_terminal_with_client_env(&env).as_deref(),
            Some(expected)
        );
    }
}

#[test]
#[cfg(unix)]
fn explicit_terminal_override_stays_ahead_of_tmux() {
    let client_env = vec![
        (
            "TMUX".to_string(),
            "/tmp/tmux-1000/default,123,0".to_string(),
        ),
        ("TMUX_PANE".to_string(), "%42".to_string()),
    ];

    let candidates = resume_terminal_candidates_with_client_env(&client_env, Some("wezterm"));
    assert_eq!(candidates.first().map(String::as_str), Some("wezterm"));
    assert_eq!(candidates.get(1).map(String::as_str), Some("tmux"));
}

#[test]
#[cfg(unix)]
fn authoritative_non_tmux_client_context_ignores_server_tmux() {
    let client_env = vec![("TERM".to_string(), "xterm-256color".to_string())];

    let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
    assert!(!candidates.iter().any(|candidate| candidate == "tmux"));
}

#[test]
#[cfg(unix)]
fn tmux_context_without_current_pane_uses_emulator_fallbacks() {
    let client_env = vec![
        (
            "TMUX".to_string(),
            "/tmp/tmux-1000/default,123,0".to_string(),
        ),
        ("KITTY_PID".to_string(), "1234".to_string()),
    ];

    let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
    assert_eq!(candidates.first().map(String::as_str), Some("kitty"));
    assert!(!candidates.iter().any(|candidate| candidate == "tmux"));
}

#[test]
#[cfg(unix)]
fn herdr_context_is_preferred_over_outer_emulator_and_tmux() {
    let client_env = vec![
        ("HERDR_ENV".to_string(), "1".to_string()),
        ("HERDR_PANE_ID".to_string(), "w2:p7".to_string()),
        ("KITTY_PID".to_string(), "1234".to_string()),
        ("TMUX".to_string(), "/tmp/tmux,1,0".to_string()),
        ("TMUX_PANE".to_string(), "%3".to_string()),
    ];

    let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
    assert_eq!(candidates.first().map(String::as_str), Some("herdr"));
    assert!(!candidates.iter().any(|candidate| candidate == "tmux"));
}

#[test]
#[cfg(unix)]
fn missing_tmux_binary_falls_back_to_detected_emulator() {
    let _guard = ENV_LOCK.lock().unwrap();
    let previous_terminal = std::env::var_os("JCODE_TERMINAL");
    unsafe {
        std::env::remove_var("JCODE_TERMINAL");
    }
    let command = TerminalCommand::new("/usr/local/bin/jcode", vec![]).client_terminal_env(vec![
        (
            "TMUX".to_string(),
            "/tmp/tmux-1000/default,123,0".to_string(),
        ),
        ("TMUX_PANE".to_string(), "%42".to_string()),
        ("KITTY_PID".to_string(), "1234".to_string()),
    ]);
    let mut attempts = Vec::new();

    let result =
        spawn_command_in_new_terminal_with(&command, Path::new("/work/dir"), |candidate| {
            let program = candidate.get_program().to_string_lossy().into_owned();
            attempts.push(program.clone());
            if program == "tmux" {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            } else {
                Ok(())
            }
        });

    match previous_terminal {
        Some(value) => unsafe { std::env::set_var("JCODE_TERMINAL", value) },
        None => unsafe { std::env::remove_var("JCODE_TERMINAL") },
    }
    assert!(matches!(result, Ok(true)));
    assert_eq!(attempts, vec!["tmux", "kitty"]);
}

#[test]
#[cfg(unix)]
fn tmux_spawn_opens_right_split_in_requesting_pane() {
    let command = TerminalCommand::new(
        "/usr/local/bin/jcode",
        vec!["--resume".to_string(), "ses_tmux".to_string()],
    )
    .client_terminal_env(vec![
        (
            "TMUX".to_string(),
            "/tmp/tmux-1000/default,123,0".to_string(),
        ),
        ("TMUX_PANE".to_string(), "%42".to_string()),
    ]);

    let cmd = build_spawn_command("tmux", &command, Path::new("/work/dir"))
        .expect("tmux spawn command should build");
    assert_eq!(cmd.get_program().to_string_lossy(), "tmux");
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            "split-window",
            "-h",
            "-t",
            "%42",
            "-c",
            "/work/dir",
            "/usr/local/bin/jcode",
            "--resume",
            "ses_tmux",
        ]
    );
    assert_eq!(env_value(&cmd, "TMUX_PANE").as_deref(), Some("%42"));
}

#[test]
#[cfg(unix)]
fn zellij_spawn_opens_right_pane_with_resume_command() {
    let command = TerminalCommand::new(
        "/opt/jcode",
        vec!["--resume".to_string(), "ses-zellij".to_string()],
    );
    let cmd = build_spawn_command("zellij", &command, Path::new("/work/tree")).unwrap();
    let args: Vec<_> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "action",
            "new-pane",
            "--direction",
            "right",
            "--cwd",
            "/work/tree",
            "--",
            "/opt/jcode",
            "--resume",
            "ses-zellij",
        ]
    );
}

#[test]
#[cfg(unix)]
fn screen_spawn_opens_window_in_requesting_session() {
    let command = TerminalCommand::new(
        "/opt/Jcode App/jcode",
        vec!["--resume".to_string(), "ses screen".to_string()],
    )
    .client_terminal_env(vec![("STY".to_string(), "1234.pts-1.host".to_string())]);
    let cmd = build_spawn_command("screen", &command, Path::new("/work/a b")).unwrap();
    let args: Vec<_> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        &args[..6],
        ["-S", "1234.pts-1.host", "-X", "screen", "sh", "-lc"]
    );
    assert_eq!(
        args[6],
        "cd '/work/a b' && exec '/opt/Jcode App/jcode' '--resume' 'ses screen'"
    );
}

#[test]
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_ghostty_spawn_preserves_cwd_and_resume_command() {
    let command = TerminalCommand::new(
        "/opt/jcode",
        vec!["--resume".to_string(), "ses-ghostty".to_string()],
    );
    let cmd = build_spawn_command("ghostty", &command, Path::new("/work/tree")).unwrap();
    let args: Vec<_> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--working-directory=/work/tree",
            "-e",
            "/opt/jcode",
            "--resume",
            "ses-ghostty",
        ]
    );
}

#[test]
#[cfg(unix)]
fn herdr_spawn_splits_calling_pane_and_runs_resume_command() {
    let command = TerminalCommand::new(
        "/usr/local/bin/jcode",
        vec!["--resume".to_string(), "ses herdr".to_string()],
    )
    .client_terminal_env(vec![
        ("HERDR_ENV".to_string(), "1".to_string()),
        ("HERDR_PANE_ID".to_string(), "w2:p7".to_string()),
        (
            "HERDR_BIN_PATH".to_string(),
            "/opt/herdr/bin/herdr".to_string(),
        ),
    ]);

    let cmd = build_spawn_command("herdr", &command, Path::new("/work/a b"))
        .expect("herdr spawn command should build");
    assert_eq!(cmd.get_program().to_string_lossy(), "sh");
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "-c");
    assert!(args[1].contains("pane split --current --direction right"));
    assert!(args[1].contains("pane run \"$pane_id\""));
    assert_eq!(args[2], "jcode-herdr-spawn");
    assert_eq!(args[3], "/opt/herdr/bin/herdr");
    assert_eq!(args[4], "/work/a b");
    assert_eq!(
        args[5],
        "'/usr/local/bin/jcode' '--resume' 'ses herdr' && exit"
    );
}

#[test]
#[cfg(unix)]
fn herdr_spawn_adapter_executes_split_then_run_with_quoted_values() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!(
        "jcode-herdr-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temporary Herdr fixture");
    let herdr = dir.join("fake herdr");
    let log = dir.join("calls.log");
    let cwd = dir.join("work a b");
    std::fs::create_dir_all(&cwd).expect("temporary Herdr working directory");
    std::fs::write(
        &herdr,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = pane ] && [ \"$2\" = split ]; then\n  printf '%s\\n' '{{\"result\":{{\"pane\":{{\"pane_id\":\"w9:p12\"}}}}}}'\nfi\n",
            log.display()
        ),
    )
    .expect("write fake Herdr");
    let mut permissions = std::fs::metadata(&herdr).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&herdr, permissions).unwrap();

    let command = TerminalCommand::new(
        "/opt/Jcode App/jcode",
        vec!["--resume".to_string(), "session with spaces".to_string()],
    )
    .client_terminal_env(vec![
        ("HERDR_ENV".to_string(), "1".to_string()),
        ("HERDR_PANE_ID".to_string(), "w1:p1".to_string()),
        ("HERDR_BIN_PATH".to_string(), herdr.display().to_string()),
    ]);
    let mut process = build_spawn_command("herdr", &command, &cwd).expect("build Herdr adapter");
    assert!(process.status().expect("run Herdr adapter").success());

    let calls = std::fs::read_to_string(log).expect("read fake Herdr calls");
    let lines: Vec<&str> = calls.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        format!(
            "pane split --current --direction right --cwd {} --focus",
            cwd.display()
        )
    );
    assert_eq!(
        lines[1],
        "pane run w9:p12 '/opt/Jcode App/jcode' '--resume' 'session with spaces' && exit"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shell_command_quotes_arguments() {
    let shell = shell_command(&["jcode".to_string(), "it's ok".to_string()]);
    #[cfg(unix)]
    assert_eq!(shell, "'jcode' 'it'\"'\"'s ok'");
}

#[test]
#[cfg(not(unix))]
fn windows_candidates_end_with_cmd_fallback() {
    let candidates = resume_terminal_candidates();
    assert!(candidates.contains(&"alacritty".to_string()));
    assert!(candidates.contains(&"wt".to_string()));
    assert_eq!(candidates.last().map(String::as_str), Some("cmd"));
}

#[test]
#[cfg(not(unix))]
fn windows_cmd_fallback_runs_jcode_under_cmd_k() {
    let command = TerminalCommand::new(
        std::path::PathBuf::from(r"C:\Program Files\jcode\jcode.exe"),
        vec!["self-dev".to_string()],
    )
    .title("jcode");
    let cmd = build_spawn_command("cmd", &command, Path::new(r"C:\Users\me")).unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(&args[..5], ["/C", "start", "jcode", "cmd.exe", "/K"]);
    assert!(args[5].contains(r#""C:\Program Files\jcode\jcode.exe""#));
    assert!(args[5].contains("self-dev"));
}

#[test]
#[cfg(unix)]
fn macos_terminal_inner_script_runs_jcode() {
    let command = TerminalCommand::new(
        std::path::PathBuf::from("/usr/local/bin/jcode"),
        vec!["--resume".to_string(), "abc-123".to_string()],
    );
    let script = macos_terminal_inner_script(&command, Path::new("/work/dir"));
    assert_eq!(
        script,
        "cd '/work/dir' && exec '/usr/local/bin/jcode' '--resume' 'abc-123'"
    );
    // Must actually exec jcode, not the broken `open -a Terminal <file>` form.
    assert!(script.contains("exec '/usr/local/bin/jcode'"));
}

#[test]
#[cfg(unix)]
fn macos_terminal_inner_script_injects_fresh_spawn() {
    let command = TerminalCommand::new(std::path::PathBuf::from("/usr/local/bin/jcode"), vec![])
        .fresh_spawn();
    let script = macos_terminal_inner_script(&command, Path::new("/tmp"));
    assert_eq!(
        script,
        "cd '/tmp' && exec env JCODE_FRESH_SPAWN=1 '/usr/local/bin/jcode'"
    );
}

#[test]
#[cfg(unix)]
fn macos_terminal_applescript_uses_do_script() {
    let command = TerminalCommand::new(
        std::path::PathBuf::from("/usr/local/bin/jcode"),
        vec!["--resume".to_string(), "abc-123".to_string()],
    );
    let applescript = macos_terminal_applescript(&command, Path::new("/work/dir"));
    assert!(applescript.contains("tell application \"Terminal\""));
    assert!(applescript.contains("do script"));
    // The shell's single quotes survive; AppleScript only escapes \\ and ".
    assert!(!applescript.contains("exec \\\""));
    assert!(applescript.contains("'/usr/local/bin/jcode'"));
}

#[test]
fn macos_ghostty_applescript_creates_tab_and_runs_resume_command() {
    let command = TerminalCommand::new(
        "/Applications/jcode's build/jcode",
        vec!["--resume".to_string(), "session ghost".to_string()],
    );

    let applescript = macos_ghostty_applescript(&command, Path::new("/Users/test/work tree"));

    assert!(applescript.contains("tell application \"Ghostty\""));
    assert!(applescript.contains("new tab in front window with configuration cfg"));
    assert!(applescript.contains("select tab createdTab"));
    assert!(applescript.contains("new window with configuration cfg"));
    assert!(
        applescript.contains("set initial working directory of cfg to \"/Users/test/work tree\"")
    );
    assert!(applescript.contains("set command of cfg to \"/bin/bash -lc"));
    assert!(applescript.contains("--resume"));
    assert!(applescript.contains("session ghost"));
    assert!(applescript.contains("jcode'"));
}

#[test]
#[cfg(unix)]
fn macos_ghostty_wrapper_falls_back_when_applescript_fails() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!(
        "jcode-ghostty-fallback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("open.log");
    for (name, body) in [
        ("osascript", "#!/bin/sh\nexit 1\n".to_string()),
        (
            "open",
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
        ),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    let status = Command::new("/bin/sh")
        .args([
            "-c",
            macos_ghostty_spawn_wrapper(),
            "jcode-ghostty-test",
            "invalid applescript",
            "cd '/work tree' && exec '/opt/jcode' '--resume' 'ses ghost'",
        ])
        .env("PATH", &dir)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "-na\nGhostty\n--args\n-e\n/bin/bash\n-lc\ncd '/work tree' && exec '/opt/jcode' '--resume' 'ses ghost'\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[cfg(unix)]
fn preferred_terminal_is_tried_before_detected_emulators() {
    // The macOS Cmd+; listener passes the user's saved terminal; it must be
    // tried before whatever the environment or installed apps suggest (#1125).
    let _guard = ENV_LOCK.lock().unwrap();
    let previous_terminal = std::env::var_os("JCODE_TERMINAL");
    unsafe {
        std::env::remove_var("JCODE_TERMINAL");
    }
    let command = TerminalCommand::new("/usr/local/bin/jcode", vec![])
        .client_terminal_env(vec![("KITTY_PID".to_string(), "1234".to_string())])
        .preferred_terminal("wezterm");
    let mut attempts = Vec::new();

    let result =
        spawn_command_in_new_terminal_with(&command, Path::new("/work/dir"), |candidate| {
            attempts.push(candidate.get_program().to_string_lossy().into_owned());
            Ok(())
        });

    match previous_terminal {
        Some(value) => unsafe { std::env::set_var("JCODE_TERMINAL", value) },
        None => unsafe { std::env::remove_var("JCODE_TERMINAL") },
    }
    assert!(matches!(result, Ok(true)));
    assert_eq!(attempts, vec!["wezterm"]);
}

#[test]
fn macos_kitty_open_command_launches_app_bundle() {
    // kitty installed as an app is usually not on the LaunchAgent's PATH,
    // so it is opened through LaunchServices like the setup scripts do.
    let command = TerminalCommand::new("/opt/jcode", vec!["--resume".to_string()])
        .title("jc")
        .fresh_spawn();
    let cmd = macos_kitty_open_command(&command, Path::new("/work tree"));
    let args: Vec<String> = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(cmd.get_program(), "open");
    assert_eq!(
        args,
        vec![
            "-na",
            "kitty",
            "--args",
            "--title",
            "jc",
            "/bin/bash",
            "-lc",
            "cd '/work tree' && exec env JCODE_FRESH_SPAWN=1 '/opt/jcode' '--resume'",
        ]
    );
}

#[test]
#[cfg(target_os = "macos")]
fn missing_kitty_is_skipped_instead_of_opened() {
    // Without the app, the kitty arm must return None so the spawn loop moves
    // on; spawning `open -na kitty` would count as success. Only asserts on a
    // Mac without kitty installed or running.
    let _guard = ENV_LOCK.lock().unwrap();
    if program_on_path("kitty") || macos_should_try_app_terminal("kitty") {
        return;
    }
    let command = TerminalCommand::new("/opt/jcode", vec![]);
    assert!(build_spawn_command("kitty", &command, Path::new("/work")).is_none());
}

// Reproduction for issue #203 part 3: when no terminal emulator can be
// spawned, the new-terminal resume path returns Ok(false), which the app
// surfaces as "No terminal found. Resume manually:".
#[test]
fn no_terminal_available_returns_ok_false() {
    let command = TerminalCommand::new(
        std::path::PathBuf::from("/usr/local/bin/jcode"),
        vec!["--resume".to_string(), "abc-123".to_string()],
    );
    let result = spawn_command_in_new_terminal_with(&command, Path::new("/tmp"), |_cmd| {
        // Simulate every candidate terminal being absent.
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    });
    assert!(matches!(result, Ok(false)));
}

#[test]
fn parse_hook_command_splits_words_and_quotes() {
    assert_eq!(
        parse_hook_command("tmux new-window --").unwrap(),
        vec!["tmux", "new-window", "--"]
    );
    assert_eq!(
        parse_hook_command("my-hook --label 'two words'").unwrap(),
        vec!["my-hook", "--label", "two words"]
    );
    assert_eq!(
        parse_hook_command(r#"hook "a \"b\" c""#).unwrap(),
        vec!["hook", r#"a "b" c"#]
    );
}

#[test]
fn parse_hook_command_rejects_bad_input() {
    assert!(parse_hook_command("").is_err());
    assert!(parse_hook_command("   ").is_err());
    assert!(parse_hook_command("hook 'unterminated").is_err());
    assert!(parse_hook_command("hook trailing\\").is_err());
}

fn env_value(cmd: &Command, key: &str) -> Option<String> {
    cmd.get_envs().find_map(|(k, v)| {
        (k.to_string_lossy() == key).then(|| {
            v.map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    })
}

#[test]
fn hook_spawn_command_appends_program_args_and_exports_metadata() {
    let command = TerminalCommand::new(
        std::path::PathBuf::from("/usr/local/bin/jcode"),
        vec!["--resume".to_string(), "ses_abc".to_string()],
    )
    .title("🦊 jcode ses_abc")
    .kind("swarm-agent")
    .session_id("ses_abc")
    .spawn_env("JCODE_SPAWN_SWARM_ID", "swarm-1")
    .fresh_spawn();

    let cmd = build_hook_spawn_command("tmux-hook --flag", &command, Path::new("/work/dir"))
        .expect("hook command should build");

    assert_eq!(cmd.get_program().to_string_lossy(), "tmux-hook");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec!["--flag", "/usr/local/bin/jcode", "--resume", "ses_abc"]
    );
    assert_eq!(
        cmd.get_current_dir(),
        Some(Path::new("/work/dir")),
        "hook should run in the session working dir"
    );

    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_KIND").as_deref(),
        Some("swarm-agent")
    );
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_SESSION_ID").as_deref(),
        Some("ses_abc")
    );
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_TITLE").as_deref(),
        Some("🦊 jcode ses_abc")
    );
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_CWD").as_deref(),
        Some("/work/dir")
    );
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_PROGRAM").as_deref(),
        Some("/usr/local/bin/jcode")
    );
    #[cfg(unix)]
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_COMMAND").as_deref(),
        Some("'/usr/local/bin/jcode' '--resume' 'ses_abc'")
    );
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_SWARM_ID").as_deref(),
        Some("swarm-1")
    );
    assert_eq!(env_value(&cmd, "JCODE_FRESH_SPAWN").as_deref(), Some("1"));
}

#[test]
#[cfg(all(unix, not(target_os = "macos")))]
fn builtin_terminal_spawn_exports_metadata_env() {
    let command = TerminalCommand::new(
        std::path::PathBuf::from("/usr/local/bin/jcode"),
        vec!["--resume".to_string(), "ses_abc".to_string()],
    )
    .kind("resume")
    .session_id("ses_abc");

    let cmd = build_spawn_command("kitty", &command, Path::new("/work/dir"))
        .expect("kitty spawn command should build");
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_KIND").as_deref(),
        Some("resume")
    );
    assert_eq!(
        env_value(&cmd, "JCODE_SPAWN_SESSION_ID").as_deref(),
        Some("ses_abc")
    );
}
