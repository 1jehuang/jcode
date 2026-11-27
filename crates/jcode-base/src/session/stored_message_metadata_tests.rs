use super::*;

fn message() -> StoredMessage {
    StoredMessage {
        id: "id\n\"\\\0界🦀".repeat(16),
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "escaped\n\"界🦀".repeat(128),
            cache_control: None,
        }]
        .into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }
}

fn legacy_metadata(message: &StoredMessage) -> StoredMessage {
    let mut metadata = message.clone();
    metadata.content = Default::default();
    metadata
}

#[test]
fn stored_metadata_all_options_exact_bytes_and_snapshot_sizes() {
    for role in [Role::User, Role::Assistant] {
        for display_role in [
            None,
            Some(StoredDisplayRole::System),
            Some(StoredDisplayRole::BackgroundTask),
            Some(StoredDisplayRole::FileActivity),
        ] {
            for timestamp in [
                None,
                Some(chrono::DateTime::<chrono::Utc>::MIN_UTC),
                Some(chrono::DateTime::<chrono::Utc>::MAX_UTC),
                chrono::DateTime::from_timestamp(-1, 999_999_999),
            ] {
                for duration in [None, Some(0), Some(u64::MAX)] {
                    for mask in 0..9 {
                        let mut message = message();
                        message.role = role.clone();
                        message.display_role = display_role;
                        message.timestamp = timestamp;
                        message.tool_duration_ms = duration;
                        message.token_usage = (mask != 8).then_some(StoredTokenUsage {
                            prompt_tokens: (mask & 1 != 0).then_some(u64::MAX),
                            input_tokens: u64::MAX,
                            output_tokens: 0,
                            cache_read_input_tokens: (mask & 2 != 0).then_some(0),
                            cache_creation_input_tokens: (mask & 4 != 0).then_some(u64::MAX),
                            request_elapsed_ms: Some(7_890),
                        });
                        assert_eq!(
                            serde_json::to_vec(&metadata(&message)).unwrap(),
                            serde_json::to_vec(&legacy_metadata(&message)).unwrap()
                        );
                        let snapshot = message.clone();
                        for _ in 0..2 {
                            assert_eq!(
                                json_bytes(&message),
                                serde_json::to_vec(&message).unwrap().len()
                            );
                            message.content.clear();
                            message.id.push_str("changed");
                        }
                        assert_eq!(
                            json_bytes(&snapshot),
                            serde_json::to_vec(&snapshot).unwrap().len()
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "paired old/new stored metadata component benchmark"]
fn stored_metadata_component_benchmark() {
    use std::{hint::black_box, time::Instant};
    for populated in [false, true] {
        let mut message = message();
        if populated {
            message.display_role = Some(StoredDisplayRole::BackgroundTask);
            message.timestamp = chrono::DateTime::from_timestamp(1_794_888_038, 123_456_789);
            message.tool_duration_ms = Some(12_345);
            message.token_usage = Some(StoredTokenUsage {
                prompt_tokens: Some(12_345),
                input_tokens: 6_789,
                output_tokens: 123,
                cache_read_input_tokens: Some(5_432),
                cache_creation_input_tokens: Some(321),
                request_elapsed_ms: Some(7_890),
            });
        }
        json_bytes(&message);
        let iterations = 100_000;
        for round in 0..5 {
            let mut old = std::time::Duration::ZERO;
            let mut new = std::time::Duration::ZERO;
            for baseline in [true, false, false, true] {
                let start = Instant::now();
                for _ in 0..iterations {
                    let message = black_box(&message);
                    black_box(if baseline {
                        let metadata = legacy_metadata(message);
                        estimate_json_bytes(&metadata).saturating_sub(2)
                            + message.content.serialized_json_bytes()
                    } else {
                        json_bytes(message)
                    });
                }
                if baseline {
                    old += start.elapsed();
                } else {
                    new += start.elapsed();
                }
            }
            eprintln!(
                "stored metadata ABBA round={round} populated={populated} iterations_per_arm={iterations} old_sum={old:?} new_sum={new:?} speedup={:.3}",
                old.as_secs_f64() / new.as_secs_f64()
            );
        }
    }
}
