#[test]
fn test_body_cache_state_keeps_multiple_width_entries() {
    let key_a = BodyCacheKey {
        width: 40,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        pin_images: true,
        inline_images_visible: true,
        images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let key_b = BodyCacheKey {
        width: 41,
        ..key_a.clone()
    };
    let different_latex = BodyCacheKey {
        latex_rendering: crate::config::LatexRenderingMode::Unicode,
        ..key_a.clone()
    };

    let prepared_a = Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("a")],
        wrapped_plain_lines: Arc::new(vec!["a".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    });
    let prepared_b = Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("b")],
        wrapped_plain_lines: Arc::new(vec!["b".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    });

    let mut cache = BodyCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone(), 3, 0);
    cache.insert(key_b.clone(), prepared_b.clone(), 3, 0);

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected width 40 cache hit");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected width 41 cache hit");

    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.entries.len(), 2);
    assert!(cache.get_exact(&different_latex).is_none());
    assert!(cache.best_incremental_base(&different_latex, 4).is_none());
}

#[test]
fn test_body_cache_state_does_not_reuse_a_different_mermaid_aspect_profile() {
    let key = BodyCacheKey {
        width: 80,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::None,
        centered: false,
        mermaid_aspect_bucket: Some(1500),
        pin_images: true,
        inline_images_visible: true,
        images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let resized_key = BodyCacheKey {
        messages_version: 2,
        mermaid_aspect_bucket: Some(2500),
        ..key.clone()
    };
    let prepared = make_prepared_messages_with_content_bytes(128, "mermaid-profile-");

    let mut cache = BodyCacheState::default();
    cache.insert(key, prepared, 1, 0);

    assert!(cache.get_exact(&resized_key).is_none());
    assert!(cache.best_incremental_base(&resized_key, 2).is_none());
}

#[test]
fn test_body_cache_state_evicts_oldest_entries() {
    let mut cache = BodyCacheState::default();

    for idx in 0..(BODY_CACHE_MAX_ENTRIES + 2) {
        let key = BodyCacheKey {
            width: 40 + idx as u16,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
            mermaid_aspect_bucket: None,
            pin_images: true,
            inline_images_visible: true,
            images_signature: (0, 0),
            expanded_images_version: 0,
            swarm_members_signature: 0,
            latex_rendering: crate::config::LatexRenderingMode::Image,
        };
        let prepared = Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from(format!("{idx}"))],
            wrapped_plain_lines: Arc::new(vec![format!("{idx}")]),
            wrapped_copy_offsets: Arc::new(vec![0]),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
            message_boundaries: Vec::new(),
            mermaid_pending_epoch: None,
        });
        cache.insert(key, prepared, idx, 0);
    }

    assert_eq!(cache.entries.len(), BODY_CACHE_MAX_ENTRIES);
    assert!(
        cache.entries.iter().all(|entry| entry.key.width >= 42),
        "oldest widths should be evicted"
    );
}

#[test]
fn test_body_cache_state_accepts_large_single_entry_within_total_budget() {
    let key = BodyCacheKey {
        width: 120,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 99,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        pin_images: true,
        inline_images_visible: true,
        images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let prepared = make_prepared_messages_with_content_bytes(3 * 1024 * 1024, "body-large-");

    assert!(estimate_prepared_messages_bytes(&prepared) > 4 * 1024 * 1024);
    assert!(estimate_prepared_messages_bytes(&prepared) < BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 60, 0);

    let hit = cache
        .get_exact(&key)
        .expect("expected large body cache entry to be retained");
    assert!(Arc::ptr_eq(&hit, &prepared));
}

#[test]
fn test_body_cache_state_retains_oversized_hot_entry() {
    let key = BodyCacheKey {
        width: 140,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        pin_images: true,
        inline_images_visible: true,
        images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let prepared = make_oversized_prepared_messages("body-oversized-");

    assert!(estimate_prepared_messages_bytes(&prepared) > BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 120, 0);

    let hit = cache
        .get_exact(&key)
        .expect("expected oversized body cache entry to be retained as hot entry");
    assert!(Arc::ptr_eq(&hit, &prepared));
    assert!(cache.entries.is_empty());
    assert_eq!(cache.oversized_entries.len(), 1);
}

#[test]
fn test_body_cache_state_keeps_two_oversized_width_entries_hot() {
    let key_a = BodyCacheKey {
        width: 140,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        pin_images: true,
        inline_images_visible: true,
        images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let key_b = BodyCacheKey {
        width: 139,
        ..key_a.clone()
    };
    let prepared_a = make_oversized_prepared_messages("body-oversized-a-");
    let prepared_b = make_oversized_prepared_messages("body-oversized-b-");

    let mut cache = BodyCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone(), 120, 0);
    cache.insert(key_b.clone(), prepared_b.clone(), 120, 0);

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected first oversized body width to remain hot");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected second oversized body width to remain hot");
    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.oversized_entries.len(), 2);
}

#[test]
fn test_body_cache_state_uses_oversized_hot_entry_as_incremental_base() {
    let key = BodyCacheKey {
        width: 140,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        pin_images: true,
        inline_images_visible: true,
        images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let prepared = make_oversized_prepared_messages("body-oversized-base-");

    assert!(estimate_prepared_messages_bytes(&prepared) > BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 120, 0);

    let base = cache
        .best_incremental_base(
            &BodyCacheKey {
                messages_version: 121,
                ..key.clone()
            },
            121,
        )
        .expect("expected oversized hot entry to remain eligible as incremental base");
    assert!(Arc::ptr_eq(&base.0, &prepared));
    assert_eq!(base.1, 120);
}

/// Regression: a deferred mermaid render completing does not change the
/// transcript (`messages_version` stays put), so the staleness must be carried
/// by the prepared body itself. A base whose pending placeholder became stale
/// (epoch advanced) must be cut at the owning message and re-rendered, not
/// reused verbatim, or the placeholder sticks on screen forever.
#[test]
fn test_build_body_from_base_rerenders_stale_mermaid_pending_tail() {
    let width = 80;
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("draw me a diagram"),
            DisplayMessage::assistant("Sure!"),
        ],
        messages_version: 1,
        ..Default::default()
    };

    let mut base = super::prepare::prepare_body(&state, width, false);
    assert!(
        base.mermaid_pending_epoch.is_none(),
        "plain text body must not carry a pending stamp"
    );

    // Simulate a body whose assistant message baked in the deferred
    // placeholder at epoch E, where the live epoch has since moved past E.
    let placeholder_line_idx = base.message_boundaries[0].wrapped_len;
    base.wrapped_lines.insert(
        placeholder_line_idx,
        Line::from(markdown::MERMAID_PENDING_PLACEHOLDER_TEXT),
    );
    Arc::make_mut(&mut base.wrapped_plain_lines).insert(
        placeholder_line_idx,
        markdown::MERMAID_PENDING_PLACEHOLDER_TEXT.to_string(),
    );
    Arc::make_mut(&mut base.wrapped_copy_offsets).insert(placeholder_line_idx, 0);
    Arc::make_mut(&mut base.wrapped_line_map).insert(
        placeholder_line_idx,
        WrappedLineMap {
            raw_line: 0,
            start_col: 0,
            end_col: 0,
        },
    );
    for boundary in &mut base.message_boundaries[1..] {
        boundary.wrapped_len += 1;
    }
    let live_epoch = crate::tui::mermaid::deferred_render_epoch();
    base.mermaid_pending_epoch = Some(live_epoch.wrapping_sub(1));

    let (rebuilt, path) = super::prepare::build_body_from_base(
        &state,
        width,
        Arc::new(base),
        state.display_messages.len(),
        0,
        state.display_messages.len(),
    );

    assert!(
        !rebuilt
            .wrapped_lines
            .iter()
            .any(markdown::line_is_mermaid_pending_placeholder),
        "stale pending placeholder must be re-rendered away (path: {path})"
    );
    assert!(
        rebuilt.mermaid_pending_epoch.is_none()
            || rebuilt.mermaid_pending_epoch == Some(crate::tui::mermaid::deferred_render_epoch()),
        "rebuilt body must not keep a stale pending stamp"
    );
}

/// A base with a *current* pending stamp (epoch unchanged: the background
/// render is still running) is still reusable as-is; rebuilding early would
/// just churn frames without new information.
#[test]
fn test_build_body_from_base_keeps_current_mermaid_pending_base() {
    let width = 80;
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("draw me a diagram"),
            DisplayMessage::assistant("Sure!"),
        ],
        messages_version: 1,
        ..Default::default()
    };

    let mut base = super::prepare::prepare_body(&state, width, false);
    base.mermaid_pending_epoch = Some(crate::tui::mermaid::deferred_render_epoch());
    let base = Arc::new(base);
    let base_ptr = Arc::as_ptr(&base) as usize;

    let (rebuilt, path) = super::prepare::build_body_from_base(
        &state,
        width,
        base,
        state.display_messages.len(),
        0,
        state.display_messages.len(),
    );

    assert_eq!(
        Arc::as_ptr(&rebuilt) as usize,
        base_ptr,
        "current-epoch pending base should be reused exactly (path: {path})"
    );
}

#[test]
fn test_prepare_body_swarm_await_wake_is_not_a_numbered_prompt() {
    let report = "🐝 **Swarm await finished**\n\nAll members done.\n\nMember statuses:\n  ✓ jaguar (ready)\n\nRecent outputs:\n\n--- jaguar (ready) ---\n[Tool: bash · Inspect graph rules]\nworker transcript";
    let base_state = TestState {
        display_messages: vec![DisplayMessage::user("first prompt")],
        messages_version: 1,
        ..Default::default()
    };
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::user(report),
            DisplayMessage::user("second prompt"),
        ],
        messages_version: 2,
        ..Default::default()
    };
    for width in [24, 80, 140] {
        let base = Arc::new(super::prepare::prepare_body(&base_state, width, false));
        let incremental = super::prepare::prepare_body_incremental(&state, width, base, 1);
        let full = super::prepare::prepare_body(&state, width, false);
        for prepared in [&*incremental, &full] {
            let text = prepared
                .wrapped_lines
                .iter()
                .map(extract_line_text)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("🐝 ✓ 1/1"), "{text}");
            assert!(!text.contains("worker transcript"), "{text}");
            assert!(!text.contains("Swarm await finished"), "{text}");
            assert_eq!(prepared.user_prompt_texts.len(), 2);
        }
    }
}

#[test]
fn test_prepare_body_incremental_reuses_unique_prepared_arc() {
    let width = 80;
    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::assistant("initial answer"),
        ],
        messages_version: 1,
        ..Default::default()
    };
    let grown_state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::assistant("initial answer"),
            DisplayMessage::user("second prompt"),
            DisplayMessage::assistant("follow-up answer"),
        ],
        messages_version: 2,
        ..Default::default()
    };

    let prepared = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    let base_ptr = Arc::as_ptr(&prepared) as usize;
    let incremented = super::prepare::prepare_body_incremental(&grown_state, width, prepared, 2);

    assert_eq!(Arc::as_ptr(&incremented) as usize, base_ptr);
    assert!(
        incremented.wrapped_lines.len() >= 4,
        "expected incremental prep to append new wrapped content"
    );
}

#[test]
fn test_prepare_body_incremental_applies_compaction_prompt_offset() {
    // When earlier prompts have been hidden by compaction, the prompt number
    // gutter must include the hidden-prompt offset on BOTH the full-rebuild
    // path (prepare_body) and the incremental fast path (prepare_body_incremental).
    // Regression test: previously the incremental path forgot the offset, so
    // newly appended prompts were rendered with small local numbers while the
    // rest of the transcript kept the offset-adjusted numbers.
    let width = 80;
    let hidden = 70usize;

    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::assistant("initial answer"),
        ],
        messages_version: 1,
        compacted_hidden_user_prompts: hidden,
        ..Default::default()
    };
    let grown_state = TestState {
        display_messages: vec![
            DisplayMessage::user("first prompt"),
            DisplayMessage::assistant("initial answer"),
            DisplayMessage::user("second prompt"),
            DisplayMessage::assistant("follow-up answer"),
        ],
        messages_version: 2,
        compacted_hidden_user_prompts: hidden,
        ..Default::default()
    };

    let prepared = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    let incremented = super::prepare::prepare_body_incremental(&grown_state, width, prepared, 2);
    let full = super::prepare::prepare_body(&grown_state, width, false);

    let prompt_number_for = |prep: &PreparedMessages, content: &str| -> Option<usize> {
        for line in &prep.wrapped_lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            if let Some((prefix, rest)) = text.split_once("› ")
                && rest.starts_with(content)
            {
                return prefix.trim().parse::<usize>().ok();
            }
        }
        None
    };

    // first prompt is hidden+1, second prompt is hidden+2 on the full path.
    assert_eq!(prompt_number_for(&full, "first prompt"), Some(hidden + 1));
    assert_eq!(prompt_number_for(&full, "second prompt"), Some(hidden + 2));

    // The incremental path must agree with the full rebuild for the appended prompt.
    assert_eq!(
        prompt_number_for(&incremented, "second prompt"),
        Some(hidden + 2),
        "incremental prep must apply the compaction prompt offset"
    );
}

#[test]
fn test_full_prep_cache_state_keeps_multiple_width_entries() {
    let key_a = FullPrepCacheKey {
        width: 40,
        height: 20,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
        inline_images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
        inline_images_visible: true,
    };
    let key_b = FullPrepCacheKey {
        width: 39,
        ..key_a.clone()
    };
    let different_latex = FullPrepCacheKey {
        latex_rendering: crate::config::LatexRenderingMode::Unicode,
        ..key_a.clone()
    };

    let prepared_a = make_prepared_chat_frame(Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("a")],
        wrapped_plain_lines: Arc::new(vec!["a".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    }));
    let prepared_b = make_prepared_chat_frame(Arc::new(PreparedMessages {
        wrapped_lines: vec![Line::from("b")],
        wrapped_plain_lines: Arc::new(vec!["b".to_string()]),
        wrapped_copy_offsets: Arc::new(vec![0]),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
        message_boundaries: Vec::new(),
        mermaid_pending_epoch: None,
    }));

    let mut cache = FullPrepCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone());
    cache.insert(key_b.clone(), prepared_b.clone());

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected width 40 full prep cache hit");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected width 39 full prep cache hit");

    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.entries.len(), 2);
    assert!(cache.get_exact(&different_latex).is_none());
}

#[test]
fn test_full_prep_cache_state_does_not_reuse_a_different_mermaid_aspect_profile() {
    let key = FullPrepCacheKey {
        width: 80,
        height: 30,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::None,
        centered: false,
        mermaid_aspect_bucket: Some(1500),
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
        inline_images_signature: (0, 0),
        inline_images_visible: true,
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
    };
    let resized_key = FullPrepCacheKey {
        mermaid_aspect_bucket: Some(2500),
        ..key.clone()
    };
    let prepared = make_prepared_chat_frame_with_content_bytes(128, "mermaid-profile-");

    let mut cache = FullPrepCacheState::default();
    cache.insert(key, prepared);

    assert!(cache.get_exact(&resized_key).is_none());
}

#[test]
fn test_full_prep_cache_state_evicts_oldest_entries() {
    let mut cache = FullPrepCacheState::default();

    for idx in 0..(FULL_PREP_CACHE_MAX_ENTRIES + 2) {
        let key = FullPrepCacheKey {
            width: 40 + idx as u16,
            height: 20,
            diff_mode: crate::config::DiffDisplayMode::Off,
            messages_version: 1,
            diagram_mode: crate::config::DiagramDisplayMode::Pinned,
            centered: false,
            mermaid_aspect_bucket: None,
            is_processing: false,
            streaming_text_len: 0,
            streaming_text_hash: 0,
            batch_progress_hash: 0,
            inline_images_signature: (0, 0),
            expanded_images_version: 0,
            swarm_members_signature: 0,
            latex_rendering: crate::config::LatexRenderingMode::Image,
            inline_images_visible: true,
        };
        let prepared = make_prepared_chat_frame(Arc::new(PreparedMessages {
            wrapped_lines: vec![Line::from(format!("{idx}"))],
            wrapped_plain_lines: Arc::new(vec![format!("{idx}")]),
            wrapped_copy_offsets: Arc::new(vec![0]),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
            message_boundaries: Vec::new(),
            mermaid_pending_epoch: None,
        }));
        cache.insert(key, prepared);
    }

    assert_eq!(cache.entries.len(), FULL_PREP_CACHE_MAX_ENTRIES);
    assert!(
        cache.entries.iter().all(|entry| entry.key.width >= 42),
        "oldest widths should be evicted"
    );
}

#[test]
fn test_full_prep_cache_state_accepts_large_single_entry_within_total_budget() {
    let key = FullPrepCacheKey {
        width: 120,
        height: 40,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 99,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
        inline_images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
        inline_images_visible: true,
    };
    let prepared = make_prepared_chat_frame_with_content_bytes(3 * 1024 * 1024, "full-large-");

    assert!(estimate_prepared_chat_frame_bytes(&prepared) < FULL_PREP_CACHE_MAX_BYTES);

    let mut cache = FullPrepCacheState::default();
    cache.insert(key.clone(), prepared.clone());

    let hit = cache
        .get_exact(&key)
        .expect("expected large full prep cache entry to be retained");
    assert!(Arc::ptr_eq(&hit, &prepared));
}

#[test]
fn test_full_prep_cache_state_retains_oversized_hot_entry() {
    let key = FullPrepCacheKey {
        width: 140,
        height: 42,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        is_processing: true,
        streaming_text_len: 4096,
        streaming_text_hash: 12345,
        batch_progress_hash: 0,
        inline_images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
        inline_images_visible: true,
    };
    let prepared = make_oversized_prepared_chat_frame("full-oversized-");

    assert!(estimate_prepared_chat_frame_bytes(&prepared) > FULL_PREP_CACHE_MAX_BYTES);

    let mut cache = FullPrepCacheState::default();
    cache.insert(key.clone(), prepared.clone());

    let hit = cache
        .get_exact(&key)
        .expect("expected oversized full prep entry to be retained as hot entry");
    assert!(Arc::ptr_eq(&hit, &prepared));
    assert!(cache.entries.is_empty());
    assert_eq!(cache.oversized_entries.len(), 1);
}

#[test]
fn test_full_prep_cache_state_keeps_two_oversized_width_entries_hot() {
    let key_a = FullPrepCacheKey {
        width: 140,
        height: 42,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 120,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        mermaid_aspect_bucket: None,
        is_processing: true,
        streaming_text_len: 4096,
        streaming_text_hash: 12345,
        batch_progress_hash: 0,
        inline_images_signature: (0, 0),
        expanded_images_version: 0,
        swarm_members_signature: 0,
        latex_rendering: crate::config::LatexRenderingMode::Image,
        inline_images_visible: true,
    };
    let key_b = FullPrepCacheKey {
        width: 139,
        ..key_a.clone()
    };
    let prepared_a = make_oversized_prepared_chat_frame("full-oversized-a-");
    let prepared_b = make_oversized_prepared_chat_frame("full-oversized-b-");

    let mut cache = FullPrepCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone());
    cache.insert(key_b.clone(), prepared_b.clone());

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected first oversized full-prep width to remain hot");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected second oversized full-prep width to remain hot");
    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.oversized_entries.len(), 2);
}

/// 1x1 transparent PNG used to exercise the real inline-image header parse.
const BODY_ANCHOR_TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn anchored_tool_image(tool_id: &str) -> crate::session::RenderedImage {
    crate::session::RenderedImage {
        history_message_index: None,
        media_type: "image/png".to_string(),
        data: BODY_ANCHOR_TINY_PNG_B64.to_string(),
        label: Some("shot.png".to_string()),
        source: crate::session::RenderedImageSource::ToolResult {
            tool_name: "read".to_string(),
        },
        anchor: Some(crate::session::RenderedImageAnchor::ToolCall {
            id: tool_id.to_string(),
        }),
    }
}

fn read_tool_call(tool_id: &str) -> crate::message::ToolCall {
    crate::message::ToolCall {
        id: tool_id.to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "shot.png"}),
        intent: None,
        thought_signature: None,
    }
}

#[test]
fn test_prepare_body_anchors_tool_image_after_tool_message() {
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("read the screenshot"),
            DisplayMessage::tool("read shot.png", read_tool_call("tool-img-1")),
            DisplayMessage::assistant("that is a screenshot"),
        ],
        messages_version: 1,
        side_pane_images: vec![anchored_tool_image("tool-img-1")],
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };

    let prepared = super::prepare::prepare_body(&state, 80, false);
    assert_eq!(
        prepared.image_regions.len(),
        1,
        "anchored image should produce exactly one Fit region in the body"
    );
    let region = &prepared.image_regions[0];
    assert_eq!(region.render, jcode_tui_messages::ImageRegionRender::Fit);
    assert!(region.width > 0);
    assert!(region.height > 0, "tool images are shown by default");

    // The region must sit between the tool message and the assistant reply.
    let plain = &prepared.wrapped_plain_lines;
    let assistant_line = plain
        .iter()
        .position(|line| line.contains("that is a screenshot"))
        .expect("assistant reply should render");
    assert!(
        region.abs_line_idx < assistant_line,
        "image region (line {}) should render before the assistant reply (line {})",
        region.abs_line_idx,
        assistant_line
    );
    let label_line = plain
        .iter()
        .position(|line| line.contains("shot.png") && line.contains("1×1"))
        .expect("image label line should render");
    assert!(
        label_line < region.abs_line_idx,
        "label should sit directly above the image region"
    );
}

#[test]
fn test_prepare_body_incremental_anchors_image_on_new_tool_message() {
    let base_state = TestState {
        display_messages: vec![DisplayMessage::user("read the screenshot")],
        messages_version: 1,
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };
    let grown_state = TestState {
        display_messages: vec![
            DisplayMessage::user("read the screenshot"),
            DisplayMessage::tool("read shot.png", read_tool_call("tool-img-2")),
        ],
        messages_version: 2,
        side_pane_images: vec![anchored_tool_image("tool-img-2")],
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };

    let prepared = Arc::new(super::prepare::prepare_body(&base_state, 80, false));
    assert!(prepared.image_regions.is_empty());
    let incremented = super::prepare::prepare_body_incremental(&grown_state, 80, prepared, 1);
    assert_eq!(
        incremented.image_regions.len(),
        1,
        "incremental append should inject the anchored image region"
    );
    assert_eq!(
        incremented.image_regions[0].render,
        jcode_tui_messages::ImageRegionRender::Fit
    );

    // Incremental output must match a full rebuild.
    let full = super::prepare::prepare_body(&grown_state, 80, false);
    assert_eq!(full.image_regions.len(), 1);
    assert_eq!(
        full.image_regions[0].abs_line_idx,
        incremented.image_regions[0].abs_line_idx
    );
}

#[test]
fn test_prepare_body_skips_anchored_images_when_pin_images_off() {
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("read the screenshot"),
            DisplayMessage::tool("read shot.png", read_tool_call("tool-img-3")),
        ],
        messages_version: 1,
        side_pane_images: vec![anchored_tool_image("tool-img-3")],
        pin_images: false,
        inline_images_visible: true,
        ..Default::default()
    };

    let prepared = super::prepare::prepare_body(&state, 80, false);
    assert!(
        prepared.image_regions.is_empty(),
        "hidden images must not inject regions into the body"
    );
}

#[test]
fn test_prepare_body_collapses_anchored_images_when_inline_images_hidden() {
    let state = TestState {
        display_messages: vec![
            DisplayMessage::user("read the screenshot"),
            DisplayMessage::tool("read shot.png", read_tool_call("tool-img-4")),
        ],
        messages_version: 1,
        side_pane_images: vec![anchored_tool_image("tool-img-4")],
        pin_images: true,
        inline_images_visible: false,
        ..Default::default()
    };

    let prepared = super::prepare::prepare_body(&state, 80, false);
    assert!(
        prepared
            .image_regions
            .iter()
            .all(|region| region.height == 0),
        "collapsed images must not emit drawable regions"
    );
    let text = prepared.wrapped_plain_lines.join("\n");
    assert!(
        text.contains("shot.png"),
        "label stub should remain visible: {text:?}"
    );
    assert!(
        text.contains("show image"),
        "show badge should render on the stub: {text:?}"
    );
}

/// Assert two prepared bodies are byte-for-byte equivalent across every
/// observable array. Used to prove prefix-reuse output matches a fresh full
/// build.
#[path = "cache_equivalence_tests.rs"]
mod cache_equivalence;
use cache_equivalence::assert_prepared_equivalent;
