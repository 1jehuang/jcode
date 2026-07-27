//! Regression coverage for #609: SSE stream data corruption.
use super::*;

#[test]
fn multibyte_char_split_across_chunks_is_not_dropped() {
    // "读" is 3 bytes; split it after the first byte.
    let full = "读取文件".as_bytes().to_vec();
    let mut decoder = Utf8StreamDecoder::new();
    let mut out = String::new();
    out.push_str(&decoder.decode(&full[..1]));
    assert!(
        decoder.has_pending_bytes(),
        "partial char must be held back"
    );
    out.push_str(&decoder.decode(&full[1..]));
    out.push_str(&decoder.flush());
    assert_eq!(out, "读取文件");
}

#[test]
fn every_single_byte_split_point_round_trips() {
    // State-space sweep: splitting at any byte offset must reproduce the input.
    let text = "hello 读取 世界 mixed ascii";
    let bytes = text.as_bytes();
    for split in 0..=bytes.len() {
        let mut decoder = Utf8StreamDecoder::new();
        let mut out = decoder.decode(&bytes[..split]);
        out.push_str(&decoder.decode(&bytes[split..]));
        out.push_str(&decoder.flush());
        assert_eq!(out, text, "failed at split offset {split}");
    }
}

#[test]
fn byte_at_a_time_delivery_round_trips() {
    let text = "读取文件 with ascii tail";
    let mut decoder = Utf8StreamDecoder::new();
    let mut out = String::new();
    for byte in text.as_bytes() {
        out.push_str(&decoder.decode(&[*byte]));
    }
    out.push_str(&decoder.flush());
    assert_eq!(out, text);
}

#[test]
fn genuinely_invalid_bytes_do_not_discard_surrounding_text() {
    let mut decoder = Utf8StreamDecoder::new();
    let mut bytes = b"good".to_vec();
    bytes.push(0xFF); // never valid in UTF-8
    bytes.extend_from_slice(b"tail");
    let out = decoder.decode(&bytes);
    assert!(out.starts_with("good"), "leading good text kept: {out:?}");
    assert!(out.ends_with("tail"), "trailing good text kept: {out:?}");
    assert!(out.contains('\u{FFFD}'));
}

#[test]
fn well_formed_single_object_is_not_treated_as_concatenated() {
    assert_eq!(split_concatenated_json(r#"{"id":"a"}"#), None);
}

#[test]
fn concatenated_objects_are_split() {
    let split = split_concatenated_json(r#"{"id":"a"}{"id":"b"}"#).expect("split");
    assert_eq!(split.objects, vec![r#"{"id":"a"}"#, r#"{"id":"b"}"#]);
    assert_eq!(split.trailing_partial, None);
}

#[test]
fn concatenation_with_embedded_data_prefix_is_split() {
    let split = split_concatenated_json(r#"{"id":"a"}data: {"id":"b"}"#).expect("split");
    assert_eq!(split.objects, vec![r#"{"id":"a"}"#, r#"{"id":"b"}"#]);
}

#[test]
fn braces_inside_strings_do_not_confuse_the_splitter() {
    let payload = r#"{"content":"a } b { c"}{"id":"second"}"#;
    let split = split_concatenated_json(payload).expect("split");
    assert_eq!(
        split.objects,
        vec![r#"{"content":"a } b { c"}"#, r#"{"id":"second"}"#]
    );
}

#[test]
fn escaped_quote_inside_string_is_handled() {
    let payload = r#"{"content":"say \"hi\" }"}{"id":"second"}"#;
    let split = split_concatenated_json(payload).expect("split");
    assert_eq!(
        split.objects,
        vec![r#"{"content":"say \"hi\" }"}"#, r#"{"id":"second"}"#]
    );
}

#[test]
fn truncated_trailing_object_is_reported_for_carry_over() {
    let split =
        split_concatenated_json(r#"{"choices":[{"delta":{"content":"Hello "#).expect("split");
    assert!(split.objects.is_empty());
    assert_eq!(
        split.trailing_partial.as_deref(),
        Some(r#"{"choices":[{"delta":{"content":"Hello "#)
    );
}

#[test]
fn split_object_rejoins_into_valid_json() {
    // Event 1 truncated, event 2 carries the remainder.
    let first = r#"{"choices":[{"delta":{"content":"Hello "#;
    let second = r#"world"}}]}"#;
    let split = split_concatenated_json(first).expect("partial");
    let mut rejoined = split.trailing_partial.expect("partial text");
    rejoined.push_str(second);
    assert_eq!(
        rejoined,
        r#"{"choices":[{"delta":{"content":"Hello world"}}]}"#
    );
    // Brace-balanced after rejoin, so a downstream JSON parse will succeed.
    assert_eq!(split_concatenated_json(&rejoined), None);
}
