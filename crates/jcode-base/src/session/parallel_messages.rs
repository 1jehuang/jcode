//! Parallel deserialization of a session's `messages` array.
//!
//! Resume parses the whole snapshot on the reconnect critical path, and the
//! `messages` array is nearly all of it (35 of 36 MB for a long session).
//! serde_json parses on one thread, so this first scans the array into borrowed
//! raw slices (a fast, allocation-light pass) and then deserializes the
//! elements on a few threads. The on-disk format is unchanged.
//!
//! Raw elements are owned (`Box<RawValue>`) so this works with every
//! deserializer (`from_str`, readers, `from_value`). Measured on a 35MB
//! session, the owned scan costs 14-25ms versus 11ms borrowed.

use jcode_session_types::StoredMessage;
use serde::de::{Deserializer, Error as _, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

/// Below this many messages, threading costs more than it saves.
const PARALLEL_MIN_MESSAGES: usize = 512;
const MAX_THREADS: usize = 8;

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<StoredMessage>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_seq(RawSeqVisitor)
}

struct RawSeqVisitor;

impl<'de> Visitor<'de> for RawSeqVisitor {
    type Value = Vec<StoredMessage>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a sequence of stored messages")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut raws: Vec<Box<RawValue>> = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(raw) = seq.next_element::<Box<RawValue>>()? {
            raws.push(raw);
        }
        parse_raw_messages(&raws).map_err(A::Error::custom)
    }
}

fn parse_one(raw: &RawValue) -> Result<StoredMessage, serde_json::Error> {
    serde_json::from_str(raw.get())
}

fn parse_raw_messages(raws: &[Box<RawValue>]) -> Result<Vec<StoredMessage>, serde_json::Error> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(MAX_THREADS);
    if raws.len() < PARALLEL_MIN_MESSAGES || threads < 2 {
        return raws.iter().map(|raw| parse_one(raw)).collect();
    }
    let chunk = raws.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = raws
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|raw| parse_one(raw))
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect();
        let mut messages = Vec::with_capacity(raws.len());
        for handle in handles {
            match handle.join() {
                Ok(part) => messages.extend(part?),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        Ok(messages)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Holder {
        #[serde(deserialize_with = "super::deserialize")]
        messages: Vec<StoredMessage>,
    }

    fn message_json(i: usize) -> String {
        format!(
            r#"{{"id":"m{i}","role":"{}","content":[{{"type":"text","text":"hello \"{i}\" \u00e9"}}]}}"#,
            if i % 2 == 0 { "user" } else { "assistant" }
        )
    }

    fn doc(n: usize) -> String {
        let items: Vec<String> = (0..n).map(message_json).collect();
        format!(r#"{{"messages":[{}]}}"#, items.join(","))
    }

    fn sequential(n: usize) -> Vec<StoredMessage> {
        (0..n)
            .map(|i| serde_json::from_str(&message_json(i)).unwrap())
            .collect()
    }

    fn ids(messages: &[StoredMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect()
    }

    #[test]
    fn parallel_parse_matches_sequential_in_order() {
        for n in [0, 1, PARALLEL_MIN_MESSAGES - 1, PARALLEL_MIN_MESSAGES, 3001] {
            let parsed: Holder = serde_json::from_str(&doc(n)).unwrap();
            assert_eq!(ids(&parsed.messages), ids(&sequential(n)), "n={n}");
        }
    }

    #[test]
    fn reader_and_value_inputs_parse_identically() {
        let n = PARALLEL_MIN_MESSAGES + 7;
        let text = doc(n);
        let parsed: Holder = serde_json::from_reader(text.as_bytes()).unwrap();
        assert_eq!(ids(&parsed.messages), ids(&sequential(n)));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let parsed: Holder = serde_json::from_value(value).unwrap();
        assert_eq!(ids(&parsed.messages), ids(&sequential(n)));
    }

    #[test]
    fn a_malformed_message_is_an_error_not_a_silent_drop() {
        let n = PARALLEL_MIN_MESSAGES + 1;
        let text = doc(n).replacen(r#""role":"user""#, r#""role":"nobody""#, 1);
        assert!(serde_json::from_str::<Holder>(&text).is_err());
    }
}
