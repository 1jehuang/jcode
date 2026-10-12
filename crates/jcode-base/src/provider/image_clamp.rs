//! Clamp outbound image dimensions before sending a request to a provider.
//!
//! Anthropic's Messages API enforces a per-image pixel cap that depends on how
//! many images the request carries:
//!
//! | images in request | max edge per image |
//! | ------------------ | ------------------ |
//! | `> 20`             | `2000px`           |
//! | `<= 20`            | `8000px`           |
//!
//! jcode previously only enforced a 20 MB byte cap when reading images and
//! never clamped pixel dimensions, so a session that accumulated more than ~20
//! large screenshots would fail on resume with:
//!
//! ```text
//! image dimensions exceed max allowed size for many-image requests: 2000 pixels
//! ```
//!
//! This module downscales oversized images (preserving aspect ratio) so the
//! request stays within provider per-image limits. The clamp runs at the
//! `MultiProvider` chokepoint, so every provider benefits and the limits only
//! ever shrink genuinely oversized images. See issue #381.
//!
//! Separately from pixel dimensions, Anthropic also rejects any single image
//! whose **base64 payload** exceeds 10 MB, regardless of how many images the
//! request carries:
//!
//! ```text
//! image exceeds 10 MB maximum: 24621232 bytes > 10485760 bytes
//! ```
//!
//! A screenshot can stay well under the pixel cap yet still blow past this byte
//! cap (e.g. a large, high-detail PNG). When that happens we re-encode the
//! image as JPEG and progressively downscale it until its base64 payload fits
//! within budget.

use base64::Engine as _;
use jcode_message_types::{ContentBlock, Message};

use crate::image_normalize::{normalize_image_bytes, sniff_provider_media_type};

/// Prefix of the label the agent emits after an image attached to a tool
/// result (see `agent::tools`). Folded together with an omitted image.
const TOOL_IMAGE_LABEL_PREFIX: &str = "[Attached image associated with the preceding tool result: ";

/// Replace the image at `index` with a textual `marker`.
///
/// When the image belongs to a tool result (the block before it is a
/// `ToolResult`), the marker and any trailing image label are folded into that
/// result's text instead of becoming sibling text blocks. Anthropic requires
/// the results of a parallel tool-call turn to be contiguous, so a stray text
/// block between them would itself break the request.
///
/// Returns the index of the next block to examine.
fn omit_image_block(content: &mut Vec<ContentBlock>, index: usize, marker: String) -> usize {
    let folds_into_tool_result =
        index > 0 && matches!(content[index - 1], ContentBlock::ToolResult { .. });
    if !folds_into_tool_result {
        content[index] = ContentBlock::Text {
            text: marker,
            cache_control: None,
        };
        return index + 1;
    }
    content.remove(index);
    let mut note = marker;
    if let Some(ContentBlock::Text { text, .. }) = content.get(index)
        && text.starts_with(TOOL_IMAGE_LABEL_PREFIX)
    {
        note.push('\n');
        note.push_str(text);
        content.remove(index);
    }
    if let ContentBlock::ToolResult {
        content: result, ..
    } = &mut content[index - 1]
    {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&note);
    }
    index
}

/// Build a request-safe copy of `messages` when the selected provider/model
/// cannot accept image input. Replacing each image with a small textual marker
/// preserves message and tool-call ordering while ensuring no encoded image
/// data reaches a text-only provider. Returns `None` when no rewrite is needed
/// so vision-capable and image-free requests avoid cloning their history.
pub(crate) fn filter_unsupported_outbound_images(
    messages: &[Message],
    supports_image_input: bool,
) -> Option<Vec<Message>> {
    if supports_image_input
        || !messages
            .iter()
            .flat_map(|message| message.content.iter())
            .any(|block| matches!(block, ContentBlock::Image { .. }))
    {
        return None;
    }

    let mut filtered = messages.to_vec();
    for message in &mut filtered {
        let mut index = 0;
        while index < message.content.len() {
            if let ContentBlock::Image { media_type, .. } = &message.content[index] {
                let marker = format!(
                    "[Image omitted: this provider/model does not support image input; media_type={media_type}]"
                );
                index = omit_image_block(&mut message.content, index, marker);
                continue;
            }
            index += 1;
        }
    }
    Some(filtered)
}

/// Max edge (px) allowed when a request carries more than this many images.
const MANY_IMAGE_THRESHOLD: usize = 20;
/// Per-image max edge when a request carries `> MANY_IMAGE_THRESHOLD` images.
const MANY_IMAGE_MAX_EDGE: u32 = 2000;
/// Per-image max edge when a request carries `<= MANY_IMAGE_THRESHOLD` images.
const FEW_IMAGE_MAX_EDGE: u32 = 8000;
/// Hard per-image base64 byte cap enforced by Anthropic's Messages API. Any
/// single image whose base64 string is larger than this is rejected outright.
const IMAGE_BASE64_BYTE_LIMIT: usize = 10 * 1024 * 1024;
/// Target base64 size we re-encode oversized images down to. Kept a little
/// under the hard limit so we always land safely inside the cap.
const IMAGE_BASE64_BYTE_TARGET: usize = 9 * 1024 * 1024;

/// Inspect `messages` and, if any `ContentBlock::Image` exceeds the per-image
/// edge limit implied by the total image count, or the per-image base64 byte
/// cap, return a clamped clone of the messages. Returns `None` when no
/// downscaling is required so the common path avoids cloning the (potentially
/// large) message vector.
pub(crate) fn clamp_outbound_images(messages: &[Message]) -> Option<Vec<Message>> {
    let image_count = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| matches!(b, ContentBlock::Image { .. }))
        .count();
    if image_count == 0 {
        return None;
    }

    let max_edge = if image_count > MANY_IMAGE_THRESHOLD {
        MANY_IMAGE_MAX_EDGE
    } else {
        FEW_IMAGE_MAX_EDGE
    };

    // Cheap pre-scan: the base64 byte check is just a string length, the
    // dimension probe is header-only, and the media-type sniff only reads a
    // handful of magic bytes. Only do the expensive decode/clone when at least
    // one image actually exceeds a limit or carries a mismatched media type.
    let needs_change = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Image { media_type, data } => Some((media_type, data)),
            _ => None,
        })
        .any(|(media_type, data)| {
            data.len() > IMAGE_BASE64_BYTE_LIMIT
                || image_exceeds_edge(data, max_edge)
                || format_needs_fix(media_type, data)
        });
    if !needs_change {
        return None;
    }

    let mut clamped = messages.to_vec();
    let mut changed = false;
    for message in &mut clamped {
        let mut index = 0;
        while index < message.content.len() {
            if let ContentBlock::Image { media_type, data } = &mut message.content[index] {
                // Formats no provider accepts (BMP, ICO, TIFF, HEIC, ...) are
                // converted to PNG when possible and otherwise replaced by a
                // text marker. This only rewrites the request copy, so it also
                // un-sticks sessions whose history already holds such an image
                // (#1712).
                match ensure_provider_format(media_type, data) {
                    Ok(converted) => changed |= converted,
                    Err(reason) => {
                        let marker = format!("[Image omitted: {media_type} {reason}]");
                        index = omit_image_block(&mut message.content, index, marker);
                        changed = true;
                        continue;
                    }
                }
                // Correct a mismatched label first (e.g. JPEG bytes tagged as
                // `image/png`) so downstream re-encoding decisions and the wire
                // payload agree. Providers reject a request outright when the
                // declared media type disagrees with the image's magic bytes.
                if reconcile_media_type(media_type, data) {
                    changed = true;
                }
                if clamp_image_block(media_type, data, max_edge) {
                    changed = true;
                }
            }
            index += 1;
        }
    }

    changed.then_some(clamped)
}

/// True when the block cannot go on the wire as-is: the payload is not valid
/// base64, its bytes are not a provider-accepted format, or the declared
/// `media_type` disagrees with the magic bytes.
fn format_needs_fix(media_type: &str, data_b64: &str) -> bool {
    let Some(bytes) = decode_b64(data_b64) else {
        return true;
    };
    match sniff_provider_media_type(&bytes) {
        Some(actual) => !media_type.eq_ignore_ascii_case(actual),
        None => true,
    }
}

/// Make sure the payload is a provider-accepted format. Returns `Ok(true)` when
/// it was converted (to PNG), `Ok(false)` when it already was, and `Err` with a
/// reason when it cannot be sent at all.
fn ensure_provider_format(media_type: &mut String, data: &mut String) -> Result<bool, String> {
    let Some(bytes) = decode_b64(data) else {
        return Err("data is not valid base64, so it cannot be sent to the model".into());
    };
    if sniff_provider_media_type(&bytes).is_some() {
        return Ok(false);
    }
    let image = normalize_image_bytes(bytes)?;
    *media_type = image.media_type.to_string();
    *data = base64::engine::general_purpose::STANDARD.encode(&image.data);
    Ok(true)
}

/// Rewrite `media_type` to match the payload's magic bytes when they disagree.
/// Returns `true` only when the label was actually corrected.
fn reconcile_media_type(media_type: &mut String, data_b64: &str) -> bool {
    let Some(bytes) = decode_b64(data_b64) else {
        return false;
    };
    match sniff_provider_media_type(&bytes) {
        Some(actual) if !media_type.eq_ignore_ascii_case(actual) => {
            *media_type = actual.to_string();
            true
        }
        _ => false,
    }
}

/// Return true when the base64-encoded image's larger edge exceeds `max_edge`,
/// using a cheap header-only dimension probe. When the dimensions cannot be
/// determined cheaply, returns true so the full decode path can make the final
/// decision (and skip re-encoding if it turns out to be within limits).
fn image_exceeds_edge(data_b64: &str, max_edge: u32) -> bool {
    match decode_b64(data_b64) {
        Some(bytes) => match probe_dimensions(&bytes) {
            Some((w, h)) => w > max_edge || h > max_edge,
            // Unknown format/dimensions: let the decode path decide.
            None => true,
        },
        None => false,
    }
}

/// Clamp a single image block in place so it satisfies both the per-image edge
/// limit and the per-image base64 byte cap. Updates `media_type`/`data` and
/// returns `true` only when the image was actually rewritten; returns `false`
/// (leaving the block untouched) when no change is needed or the image cannot be
/// processed.
fn clamp_image_block(media_type: &mut String, data: &mut String, max_edge: u32) -> bool {
    let probe_exceeds_edge = image_exceeds_edge(data, max_edge);
    let exceeds_bytes = data.len() > IMAGE_BASE64_BYTE_LIMIT;
    if !probe_exceeds_edge && !exceeds_bytes {
        return false;
    }

    let Some(bytes) = decode_b64(data) else {
        return false;
    };
    let Ok(img) = image::load_from_memory(&bytes) else {
        return false;
    };

    let (w, h) = (img.width(), img.height());
    let exceeds_edge = w > max_edge || h > max_edge;
    // The header probe is conservative (it reports unknown formats as "too
    // large"); bail out cheaply once we know the real dimensions are fine and
    // the byte budget is satisfied.
    if !exceeds_edge && !exceeds_bytes {
        return false;
    }

    // Stage 1: clamp pixel dimensions. `resize` preserves aspect ratio.
    let img = if exceeds_edge {
        img.resize(max_edge, max_edge, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    // Stage 2: enforce the byte budget. When the payload is too large we
    // re-encode as JPEG (far smaller than PNG for screenshots/photos) and
    // progressively downscale until it fits.
    if exceeds_bytes {
        if let Some(encoded) = encode_within_byte_budget(&img, IMAGE_BASE64_BYTE_TARGET) {
            *media_type = "image/jpeg".to_string();
            *data = encoded;
            return true;
        }
        return false;
    }

    // Edge-only clamp: re-encode in the original format at the reduced size.
    match encode_same_format(media_type, &img) {
        Some(encoded) => {
            *data = encoded;
            true
        }
        None => false,
    }
}

/// Re-encode `img` in the format implied by `media_type` (JPEG for
/// jpeg/jpg, PNG otherwise) and return the base64 payload.
fn encode_same_format(media_type: &str, img: &image::DynamicImage) -> Option<String> {
    let format = if media_type.eq_ignore_ascii_case("image/jpeg")
        || media_type.eq_ignore_ascii_case("image/jpg")
    {
        image::ImageFormat::Jpeg
    } else {
        image::ImageFormat::Png
    };

    let mut out = std::io::Cursor::new(Vec::new());
    if format == image::ImageFormat::Jpeg {
        // JPEG cannot encode an alpha channel; flatten to RGB first.
        image::DynamicImage::ImageRgb8(img.to_rgb8())
            .write_to(&mut out, format)
            .ok()?;
    } else {
        img.write_to(&mut out, format).ok()?;
    }
    Some(base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

/// Encode `img` as JPEG, shrinking it until the resulting base64 payload is
/// `<= target_b64_bytes`. Returns `None` if it cannot be brought under budget.
fn encode_within_byte_budget(img: &image::DynamicImage, target_b64_bytes: usize) -> Option<String> {
    let mut current = img.clone();
    for _ in 0..10 {
        let raw = encode_jpeg(&current)?;
        if base64_len(raw.len()) <= target_b64_bytes {
            return Some(base64::engine::general_purpose::STANDARD.encode(raw));
        }

        // base64 length tracks encoded bytes, which scale roughly with pixel
        // area, so shrink each edge by ~sqrt(target / current). Clamp the ratio
        // so we always make progress without overshooting to a 1px thumbnail.
        let ratio = (target_b64_bytes as f64 / base64_len(raw.len()) as f64)
            .sqrt()
            .clamp(0.1, 0.9);
        let nw = ((current.width() as f64 * ratio).round() as u32).max(1);
        let nh = ((current.height() as f64 * ratio).round() as u32).max(1);
        if nw == current.width() && nh == current.height() {
            break;
        }
        current = current.resize(nw, nh, image::imageops::FilterType::Triangle);
    }

    // Final check at the smallest size reached.
    let raw = encode_jpeg(&current)?;
    (base64_len(raw.len()) <= target_b64_bytes)
        .then(|| base64::engine::general_purpose::STANDARD.encode(raw))
}

/// Encode an image as JPEG bytes (flattening any alpha channel to RGB).
fn encode_jpeg(img: &image::DynamicImage) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8())
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .ok()?;
    Some(out.into_inner())
}

/// Length of the standard base64 encoding (with padding) of `n` raw bytes.
fn base64_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

fn decode_b64(data_b64: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(data_b64.trim())
        .ok()
}

/// Cheap header-only dimension probe for PNG/JPEG/GIF, mirroring the probe used
/// when reading images. Returns `None` for unknown formats.
fn probe_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG: signature + IHDR chunk.
    if data.len() > 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((width, height));
    }

    // JPEG: scan for SOF0/SOF2 markers.
    if data.len() > 2 && data[0] == 0xFF && data[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if marker == 0xC0 || marker == 0xC2 {
                let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((width, height));
            }
            if i + 3 < data.len() {
                let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += 2 + len;
            } else {
                break;
            }
        }
    }

    // GIF: header carries dimensions directly.
    if data.len() > 10 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
        let width = u16::from_le_bytes([data[6], data[7]]) as u32;
        let height = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((width, height));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use jcode_message_types::Role;

    fn encode_png(w: u32, h: u32) -> String {
        let img = RgbImage::from_pixel(w, h, image::Rgb([10, 20, 30]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    /// Encode a high-entropy (noise) PNG so the payload barely compresses and
    /// reliably exceeds the per-image base64 byte cap. Solid-color PNGs compress
    /// to a few KB and cannot exercise the byte path.
    fn encode_noise_png(w: u32, h: u32) -> String {
        let mut img = RgbImage::new(w, h);
        // Simple deterministic LCG so the test is reproducible.
        let mut state: u32 = 0x1234_5678;
        for pixel in img.pixels_mut() {
            let mut next = || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            };
            *pixel = image::Rgb([next(), next(), next()]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    fn image_message(data: String) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    fn image_message_with(media_type: &str, data: String) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Image {
                media_type: media_type.to_string(),
                data,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }
    }

    fn encode_jpeg_b64(w: u32, h: u32) -> String {
        let img = RgbImage::from_pixel(w, h, image::Rgb([200, 100, 50]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Jpeg)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    fn dims(data_b64: &str) -> (u32, u32) {
        let bytes = decode_b64(data_b64).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn no_images_returns_none() {
        let messages = vec![Message::user("hello")];
        assert!(clamp_outbound_images(&messages).is_none());
    }

    #[test]
    fn text_only_provider_history_replaces_images_before_request() {
        let mut message = image_message("encoded-image-data".to_string());
        message.content.push(ContentBlock::Text {
            text: "keep this text".to_string(),
            cache_control: None,
        });
        let messages = vec![message];

        let filtered = filter_unsupported_outbound_images(&messages, false)
            .expect("text-only requests containing images must be rewritten");

        assert!(filtered.iter().all(|message| {
            message
                .content
                .iter()
                .all(|block| !matches!(block, ContentBlock::Image { .. }))
        }));
        assert!(matches!(
            &filtered[0].content[0],
            ContentBlock::Text { text, .. }
                if text == "[Image omitted: this provider/model does not support image input; media_type=image/png]"
        ));
        assert!(matches!(
            &filtered[0].content[1],
            ContentBlock::Text { text, .. } if text == "keep this text"
        ));
        assert!(matches!(
            &messages[0].content[0],
            ContentBlock::Image { data, .. } if data == "encoded-image-data"
        ));
    }

    #[test]
    fn image_capable_provider_history_is_not_rewritten() {
        let messages = vec![image_message("encoded-image-data".to_string())];

        assert!(filter_unsupported_outbound_images(&messages, true).is_none());
        assert!(matches!(
            &messages[0].content[0],
            ContentBlock::Image { data, .. } if data == "encoded-image-data"
        ));
    }

    #[test]
    fn text_only_image_free_history_is_not_cloned() {
        let messages = vec![Message::user("hello")];
        assert!(filter_unsupported_outbound_images(&messages, false).is_none());
    }

    #[test]
    fn within_limits_returns_none() {
        let messages = vec![image_message(encode_png(100, 100))];
        assert!(clamp_outbound_images(&messages).is_none());
    }

    #[test]
    fn single_large_image_uses_few_image_limit() {
        // One image, larger than the many-image cap (2000) but under the
        // few-image cap (8000): must NOT be downscaled.
        let messages = vec![image_message(encode_png(3000, 1000))];
        assert!(clamp_outbound_images(&messages).is_none());
    }

    #[test]
    fn many_images_clamp_to_2000() {
        // 21 images, each 2500px wide -> exceeds the many-image 2000px cap.
        let messages: Vec<Message> = (0..21)
            .map(|_| image_message(encode_png(2500, 1250)))
            .collect();
        let clamped = clamp_outbound_images(&messages).expect("should clamp many oversized images");
        for m in &clamped {
            if let ContentBlock::Image { data, .. } = &m.content[0] {
                let (w, h) = dims(data);
                assert!(w <= 2000 && h <= 2000, "image not clamped: {w}x{h}");
                // Aspect ratio preserved (2:1).
                assert_eq!(w, 2000);
                assert_eq!(h, 1000);
            }
        }
    }

    #[test]
    fn few_images_clamp_to_8000() {
        let messages = vec![image_message(encode_png(9000, 3000))];
        let clamped = clamp_outbound_images(&messages).expect("should clamp oversized image");
        if let ContentBlock::Image { data, .. } = &clamped[0].content[0] {
            let (w, h) = dims(data);
            assert!(w <= 8000 && h <= 8000);
            assert_eq!(w, 8000);
            // 9000x3000 -> 8000 wide, height rounds to nearest.
            assert!((2666..=2667).contains(&h), "unexpected height {h}");
        }
    }

    #[test]
    fn oversized_base64_image_is_reencoded_under_byte_cap() {
        // A 2000x2000 noise PNG stays well within the per-image *pixel* caps
        // but its base64 payload blows past the 10 MB per-image byte cap.
        let data = encode_noise_png(2000, 2000);
        assert!(
            data.len() > IMAGE_BASE64_BYTE_LIMIT,
            "test image base64 ({} bytes) should exceed the byte cap",
            data.len()
        );

        let messages = vec![image_message(data)];
        let clamped =
            clamp_outbound_images(&messages).expect("oversized base64 image should be clamped");
        match &clamped[0].content[0] {
            ContentBlock::Image { media_type, data } => {
                assert!(
                    data.len() <= IMAGE_BASE64_BYTE_LIMIT,
                    "clamped image base64 ({} bytes) must fit within the byte cap",
                    data.len()
                );
                // Re-encoded as JPEG to hit the smaller payload.
                assert_eq!(media_type, "image/jpeg");
                // Still decodable and within pixel limits.
                let (w, h) = dims(data);
                assert!(w <= 2000 && h <= 2000, "unexpected dims {w}x{h}");
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }

    #[test]
    fn jpeg_bytes_mislabeled_as_png_are_relabeled() {
        // The exact production failure: JPEG payload declared as image/png.
        // Anthropic rejects this with a 400 unless we reconcile the label.
        let data = encode_jpeg_b64(64, 64);
        let messages = vec![image_message_with("image/png", data.clone())];
        let fixed = clamp_outbound_images(&messages).expect("mislabeled image should be corrected");
        match &fixed[0].content[0] {
            ContentBlock::Image {
                media_type,
                data: out,
            } => {
                assert_eq!(media_type, "image/jpeg", "label must match JPEG bytes");
                // Bytes are left untouched, only the label changes.
                assert_eq!(out, &data);
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }

    #[test]
    fn png_bytes_mislabeled_as_jpeg_are_relabeled() {
        let data = encode_png(64, 64);
        let messages = vec![image_message_with("image/jpeg", data.clone())];
        let fixed = clamp_outbound_images(&messages).expect("mislabeled image should be corrected");
        match &fixed[0].content[0] {
            ContentBlock::Image {
                media_type,
                data: out,
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(out, &data);
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }

    #[test]
    fn correctly_labeled_image_is_not_touched() {
        let data = encode_jpeg_b64(64, 64);
        let messages = vec![image_message_with("image/jpeg", data)];
        assert!(
            clamp_outbound_images(&messages).is_none(),
            "correctly labeled small image must not trigger a clone"
        );
    }

    #[test]
    fn media_type_match_is_case_insensitive() {
        let data = encode_png(32, 32);
        // Upper-case label for the same PNG bytes: must be treated as a match.
        let messages = vec![image_message_with("IMAGE/PNG", data)];
        assert!(
            clamp_outbound_images(&messages).is_none(),
            "case-insensitive label match must not be rewritten"
        );
    }

    #[test]
    fn sniff_detects_common_formats() {
        assert_eq!(
            sniff_provider_media_type(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(
            sniff_provider_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_provider_media_type(b"GIF89a....."), Some("image/gif"));
        assert_eq!(
            sniff_provider_media_type(b"RIFF\0\0\0\0WEBP...."),
            Some("image/webp")
        );
        assert_eq!(sniff_provider_media_type(b"not an image"), None);
    }

    fn bmp_b64() -> String {
        let img = RgbImage::from_pixel(4, 4, image::Rgb([200, 10, 10]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Bmp)
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    fn tool_result_block(id: &str, content: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: None,
        }
    }

    /// #1712: a persisted `image/bmp` block must be converted before it reaches
    /// the wire, which also recovers sessions already holding one.
    #[test]
    fn bmp_block_is_converted_to_png() {
        let messages = vec![image_message_with("image/bmp", bmp_b64())];
        let out = clamp_outbound_images(&messages).expect("bmp must be rewritten");
        let ContentBlock::Image { media_type, data } = &out[0].content[0] else {
            panic!("expected image block");
        };
        assert_eq!(media_type, "image/png");
        let bytes = decode_b64(data).unwrap();
        assert_eq!(sniff_provider_media_type(&bytes), Some("image/png"));
        assert_eq!(dims(data), (4, 4));
    }

    /// TIFF bytes behind a `.png` label must not go out as `image/png`.
    #[test]
    fn tiff_bytes_labelled_png_are_converted() {
        let img = RgbImage::from_pixel(3, 3, image::Rgb([1, 2, 3]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Tiff)
            .unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(out.into_inner());
        let messages = vec![image_message_with("image/png", data)];
        let out = clamp_outbound_images(&messages).expect("tiff must be rewritten");
        let ContentBlock::Image { media_type, data } = &out[0].content[0] else {
            panic!("expected image block");
        };
        assert_eq!(media_type, "image/png");
        assert_eq!(
            sniff_provider_media_type(&decode_b64(data).unwrap()),
            Some("image/png")
        );
    }

    /// An undecodable image inside a tool result becomes a note folded into
    /// that result, keeping parallel tool results contiguous.
    #[test]
    fn undecodable_tool_image_folds_marker_into_tool_result() {
        let mut heic = vec![0, 0, 0, 24];
        heic.extend_from_slice(b"ftypheic");
        heic.extend_from_slice(&[0; 16]);
        let heic = base64::engine::general_purpose::STANDARD.encode(heic);
        let messages = vec![Message {
            role: Role::User,
            content: vec![
                tool_result_block("toolu_1", "Image: x.heic"),
                ContentBlock::Image {
                    media_type: "image/heic".into(),
                    data: heic,
                },
                ContentBlock::Text {
                    text: format!("{TOOL_IMAGE_LABEL_PREFIX}x.heic]"),
                    cache_control: None,
                },
                tool_result_block("toolu_2", "second"),
            ],
            timestamp: None,
            tool_duration_ms: None,
        }];
        let out = clamp_outbound_images(&messages).expect("heic must be rewritten");
        let content = &out[0].content;
        assert_eq!(content.len(), 2, "tool results must stay contiguous");
        let ContentBlock::ToolResult { content: first, .. } = &content[0] else {
            panic!("expected tool result");
        };
        assert!(first.starts_with("Image: x.heic\n[Image omitted: image/heic HEIC"));
        assert!(first.contains(TOOL_IMAGE_LABEL_PREFIX));
        assert!(matches!(
            &content[1],
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "toolu_2"
        ));
    }

    #[test]
    fn invalid_base64_image_is_omitted() {
        let messages = vec![image_message_with("image/png", "@@not base64@@".into())];
        let out = clamp_outbound_images(&messages).expect("must be rewritten");
        assert!(matches!(
            &out[0].content[0],
            ContentBlock::Text { text, .. } if text.starts_with("[Image omitted: image/png")
        ));
    }
}
