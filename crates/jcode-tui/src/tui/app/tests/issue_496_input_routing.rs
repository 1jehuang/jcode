// Issue #496: input routing on interactive prompts.
//
// 1. `/cancel` typed at top level must never fall through to skill parsing
//    ("Unknown skill: /cancel"); it interrupts a running turn or reports that
//    nothing is in progress.
// 2. While an API-key prompt is pending, the command palette is suppressed
//    (only /cancel is suggested), because the composer is an answer box.
// 3. A short digit string like "1" typed at the API-key prompt is a menu
//    selection mistake, not a key; it must be rejected, not saved.

#[test]
fn test_cancel_command_idle_reports_nothing_to_cancel() {
    let mut app = create_test_app();

    app.set_input_for_test("/cancel");
    app.submit_input();

    let last = app
        .display_messages()
        .last()
        .expect("cancel should produce a message");
    assert_eq!(last.role, "system");
    assert!(
        last.content.contains("Nothing to cancel"),
        "expected nothing-to-cancel notice, got: {}",
        last.content
    );
    // Regression: this used to fall through to skill parsing.
    assert!(
        !last.content.contains("Unknown skill"),
        "'/cancel' must not be parsed as a skill: {}",
        last.content
    );
}

#[test]
fn test_cancel_command_processing_requests_interrupt() {
    let mut app = create_test_app();
    app.is_processing = true;

    app.set_input_for_test("/cancel");
    app.submit_input();

    assert!(app.cancel_requested, "processing turn must be interrupted");
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("Unknown skill")),
        "'/cancel' must not be parsed as a skill"
    );
}

#[test]
fn test_stop_command_processing_requests_interrupt() {
    let mut app = create_test_app();
    app.is_processing = true;

    app.set_input_for_test("/stop");
    app.submit_input();

    assert!(app.cancel_requested, "'/stop' must interrupt like /cancel");
}

fn pending_api_key_login() -> crate::tui::app::PendingLogin {
    crate::tui::app::PendingLogin::ApiKeyProfile {
        provider_id: "openrouter".to_string(),
        provider: "OpenRouter".to_string(),
        auth_method: "api_key".to_string(),
        docs_url: "https://openrouter.ai/keys".to_string(),
        env_file: "openrouter.env".to_string(),
        key_name: "OPENROUTER_API_KEY".to_string(),
        default_model: None,
        endpoint: None,
        api_key_optional: false,
        openai_compatible_profile: None,
    }
}

#[test]
fn test_command_palette_suppressed_while_api_key_prompt_pending() {
    let mut app = create_test_app();
    app.pending_login = Some(pending_api_key_login());

    // Typing a slash on the key prompt must not open the full palette.
    app.set_input_for_test("/");
    let suggestions = app.command_suggestions();
    assert_eq!(
        suggestions,
        vec![("/cancel".to_string(), "Cancel the pending prompt")],
        "only /cancel may be suggested while a login prompt is pending"
    );

    // Non-slash input (the key itself) gets no suggestions at all.
    app.set_input_for_test("sk-or-abc");
    assert!(app.command_suggestions().is_empty());

    // Prefixes of /cancel keep the single suggestion; other commands do not.
    app.set_input_for_test("/can");
    assert_eq!(app.command_suggestions().len(), 1);
    app.set_input_for_test("/model");
    assert!(app.command_suggestions().is_empty());
}

#[test]
fn test_menu_number_rejected_as_api_key() {
    let mut app = create_test_app();
    app.pending_login = Some(pending_api_key_login());

    app.set_input_for_test("1");
    app.submit_input();

    let last = app
        .display_messages()
        .last()
        .expect("menu-number input should produce an error message");
    assert_eq!(last.role, "error");
    assert!(
        last.content.contains("menu selection"),
        "expected menu-selection rejection, got: {}",
        last.content
    );
    // The prompt survives so the user can paste the real key.
    assert!(
        matches!(
            app.pending_login,
            Some(crate::tui::app::PendingLogin::ApiKeyProfile { .. })
        ),
        "API key prompt must remain pending after rejecting menu-number input"
    );
    // Nothing was persisted.
    assert!(std::env::var("OPENROUTER_API_KEY").map_or(true, |v| v != "1"));
}

#[test]
fn test_cancel_still_cancels_pending_api_key_prompt() {
    let mut app = create_test_app();
    app.pending_login = Some(pending_api_key_login());

    app.set_input_for_test("/cancel");
    app.submit_input();

    assert!(app.pending_login.is_none(), "/cancel must clear the prompt");
    let last = app.display_messages().last().expect("message expected");
    assert!(
        last.content.contains("Login cancelled"),
        "expected login-cancelled notice, got: {}",
        last.content
    );
}
