use super::*;

#[test]
fn centered_mode_centers_unstructured_messages_and_preserves_structured_left_blocks() {
    for role in ["user", "assistant", "meta", "usage", "error", "memory"] {
        assert_eq!(
            default_message_alignment(role, true),
            ratatui::layout::Alignment::Center,
            "role {role} should default to centered alignment"
        );
    }
    for role in ["tool", "system", "swarm", "background_task"] {
        assert_eq!(
            default_message_alignment(role, true),
            ratatui::layout::Alignment::Left,
            "role {role} should keep left/default alignment"
        );
    }
}

#[test]
fn prepare_body_preserves_multiline_user_prompt_lines() {
    let mut lines = Vec::new();
    let mut raw_plain_lines = Vec::new();
    let mut line_raw_overrides = Vec::new();
    let mut line_copy_offsets = Vec::new();
    let mut user_line_indices = Vec::new();

    push_user_prompt_lines(
        &mut lines,
        &mut raw_plain_lines,
        &mut line_raw_overrides,
        &mut line_copy_offsets,
        &mut user_line_indices,
        1,
        user_color(),
        "first line\nsecond line\n\nthird line",
        ratatui::layout::Alignment::Left,
    );

    let plain: Vec<String> = lines.iter().map(ui::line_plain_text).collect();

    assert_eq!(plain.len(), 4);
    assert_eq!(plain[0], "1› first line");
    assert_eq!(plain[1], "   second line");
    assert_eq!(plain[2], "   ");
    assert_eq!(plain[3], "   third line");
    assert_eq!(
        raw_plain_lines,
        vec!["first line", "second line", "", "third line"]
    );
    assert_eq!(user_line_indices, vec![0]);
    assert_eq!(line_copy_offsets, vec![3, 3, 3, 3]);
}

/// Regression coverage for issue #344: loading older compacted history above
/// an unchanged tail must be detected as a suffix match so scrolling to the
/// start of a long session reuses the prepared tail instead of re-rendering
/// the whole transcript per chunk.
#[test]
fn matching_suffix_len_detects_prepended_history() {
    use jcode_tui_messages::MessageBoundary;

    let old: Vec<DisplayMessage> = (0..4)
        .map(|i| DisplayMessage::system(format!("msg {i}")))
        .collect();

    // Base prepared from the old transcript: boundaries in transcript order.
    let base = PreparedMessages {
        wrapped_lines: Vec::new(),
        wrapped_plain_lines: Arc::new(Vec::new()),
        wrapped_copy_offsets: Arc::new(Vec::new()),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: old
            .iter()
            .map(|m| MessageBoundary {
                msg_hash: m.stable_cache_hash(),
                wrapped_len: 0,
                raw_len: 0,
                user_prompt_len: 0,
            })
            .collect(),
        mermaid_pending_epoch: None,
    };

    // New transcript: two older-history messages prepended, tail unchanged.
    let mut new_msgs: Vec<DisplayMessage> = vec![
        DisplayMessage::system("older history a"),
        DisplayMessage::system("older history b"),
    ];
    new_msgs.extend(old.iter().cloned());
    assert_eq!(matching_suffix_len(&base, &new_msgs), 4);

    // Changed tail: no suffix reuse.
    let mut changed = new_msgs.clone();
    changed.last_mut().unwrap().content = "edited".to_string();
    assert_eq!(matching_suffix_len(&base, &changed), 0);

    // Identical transcript: full suffix match.
    assert_eq!(matching_suffix_len(&base, &old), 4);
}

/// The prepared-header cache trades staleness for avoiding a bundle of disk
/// probes (auth files, goal JSON, skill overlay, update-channel stats) that
/// measured ~22ms for the auth probe alone. Its TTL is therefore sized for slow
/// background drift, which is only safe because credential changes are caught
/// by the signature instead. Guard that: a generation bump must be visible to
/// the signature so `/login` repaints on the next frame.
#[test]
fn auth_generation_change_invalidates_the_header_signature() {
    let before = crate::auth::auth_status_generation();
    crate::auth::bump_auth_status_generation_for_tests();
    let after = crate::auth::auth_status_generation();

    assert_ne!(
        before, after,
        "a credential change must bump the generation so the header signature \
         changes and the auth inventory repaints immediately"
    );
}

/// The TTL must stay sized to the underlying data rather than the frame rate.
/// A sub-second TTL reintroduces the bimodal cost this cache exists to remove:
/// every lapse pays a full disk-probe rebuild (p50 48ms in TUI_SLOW_FRAME logs)
/// to usually produce a byte-identical header.
#[test]
fn header_cache_ttl_is_not_sized_to_the_frame_rate() {
    assert!(
        HEADER_PREP_CACHE_TTL >= std::time::Duration::from_secs(5),
        "header TTL {HEADER_PREP_CACHE_TTL:?} is short enough to rebuild on a \
         render-loop cadence; user-visible changes are covered by the \
         signature, so the TTL should only bound slow background drift"
    );
}
