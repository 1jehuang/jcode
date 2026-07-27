// Tests for the per-frame `command_suggestions()` memo.
//
// The memo exists purely as a performance measure: the suggestion list is read
// ~8 times while composing a single frame. It must never change what the user
// sees, so these tests pin the observable behavior (equality with the uncached
// path across state transitions) rather than the caching mechanics.

/// The memo must be transparent: for any given state, the cached read has to
/// agree with a freshly computed one.
#[test]
fn cached_suggestions_match_uncached_across_input_mutations() {
    let mut app = create_test_app();

    for input in [
        "", "/", "/m", "/mod", "/model", "/help", "/conifg", "/goals ", "/rewind ", "not a command",
        "/",
    ] {
        app.input = input.to_string();
        app.cursor_pos = app.input.len();

        let signature = app.command_suggestions_signature();
        let expected = app.command_suggestions_uncached(&signature);

        // First read populates the memo, second read hits it. Both must equal
        // the uncached result.
        let first = app.command_suggestions();
        let second = app.command_suggestions();

        assert_eq!(
            first, expected,
            "cached suggestions diverged from uncached for input {input:?}"
        );
        assert_eq!(
            second, expected,
            "memo hit diverged from uncached for input {input:?}"
        );
    }
}

/// Typing must invalidate the memo even within a single frame, since the entry
/// is keyed on the exact input buffer.
#[test]
fn editing_input_within_a_frame_invalidates_the_memo() {
    let mut app = create_test_app();

    app.input = "/hel".to_string();
    let for_hel = app.command_suggestions();

    // No epoch bump: simulate a second read after an edit inside one frame.
    app.input = "/mod".to_string();
    let for_mod = app.command_suggestions();

    assert_ne!(
        for_hel, for_mod,
        "changing the input must not return the previous input's suggestions"
    );
    assert!(
        for_mod.iter().any(|(cmd, _)| cmd.starts_with("/model")),
        "expected /model suggestions after typing /mod, got {for_mod:?}"
    );
}

/// The memo is scoped to one frame because the suggestion list also depends on
/// mutable session state that is not part of the key. Advancing the epoch (as
/// `draw` does per frame) must force recomputation even when the input buffer
/// is byte-identical.
#[test]
fn advancing_the_epoch_forces_recomputation_for_identical_input() {
    let mut app = create_test_app();
    app.input = "/rewind ".to_string();

    let before = app.command_suggestions();

    // Mutate state the key deliberately does not track, then advance the frame.
    app.session.messages.push(crate::session::StoredMessage {
        id: "msg-rewind-target".to_string(),
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "a new rewind target".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    app.advance_command_suggestions_epoch();

    let after = app.command_suggestions();
    let signature = app.command_suggestions_signature();
    let expected = app.command_suggestions_uncached(&signature);

    assert_eq!(
        after, expected,
        "after an epoch bump the memo must reflect current session state"
    );
    assert_ne!(
        before, after,
        "a new rewind target should appear once the frame advances"
    );
}

/// A pending interactive prompt turns the composer into an answer box. That
/// transition changes the answer without touching the input buffer, so it must
/// be part of the memo key.
#[test]
fn pending_prompt_transition_invalidates_the_memo() {
    let mut app = create_test_app();
    app.input = "/c".to_string();

    let normal = app.command_suggestions();
    assert!(
        normal.len() > 1,
        "expected the full palette before a prompt is pending, got {normal:?}"
    );

    // Enter a pending-login state with the same input buffer and no epoch bump.
    app.pending_login = Some(PendingLogin::ClaudeAccount {
        verifier: "test-verifier".to_string(),
        label: "test-account".to_string(),
        redirect_uri: None,
    });
    let pending = app.command_suggestions();

    assert_eq!(
        pending,
        vec![("/cancel".to_string(), "Cancel the pending prompt")],
        "a pending prompt must suppress the palette down to /cancel"
    );
}
