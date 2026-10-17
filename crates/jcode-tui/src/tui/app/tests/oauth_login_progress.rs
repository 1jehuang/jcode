#[test]
fn oauth_login_progress_disclosure_and_success_preserve_history() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::system("Keep unrelated history"));
    let original = app.display_messages.len();
    app.handle_debug_command("login-progress-fixture");
    let index = app.display_messages.len() - 1;
    assert!(app.login_input_hint().unwrap().contains("not sent to AI"));
    assert!(
        !jcode_tui_messages::parse_collapsible_swarm_content(&app.display_messages[index].content)
            .unwrap()
            .expanded
    );
    assert!(app.toggle_swarm_message_expand(index));
    assert!(
        jcode_tui_messages::parse_collapsible_swarm_content(&app.display_messages[index].content)
            .unwrap()
            .expanded
    );
    assert!(app.toggle_swarm_message_expand(index));
    app.handle_debug_command("login-progress-exchange-fixture");
    assert!(app.login_input_hint().unwrap().contains("exchanging"));
    assert!(!app.display_messages[index].content.contains("https://"));
    app.handle_debug_command("login-progress-success-fixture");
    assert_eq!(app.display_messages.len(), original + 1);
    assert_eq!(
        app.display_messages[original - 1].content,
        "Keep unrelated history"
    );
    assert!(
        app.display_messages[index]
            .content
            .contains("login complete")
    );
    assert!(app.display_messages[index].title.is_none());
    assert!(app.pending_login.is_none());
    assert!(app.login_input_hint().is_none());
    assert!(!app.toggle_swarm_message_expand(index));
}

#[test]
fn oauth_login_progress_cancel_removes_expanded_url_and_button() {
    let mut app = create_test_app();
    app.handle_debug_command("login-progress-fixture");
    let index = app.display_messages.len() - 1;
    app.toggle_swarm_message_expand(index);
    app.set_input_for_test("/cancel");
    app.submit_input();
    assert_eq!(
        app.display_messages[index].content,
        "OpenAI login cancelled."
    );
    assert!(app.display_messages[index].title.is_none());
    assert!(app.pending_login.is_none());
    assert!(!app.toggle_swarm_message_expand(index));
}

#[test]
fn oauth_login_progress_exchange_input_never_becomes_ai_prompt() {
    let mut app = create_test_app();
    app.handle_debug_command("login-progress-fixture");
    app.handle_debug_command("login-progress-exchange-fixture");
    let before = app.display_messages.len();
    let session_before = app.session.messages.len();
    app.set_input_for_test("http://localhost:1455/auth/callback?code=secret&state=secret");
    app.submit_input();
    assert_eq!(app.display_messages.len(), before);
    assert_eq!(app.session.messages.len(), session_before);
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::Exchanging { .. })
    ));
    assert!(!app.is_processing);
    app.set_input_for_test("/cancel");
    app.submit_input();
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::Exchanging { .. })
    ));
}

#[test]
fn oauth_login_progress_failure_replaces_card_and_clears_typed_callback() {
    let mut app = create_test_app();
    app.handle_debug_command("login-progress-fixture");
    let index = app.display_messages.len() - 1;
    app.toggle_swarm_message_expand(index);
    app.set_input_for_test("http://localhost:1455/auth/callback?code=secret");
    app.handle_login_completed(LoginCompleted {
        provider: "openai".to_string(),
        success: false,
        message: "Authorization denied".to_string(),
    });
    assert!(app.input.is_empty());
    assert!(app.pending_login.is_none());
    assert!(
        app.display_messages[index]
            .content
            .contains("Authorization denied")
    );
    assert!(
        app.display_messages[index]
            .content
            .contains("Retry /login openai")
    );
    assert!(!app.display_messages[index].content.contains("https://"));
    assert!(app.display_messages[index].title.is_none());
}

#[test]
fn oauth_login_progress_enter_shortcuts_cannot_queue_or_launch_credentials() {
    for alternate in [false, true] {
        for queue in [false, true] {
            let mut app = create_test_app();
            app.handle_debug_command("login-progress-fixture");
            app.handle_debug_command("login-progress-exchange-fixture");
            app.is_processing = true;
            app.queue_mode = queue;
            app.route_next_prompt_to_new_session = true;
            let before = app.display_messages.len();
            let session_before = app.session.messages.len();
            app.set_input_for_test("http://localhost:1455/auth/callback?code=secret&state=secret");
            if alternate {
                super::input::handle_alternate_enter(&mut app);
            } else {
                super::input::handle_enter(&mut app);
            }
            assert!(matches!(
                app.pending_login,
                Some(PendingLogin::Exchanging { .. })
            ));
            assert!(app.queued_messages.is_empty());
            assert!(app.interleave_message.is_none());
            assert_eq!(app.display_messages.len(), before);
            assert_eq!(app.session.messages.len(), session_before);
            assert!(
                app.route_next_prompt_to_new_session,
                "login must not consume or use prompt routing"
            );
        }
    }
}

#[test]
fn oauth_login_progress_foreign_completion_keeps_auth_input_captured() {
    let mut app = create_test_app();
    app.pending_login = Some(PendingLogin::ClaudeAccount {
        verifier: "fixture".to_string(),
        label: "fixture".to_string(),
        redirect_uri: None,
    });
    app.set_input_for_test("secret-claude-code");
    app.handle_login_completed(LoginCompleted {
        provider: "openai".to_string(),
        success: false,
        message: "Old OpenAI attempt failed".to_string(),
    });
    assert!(matches!(
        app.pending_login,
        Some(PendingLogin::ClaudeAccount { .. })
    ));
    assert_eq!(app.input, "secret-claude-code");
    assert!(app.login_input_hint().unwrap().contains("not sent to AI"));
}

#[test]
fn oauth_login_progress_preflight_is_honest_when_browser_and_listener_fail() {
    let mut app = create_test_app();
    app.show_openai_login_progress(
        "fixture",
        "https://example.invalid/sign-in",
        false,
        false,
        "Manual fallback",
    );
    let content = &app.display_messages.last().unwrap().content;
    assert!(content.contains("Open the sign-in link manually"));
    assert!(content.contains("local listener unavailable"));
    assert!(!content.contains("Browser launch requested"));
    assert!(!content.contains("callback received"));
}

#[test]
fn oauth_login_progress_real_frame_has_clickable_link_and_expand_button() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.handle_debug_command("login-progress-fixture");
    let index = app.display_messages.len() - 1;
    let backend = ratatui::backend::TestBackend::new(90, 36);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let collapsed = render_and_snap(&app, &mut terminal);
    assert!(!collapsed.contains("client_id=jcode-fixture"));
    assert!(collapsed.contains("Login only:"));
    let find = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>, needle: &str| {
        let buf = terminal.backend().buffer();
        for row in 0..buf.area().height {
            let line = (0..buf.area().width)
                .map(|col| buf[(col, row)].symbol())
                .collect::<String>();
            if let Some(byte) = line.find(needle) {
                return (
                    unicode_width::UnicodeWidthStr::width(&line[..byte]) as u16,
                    row,
                );
            }
        }
        panic!("missing {needle} in frame");
    };
    let (link_col, link_row) = find(&terminal, "Open OpenAI sign-in");
    assert_eq!(
        crate::tui::ui::oauth_login_link_message_from_screen(link_col + 2, link_row),
        Some(index)
    );
    let (col, row) = find(&terminal, "▸ expand");
    assert!(app.try_toggle_swarm_expand_at(col + 2, row));
    let expanded = render_and_snap(&app, &mut terminal);
    assert!(expanded.contains("client_id=jcode-fixture"));
    let (col, row) = find(&terminal, "▾ collapse");
    assert!(app.try_toggle_swarm_expand_at(col + 2, row));
    let collapsed = render_and_snap(&app, &mut terminal);
    assert!(!collapsed.contains("client_id=jcode-fixture"));
    app.handle_debug_command("login-progress-success-fixture");
    let success = render_and_snap(&app, &mut terminal);
    assert!(success.contains("OpenAI login complete"));
    assert!(!success.contains("▸ expand"));
    assert!(!success.contains("Open OpenAI sign-in"));
    assert!(!success.contains("Login only:"));
}
