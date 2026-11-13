// Configurable key to copy the last assistant answer (#1929).
// `include!`d into `crate::tui::app::tests`, so KeyCode/KeyModifiers/TuiState
// come from sibling includes.
use jcode_tui_markdown::REASONING_SENTINEL;

#[test]
fn copy_last_response_default_is_unbound_and_ctrl_x_ctrl_y_is_recognized() {
    use crate::tui::keybind::{ChordSequence, SequenceMatch};

    let default =
        ChordSequence::load(&jcode_config_types::KeybindingsConfig::default().copy_last_response);
    assert!(
        !default.is_bound(),
        "copy_last_response must default unbound, got {:?}",
        default.label()
    );
    assert!(!ChordSequence::load("off").is_bound());
    assert!(!ChordSequence::load("none").is_bound());
    assert!(!ChordSequence::load("disabled").is_bound());

    let mut seq = ChordSequence::load("ctrl+x ctrl+y");
    assert!(seq.is_bound(), "ctrl+x ctrl+y must parse as a sequence");
    assert_eq!(
        seq.feed(KeyCode::Char('x'), KeyModifiers::CONTROL),
        SequenceMatch::Prefix
    );
    assert_eq!(
        seq.feed(KeyCode::Char('y'), KeyModifiers::CONTROL),
        SequenceMatch::Complete
    );
}

#[test]
fn copy_last_answer_text_is_markdown_without_reasoning_or_tool_output() {
    use super::copy_last_response::last_assistant_answer_text;

    let reasoning =
        DisplayMessage::reasoning(format!("*{REASONING_SENTINEL}scratch{REASONING_SENTINEL}*"));
    let tool = DisplayMessage::tool_text("tool stdout that must not be copied");
    let user = DisplayMessage::user("please explain");
    let earlier = DisplayMessage::assistant("stale answer");
    let last = DisplayMessage::assistant("# Hello\n\nUse `code` here.");

    assert_eq!(
        last_assistant_answer_text(&[user, earlier, reasoning, tool, last]),
        Some("# Hello\n\nUse `code` here.".to_string())
    );
}

#[test]
fn copy_last_answer_text_strips_in_message_reasoning_prefix() {
    use super::copy_last_response::last_assistant_answer_text;

    let content = format!(
        "*{0}thinking out loud{0}*  \n\nThe real answer is 42.",
        REASONING_SENTINEL
    );
    let msg = DisplayMessage::assistant(content);
    assert_eq!(
        last_assistant_answer_text(&[msg]),
        Some("\nThe real answer is 42.".to_string())
    );
}

#[test]
fn copy_last_answer_text_none_when_transcript_has_no_answer() {
    use super::copy_last_response::last_assistant_answer_text;

    assert_eq!(last_assistant_answer_text(&[]), None);
    assert_eq!(
        last_assistant_answer_text(&[
            DisplayMessage::user("hi"),
            DisplayMessage::tool_text("ls output"),
            DisplayMessage::reasoning("still thinking"),
        ]),
        None
    );
}

#[test]
fn copy_last_answer_notice_uses_thousands_separators() {
    assert_eq!(
        super::copy_last_response::copied_last_answer_notice(1234),
        "Copied the last answer (1,234 characters)"
    );
    assert_eq!(
        super::copy_last_response::copied_last_answer_notice(7),
        "Copied the last answer (7 characters)"
    );
}

#[test]
fn copy_last_answer_writes_test_clipboard_and_reports_character_count() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    let answer = "# Hello\n\nUse `code` here.";
    app.push_display_message(DisplayMessage::user("please explain"));
    app.push_display_message(DisplayMessage::tool_text("ignored tool output"));
    app.push_display_message(DisplayMessage::assistant(answer));

    super::copy_last_response::copy_last_answer(&mut app);

    let copied = clipboard.text();
    let notice = app.status_notice();
    eprintln!("copied text: {copied:?}");
    eprintln!("status notice: {notice:?}");
    assert_eq!(copied.as_deref(), Some(answer));
    assert_eq!(
        notice.as_deref(),
        Some(super::copy_last_response::copied_last_answer_notice(answer.chars().count()).as_str())
    );
}

#[test]
fn copy_last_answer_without_assistant_leaves_clipboard_and_reports_empty() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    crate::tui::app::helpers::copy_to_clipboard("sentinel-not-an-answer");
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::user("hello"));
    app.push_display_message(DisplayMessage::tool_text("tool only"));

    super::copy_last_response::copy_last_answer(&mut app);

    let copied = clipboard.text();
    let notice = app.status_notice();
    eprintln!("copied text: {copied:?}");
    eprintln!("status notice: {notice:?}");
    assert_eq!(copied.as_deref(), Some("sentinel-not-an-answer"));
    assert_eq!(notice.as_deref(), Some("No answer to copy yet"));
}

#[test]
fn copy_last_response_ctrl_x_ctrl_y_hotkey_copies_last_answer() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    app.toggle_keys.copy_last_response = crate::tui::keybind::ChordSequence::load("ctrl+x ctrl+y");
    app.push_display_message(DisplayMessage::assistant("hotkey answer"));

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(
        clipboard.text().as_deref(),
        Some(""),
        "Ctrl+X alone is only the prefix; nothing is copied yet"
    );

    app.handle_key(KeyCode::Char('y'), KeyModifiers::CONTROL)
        .unwrap();
    let copied = clipboard.text();
    let notice = app.status_notice();
    eprintln!("copied text: {copied:?}");
    eprintln!("status notice: {notice:?}");
    assert_eq!(copied.as_deref(), Some("hotkey answer"));
    assert!(
        notice
            .as_deref()
            .is_some_and(|n| n.contains("Copied the last answer")),
        "got {notice:?}"
    );
}

fn arm_pending_fallback_offer(app: &mut App) {
    app.pending_fallback_offer = Some(PendingFallbackOffer {
        selection: crate::provider::RouteSelection {
            model: "fallback-model".into(),
            runtime_key: jcode_provider_core::RuntimeKey::AnthropicApiKey,
            api_method: "claude-api".into(),
            provider_label: "Claude".into(),
            detail: String::new(),
        },
        target_label: "fallback-model".into(),
        from_label: "current-model".into(),
        remote_resend: None,
    });
}

#[test]
fn copy_last_response_pending_sequence_beats_fallback_offer() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let mut app = create_test_app();
    app.toggle_keys.copy_last_response = crate::tui::keybind::ChordSequence::load("ctrl+x ctrl+y");
    app.push_display_message(DisplayMessage::assistant("copy me, do not fallback"));
    arm_pending_fallback_offer(&mut app);
    let model_before = app.session.model.clone();

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    app.handle_key(KeyCode::Char('y'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(
        clipboard.text().as_deref(),
        Some("copy me, do not fallback"),
        "completing the copy sequence must copy the last answer"
    );
    assert!(
        app.pending_fallback_offer.is_some(),
        "Ctrl+Y completing a started copy sequence must not apply the fallback offer"
    );
    assert!(
        !app.pending_turn,
        "fallback resend must not be queued when the copy sequence completes"
    );
    assert_eq!(app.session.model, model_before);
}

#[test]
fn copy_last_response_pending_sequence_is_fed_before_offers() {
    let local = include_str!("../input.rs");
    let remote = include_str!("../remote/key_handling/key_events.rs");
    for (name, src) in [("local", local), ("remote", remote)] {
        let pending = src
            .find("handle_key_if_pending")
            .unwrap_or_else(|| panic!("{name} key path must feed a pending copy sequence first"));
        let fallback = src
            .find("pending_fallback_offer")
            .unwrap_or_else(|| panic!("{name} key path must still check the fallback offer"));
        let merge = src
            .find("merge_offer_key_matches")
            .unwrap_or_else(|| panic!("{name} key path must still check the merge offer"));
        assert!(
            pending < fallback,
            "{name}: pending copy sequence must be fed before the fallback offer"
        );
        assert!(
            pending < merge,
            "{name}: pending copy sequence must be fed before the merge offer"
        );
    }
}

#[test]
fn copy_last_answer_text_preserves_code_block_whitespace() {
    use super::copy_last_response::last_assistant_answer_text;

    let answer = "```python\nprint(\"hello\")  \n\n\nprint(\"world\")\n```\n";
    let msg = DisplayMessage::assistant(answer);
    assert_eq!(
        last_assistant_answer_text(&[msg]),
        Some(answer.to_string()),
        "copy must keep blank lines, trailing spaces, and the trailing newline"
    );
}

#[test]
fn copy_last_response_abandoned_prefix_does_not_copy_after_picker() {
    let _render_lock = scroll_render_test_lock();
    let clipboard = CapturedClipboard::new();
    let runtime = tokio::runtime::Runtime::new().expect("test runtime");
    let _guard = runtime.enter();
    let mut app = create_test_app();
    app.toggle_keys.copy_last_response = crate::tui::keybind::ChordSequence::load("ctrl+x ctrl+y");
    app.push_display_message(DisplayMessage::assistant("should not copy yet"));

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        app.toggle_keys.copy_last_response.is_pending(),
        "Ctrl+X must arm the copy prefix"
    );

    app.open_session_picker();
    assert!(app.session_picker_overlay.is_some());
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.session_picker_overlay.is_none());
    assert!(
        !app.toggle_keys.copy_last_response.is_pending(),
        "opening or closing the session picker must drop the copy prefix"
    );

    app.handle_key(KeyCode::Char('y'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(
        clipboard.text().as_deref(),
        Some(""),
        "standalone Ctrl+Y after an abandoned prefix must not copy"
    );

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    app.handle_key(KeyCode::Char('y'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(
        clipboard.text().as_deref(),
        Some("should not copy yet"),
        "a full sequence after the picker must still copy"
    );
}

#[test]
fn copy_last_answer_clipboard_tests_share_the_render_lock() {
    let src = include_str!("copy_last_response.rs");
    assert!(
        src.contains("scroll_render_test_lock"),
        "clipboard tests must serialize on the shared render lock"
    );
    assert!(
        src.contains("CapturedClipboard"),
        "clipboard tests must use CapturedClipboard"
    );
    let private = format!("{}{}", "clipboard_test", "_lock");
    assert!(
        !src.contains(&private),
        "do not keep a private clipboard mutex alongside TEST_CLIPBOARD"
    );
}
