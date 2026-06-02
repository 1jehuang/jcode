//! End-to-end tests for thinking-burst delta wrapping in streaming paths.
//!
//! Verifies that the `💭 ` marker is emitted exactly once per thinking burst
//! even when the provider fragments reasoning across many `ThinkingDelta`
//! events. Regression test for a bug where every delta was wrapped with
//! `format!("💭 {}\n", text)`; once markdown collapsed the single newlines
//! into spaces, the 💭 emoji ended up interleaved every ~10 characters across
//! the visible response on Ghostty / macOS.

use crate::test_support::*;
use tokio::sync::broadcast;

/// Set `JCODE_SHOW_THINKING=true` for the duration of the test so the streaming
/// paths actually surface `ThinkingDelta` events as `TextDelta` payloads. We
/// use the env override instead of writing `config.toml` because the global
/// config cache only re-fingerprints the env keys it explicitly tracks, and
/// `JCODE_SHOW_THINKING` is one of them — that gives us a reliable per-test
/// toggle that survives the `OnceLock`-backed cache across multiple tests in
/// the same process.
fn enable_show_thinking() -> EnvVarGuard {
    let guard = EnvVarGuard::set("JCODE_SHOW_THINKING", "true");
    // The global config cache has a 500 ms throttle in non-test builds (the
    // `jcode-base` crate is compiled as a dep here, so `cfg!(test)` is false
    // for it). Force a reload immediately so the next `config()` call observes
    // our env override instead of a cached snapshot from a previous test.
    jcode::config::invalidate_config_cache();
    guard
}

/// Counterpart to `enable_show_thinking` for tests that want to assert the
/// thinking path is fully suppressed: also invalidate the cache so we don't
/// inherit a `show_thinking = true` snapshot from a sibling test.
fn ensure_show_thinking_disabled() {
    jcode::config::invalidate_config_cache();
}

/// Drain every `ServerEvent` that arrives on the broadcast receiver until the
/// agent task finishes (the sender is dropped once `run_once_streaming`
/// returns).
async fn collect_server_events(mut rx: broadcast::Receiver<ServerEvent>) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    loop {
        match rx.recv().await {
            Ok(event) => events.push(event),
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
        }
    }
    events
}

/// Concatenate every `TextDelta { text }` payload in order, mirroring what a
/// remote TUI client renders into the transcript.
fn reconstruct_text(events: &[ServerEvent]) -> String {
    let mut out = String::new();
    for event in events {
        if let ServerEvent::TextDelta { text } = event {
            out.push_str(text);
        }
    }
    out
}

/// Regression: a single thinking burst fragmented into many `ThinkingDelta`
/// events must surface exactly one `💭 ` marker, not one per delta.
#[tokio::test]
async fn streaming_thinking_emits_single_marker_per_burst() -> Result<()> {
    let _env = setup_test_env()?;
    let _show_thinking = enable_show_thinking();

    let provider = MockProvider::new();
    // Fragmented thinking burst: 8 tiny deltas the way reasoning providers
    // actually stream them today (Claude / OpenAI / Gemini).
    provider.queue_response(vec![
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingDelta("I".into()),
        StreamEvent::ThinkingDelta("'m".into()),
        StreamEvent::ThinkingDelta(" reviewing".into()),
        StreamEvent::ThinkingDelta(" the".into()),
        StreamEvent::ThinkingDelta(" memory".into()),
        StreamEvent::ThinkingDelta(" files".into()),
        StreamEvent::ThinkingDelta(" in".into()),
        StreamEvent::ThinkingDelta(" jcode.".into()),
        StreamEvent::ThinkingEnd,
        StreamEvent::TextDelta("## Answer\n\nDone.".into()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        },
        StreamEvent::SessionId("session-thinking-1".into()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let (tx, rx) = broadcast::channel(256);
    let collector = tokio::spawn(collect_server_events(rx));
    agent.run_once_streaming("Test", tx).await?;
    let events = collector.await?;

    let stream_text = reconstruct_text(&events);

    // Exactly one 💭 in the entire reconstructed stream.
    assert_eq!(
        stream_text.matches("💭").count(),
        1,
        "expected exactly one 💭 marker per thinking burst, got stream={:?}",
        stream_text
    );

    // The marker prefixes the very first thinking fragment.
    assert!(
        stream_text.starts_with("💭 I"),
        "expected 💭 to lead the burst, got stream={:?}",
        stream_text
    );

    // The reconstructed thinking body matches the concatenated fragments
    // with no interleaved newlines or stray markers between them.
    assert!(
        stream_text.contains("💭 I'm reviewing the memory files in jcode."),
        "expected thinking body reconstruct cleanly, got stream={:?}",
        stream_text
    );

    // ThinkingEnd must insert a blank line so the answer does not visually
    // fuse onto the tail of the thinking burst.
    assert!(
        stream_text.contains("💭 I'm reviewing the memory files in jcode.\n\n## Answer"),
        "expected `\\n\\n` separator between thinking burst and answer, got stream={:?}",
        stream_text
    );

    Ok(())
}

/// A turn that contains two separate thinking bursts (separated by tool use or
/// inter-burst answer text) must emit one 💭 marker per burst, not one total.
#[tokio::test]
async fn streaming_thinking_resets_prefix_between_bursts() -> Result<()> {
    let _env = setup_test_env()?;
    let _show_thinking = enable_show_thinking();

    let provider = MockProvider::new();
    provider.queue_response(vec![
        // First burst.
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingDelta("first".into()),
        StreamEvent::ThinkingDelta(" burst".into()),
        StreamEvent::ThinkingEnd,
        StreamEvent::TextDelta("Interlude.\n".into()),
        // Second burst.
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingDelta("second".into()),
        StreamEvent::ThinkingDelta(" burst".into()),
        StreamEvent::ThinkingEnd,
        StreamEvent::TextDelta("Answer.".into()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        },
        StreamEvent::SessionId("session-thinking-2".into()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let (tx, rx) = broadcast::channel(256);
    let collector = tokio::spawn(collect_server_events(rx));
    agent.run_once_streaming("Test", tx).await?;
    let events = collector.await?;

    let stream_text = reconstruct_text(&events);

    assert_eq!(
        stream_text.matches("💭").count(),
        2,
        "expected one 💭 marker per burst (2 total), got stream={:?}",
        stream_text
    );
    assert!(stream_text.contains("💭 first burst"));
    assert!(stream_text.contains("💭 second burst"));

    Ok(())
}

/// When `display.show_thinking = false`, no `💭` marker (or any thinking
/// payload at all) must reach the broadcast stream, but the answer text must
/// still arrive cleanly with no spurious separator from the suppressed burst.
#[tokio::test]
async fn streaming_thinking_hidden_emits_no_marker() -> Result<()> {
    let _env = setup_test_env()?;
    // Don't call enable_show_thinking(); rely on the config-types default
    // (`show_thinking = false`). Still flush the cache so a sibling test's
    // `show_thinking = true` snapshot doesn't leak into this assertion.
    ensure_show_thinking_disabled();

    let provider = MockProvider::new();
    provider.queue_response(vec![
        StreamEvent::ThinkingStart,
        StreamEvent::ThinkingDelta("hidden".into()),
        StreamEvent::ThinkingDelta(" reasoning".into()),
        StreamEvent::ThinkingEnd,
        StreamEvent::TextDelta("Just the answer.".into()),
        StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        },
        StreamEvent::SessionId("session-thinking-3".into()),
    ]);

    let provider: Arc<dyn jcode::provider::Provider> = Arc::new(provider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);

    let (tx, rx) = broadcast::channel(256);
    let collector = tokio::spawn(collect_server_events(rx));
    agent.run_once_streaming("Test", tx).await?;
    let events = collector.await?;

    let stream_text = reconstruct_text(&events);

    assert_eq!(
        stream_text.matches("💭").count(),
        0,
        "expected zero 💭 markers when show_thinking is disabled, got stream={:?}",
        stream_text
    );
    assert!(
        !stream_text.contains("hidden reasoning"),
        "expected thinking body to be suppressed, got stream={:?}",
        stream_text
    );
    assert!(
        stream_text.contains("Just the answer."),
        "expected the visible answer to still stream, got stream={:?}",
        stream_text
    );

    Ok(())
}
