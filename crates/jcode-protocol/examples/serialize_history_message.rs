//! Serialize a `HistoryMessage` and round-trip it through JSON.
//!
//! Demonstrates the wire shape of the most-used DTO in
//! `jcode_protocol::HistoryMessage` and the four-value
//! `TranscriptMode` enum. The point is to be the smallest possible
//! "hello, world" for a downstream SDK that wants to talk to a
//! running jcode server over the protocol socket.
//!
//! Run with: `cargo run -p jcode-protocol --example serialize_history_message`

use jcode_protocol::{HistoryMessage, TranscriptMode};

fn main() {
    let original = HistoryMessage {
        role: "user".to_string(),
        content: "Show me the recent session activity.".to_string(),
        response_stats: None,
        tool_calls: None,
        tool_data: None,
    };

    let json = serde_json::to_string(&original).expect("serialize");
    println!("serialized:");
    println!("  {json}");

    let parsed: HistoryMessage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.role, original.role);
    assert_eq!(parsed.content, original.content);
    println!("round-trip: ok (role + content match)");

    for mode in [
        TranscriptMode::Insert,
        TranscriptMode::Append,
        TranscriptMode::Replace,
        TranscriptMode::Send,
    ] {
        let s = serde_json::to_string(&mode).expect("serialize enum");
        println!("  TranscriptMode::{mode:?} -> {s}");
    }
}
