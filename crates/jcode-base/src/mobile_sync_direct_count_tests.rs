use super::*;

// Frozen before-round counting sink, including default partial-write loop.
fn legacy_count(value: &impl Serialize) -> serde_json::Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[test]
fn direct_count_matches_serialized_bytes_and_errors() {
    use serde_json::json;
    for input in [
        json!({}),
        json!({"input":"🦀\"\n\u{0}".repeat(1000)}),
        json!({"n":[i64::MIN,u64::MAX,-0.0,1e-300,1e300]}),
    ] {
        assert_eq!(
            serialized_byte_len(&input).unwrap(),
            serde_json::to_vec(&input).unwrap().len()
        );
    }
    struct FailAfterPrefix;
    impl Serialize for FailAfterPrefix {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::{Error, SerializeSeq};
            let mut seq = serializer.serialize_seq(Some(2))?;
            seq.serialize_element("prefix 🦀")?;
            Err(S::Error::custom("original error"))
        }
    }
    assert_eq!(
        legacy_count(&FailAfterPrefix).unwrap_err().to_string(),
        serialized_byte_len(&FailAfterPrefix)
            .unwrap_err()
            .to_string()
    );
}

#[test]
#[ignore = "paired direct write_all counting writer benchmark"]
fn direct_count_abba() {
    use serde_json::json;
    use std::{hint::black_box, time::Instant};
    for (name, text) in [
        ("small", "hello".to_string()),
        ("large", "x".repeat(100000)),
        ("escaping", "\n\u{0}\"🦀".repeat(2000)),
    ] {
        let calls = vec![WireToolCall {
            id: "id".into(),
            name: "tool".into(),
            input: json!({"text":text,"n":3.25}),
        }];
        let results = vec![WireToolResult {
            tool_use_id: "id".into(),
            content: text,
            is_error: false,
        }];
        let input = (&calls, &results);
        assert_eq!(
            legacy_count(&input).unwrap(),
            serialized_byte_len(&input).unwrap()
        );
        for pair in 0..9 {
            let old = || {
                let start = Instant::now();
                for _ in 0..100 {
                    black_box(legacy_count(black_box(&input)).unwrap());
                }
                start.elapsed()
            };
            let new = || {
                let start = Instant::now();
                for _ in 0..100 {
                    black_box(serialized_byte_len(black_box(&input)).unwrap());
                }
                start.elapsed()
            };
            let (a, b) = if pair % 2 == 0 {
                (old(), new())
            } else {
                let b = new();
                (old(), b)
            };
            eprintln!(
                "direct-count {name} {pair}: old={a:?} new={b:?} ratio={:.6}",
                b.as_secs_f64() / a.as_secs_f64()
            );
        }
    }
}
