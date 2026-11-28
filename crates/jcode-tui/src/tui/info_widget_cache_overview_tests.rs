use super::super as widget;
use super::{lines_text, lines_text_concat};
use ratatui::layout::Rect;
use widget::{
    CacheHitInfo, CacheMissAttribution, InfoWidgetData, UsageInfo, UsageProvider, WidgetKind,
    render_kv_cache_widget,
};

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
    assert!(WidgetKind::all_by_priority().contains(&WidgetKind::KvCache));
    let overview = widget::render_overview_framed(&data, Rect::new(0, 0, 60, 30))
        .expect("cache information in Overview");
    let overview_text = lines_text_concat(&overview.all_lines());
    for fact in [
        "KV cache 90% yield",
        "last 94%",
        "session 39%",
        "20>",
        "69k miss",
        "provider switch",
        "69k missed",
    ] {
        assert!(
            overview_text.contains(fact),
            "missing {fact}: {overview_text}"
        );
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
fn overview_cache_height_and_fallback_match_all_miss_states() {
    for miss_count in [0, 1, 5, 6] {
        for prompt_tokens in [None, Some(1_000)] {
            let data = InfoWidgetData {
                cache_hit_info: Some(CacheHitInfo {
                    prompt_tokens,
                    read_tokens: 800,
                    miss_attributions: (0..miss_count)
                        .map(|index| CacheMissAttribution {
                            turn_number: index + 1,
                            call_index: 1,
                            missed_tokens: 1_000,
                            reason: "provider switch".into(),
                        })
                        .collect(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            for width in [0, 1, 4, 12, 30, 60] {
                let inner = Rect::new(0, 0, width, 30);
                let layout = widget::compute_page_layout(&data, width as usize, inner.height);
                let lines = widget::render_page(widget::InfoPageKind::CompactOnly, &data, inner);
                assert_eq!(layout.pages[0].height as usize, lines.len());
                assert!(lines.iter().all(|line| line.width() <= width as usize));
                let height = lines.len() as u16;
                assert!(!widget::overview_shows_section_of(
                    WidgetKind::KvCache,
                    &data,
                    Rect::new(0, 0, width + 2, height + 1),
                ));
                assert!(widget::overview_shows_section_of(
                    WidgetKind::KvCache,
                    &data,
                    Rect::new(0, 0, width + 2, height + 2),
                ));
            }
            let compact =
                widget::cache_render::render_kv_cache_compact(&data, Rect::new(0, 0, 60, 30));
            let text = lines_text_concat(&compact);
            if miss_count == 0 {
                assert!(text.contains("no misses"), "{text}");
            } else {
                assert!(text.contains("provider switch"), "{text}");
                assert!(text.contains(&format!("{miss_count}k missed")), "{text}");
            }
            if miss_count > 5 {
                assert!(text.contains("+1 more"), "{text}");
            }
        }
    }
}

#[test]
fn cache_is_available_alone_and_joins_other_overview_sections() {
    let mut data = InfoWidgetData {
        cache_hit_info: Some(CacheHitInfo::default()),
        ..Default::default()
    };
    assert!(data.available_widgets().contains(&WidgetKind::KvCache));
    assert!(!data.has_data_for(WidgetKind::Overview));
    data.queue_mode = Some(true);
    assert!(data.has_data_for(WidgetKind::Overview));
}

#[test]
fn overview_limit_reset_and_account_rows_match_height_at_narrow_widths() {
    for resets in [None, Some("2030-01-01T00:00:00Z".to_string())] {
        for primary in [false, true] {
            for hint in [None, Some("account hint".to_string())] {
                let mut usage = UsageInfo {
                    provider: UsageProvider::OpenAI,
                    limit_reached: true,
                    account_hint: hint.clone(),
                    ..Default::default()
                };
                if primary {
                    usage.five_hour_resets_at = resets.clone();
                } else {
                    usage.seven_day_resets_at = resets.clone();
                }
                let data = InfoWidgetData {
                    usage_info: Some(usage),
                    ..Default::default()
                };
                for width in [0, 1, 4, 12, 60] {
                    let inner = Rect::new(0, 0, width, 30);
                    let layout = widget::compute_page_layout(&data, width as usize, 30);
                    let lines =
                        widget::render_page(widget::InfoPageKind::CompactOnly, &data, inner);
                    assert_eq!(layout.pages[0].height as usize, lines.len());
                    assert!(lines.iter().all(|line| line.width() <= width as usize));
                    if width == 60 {
                        let text = lines_text_concat(&lines);
                        assert!(text.contains("Usage limit reached"));
                        assert_eq!(text.contains("Reset "), resets.is_some());
                        assert_eq!(text.contains("account hint"), hint.is_some());
                    }
                }
            }
        }
    }
}
