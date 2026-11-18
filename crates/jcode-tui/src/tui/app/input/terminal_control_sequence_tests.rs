use super::strip_terminal_control_sequences;

/// Remnants of terminal reports must never reach the composer (#540).
#[test]
fn strips_escape_and_bare_report_remnants() {
    for (input, expected) in [
        // Full mouse report, and the bare tail left by a torn read.
        ("\x1b[<65;50;24M", ""),
        ("[<65;50;24M", ""),
        ("hi[<65;50;24Mthere", "hithere"),
        ("[<65;50;24m", ""),
        // Bracketed paste markers and cursor/focus reports.
        ("[200~", ""),
        ("[201~", ""),
        ("[12;40R", ""),
        ("[1I", ""),
        ("[1O", ""),
        // 8-bit CSI introducer.
        ("\u{9b}[<65;50;24M", ""),
        // Stray C0 controls, but tabs and newlines survive.
        ("a\x07b", "ab"),
        ("a\x1b🎉b", "ab"),
        ("a\tb\nc", "a\tb\nc"),
        // Truncated escape with no final byte: drop the remnant.
        ("\x1b[<65;5", ""),
    ] {
        assert_eq!(
            strip_terminal_control_sequences(input),
            expected,
            "input {input:?} should sanitize to {expected:?}"
        );
    }
}

/// The guard must not eat text a user actually typed. Being too aggressive
/// here is worse than missing a remnant.
#[test]
fn preserves_ordinary_bracketed_text() {
    for input in [
        "array[0]",
        "list[1] = list[2]",
        "[TODO] fix this",
        "see docs[1] and notes[2]",
        "fn f(v: Vec<u8>) -> [u8; 4]",
        "a[b]c",
        "[]",
        "[",
        "]",
        "[abc]",
        "[1]",
        "[12;40]",
        "plain text with no brackets",
        "emoji 🎉 and accents café",
        "match x { [a, b] => a + b }",
    ] {
        assert_eq!(
            strip_terminal_control_sequences(input),
            input,
            "input {input:?} must be preserved verbatim"
        );
    }
}

/// Only actual OSC control framing authorizes removal (#970).
#[test]
fn strips_late_osc_color_replies() {
    use super::strip_osc_color_replies;
    for (input, expected) in [
        ("\x1b]11;rgb:3030/3434/4646\x07", ""),
        ("hi\x1b]11;rgb:3030/3434/4646\x1b\\there", "hithere"),
        ("\u{9d}11;rgb:30/34/46\u{9c}", ""),
        (
            "\x1b]10;rgb:cdcd/d6d6/f4f4\x07\x1b]11;rgb:0000/0000/0000\x07",
            "",
        ),
        ("\x1b]11;rgba:ffff/ffff/ffff/ffff\x1b\\", ""),
    ] {
        assert_eq!(
            strip_osc_color_replies(input, input.len()),
            Some((expected.to_string(), expected.len())),
            "{input:?}"
        );
        assert_eq!(strip_terminal_control_sequences(input), expected);
    }
    for text in [
        "Explain 11;rgb:30/34/46 please",
        "11;rgb:aaaa/bbbb/cccc",
        "]11;rgb:30/34/46\\hello",
        "11;rgb:30/34/46\x07",
        "rgb:3030/3434/4646",
        "111;rgb:30/34/46",
        "use rgb(1,2,3)",
        "\x1b]11;rgb:",
        "\x1b]11;rgb:3030/34",
        "\x1b]11;rgb:3030/3434/46",
        "\x1b]11;rgb:3030/3434/4646",
        "\x1b]11;rgb:30/34/466\x07",
    ] {
        assert_eq!(strip_osc_color_replies(text, text.len()), None, "{text:?}");
    }

    let input = "é\x1b]11;rgb:30/34/46\x07cd";
    let end = input.len() - 2;
    for cursor in [2, 5, end, input.len()] {
        assert_eq!(
            strip_osc_color_replies(input, cursor),
            Some(("écd".to_string(), if cursor == input.len() { 4 } else { 2 }))
        );
    }
}

#[test]
fn ordinary_rgb_survives_public_key_paste_and_text_input() {
    use crate::tui::app::tests::create_test_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    for text in [
        "Explain 11;rgb:30/34/46 please",
        "11;rgb:aaaa/bbbb/cccc",
        "]11;rgb:30/34/46\\",
        "10;rgb:cdcd/d6d6/f4f411;rgb:0000/0000/0000",
    ] {
        for path in 0..3 {
            let mut app = create_test_app();
            app.set_input_for_test("draft ".to_string());
            match path {
                0 => {
                    for ch in text.chars() {
                        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                    }
                }
                1 => app.handle_paste(text.to_string()),
                _ => {
                    assert!(super::handle_text_input(&mut app, text));
                }
            }
            assert_eq!(app.input, format!("draft {text}"), "path {path}");
            assert_eq!(app.cursor_pos, app.input.len());
            assert!(!app.terminal_reply_filter.is_pending());
        }
    }
}

#[test]
fn framed_color_replies_survive_every_public_text_split() {
    use crate::tui::app::tests::create_test_app;
    for response in [
        "\x1b]11;rgb:3030/3434/4646\x07",
        "\x1b]11;rgb:30/34/46\x1b\\",
        "\u{9d}11;rgb:aaaa/bbbb/cccc\u{9c}",
    ] {
        for split in (0..=response.len()).filter(|&at| response.is_char_boundary(at)) {
            let mut app = create_test_app();
            app.set_input_for_test("draft ".to_string());
            super::handle_text_input(&mut app, &response[..split]);
            assert_eq!(app.input(), "draft ", "rendered prefix at split {split}");
            assert_eq!(app.cursor_pos, app.input.len());
            assert!(app.input_undo_stack.is_empty());
            super::handle_text_input(&mut app, &response[split..]);
            assert_eq!(app.input, "draft ", "{response:?} split at {split}");
            assert_eq!(app.cursor_pos, app.input.len());
            super::handle_text_input(&mut app, "11;rgb:aaaa/bbbb/cccc");
            assert_eq!(app.input, "draft 11;rgb:aaaa/bbbb/cccc");
        }
    }
}

#[test]
fn framed_color_replies_are_removed_from_public_paste() {
    use crate::tui::app::tests::create_test_app;
    for terminator in ["\x07", "\x1b\\"] {
        let mut app = create_test_app();
        app.set_input_for_test("draft ".to_string());
        app.handle_paste(format!("before\x1b]11;rgb:aaaa/bbbb/cccc{terminator}after"));
        assert_eq!(app.input, "draft beforeafter");
        assert_eq!(app.cursor_pos, app.input.len());
    }
}

#[test]
fn streamed_alt_osc_keys_require_the_introducer_and_terminator() {
    use crate::tui::app::tests::create_test_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for (end, modifiers) in [('g', KeyModifiers::CONTROL), ('\\', KeyModifiers::ALT)] {
        let mut app = create_test_app();
        app.set_input_for_test("draft ".to_string());
        app.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT));
        for ch in "11;rgb:aaaa/bbbb/cccc".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            assert_eq!(app.input(), "draft ");
            assert_eq!(app.cursor_pos, app.input.len());
        }
        assert_eq!(app.input(), "draft ");
        assert!(app.terminal_reply_filter.is_pending());
        assert!(app.input_undo_stack.is_empty());
        app.handle_key_event(KeyEvent::new(KeyCode::Char(end), modifiers));
        assert_eq!(app.input, "draft ");
        for ch in "11;rgb:aaaa/bbbb/cccc".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(app.input, "draft 11;rgb:aaaa/bbbb/cccc");
        assert_eq!(app.cursor_pos, app.input.len());
    }
}

#[test]
fn unterminated_osc_never_reaches_public_submit_or_transport() {
    use crate::tui::app::ContentBlock;
    use crate::tui::app::tests::create_test_app;
    for partial in [
        "\x1b]",
        "\x1b]11;rgb:aaaa/bbbb/cccc",
        "\u{9d}11;rgb:30/34/46\x1b",
    ] {
        let mut app = create_test_app();
        app.set_input_for_test("Explain 11;rgb:30/34/46 please");
        super::handle_text_input(&mut app, partial);
        assert_eq!(app.input(), "Explain 11;rgb:30/34/46 please");
        assert!(app.terminal_reply_filter.is_pending());
        assert!(app.input_undo_stack.is_empty());
        app.submit_input();
        let message = app.session.messages.last().unwrap();
        assert!(
            matches!(message.content.as_slice(), [ContentBlock::Text { text, .. }]
            if text == "Explain 11;rgb:30/34/46 please")
        );
        assert!(!app.terminal_reply_filter.is_pending());
        assert!(app.input_undo_stack.is_empty());

        let mut app = create_test_app();
        app.set_input_for_test("11;rgb:aaaa/bbbb/cccc");
        super::handle_text_input(&mut app, partial);
        let prepared = super::take_prepared_input(&mut app);
        assert_eq!(prepared.raw_input, "11;rgb:aaaa/bbbb/cccc");
        assert_eq!(prepared.expanded, "11;rgb:aaaa/bbbb/cccc");
        assert_eq!(prepared.routing_input(), "11;rgb:aaaa/bbbb/cccc");
        assert!(!app.terminal_reply_filter.is_pending());
        assert!(app.input_undo_stack.is_empty());
    }
}

#[test]
fn public_undo_cannot_resurrect_complete_or_unterminated_osc() {
    use crate::tui::app::tests::create_test_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for suffix in ["", "\x07", "\x1b\\"] {
        let mut app = create_test_app();
        app.set_input_for_test("draft ");
        app.handle_paste("kept".to_string());
        let snapshots = app.input_undo_stack.clone();
        for ch in format!("\x1b]11;rgb:aaaa/bbbb/cccc{suffix}").chars() {
            super::handle_text_input(&mut app, &ch.to_string());
            assert_eq!(app.input(), "draft kept");
            assert_eq!(app.input_undo_stack, snapshots);
            assert_eq!(app.cursor_pos, app.input.len());
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.input(), "draft ");
        assert!(!app.terminal_reply_filter.is_pending());
        app.handle_paste("11;rgb:aaaa/bbbb/cccc".to_string());
        assert_eq!(app.input(), "draft 11;rgb:aaaa/bbbb/cccc");
        app.handle_key_event(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.input(), "draft ");
        assert!(
            app.input_undo_stack
                .iter()
                .all(|snapshot| !snapshot.0.contains('\x1b'))
        );
    }
}

#[test]
fn oversized_or_malformed_osc_does_not_leave_stale_parser_state() {
    use crate::tui::app::tests::create_test_app;
    for text in [
        format!("\x1b]11;rgb:{}", "a".repeat(128)),
        "\x1b]not-a-color".to_string(),
    ] {
        let mut app = create_test_app();
        app.set_input_for_test("draft ");
        super::handle_text_input(&mut app, &text);
        assert!(!app.terminal_reply_filter.is_pending());
        assert!(!app.input().chars().any(char::is_control));
        let before = app.input().to_string();
        app.handle_paste("11;rgb:aaaa/bbbb/cccc".to_string());
        assert_eq!(app.input(), format!("{before}11;rgb:aaaa/bbbb/cccc"));
    }
}

#[test]
fn streamed_responses_on_empty_composer_do_not_trigger_suggestion_submission() {
    use crate::tui::app::tests::create_test_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for keys in [false, true] {
        let mut app = create_test_app();
        let message_count = app.session.messages.len();
        if keys {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT));
            for ch in "11;rgb:aaaa/bbbb/cccc".chars() {
                app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
                assert_eq!(app.input(), "");
                assert_eq!(app.session.messages.len(), message_count);
            }
            app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        } else {
            for ch in "\x1b]11;rgb:aaaa/bbbb/cccc\x07".chars() {
                super::handle_text_input(&mut app, &ch.to_string());
                assert_eq!(app.input(), "");
                assert_eq!(app.session.messages.len(), message_count);
            }
        }
        assert!(!app.terminal_reply_filter.is_pending());
        assert!(app.input_undo_stack.is_empty());
        assert_eq!(app.cursor_pos, 0);
    }
}

#[test]
fn direct_undo_discards_pending_parser_state_without_a_phantom_snapshot() {
    use crate::tui::app::tests::create_test_app;
    let mut app = create_test_app();
    app.set_input_for_test("draft ");
    app.handle_paste("kept".to_string());
    super::handle_text_input(&mut app, "\x1b]11;rgb:aaaa/bbbb/cccc");
    app.undo_input_change();
    assert_eq!(app.input(), "draft ");
    assert!(!app.terminal_reply_filter.is_pending());
    app.handle_paste("11;rgb:aaaa/bbbb/cccc".to_string());
    assert_eq!(app.input(), "draft 11;rgb:aaaa/bbbb/cccc");
}

/// Non-suspicious text must not be reallocated.
#[test]
fn borrows_when_nothing_to_strip() {
    assert!(matches!(
        strip_terminal_control_sequences("array[0] = 1"),
        std::borrow::Cow::Borrowed(_)
    ));
}
