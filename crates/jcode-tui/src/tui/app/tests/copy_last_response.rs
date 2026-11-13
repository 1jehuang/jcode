// Configurable key to copy the last assistant answer (#1929).
// `include!`d into `crate::tui::app::tests`, so KeyCode/KeyModifiers/TuiState
// come from sibling includes.
use jcode_tui_markdown::REASONING_SENTINEL;

fn clipboard_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
        Some("The real answer is 42.".to_string())
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
    let _clip = clipboard_test_lock();
    crate::tui::app::helpers::capture_clipboard_for_tests();
    let mut app = create_test_app();
    let answer = "# Hello\n\nUse `code` here.";
    app.push_display_message(DisplayMessage::user("please explain"));
    app.push_display_message(DisplayMessage::tool_text("ignored tool output"));
    app.push_display_message(DisplayMessage::assistant(answer));

    super::copy_last_response::copy_last_answer(&mut app);

    let copied = crate::tui::app::helpers::captured_clipboard_for_tests();
    let notice = app.status_notice();
    eprintln!("copied text: {copied:?}");
    eprintln!("status notice: {notice:?}");
    assert_eq!(copied.as_deref(), Some(answer));
    assert_eq!(
        notice.as_deref(),
        Some(super::copy_last_response::copied_last_answer_notice(answer.chars().count()).as_str())
    );
    crate::tui::app::helpers::stop_capturing_clipboard_for_tests();
}

#[test]
fn copy_last_answer_without_assistant_leaves_clipboard_and_reports_empty() {
    let _clip = clipboard_test_lock();
    crate::tui::app::helpers::capture_clipboard_for_tests();
    crate::tui::app::helpers::copy_to_clipboard("sentinel-not-an-answer");
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::user("hello"));
    app.push_display_message(DisplayMessage::tool_text("tool only"));

    super::copy_last_response::copy_last_answer(&mut app);

    let copied = crate::tui::app::helpers::captured_clipboard_for_tests();
    let notice = app.status_notice();
    eprintln!("copied text: {copied:?}");
    eprintln!("status notice: {notice:?}");
    assert_eq!(copied.as_deref(), Some("sentinel-not-an-answer"));
    assert_eq!(notice.as_deref(), Some("No answer to copy yet"));
    crate::tui::app::helpers::stop_capturing_clipboard_for_tests();
}

#[test]
fn copy_last_response_ctrl_x_ctrl_y_hotkey_copies_last_answer() {
    let _clip = clipboard_test_lock();
    crate::tui::app::helpers::capture_clipboard_for_tests();
    let mut app = create_test_app();
    app.toggle_keys.copy_last_response = crate::tui::keybind::ChordSequence::load("ctrl+x ctrl+y");
    app.push_display_message(DisplayMessage::assistant("hotkey answer"));

    app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(
        crate::tui::app::helpers::captured_clipboard_for_tests().as_deref(),
        Some(""),
        "Ctrl+X alone is only the prefix; nothing is copied yet"
    );

    app.handle_key(KeyCode::Char('y'), KeyModifiers::CONTROL)
        .unwrap();
    let copied = crate::tui::app::helpers::captured_clipboard_for_tests();
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
    crate::tui::app::helpers::stop_capturing_clipboard_for_tests();
}
