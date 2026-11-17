use super::*;

fn fixture(text: &str, copies: usize) -> (Vec<WireToolCall>, Vec<WireToolResult>) {
    (
        vec![WireToolCall {
            id: "call-🦀".into(),
            name: "read".into(),
            input: serde_json::json!({"path": text, "nested": [null, true, 1.25]}),
        }],
        (0..copies)
            .map(|_| WireToolResult {
                tool_use_id: "call-🦀".into(),
                content: text.into(),
                is_error: false,
            })
            .collect(),
    )
}

#[test]
fn exact_json_bytes_match_legacy_for_escaping_unicode_and_large_payloads() {
    for text in [
        String::new(),
        "ordinary".into(),
        "\"\\\n\r\t\0\u{1f}".repeat(1500),
        "🦀漢字é\u{2028}".repeat(4000),
        "x".repeat(240 * 1024),
    ] {
        for copies in [0, 1, 16] {
            let value = fixture(&text, copies);
            assert_eq!(
                serialized_byte_len(&value).unwrap(),
                serde_json::to_vec(&value).unwrap().len()
            );
        }
    }
}

#[test]
fn byte_limit_is_inclusive_and_matches_legacy() {
    // Result strings are bounded individually, but multiple results can reach
    // the exact metadata boundary without hitting any other guardrail.
    let mut blocks: Vec<_> = (0..32)
        .map(|_| ContentBlock::ToolResult {
            tool_use_id: "id".into(),
            content: String::new(),
            is_error: Some(false),
        })
        .collect();
    let (_, calls, results) = structured_tools(&blocks).unwrap();
    let overhead = serde_json::to_vec(&(&calls, &results)).unwrap().len();
    let payload = MAX_STRUCTURED_BYTES - overhead;
    for (i, block) in blocks.iter_mut().enumerate() {
        if let ContentBlock::ToolResult { content, .. } = block {
            let bytes = payload / 32 + usize::from(i < payload % 32);
            *content = "\0".repeat(bytes / 6) + &"x".repeat(bytes % 6);
        }
    }
    let (_, calls, results) = structured_tools(&blocks).expect("exact limit accepted");
    assert_eq!(
        serde_json::to_vec(&(&calls, &results)).unwrap().len(),
        MAX_STRUCTURED_BYTES
    );
    if let ContentBlock::ToolResult { content, .. } = &mut blocks[0] {
        content.push('x');
    }
    assert!(structured_tools(&blocks).is_none());
    if let ContentBlock::ToolResult { content, .. } = &mut blocks[0] {
        content.pop();
        if content.pop() == Some('\0') {
            content.push_str("xxxxx");
        }
    }
    let (_, calls, results) = structured_tools(&blocks).unwrap();
    assert_eq!(
        serialized_byte_len(&(&calls, &results)).unwrap(),
        MAX_STRUCTURED_BYTES - 1
    );
}

#[test]
fn prose_keeps_utf16_boundary_semantics() {
    let mut blocks = vec![
        ContentBlock::Text {
            text: "🦀".repeat(MAX_TEXT_CHARS),
            cache_control: None,
        },
        ContentBlock::ToolResult {
            tool_use_id: "id".into(),
            content: "ok".into(),
            is_error: None,
        },
    ];
    let (text, _, _) = structured_tools(&blocks).unwrap();
    assert_eq!(text.encode_utf16().count(), 200_000);
    if let ContentBlock::Text { text, .. } = &mut blocks[0] {
        text.push('🦀');
    }
    // Truncation marker makes this exceed the separate UTF16 prose limit.
    assert!(structured_tools(&blocks).is_none());
}

#[test]
fn serializer_errors_remain_errors() {
    struct Failure;
    impl Serialize for Failure {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("fixture error"))
        }
    }
    assert_eq!(
        serialized_byte_len(&Failure).unwrap_err().to_string(),
        serde_json::to_vec(&Failure).unwrap_err().to_string()
    );
}

#[test]
#[ignore = "paired microbenchmark, run with --profile selfdev --ignored --nocapture"]
fn paired_byte_count_benchmark() {
    use std::{hint::black_box, time::Instant};
    for (name, text, copies) in [
        ("small", "hello".into(), 1),
        ("large", "x".repeat(8000), 30),
        ("escaping", "\"\\\n\r\t\0".repeat(1300), 16),
        ("unicode", "🦀漢字é".repeat(800), 24),
        ("boundary-1", String::new(), 1),
        ("boundary", String::new(), 1),
        ("boundary+1", String::new(), 1),
    ] {
        let mut value = fixture(&text, copies);
        if name.starts_with("boundary") {
            let target = match name {
                "boundary-1" => MAX_STRUCTURED_BYTES - 1,
                "boundary+1" => MAX_STRUCTURED_BYTES + 1,
                _ => MAX_STRUCTURED_BYTES,
            };
            let overhead = serde_json::to_vec(&value).unwrap().len();
            value.1[0].content = "x".repeat(target - overhead);
        }
        let expected = serde_json::to_vec(&value).unwrap().len();
        assert_eq!(serialized_byte_len(&value).unwrap(), expected);
        let iterations = 200;
        let mut legacy = Vec::new();
        let mut counting = Vec::new();
        for round in 0..8 {
            for mode in [round % 2, 1 - round % 2] {
                let start = Instant::now();
                for _ in 0..iterations {
                    let size = if mode == 0 {
                        serde_json::to_vec(black_box(&value)).unwrap().len()
                    } else {
                        serialized_byte_len(black_box(&value)).unwrap()
                    };
                    assert_eq!(black_box(size), expected);
                }
                let ns = start.elapsed().as_nanos() / iterations;
                if mode == 0 {
                    legacy.push(ns);
                } else {
                    counting.push(ns);
                }
            }
        }
        legacy.sort();
        counting.sort();
        eprintln!(
            "{name}: bytes={expected} legacy_ns={} counting_ns={} ratio={:.3}",
            legacy[4],
            counting[4],
            counting[4] as f64 / legacy[4] as f64
        );
    }
}
