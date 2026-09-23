#[test]
fn known_slash_commands_dispatch_locally_without_starting_a_turn() {
    for (input, expected) in [
        ("/help compact", "/compact"),
        ("/help provider-test-coverage", "/provider-test-coverage"),
        ("/test help", "Usage: /test"),
    ] {
        let mut app = create_test_app();

        assert!(
            super::commands_dispatch::dispatch_local_command(&mut app, input),
            "{input} should be claimed by the local slash-command dispatcher"
        );
        assert!(
            !app.pending_turn && !app.is_processing,
            "{input} must not start an agent turn"
        );
        let message = app
            .display_messages()
            .last()
            .unwrap_or_else(|| panic!("{input} produced no display message"));
        assert_eq!(message.role, "system", "{input}: {message:?}");
        assert!(
            message.content.contains(expected),
            "{input}: expected {expected:?} in {:?}",
            message.content
        );
    }
}

#[test]
fn multiple_known_slash_commands_dispatch_in_order() {
    let mut app = create_test_app();

    assert!(super::commands_dispatch::dispatch_local_command(
        &mut app,
        "/help compact /test help"
    ));

    let messages = app.display_messages();
    assert_eq!(messages.len(), 2, "both slash commands should be handled");
    assert_eq!(messages[0].role, "system");
    assert!(messages[0].content.contains("/compact"));
    assert_eq!(messages[1].role, "system");
    assert!(messages[1].content.contains("Usage: /test"));
    assert!(!app.pending_turn && !app.is_processing);
}

#[test]
fn embedded_known_slash_command_after_whitespace_is_dispatched() {
    let mut app = create_test_app();

    assert!(super::commands_dispatch::dispatch_local_command(
        &mut app,
        "prefix /help"
    ));

    assert_eq!(app.display_messages().len(), 1);
    assert_ne!(app.display_messages()[0].role, "user");
    assert!(!app.pending_turn && !app.is_processing);
}

#[test]
fn embedded_autocomplete_replaces_only_the_active_token() {
    let mut app = create_test_app();
    app.input = "keep this /can and the rest".to_string();
    app.cursor_pos = "keep this /can".len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "keep this /cancel and the rest");
    assert_eq!(app.cursor_pos, "keep this /cancel".len());
}

#[test]
fn embedded_suggestions_follow_the_cursor_not_the_whole_buffer() {
    let mut app = create_test_app();
    app.input = "prefix /hel suffix".to_string();
    app.cursor_pos = "prefix /hel".len();

    let suggestions = app.get_suggestions_for(&app.input);
    assert!(suggestions.iter().any(|(command, _)| command == "/help"));
}

#[test]
fn quoted_and_code_known_slash_commands_remain_prompt_text() {
    let mut app = create_test_app();
    let prompt = "Explain the literal strings \"/help\", `/test`, and /tmp/shot.png.";
    app.input = prompt.to_string();
    app.cursor_pos = app.input.len();

    app.submit_input();

    assert!(
        app.pending_turn,
        "ordinary text should enter the local turn path"
    );
    assert!(app.is_processing);
    assert!(
        app.display_messages()
            .iter()
            .any(|message| { message.role == "user" && message.content == prompt })
    );
    assert!(matches!(
        app.session.messages.last().and_then(|message| message.content.last()),
        Some(crate::message::ContentBlock::Text { text, .. }) if text == prompt
    ));
    assert!(
        app.display_messages()
            .iter()
            .all(|message| message.role != "system" && message.role != "error"),
        "embedded command-looking text must not be dispatched locally"
    );
}

#[test]
fn command_prefix_literals_do_not_match_known_slash_commands() {
    for input in [
        "/helpful",
        "/testimony",
        "/pluginatic",
        "/compactly",
        "https://example.com/help",
        "/tmp/shot.png",
        "`/help`",
        "\"/test\"",
    ] {
        let mut app = create_test_app();

        assert!(
            !super::commands_dispatch::dispatch_local_command(&mut app, input),
            "{input} must not match a built-in command by prefix"
        );
        assert!(
            app.display_messages().is_empty(),
            "unmatched command-shaped text should not produce a built-in response: {input}"
        );
    }
}

#[test]
fn parser_handles_unicode_and_fenced_code_boundaries() {
    let commands = ["/help", "/review"];
    assert_eq!(
        super::slash_command_parser::find_known_commands("élan /help", &commands)
            .into_iter()
            .map(|matched| (matched.start, matched.name_end))
            .collect::<Vec<_>>(),
        vec![(6, 11)]
    );
    assert_eq!(
        super::slash_command_parser::find_known_commands("```\n/help\n``` /review", &commands),
        vec![super::slash_command_parser::SlashCommandMatch {
            start: "```\n/help\n``` ".len(),
            name_end: "```\n/help\n``` /review".len(),
        }]
    );
}
