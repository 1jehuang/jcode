//! Reproducible parsing-only comparison. This is not a live stream benchmark.
//! Run: justrust test --release -p jcode-provider-openai representative_delta_parsing -- --ignored --nocapture
use super::*;
use crate::websocket_health::{WEBSOCKET_FALLBACK_NOTICE, WebsocketEventClassification};
use std::hint::black_box;
use std::time::Instant;

fn legacy_activity(data: &str) -> bool {
    let value: Value = serde_json::from_str(data).unwrap();
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.starts_with("response.") || kind == "error")
}

// The former steady-state delta path performed these four independent Value
// parses: opaque rejection, runtime fallback, runtime activity, stream fallback.
// The final typed parse is common to both paths. Response-ID/output bookkeeping
// does not parse this fixture once the response ID is known.
fn legacy_delta(data: &str) -> Option<StreamEvent> {
    let envelope: Value = serde_json::from_str(data).unwrap();
    black_box(envelope.get("error"));
    black_box(
        envelope
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("response.")),
    );
    black_box(!legacy_activity(data) && data.to_lowercase().contains(WEBSOCKET_FALLBACK_NOTICE));
    black_box(legacy_activity(data));
    if !legacy_activity(data) && data.to_lowercase().contains(WEBSOCKET_FALLBACK_NOTICE) {
        return None;
    }
    if data
        .to_lowercase()
        .contains("stream disconnected before completion")
        && !legacy_activity(data)
    {
        return None;
    }
    let event: ResponseSseEvent = serde_json::from_str(data).unwrap();
    match event.kind.as_str() {
        "response.output_text.delta" => event
            .delta
            .filter(|delta| !delta.is_empty())
            .map(StreamEvent::TextDelta),
        _ => None,
    }
}

#[test]
#[ignore = "comparative CPU microbenchmark, run explicitly with --release --ignored --nocapture"]
fn representative_delta_parsing() {
    let data = r#"{"type":"response.output_text.delta","sequence_number":17,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":" a representative token","logprobs":[]}"#;
    let mut saw_text = false;
    let mut saw_thinking = false;
    let mut calls = HashMap::new();
    let mut completed = HashSet::new();
    let mut pending = VecDeque::new();
    let mut optimized = |data: &str| {
        let event = WebsocketEventClassification::parse(data);
        black_box(event.value().and_then(|value| value.get("error")));
        black_box(
            event
                .kind()
                .is_some_and(|kind| kind.starts_with("response.")),
        );
        black_box(event.is_fallback_notice(data));
        black_box(event.is_activity());
        parse_openai_response_event(
            data,
            &mut saw_text,
            &mut saw_thinking,
            &mut calls,
            &mut completed,
            &mut pending,
        )
    };
    assert!(
        matches!(legacy_delta(data), Some(StreamEvent::TextDelta(text)) if text == " a representative token")
    );
    assert!(
        matches!(optimized(data), Some(StreamEvent::TextDelta(text)) if text == " a representative token")
    );
    const ITERATIONS: u32 = 100_000;
    for _ in 0..2_000 {
        black_box(legacy_delta(black_box(data)));
        black_box(optimized(black_box(data)));
    }
    let mut legacy_times = Vec::new();
    let mut optimized_times = Vec::new();
    for round in 0..7 {
        // Alternate order to reduce systematic frequency/thermal bias.
        for legacy in if round % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                black_box(if legacy {
                    legacy_delta(black_box(data))
                } else {
                    optimized(black_box(data))
                });
            }
            let times = if legacy {
                &mut legacy_times
            } else {
                &mut optimized_times
            };
            times.push(start.elapsed().as_nanos() as f64 / f64::from(ITERATIONS));
        }
    }
    legacy_times.sort_by(f64::total_cmp);
    optimized_times.sort_by(f64::total_cmp);
    let before = legacy_times[3];
    let after = optimized_times[3];
    eprintln!(
        "representative delta ({} bytes), median of 7 x {ITERATIONS}: legacy {before:.1} ns/event, shared {after:.1} ns/event, {:.2}x, {:.1}% less CPU time",
        data.len(),
        before / after,
        100.0 * (before - after) / before
    );
    // No timing assertion: load and CPU frequency are outside test control.
}
