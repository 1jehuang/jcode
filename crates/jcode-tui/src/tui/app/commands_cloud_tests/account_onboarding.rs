use super::*;

struct SignedOut;

impl SignedOut {
    fn new() -> Self {
        TEST_CLOUD_ACCOUNT_MISSING.with(|missing| missing.set(true));
        Self
    }
}

impl Drop for SignedOut {
    fn drop(&mut self) {
        TEST_CLOUD_ACCOUNT_MISSING.with(|missing| missing.set(false));
    }
}

fn managed() -> CloudCommand {
    CloudCommand::Move { host: None }
}

#[test]
fn only_signed_out_managed_requests_need_an_account() {
    assert!(needs_cloud_account(&managed(), false, None, false));
    assert!(needs_cloud_account(&managed(), false, Some("  "), false));
    assert!(!needs_cloud_account(&managed(), false, None, true));
    // $JCODE_CLOUD_HOST replaces the managed default, so no account is needed.
    assert!(!needs_cloud_account(&managed(), false, Some("box"), false));
    // A cloud-attached TUI holds no local account key by design.
    assert!(!needs_cloud_account(&managed(), true, None, false));

    let own_host = CloudCommand::Move {
        host: Some("box".into()),
    };
    assert!(!needs_cloud_account(&own_host, false, None, false));
    // Naming the managed alias needs the account even with an env override.
    let explicit = CloudCommand::Replace {
        host: Some("jcode-cloud".into()),
        yes: false,
        dry_run: true,
    };
    assert!(needs_cloud_account(&explicit, false, Some("box"), false));

    for command in [
        CloudCommand::Return,
        CloudCommand::Where,
        CloudCommand::Cancel,
        CloudCommand::Reload { force: false },
    ] {
        assert!(
            !needs_cloud_account(&command, false, None, false),
            "{command:?}"
        );
    }
}

#[test]
fn signed_out_cloud_shows_setup_steps_and_queues_nothing() {
    let _signed_out = SignedOut::new();
    for input in ["/cloud", "/cloud --dry-run", "/cloud replace --dry-run"] {
        let mut app = crate::tui::app::tests::create_test_app();
        let session = super::super::super::commands::active_session_id(&app);
        assert!(handle_cloud_command(&mut app, input, &session));
        assert!(app.pending_cloud_handoff.is_none(), "{input}");
        assert!(app.cloud_job.is_none(), "{input}");
        assert!(app.cloud_recovery_context.is_none(), "{input}");
        assert!(!app.has_pending_cloud_recovery(), "{input}");
        let message = app.display_messages.last().unwrap();
        assert_eq!(message.role, "system", "{input}");
        assert!(message.content.contains("/login jcode"), "{input}");
        assert!(
            message
                .content
                .contains(crate::subscription_catalog::JCODE_PRICING_URL),
            "{input}"
        );
        assert!(message.content.contains("/cloud <ssh-alias>"), "{input}");
        assert!(message.content.contains("Nothing was moved."), "{input}");
    }
}

#[test]
fn signed_out_own_host_and_return_still_queue() {
    let _signed_out = SignedOut::new();
    for input in ["/cloud box", "/local"] {
        let mut app = crate::tui::app::tests::create_test_app();
        let session = super::super::super::commands::active_session_id(&app);
        assert!(handle_cloud_command(&mut app, input, &session));
        assert!(app.pending_cloud_handoff.is_some(), "{input}");
    }
}

#[test]
fn cli_account_failures_become_guidance_not_agent_diagnosis() {
    use crate::subscription_catalog as catalog;
    let cases = [
        (
            format!("Error: {}", catalog::cloud_account_required_message()),
            CloudAccountIssue::SignedOut,
            "create an account",
        ),
        (
            format!(
                "[cloud 1/3] waking\nError: {}",
                catalog::cloud_sign_in_expired_message()
            ),
            CloudAccountIssue::SignInExpired,
            "sign in again",
        ),
        (
            format!("Error: {}", catalog::cloud_not_entitled_message()),
            CloudAccountIssue::NotEntitled,
            "No new sign-in is needed",
        ),
    ];
    for (details, issue, expected) in cases {
        assert_eq!(cloud_account_issue(&details), Some(issue));
        let mut app = crate::tui::app::tests::create_test_app();
        let session = super::super::super::commands::active_session_id(&app);
        show_cloud_failure(&mut app, "/cloud", &session, Some(1), &details);
        assert!(app.cloud_recovery_context.is_none(), "{issue:?}");
        assert!(!app.has_pending_cloud_recovery(), "{issue:?}");
        let message = &app.display_messages.last().unwrap().content;
        assert!(message.contains(expected), "{issue:?}: {message}");
        assert!(!message.contains("The agent will investigate"), "{issue:?}");
    }
    assert_eq!(cloud_account_issue("Error: permission denied"), None);
}

#[test]
fn cloud_help_names_the_account_requirement() {
    let app = crate::tui::app::tests::create_test_app();
    for topic in ["cloud", "/cloud", "local"] {
        let help = app.command_help(topic).expect("cloud help should exist");
        assert!(help.contains("/login jcode"), "{topic}");
        assert!(help.contains("/cloud <ssh-alias>"), "{topic}");
    }
}
