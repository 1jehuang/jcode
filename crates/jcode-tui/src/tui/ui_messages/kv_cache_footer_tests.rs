#[test]
fn preserves_prefix_evidence_tag_and_keeps_footer_width_bounded() {
    let body = "KV cache miss [low read] [prefix unchanged] turn 2 call 3: 4.35K cached / 20K expected tokens: ~15.6K tokens resent (cause unconfirmed).";
    assert_eq!(
        super::kv_cache_footer_summary(body).as_deref(),
        Some("15.6K KV cache miss · low read · prefix unchanged · 22% hit")
    );
    let message = super::DisplayMessage::system(format!("ℹ️ {body}"));
    for width in [0_u16, 1, 2, 3, 12, 30, 80, 120] {
        let lines = super::render_kv_cache_footer(&message, width).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].width() <= usize::from(width));
        if width >= 80 {
            assert!(lines[0].to_string().contains("prefix unchanged"));
        }
    }
    let unrelated =
        "KV cache miss [low read] turn 2: ~15K tokens resent (detail mentions [prefix unchanged]).";
    assert!(
        !super::kv_cache_footer_summary(unrelated)
            .unwrap()
            .contains("prefix unchanged")
    );
}

#[test]
fn cache_miss_footer_colors_only_the_label_by_absolute_token_loss() {
    for (amount, expected) in [
        ("0", super::dim_color()),
        ("4.2K", super::dim_color()),
        ("9,999", super::dim_color()),
        ("10K", super::warning_color()),
        ("20.4k", super::warning_color()),
        ("49,999", super::warning_color()),
        ("50K", super::error_color()),
        ("86.1K", super::error_color()),
        ("1.2M", super::error_color()),
        ("unknown", super::warning_color()),
        ("NaN", super::warning_color()),
        ("inf", super::warning_color()),
        ("-1", super::warning_color()),
    ] {
        // Hit percentage is intentionally identical across severities.
        let message = super::DisplayMessage::system(format!(
            "ℹ️ KV cache miss [low read] [prefix unchanged]: 4K cached / 20K expected tokens: ~{amount} tokens resent"
        ));
        let lines = super::render_kv_cache_footer(&message, 160).unwrap();
        assert_eq!(lines[0].spans[0].style.fg, Some(super::dim_color()));
        assert_eq!(lines[0].spans[1].style.fg, Some(expected), "{amount}");
        assert_eq!(lines[0].spans[1].content, format!("{amount} KV cache miss"));
        assert_eq!(lines[0].spans[2].style.fg, Some(super::dim_color()));
        assert_eq!(
            lines[0].spans[2].content,
            " · low read · prefix unchanged · 20% hit"
        );
        for width in 0..=100 {
            let clipped = super::render_kv_cache_footer(&message, width).unwrap();
            assert_eq!(clipped.len(), 1);
            assert!(
                clipped[0].width() <= usize::from(width),
                "{amount}, {width}"
            );
        }
    }
}

#[test]
fn missing_amount_stays_amber_and_refreshes_are_not_loss_severity() {
    for body in [
        "KV cache miss [low read]: provider reported no cache read",
        "KV cache miss without structured evidence",
    ] {
        let message = super::DisplayMessage::system(body);
        let lines = super::render_kv_cache_footer(&message, 120).unwrap();
        assert_eq!(lines[0].spans[1].style.fg, Some(super::warning_color()));
    }
    for (icon, color) in [("ℹ️ ", super::dim_color()), ("⚠️ ", super::warning_color())] {
        let message = super::DisplayMessage::system(format!(
            "{icon}KV cache refresh [compaction]: ~100K tokens resent"
        ));
        let lines = super::render_kv_cache_footer(&message, 120).unwrap();
        assert_eq!(lines[0].spans[1].style.fg, Some(color));
        assert_eq!(lines[0].spans[2].style.fg, Some(super::dim_color()));
    }
}

#[test]
fn only_system_cache_notices_attach() {
    let body = "KV cache miss [low read]: ~4K tokens resent";
    let message = super::DisplayMessage::system(format!("ℹ️ {body}"));
    assert!(!super::needs_message_separator(&message));
    let lines = super::render_kv_cache_footer(&message, 80).unwrap();
    assert_eq!(lines[0].spans[1].style.fg, Some(super::dim_color()));
    let assistant = super::DisplayMessage::assistant(body);
    assert!(super::needs_message_separator(&assistant));
    assert!(super::render_kv_cache_footer(&assistant, 80).is_none());
    assert!(super::needs_message_separator(
        &super::DisplayMessage::system("ordinary notice")
    ));
    let refresh = super::DisplayMessage::system("KV cache refresh [compaction]: ~4K tokens resent");
    assert!(!super::needs_message_separator(&refresh));
    assert_eq!(
        super::render_kv_cache_footer(&refresh, 80).unwrap()[0].spans[1]
            .style
            .fg,
        Some(super::dim_color())
    );
}

#[test]
fn summarizes_miss_reason_missed_and_hit_pct() {
    let s = super::kv_cache_footer_summary(
        "KV cache miss [provider switch] turn 3 call 2: 28.6K cached / 47.6K expected tokens: ~19.0K tokens resent (x). See KV_CACHE_USAGE in logs.",
    );
    assert_eq!(
        s.as_deref(),
        Some("19.0K KV cache miss · provider switch · 60% hit")
    );
}
