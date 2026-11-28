use super::*;

struct TestHome {
    guards: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _sandbox: crate::auth::test_sandbox::AuthTestSandbox,
    active_override: Option<String>,
}
impl TestHome {
    fn new() -> Self {
        let active_override = crate::auth::claude::get_active_account_override();
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let mut guards = Vec::new();
        for (key, value) in [
            ("HOME", sandbox.root().to_path_buf()),
            ("JCODE_GLOBAL_HOME", sandbox.root().join("global")),
            ("CLAUDE_CONFIG_DIR", sandbox.root().join("claude")),
            ("XDG_CONFIG_HOME", sandbox.root().join("xdg")),
        ] {
            guards.push((key, std::env::var_os(key)));
            crate::env::set_var(key, value);
        }
        for (key, value) in [
            ("JCODE_TEST_SESSION", "1"),
            ("CLAUDE_CODE_OAUTH_TOKEN", ""),
            ("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", ""),
            ("JCODE_SSH_REMOTE", ""),
            ("JCODE_CLAUDE_LOGIN_METHOD", "cli"),
        ] {
            guards.push((key, std::env::var_os(key)));
            crate::env::set_var(key, value);
        }
        crate::env::remove_var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES");
        Self {
            guards,
            _sandbox: sandbox,
            active_override,
        }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        crate::auth::claude::set_active_account_override(self.active_override.take());
        for (key, value) in self.guards.drain(..) {
            match value {
                Some(v) => crate::env::set_var(key, v),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[test]
fn cli_default_enter_keeps_instance_environment_and_does_not_save_credentials() {
    let _env = TestHome::new();
    let mut app = crate::tui::app::tests::create_test_app();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime = runtime.enter();
    app.start_claude_login();
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::ClaudeMethodChoice)
    ));
    let prompt = &app.display_messages().last().unwrap().content;
    assert!(prompt.contains("Claude Code CLI (default)"));
    assert!(prompt.contains("Enter for 2"));
    app.pending_login = None;
    app.handle_login_input(PendingLogin::ClaudeMethodChoice, String::new());
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::ClaudeCli { .. })
    ));
    let help = &app.display_messages().last().unwrap().content;
    assert!(help.contains("--claude-code"));
    assert!(help.contains("--claude-code-completion"));
    assert!(help.contains("same environment"));
    assert!(!crate::auth::claude::jcode_path().unwrap().exists());
}

#[test]
fn explicit_oauth_preference_marks_its_default_without_starting_a_login() {
    let _env = TestHome::new();
    crate::env::set_var("JCODE_CLAUDE_LOGIN_METHOD", "oauth");
    let mut app = crate::tui::app::tests::create_test_app();
    app.start_claude_login();
    let prompt = &app.display_messages().last().unwrap().content;
    assert!(prompt.contains("Jcode OAuth (default)"));
    assert!(prompt.contains("Enter for 1"));
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::ClaudeMethodChoice)
    ));
}

#[test]
fn method_choice_rejects_unknown_input_and_cancels_without_saving_credentials() {
    let _env = TestHome::new();
    let mut app = crate::tui::app::tests::create_test_app();
    app.handle_login_input(PendingLogin::ClaudeMethodChoice, "unknown".into());
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::ClaudeMethodChoice)
    ));
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("Choose 1")
    );
    let pending = app.pending_login.take().unwrap();
    app.handle_login_input(pending, "/cancel".into());
    assert!(app.pending_login.is_none());
    assert!(!crate::auth::claude::jcode_path().unwrap().exists());
}

#[test]
fn cli_login_on_an_ssh_connection_stays_on_the_remote_host() {
    let _env = TestHome::new();
    crate::env::set_var("JCODE_SSH_REMOTE", "fixture-host");
    let mut app = crate::tui::app::tests::create_test_app();
    app.handle_login_input(PendingLogin::ClaudeMethodChoice, "cli".into());
    let help = &app.display_messages().last().unwrap().content;
    assert!(help.contains("remote host"));
    assert!(help.contains("--claude-code"));
    assert!(!crate::auth::claude::jcode_path().unwrap().exists());
}

#[path = "auth_claude_cli_wait_tests.rs"]
mod completion;
