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
