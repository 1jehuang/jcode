use super::*;
use crate::{stable_hash_bytes, stable_message_hash};

fn legacy_metadata(message: &Message) -> Message {
    Message {
        role: message.role.clone(),
        content: Default::default(),
        timestamp: message.timestamp,
        tool_duration_ms: message.tool_duration_ms,
    }
}

fn message() -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "escaped\n\"\\\0界🦀".repeat(128),
            cache_control: None,
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn exact_metadata_bytes_lengths_hashes_and_snapshots() {
    for role in [Role::User, Role::Assistant] {
        for timestamp in [
            None,
            Some(chrono::DateTime::<chrono::Utc>::MIN_UTC),
            Some(chrono::DateTime::<chrono::Utc>::MAX_UTC),
            chrono::DateTime::from_timestamp(-1, 999_999_999),
        ] {
            for duration in [None, Some(0), Some(u64::MAX)] {
                let mut message = message();
                message.role = role.clone();
                message.timestamp = timestamp;
                message.tool_duration_ms = duration;
                let legacy = serde_json::to_vec(&legacy_metadata(&message)).unwrap();
                assert_eq!(
                    serde_json::to_vec(&message.metadata_only()).unwrap(),
                    legacy
                );
                assert_eq!(json_bytes(&message.metadata_only()).unwrap(), legacy.len());
                let snapshot = message.clone();
                for _ in 0..2 {
                    let bytes = serde_json::to_vec(&message).unwrap();
                    assert_eq!(message.serialized_json_bytes(), bytes.len());
                    assert_eq!(stable_message_hash(&message), stable_hash_bytes(&bytes));
                    message.content.clear();
                }
                let bytes = serde_json::to_vec(&snapshot).unwrap();
                assert_eq!(snapshot.serialized_json_bytes(), bytes.len());
                assert_eq!(stable_message_hash(&snapshot), stable_hash_bytes(&bytes));
            }
        }
    }
}

#[test]
fn counting_writer_preserves_serialization_errors() {
    struct Failure;
    impl serde::Serialize for Failure {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("expected failure"))
        }
    }
    assert_eq!(
        json_bytes(&Failure).unwrap_err().to_string(),
        serde_json::to_vec(&Failure).unwrap_err().to_string()
    );
}

#[test]
#[ignore = "paired old/new metadata component benchmark"]
fn metadata_component_benchmark() {
    use std::{hint::black_box, time::Instant};
    for populated in [false, true] {
        let mut message = message();
        if populated {
            message.timestamp = chrono::DateTime::from_timestamp(1_794_888_038, 123_456_789);
            message.tool_duration_ms = Some(12_345);
        }
        stable_message_hash(&message);
        message.serialized_json_bytes();
        let iterations = 100_000;
        for round in 0..5 {
            for hash in [false, true] {
                let mut old = std::time::Duration::ZERO;
                let mut new = std::time::Duration::ZERO;
                for baseline in [true, false, false, true] {
                    let start = Instant::now();
                    for _ in 0..iterations {
                        let message = black_box(&message);
                        black_box(if baseline {
                            let metadata = serde_json::to_vec(&legacy_metadata(message)).unwrap();
                            if hash {
                                message
                                    .content
                                    .cached_message_hash(&metadata, || unreachable!())
                                    as usize
                            } else {
                                metadata.len().saturating_sub(2)
                                    + message.content.serialized_json_bytes()
                            }
                        } else if hash {
                            stable_message_hash(message) as usize
                        } else {
                            message.serialized_json_bytes()
                        });
                    }
                    if baseline {
                        old += start.elapsed();
                    } else {
                        new += start.elapsed();
                    }
                }
                eprintln!(
                    "metadata ABBA round={round} populated={populated} warm_hash={hash} iterations_per_arm={iterations} old_sum={old:?} new_sum={new:?} speedup={:.3}",
                    old.as_secs_f64() / new.as_secs_f64()
                );
            }
        }
    }
}
