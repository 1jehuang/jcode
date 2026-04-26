#[test]
fn test_body_cache_state_keeps_multiple_width_entries() {
    let key_a = BodyCacheKey {
        width: 40,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
    };
    let key_b = BodyCacheKey {
        width: 41,
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
    });

    let mut cache = BodyCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone(), 3);
    cache.insert(key_b.clone(), prepared_b.clone(), 3);

    let hit_a = cache
        .get_exact(&key_a)
        .expect("expected width 40 cache hit");
    let hit_b = cache
        .get_exact(&key_b)
        .expect("expected width 41 cache hit");

    assert!(Arc::ptr_eq(&hit_a, &prepared_a));
    assert!(Arc::ptr_eq(&hit_b, &prepared_b));
    assert_eq!(cache.entries.len(), 2);
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
        });
        cache.insert(key, prepared, idx);
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
    };
    let prepared = make_prepared_messages_with_content_bytes(3 * 1024 * 1024, "body-large-");

    assert!(estimate_prepared_messages_bytes(&prepared) > 4 * 1024 * 1024);
    assert!(estimate_prepared_messages_bytes(&prepared) < BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 60);

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
    };
    let prepared = make_oversized_prepared_messages("body-oversized-");

    assert!(estimate_prepared_messages_bytes(&prepared) > BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 120);

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
    };
    let key_b = BodyCacheKey {
        width: 139,
        ..key_a.clone()
    };
    let prepared_a = make_oversized_prepared_messages("body-oversized-a-");
    let prepared_b = make_oversized_prepared_messages("body-oversized-b-");

    let mut cache = BodyCacheState::default();
    cache.insert(key_a.clone(), prepared_a.clone(), 120);
    cache.insert(key_b.clone(), prepared_b.clone(), 120);

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
    };
    let prepared = make_oversized_prepared_messages("body-oversized-base-");

    assert!(estimate_prepared_messages_bytes(&prepared) > BODY_CACHE_MAX_BYTES);

    let mut cache = BodyCacheState::default();
    cache.insert(key.clone(), prepared.clone(), 120);

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
fn test_full_prep_cache_state_keeps_multiple_width_entries() {
    let key_a = FullPrepCacheKey {
        width: 40,
        height: 20,
        diff_mode: crate::config::DiffDisplayMode::Off,
        messages_version: 1,
        diagram_mode: crate::config::DiagramDisplayMode::Pinned,
        centered: false,
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
    };
    let key_b = FullPrepCacheKey {
        width: 39,
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
            is_processing: false,
            streaming_text_len: 0,
            streaming_text_hash: 0,
            batch_progress_hash: 0,
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
        is_processing: false,
        streaming_text_len: 0,
        streaming_text_hash: 0,
        batch_progress_hash: 0,
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
        is_processing: true,
        streaming_text_len: 4096,
        streaming_text_hash: 12345,
        batch_progress_hash: 0,
    };
    let prepared = make_oversized_prepared_chat_frame("full-oversized-");

    assert!(estimate_prepared_chat_frame_bytes(&prepared) <= FULL_PREP_CACHE_MAX_BYTES);

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
        is_processing: true,
        streaming_text_len: 4096,
        streaming_text_hash: 12345,
        batch_progress_hash: 0,
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
