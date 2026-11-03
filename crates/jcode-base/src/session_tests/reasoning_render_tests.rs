use super::*;

#[test]
fn test_render_messages_renders_reasoning_before_answer_in_stored_order() {
    // Regression: providers persist the assistant turn as `[Text, ReasoningTrace,
    // ToolUse]` (see agent/turn_loops.rs push order). On resume/re-render the
    // reasoning must still appear *before* the answer text to match the live
    // streaming order, even though the Text block is stored first.
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "full");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_order_test".to_string(),
        None,
        Some("render reasoning order test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
            ContentBlock::ReasoningTrace {
                text: "step one\nstep two".to_string(),
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    assert!(
        content.contains(&format!("*{0}step one{0}*", REASONING_SENTINEL)),
        "expected reasoning markup, got: {content:?}"
    );
    assert!(content.contains("Here is the answer."));
    let reasoning_pos = content.find("step two").unwrap();
    let answer_pos = content.find("Here is the answer.").unwrap();
    assert!(
        reasoning_pos < answer_pos,
        "reasoning should precede the answer text even when stored after it: {content:?}"
    );
}

#[test]
fn test_render_messages_renders_persisted_reasoning() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "full");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_test".to_string(),
        None,
        Some("render reasoning test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ReasoningTrace {
                text: "step one\nstep two".to_string(),
            },
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    // Reasoning lines are rendered as dim/italic markup with the sentinel.
    assert!(
        content.contains(&format!("*{0}step one{0}*", REASONING_SENTINEL)),
        "expected reasoning markup, got: {content:?}"
    );
    assert!(
        content.contains(&format!("*{0}step two{0}*", REASONING_SENTINEL)),
        "expected reasoning markup, got: {content:?}"
    );
    // Answer text follows the reasoning block.
    assert!(content.contains("Here is the answer."));
    let reasoning_end = content.find("step two").unwrap();
    let answer_start = content.find("Here is the answer.").unwrap();
    assert!(
        reasoning_end < answer_start,
        "reasoning should precede the answer text: {content:?}"
    );
}

#[test]
fn test_render_messages_renders_legacy_reasoning_variant() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "full");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_legacy_reasoning_test".to_string(),
        None,
        Some("render legacy reasoning test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Reasoning {
            text: "legacy thought".to_string(),
        }],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    assert!(
        rendered[0]
            .content
            .contains(&format!("*{0}legacy thought{0}*", REASONING_SENTINEL)),
        "expected legacy reasoning markup, got: {:?}",
        rendered[0].content
    );
}

#[test]
fn test_render_messages_hides_persisted_reasoning_in_current_mode() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "current");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_current_test".to_string(),
        None,
        Some("render reasoning current test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ReasoningTrace {
                text: "step one\nstep two\nstep three".to_string(),
            },
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    // In `current` mode only the *live* reasoning block is ever shown; it streams
    // then is discarded once the model answers. Re-rendered history therefore
    // shows no past reasoning at all (no trace line, no lines, no sentinel).
    assert!(
        !content.contains(REASONING_SENTINEL),
        "no reasoning markup expected in current mode on reload: {content:?}"
    );
    assert!(
        !content.contains("step one")
            && !content.contains("step two")
            && !content.contains("thought"),
        "individual reasoning lines/trace must not be replayed in current mode: {content:?}"
    );
    // The answer text is preserved.
    assert!(content.contains("Here is the answer."));
}

#[test]
fn test_render_messages_hides_persisted_reasoning_in_off_mode() {
    use jcode_render_core::REASONING_SENTINEL;

    let _env_lock = lock_env();
    let _mode = EnvVarGuard::set("JCODE_REASONING_DISPLAY", "off");
    crate::config::invalidate_config_cache();

    let mut session = Session::create_with_id(
        "session_render_reasoning_off_test".to_string(),
        None,
        Some("render reasoning off test".to_string()),
    );

    session.add_message(
        Role::Assistant,
        vec![
            ContentBlock::ReasoningTrace {
                text: "secret thought".to_string(),
            },
            ContentBlock::Text {
                text: "Here is the answer.".to_string(),
                cache_control: None,
            },
        ],
    );

    let rendered = render_messages(&session);
    assert_eq!(rendered.len(), 1);
    let content = &rendered[0].content;
    assert!(
        !content.contains(REASONING_SENTINEL) && !content.contains("secret thought"),
        "reasoning must be hidden entirely in off mode: {content:?}"
    );
    assert!(content.contains("Here is the answer."));
}
