use super::*;

#[test]
fn compaction_notice_preserves_known_usage_and_handles_missing_metadata() {
    let mut event = crate::compaction::CompactionEvent {
        trigger: "auto_recovery".into(),
        pre_tokens: None,
        post_tokens: None,
        tokens_saved: None,
        duration_ms: None,
        messages_dropped: None,
        messages_compacted: None,
        summary_chars: None,
        active_messages: None,
    };
    let notice = |event: &crate::compaction::CompactionEvent| {
        App::format_compaction_complete_message(event, 200_000)
    };
    assert_eq!(notice(&event), "📦 Compacted (automatic recovery)");

    event.messages_dropped = Some(1);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): 1 message dropped"
    );
    event.messages_dropped = Some(1_234);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): 1,234 messages dropped"
    );
    event.messages_compacted = Some(1);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): 1 older message summarized"
    );
    event.messages_compacted = Some(0);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): 1,234 messages dropped"
    );

    event.pre_tokens = Some(123_456);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): from ~123,456 tokens (61.7%)"
    );
    assert_eq!(
        App::format_compaction_complete_message(&event, 0),
        "📦 Compacted (automatic recovery): from ~123,456 tokens"
    );
    event.pre_tokens = None;
    event.post_tokens = Some(0);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): now ~0 tokens (0.0%)"
    );
    event.post_tokens = Some(4_321);
    assert_eq!(
        App::format_compaction_complete_message(&event, 0),
        "📦 Compacted (automatic recovery): now ~4,321 tokens"
    );
    event.pre_tokens = Some(123_456);
    assert_eq!(
        notice(&event),
        "📦 Compacted (automatic recovery): ~123,456 tokens (61.7%) → ~4,321 tokens (2.2%)"
    );
}
