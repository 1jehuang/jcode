use super::*;
use crate::session::StoredCompactionState;

fn text(value: impl Into<String>) -> ContentBlock {
    ContentBlock::Text {
        text: value.into(),
        cache_control: None,
    }
}

fn compacted_tool_session(turns: usize) -> Session {
    let mut session = Session::create_with_id("prompt-count-regression".into(), None, None);
    for turn in 0..turns {
        session.add_message(Role::User, vec![text(format!("prompt {turn}"))]);
        for tool in 0..12 {
            session.add_message(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: format!("tool-{turn}-{tool}"),
                    content: "tool output".into(),
                    is_error: None,
                }],
            );
        }
    }
    session.compaction = Some(StoredCompactionState {
        summary_text: "summary".into(),
        openai_encrypted_content: None,
        covers_up_to_turn: turns,
        original_turn_count: turns,
        compacted_count: session.messages.len(),
    });
    session.add_message(Role::User, vec![text("current prompt")]);
    session
}

#[test]
fn compacted_tool_results_are_not_hidden_user_prompts_or_turn_boundaries() {
    let session = compacted_tool_session(10);
    for window in [0, 1, 64, usize::MAX] {
        let (rendered, _, info) =
            render_messages_and_images_with_compacted_history(&session, window);
        let info = info.unwrap();
        let visible_prompts = rendered
            .iter()
            .filter(|message| message.role == "user")
            .count();
        assert_eq!(
            info.hidden_user_prompts + visible_prompts,
            11,
            "window={window}"
        );
        assert!(rendered[0].content.ends_with(&format!(
            " Hidden user prompts: {}.",
            info.hidden_user_prompts
        )));
        // A partial window must snap back to a real prompt, not a tool result
        // envelope whose provider role happens to be user.
        assert_eq!(rendered[1].role, "user", "window={window}");
    }
}

#[test]
fn compacted_single_tool_heavy_turn_is_never_truncated() {
    let mut session = compacted_tool_session(1);
    let tool_result = session.messages[1].clone();
    for _ in 0..100 {
        session.messages.insert(1, tool_result.clone());
    }
    session.compaction.as_mut().unwrap().compacted_count = session.messages.len() - 1;
    let (_, _, info) = render_messages_and_images_with_compacted_history(&session, 1);
    let info = info.unwrap();
    assert_eq!(info.hidden_user_prompts, 0);
    assert_eq!(info.remaining_messages, 0);
}

#[test]
fn compacted_synthetic_user_envelopes_do_not_count_as_prompts() {
    let mut session = compacted_tool_session(1);
    for value in [
        "<system-reminder>internal continuation</system-reminder>",
        "[Attached image associated with the preceding tool result: diagram]",
        "You have 1 incomplete todo. Continue working, or update the todo tool.\nActionable todos:\n- capture: Capture input",
        "[Scheduled task]\nbackground work",
    ] {
        session.add_message(Role::User, vec![text(value)]);
        assert!(
            !stored_message_is_user_turn(session.messages.last().unwrap()),
            "{value}"
        );
    }
}

#[test]
fn compacted_background_wakes_match_effective_display_role() {
    let mut session = compacted_tool_session(10);
    for content in [
        "  🐝 **Swarm await finished**\n\nAll members done.",
        "**Background task** `build` · `bash` · ✓ completed · 7.1s · exit 0\n\n_Full output:_ `bg action=output task_id=build`",
        "**Background task progress** `build` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
    ] {
        assert!(crate::message::is_background_task_notification_content(
            content
        ));
        session.add_message(Role::User, vec![text(content)]);
        let wake = session.messages.pop().unwrap();
        assert!(!stored_message_is_user_turn(&wake));
        // Legacy wakes in the compacted prefix must not increase the absolute
        // prompt offset, even though they keep role=user in the raw history.
        session.messages.insert(1, wake);
        session.compaction.as_mut().unwrap().compacted_count += 1;
    }
    for window in [0, 1, 64, usize::MAX] {
        let (rendered, _, info) =
            render_messages_and_images_with_compacted_history(&session, window);
        let visible_human_prompts = rendered
            .iter()
            .filter(|message| {
                message.role == "user"
                    && !crate::message::is_background_task_notification_content(&message.content)
            })
            .count();
        assert_eq!(
            info.unwrap().hidden_user_prompts + visible_human_prompts,
            11,
            "window={window}"
        );
    }
    // Ordinary human discussion of background work must still be a prompt.
    session.add_message(
        Role::User,
        vec![text("Please explain background task progress")],
    );
    assert!(stored_message_is_user_turn(
        session.messages.last().unwrap()
    ));
}
