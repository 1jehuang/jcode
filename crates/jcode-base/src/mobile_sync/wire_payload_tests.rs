use super::*;
use std::hint::black_box;
use std::time::Instant;

fn message(seq: usize, text: String) -> WireMessage {
    WireMessage {
        seq,
        role: "assistant",
        text,
        display_text: None,
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        images: Vec::new(),
        created_at: None,
    }
}

fn legacy_chunks(messages: &[WireMessage]) -> Vec<&[WireMessage]> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (index, message) in messages.iter().enumerate() {
        let size = serde_json::to_vec(message).map_or(0, |v| v.len());
        if index > start
            && (bytes + size > 3 * 1024 * 1024 || index - start >= MAX_MESSAGES_PER_UPLOAD)
        {
            chunks.push(&messages[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += size;
    }
    if start < messages.len() {
        chunks.push(&messages[start..]);
    }
    chunks
}

#[test]
fn counted_chunk_boundaries_match_allocating_oracle() {
    for count in [0, 1, MAX_MESSAGES_PER_UPLOAD, MAX_MESSAGES_PER_UPLOAD + 1] {
        let messages: Vec<_> = (0..count)
            .map(|seq| message(seq, "é\"\\\n🦀".repeat(30)))
            .collect();
        assert_eq!(message_chunks(&messages), legacy_chunks(&messages));
    }
    let empty = message(0, String::new());
    let overhead = serde_json::to_vec(&empty).unwrap().len();
    for delta in [-1isize, 0, 1] {
        let target = (3 * 1024 * 1024isize + delta) as usize;
        let messages = vec![
            message(0, "a".repeat(target - overhead)),
            message(1, "next".into()),
            message(2, "\"\n\u{0}🦀".repeat(1024)),
        ];
        assert_eq!(serialized_byte_len(&messages[0]).unwrap(), target);
        assert_eq!(message_chunks(&messages), legacy_chunks(&messages));
    }
    let mut complex = message(0, "prose".into());
    complex.display_text = Some("visible \n 🦀".into());
    complex.tool_calls.push(WireToolCall {
        id: "call".into(),
        name: "test".into(),
        input: serde_json::json!({"a": [null, 1, "\"\\🦀"]}),
    });
    complex.tool_results.push(WireToolResult {
        tool_use_id: "call".into(),
        content: "output".repeat(1000),
        is_error: false,
    });
    complex.images.push(WireImage {
        media_type: "image/jpeg".into(),
        data: "abcd".repeat(2000),
    });
    complex.created_at = Some("2026-11-17T00:00:00Z".into());
    assert_eq!(
        serialized_byte_len(&complex).unwrap(),
        serde_json::to_vec(&complex).unwrap().len()
    );
}

#[test]
fn consuming_rgb_conversion_preserves_pixels_and_jpeg_bytes() {
    let rgb =
        image::RgbImage::from_fn(67, 49, |x, y| image::Rgb([x as u8, y as u8, (x + y) as u8]));
    let rgba = image::RgbaImage::from_fn(67, 49, |x, y| {
        image::Rgba([x as u8, y as u8, 88, (x * y) as u8])
    });
    let gray = image::GrayImage::from_fn(67, 49, |x, y| image::Luma([(x + y) as u8]));
    let inputs = [
        image::DynamicImage::ImageRgb8(rgb),
        image::DynamicImage::ImageRgba8(rgba),
        image::DynamicImage::ImageLuma8(gray),
    ];
    for input in inputs {
        let thumbnail = input.thumbnail(43, 43);
        let expected = thumbnail.to_rgb8();
        let actual = thumbnail.into_rgb8();
        assert_eq!(actual, expected);
        let encode = |pixels: &image::RgbImage| {
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 75)
                .encode_image(pixels)
                .unwrap();
            bytes
        };
        assert_eq!(encode(&actual), encode(&expected));
    }
}

#[test]
fn image_fragment_preflight_preserves_original_fragment_boundaries() {
    for text in [
        "",
        "plain prose",
        "!",
        "![",
        "![bad ![ok](path)",
        "![a](x)![b](y)",
        "🦀 ![a](x)",
    ] {
        let expected: Vec<_> = text.split("![").skip(1).take(64).collect();
        let mut fragments = text.split("![").skip(1).take(64).peekable();
        let empty = fragments.peek().is_none();
        assert_eq!(empty, expected.is_empty());
        assert_eq!(fragments.collect::<Vec<_>>(), expected);
    }
    assert!(generated_images("plain assistant prose with no image destination").is_empty());
}

#[test]
#[ignore = "paired component benchmark, run explicitly"]
fn benchmark_wire_payload_allocations() {
    let messages: Vec<_> = (0..128)
        .map(|seq| message(seq, "tool output: \"é🦀\"\n".repeat(2000)))
        .collect();
    assert_eq!(message_chunks(&messages), legacy_chunks(&messages));
    for pair in 0..5 {
        let legacy = || {
            let start = Instant::now();
            for _ in 0..10 {
                black_box(legacy_chunks(black_box(&messages)));
            }
            start.elapsed()
        };
        let counted = || {
            let start = Instant::now();
            for _ in 0..10 {
                black_box(message_chunks(black_box(&messages)));
            }
            start.elapsed()
        };
        let (before, after) = if pair % 2 == 0 {
            (legacy(), counted())
        } else {
            let after = counted();
            (legacy(), after)
        };
        eprintln!("chunk pair {pair}: allocating={before:?} counted={after:?}");
    }
    let rgb = image::RgbImage::from_pixel(1024, 1024, image::Rgb([12, 34, 56]));
    for consume in [false, true] {
        let inputs: Vec<_> = (0..64)
            .map(|_| image::DynamicImage::ImageRgb8(rgb.clone()))
            .collect();
        let start = Instant::now();
        for input in inputs {
            if consume {
                black_box(input.into_rgb8());
            } else {
                black_box(input.to_rgb8());
            }
        }
        eprintln!("rgb consume={consume}: {:?}", start.elapsed());
    }
}
