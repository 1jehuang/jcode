#[test]
fn kv_cache_widget_shows_session_hit_ratio() {
    let data = InfoWidgetData {
        cache_hit_info: Some(CacheHitInfo {
            reported_input_tokens: 20_000,
            prompt_tokens: Some(38_000),
            last_prompt_tokens: Some(10_000),
            read_tokens: 15_000,
            creation_tokens: 3_000,
            optimal_input_tokens: 16_667,
            optimal_read_tokens: None,
            last_reported_input_tokens: Some(10_000),
            last_read_tokens: Some(9_400),
            last_creation_tokens: Some(0),
            last_optimal_input_tokens: Some(9_895),
            miss_attributions: vec![CacheMissAttribution {
                turn_number: 20,
                call_index: 1,
                missed_tokens: 69_000,
                reason: "provider switch".to_string(),
            }],
        }),
        ..Default::default()
    };

    assert!(data.has_data_for(WidgetKind::KvCache));
    assert!(!WidgetKind::all_by_priority().contains(&WidgetKind::KvCache));
    let overview = super::render_overview_framed(&data, Rect::new(0, 0, 60, 30));
    if let Some(overview) = overview {
        assert!(!lines_text(&overview.all_lines()).contains("KV cache"));
    }
    let framed = render_kv_cache_widget(&data, Rect::new(0, 0, 40, 5));
    let text = lines_text(&framed.all_lines());

    // Body: the rates line plus one miss row. Headline and totals ride the border.
    assert_eq!(framed.lines.len(), 2);
    assert!(text.contains("KV cache"));
    assert!(text.contains("yield"));
    assert!(text.contains("90%"));
    assert!(text.contains("last "));
    assert!(text.contains("94%"));
    assert!(text.contains("session "));
    assert!(text.contains("39%"));
    assert!(text.contains("69k\n missed"), "{text}");
    assert!(text.contains("20>"));
    assert!(text.contains("69k miss"));
    assert!(text.contains("provider switch"));
}

#[test]
fn kv_cache_yield_pairs_reads_with_the_optimal_scope() {
    let mut cache = CacheHitInfo {
        prompt_tokens: Some(30_000),
        read_tokens: 16_000,
        optimal_input_tokens: 10_000,
        optimal_read_tokens: Some(9_000),
        ..Default::default()
    };
    assert!((cache.optimal_ratio().unwrap() - 0.9).abs() < 0.0001);
    let text = lines_text_concat(&[super::render_kv_cache_summary_line(&cache)]);
    assert!(text.contains("yield 90%"), "{text}");
    cache.optimal_read_tokens = None;
    assert_eq!(cache.optimal_ratio(), Some(1.0));
}
