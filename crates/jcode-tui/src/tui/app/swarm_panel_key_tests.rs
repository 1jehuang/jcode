use super::{
    SwarmPanelAction, swarm_agent_shortcut_index, swarm_panel_action_for_key,
    swarm_popout_launch_context,
};
use crate::tui::TuiState as _;
use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn agent_badge_chords_decode_only_alt_digits() {
    for (index, digit) in "1234567890".chars().enumerate() {
        assert_eq!(
            swarm_agent_shortcut_index(KeyCode::Char(digit), KeyModifiers::ALT),
            Some(index)
        );
        for modifiers in [
            KeyModifiers::NONE,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT | KeyModifiers::CONTROL,
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ] {
            assert_eq!(
                swarm_agent_shortcut_index(KeyCode::Char(digit), modifiers),
                None
            );
        }
    }
    assert_eq!(
        swarm_agent_shortcut_index(KeyCode::Char('o'), KeyModifiers::ALT),
        None
    );
}

#[test]
fn direct_swarm_shortcuts_work_without_panel_focus_and_empty_swarm_is_safe() {
    let mut app = crate::tui::app::App::new_for_remote(None);
    assert!(!app.handle_swarm_agent_shortcut(KeyCode::Char('1'), KeyModifiers::ALT));
    assert!(!app.handle_swarm_agent_shortcut(KeyCode::Char('p'), KeyModifiers::NONE));
    assert!(
        !app.handle_swarm_agent_shortcut(
            KeyCode::Char('p'),
            KeyModifiers::ALT | KeyModifiers::SHIFT
        )
    );
    assert!(app.handle_swarm_agent_shortcut(KeyCode::Char('p'), KeyModifiers::ALT));
}

#[test]
fn popout_request_preserves_selected_session_and_uses_connected_socket_only_locally() {
    let socket = std::path::Path::new("/tmp/connected socket.sock");
    let context = swarm_popout_launch_context(false, socket);
    assert!(
        context
            .runtime_env
            .iter()
            .any(|(key, value)| { key == "JCODE_SOCKET" && value == "/tmp/connected socket.sock" })
    );

    let remote_context = swarm_popout_launch_context(true, socket);
    assert!(remote_context.runtime_env.is_empty());
}

#[test]
fn local_popout_uses_shared_launcher_command_but_ssh_remains_client_mode() {
    let exe = std::path::Path::new(r"C:\Program Files\jcode\jcode.exe");
    let context = jcode_app_core::session_launch::SessionSpawnContext::default()
        .with_runtime_env(vec![("JCODE_SOCKET".into(), r"C:\connected.sock".into())]);
    let local = super::swarm_agent_terminal_command(exe, "ses_local", &context);
    assert_eq!(local.program, exe);
    assert_eq!(local.args[0], "--fresh-spawn");
    assert_eq!(local.args[1], "--resume");
    assert_eq!(local.args[2], "ses_local");
    assert_eq!(&local.args[3..], ["--socket", r"C:\connected.sock"]);
    assert_eq!(local.session_id.as_deref(), Some("ses_local"));
    assert!(local.fresh_spawn);
    assert_eq!(
        local.runtime_env,
        [("JCODE_SOCKET".into(), r"C:\connected.sock".into())]
    );

    let mut selected = None;
    let result = super::spawn_swarm_agent_window_with_launchers(
        exe,
        "ses_local",
        std::path::Path::new(r"C:\work"),
        &context,
        false,
        |command, _cwd| {
            selected = Some(command.clone());
            Ok(true)
        },
        |_, _, _| panic!("local daemon popout must use the shared launcher"),
    )
    .unwrap();
    assert!(result);
    let selected = selected.unwrap();
    assert_eq!(selected.program, local.program);
    assert_eq!(selected.args, local.args);
    assert_eq!(selected.title, local.title);
    assert_eq!(selected.fresh_spawn, local.fresh_spawn);
    assert_eq!(selected.kind, local.kind);
    assert_eq!(selected.session_id, local.session_id);
    assert_eq!(selected.extra_env, local.extra_env);
    assert_eq!(selected.runtime_env, local.runtime_env);
    assert_eq!(selected.client_terminal_env, local.client_terminal_env);

    let mut ssh_called = false;
    super::spawn_swarm_agent_window_with_launchers(
        exe,
        "ses_remote",
        std::path::Path::new(r"C:\work"),
        &context,
        true,
        |_, _| panic!("SSH popout must keep its existing client launcher"),
        |_, session_id, _| {
            ssh_called = true;
            assert_eq!(session_id, "ses_remote");
            Ok(true)
        },
    )
    .unwrap();
    assert!(ssh_called);
}

#[test]
fn swarm_popout_daemon_client_preserves_runtime_and_ssh_stays_context_free() {
    const CHILD: &str = "JCODE_TEST_SWARM_POPOUT_CHILD";
    let scenario = std::env::var(CHILD).ok();
    if scenario.is_none() {
        let wrapper_home = tempfile::tempdir().expect("disposable wrapper home");
        let test_name = std::thread::current()
            .name()
            .expect("named test thread")
            .to_owned();
        for (scenario, ssh_host) in [("daemon", None), ("ssh", Some("remote.example"))] {
            let home = wrapper_home.path().join(scenario);
            let jcode_home = home.join(".jcode");
            let runtime = home.join("runtime");
            std::fs::create_dir_all(&jcode_home).expect("create disposable JCODE_HOME");
            std::fs::create_dir_all(&runtime).expect("create disposable runtime");
            let socket = runtime.join("connected.sock");
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", &test_name, "--nocapture"])
                .env_clear()
                .env(CHILD, scenario)
                .env("HOME", &home)
                .env("JCODE_HOME", &jcode_home)
                .env("JCODE_RUNTIME_DIR", &runtime)
                .env("JCODE_SOCKET", &socket);
            if let Some(ssh_host) = ssh_host {
                command.env("JCODE_SSH_REMOTE", ssh_host);
            }
            let output = command
                .output()
                .expect("run isolated pop-out regression child");
            assert!(
                output.status.success(),
                "isolated {scenario} pop-out regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains(&format!("SWARM_POPOUT_{scenario}_VERIFIED")),
                "isolated {scenario} assertions did not run:\n{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        return;
    }

    let scenario = scenario.unwrap();
    let ssh = scenario == "ssh";
    assert_eq!(crate::tui::is_ssh_remote(), ssh);

    let coordinator_id = "ses_popout_coordinator";
    let mut app =
        crate::tui::app::App::new_for_remote_with_options(Some(coordinator_id.to_string()), true);
    assert!(
        app.is_remote,
        "normal and SSH TUI clients are remote clients"
    );
    app.remote_session_id = Some(coordinator_id.to_string());
    app.swarm_enabled = true;
    let member = |id: &str, name: &str| crate::protocol::SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(name.to_string()),
        status: "running".to_string(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(1),
        output_tail: None,
        report_back_to_session_id: Some(coordinator_id.to_string()),
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    };
    app.remote_swarm_members = vec![
        member("ses_popout_agent_a", "Agent A"),
        member("ses_popout_agent_b", "Agent B"),
    ];
    app.swarm_panel_selected = 1;

    let displayed = app.inline_swarm_members();
    assert_eq!(
        displayed.len(),
        2,
        "fixture reaches the inline display filter"
    );
    assert_eq!(app.swarm_panel_selected(), 1);
    let display_order = crate::tui::info_widget::swarm_gallery::members_display_order(&displayed);
    let selected_id = display_order[app.swarm_panel_selected].clone();
    let selected_label = displayed
        .iter()
        .find(|member| member.session_id == selected_id)
        .and_then(|member| member.friendly_name.as_deref())
        .expect("selected display member label");

    let mut captured = None;
    app.pop_out_selected_swarm_agent_with_launcher(|exe, session_id, cwd, context| {
        captured = Some((
            exe.to_path_buf(),
            session_id.to_string(),
            cwd.to_path_buf(),
            context.clone(),
        ));
        Ok(true)
    });
    let (exe, session_id, cwd, context) = captured.expect("production launch request captured");
    assert_eq!(exe, super::launch_client_executable());
    assert_eq!(cwd, std::env::current_dir().unwrap());
    assert_eq!(
        session_id, selected_id,
        "pop-out uses selected display member"
    );
    assert_eq!(
        app.status_notice().as_deref(),
        Some(format!("Opened {selected_label} in a new window").as_str()),
        "success notice is constructed by the production pop-out path"
    );

    if ssh {
        assert!(context.kind.is_none());
        assert!(context.extra_env.is_empty());
        assert!(context.runtime_env.is_empty());
        assert!(context.client_terminal_env.is_empty());
        println!("SWARM_POPOUT_ssh_VERIFIED");
    } else {
        let runtime = std::env::var("JCODE_RUNTIME_DIR").unwrap();
        let home = std::env::var("JCODE_HOME").unwrap();
        let socket = std::env::var("JCODE_SOCKET").unwrap();
        let runtime_value = |key: &str| {
            context
                .runtime_env
                .iter()
                .find_map(|(name, value)| (name == key).then_some(value.as_str()))
                .unwrap_or_else(|| panic!("missing {key} from production launch context"))
        };
        assert_eq!(runtime_value("JCODE_HOME"), home);
        assert_eq!(runtime_value("JCODE_RUNTIME_DIR"), runtime);
        assert_eq!(runtime_value("JCODE_SOCKET"), socket);
        println!("SWARM_POPOUT_daemon_VERIFIED");
    }
}

/// Plain typing (letters, space, enter, arrows without alt) must pass
/// through so the user can keep writing into the chat input while the
/// panel is focused.
#[test]
fn plain_typing_is_not_captured() {
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Char('o'),
        KeyCode::Char('g'),
        KeyCode::Char('G'),
        KeyCode::Char(' '),
        KeyCode::Enter,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Backspace,
    ] {
        let mods = if code == KeyCode::Char('G') {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        assert_eq!(
            swarm_panel_action_for_key(code, mods),
            None,
            "{code:?} must pass through to the chat input"
        );
    }
}

#[test]
fn alt_chords_drive_the_panel() {
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Down, KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectNext)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Up, KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectPrev)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('j'), KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectNext)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('k'), KeyModifiers::ALT),
        Some(SwarmPanelAction::SelectPrev)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('o'), KeyModifiers::ALT),
        Some(SwarmPanelAction::PopOut)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Enter, KeyModifiers::ALT),
        Some(SwarmPanelAction::PopOut)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('P'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        Some(SwarmPanelAction::OpenPrompt)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Char('p'), KeyModifiers::ALT | KeyModifiers::SHIFT),
        Some(SwarmPanelAction::OpenPrompt)
    );
    assert_eq!(
        swarm_panel_action_for_key(KeyCode::Esc, KeyModifiers::NONE),
        Some(SwarmPanelAction::Exit)
    );
}

#[test]
fn ctrl_chords_pass_through() {
    for code in [KeyCode::Char('j'), KeyCode::Char('o'), KeyCode::Down] {
        assert_eq!(
            swarm_panel_action_for_key(code, KeyModifiers::CONTROL),
            None,
            "{code:?}+ctrl belongs to other handlers"
        );
    }
}
