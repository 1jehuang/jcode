use super::STREAM_KEEPALIVE_PONG_ID;
use crate::protocol::ServerEvent;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{self, MissedTickBehavior};

fn stream_keepalive_interval() -> Duration {
    if cfg!(test) {
        Duration::from_millis(50)
    } else {
        Duration::from_secs(30)
    }
}

pub(super) fn stream_keepalive_ticker() -> time::Interval {
    let interval = stream_keepalive_interval();
    let mut ticker = time::interval_at(time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

pub(super) fn send_stream_keepalive_broadcast(event_tx: &broadcast::Sender<ServerEvent>) {
    let _ = event_tx.send(ServerEvent::Pong {
        id: STREAM_KEEPALIVE_PONG_ID,
    });
}

pub(super) fn send_stream_keepalive_mpsc(event_tx: &mpsc::UnboundedSender<ServerEvent>) {
    let _ = event_tx.send(ServerEvent::Pong {
        id: STREAM_KEEPALIVE_PONG_ID,
    });
}

/// Wrap a single `ThinkingDelta` chunk for transport to the TUI.
///
/// Providers fragment reasoning into many small deltas. The TUI shows a single
/// `💭 ` marker at the head of a thinking burst; appending it to every chunk would
/// render `💭 chunk 💭 chunk 💭 chunk…` interleaved with the eventual answer once
/// markdown collapses single newlines into spaces. Caller owns `prefix_emitted`
/// and resets it on every `ThinkingStart` / `ThinkingEnd` / `ThinkingDone`.
pub(super) fn format_thinking_delta_payload(text: &str, prefix_emitted: &mut bool) -> String {
    if !*prefix_emitted {
        *prefix_emitted = true;
        format!("💭 {}", text)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::format_thinking_delta_payload;

    #[test]
    fn first_delta_in_burst_gets_prefix() {
        let mut emitted = false;
        let payload = format_thinking_delta_payload("Hello", &mut emitted);
        assert_eq!(payload, "💭 Hello");
        assert!(emitted);
    }

    #[test]
    fn subsequent_deltas_in_burst_are_raw() {
        let mut emitted = false;
        let _ = format_thinking_delta_payload("Hello", &mut emitted);
        let second = format_thinking_delta_payload(" world", &mut emitted);
        let third = format_thinking_delta_payload(", how are you", &mut emitted);
        assert_eq!(second, " world");
        assert_eq!(third, ", how are you");
    }

    #[test]
    fn reconcatenated_stream_has_exactly_one_prefix() {
        // Simulate a provider that fragments thinking into many chunks.
        let chunks = [
            "I",
            "'m",
            " reviewing",
            " the",
            " memory",
            " files",
            " in",
            " jcode",
            ".",
        ];
        let mut emitted = false;
        let joined: String = chunks
            .iter()
            .map(|c| format_thinking_delta_payload(c, &mut emitted))
            .collect();
        assert_eq!(joined, "💭 I'm reviewing the memory files in jcode.");
        assert_eq!(joined.matches("💭").count(), 1);
    }

    #[test]
    fn burst_boundary_resets_prefix() {
        // Caller resets the flag at burst boundaries (ThinkingStart/End/Done).
        let mut emitted = false;
        let _ = format_thinking_delta_payload("first burst", &mut emitted);
        // Boundary reached -> caller flips back to false.
        emitted = false;
        let next = format_thinking_delta_payload("second burst", &mut emitted);
        assert_eq!(next, "💭 second burst");
    }
}
