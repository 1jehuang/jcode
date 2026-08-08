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
//!
//! Finally, providers only accept a small set of image formats
//! (`image/jpeg`, `image/png`, `image/gif`, `image/webp`). Reading a favicon
//! (`image/x-icon`) or a `.bmp` used to send an unsupported payload straight
//! through and the request failed with:
//!
//! ```text
//! The image data you provided does not represent a valid image.
//! ```
//!
//! Those blocks are transcoded to PNG here instead.

use base64::Engine as _;
use jcode_message_types::{ContentBlock, Message};

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
        for block in &mut message.content {
            if let ContentBlock::Image { media_type, .. } = block {
                *block = ContentBlock::Text {
                    text: format!(
                        "[Image omitted: this provider/model does not support image input; media_type={media_type}]"
                    ),
                    cache_control: None,
                };
            }
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
/// Image media types every supported provider accepts on the wire.
const PROVIDER_SUPPORTED_MEDIA_TYPES: [&str; 4] =
    ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// True when `media_type` is not one of the formats providers accept, so the
/// block must be transcoded before it can be sent.
fn media_type_unsupported(media_type: &str) -> bool {
    !PROVIDER_SUPPORTED_MEDIA_TYPES
        .iter()
        .any(|ok| media_type.eq_ignore_ascii_case(ok))
}

/// Transcode an unsupported image (ICO, BMP, ...) to PNG in place. Returns
/// `true` only when the block was rewritten; a payload we cannot decode is left
/// untouched so behaviour never regresses versus sending it as-is.
fn transcode_unsupported_format(media_type: &mut String, data: &mut String) -> bool {
    if !media_type_unsupported(media_type) {
        return false;
    }
    let Some(bytes) = decode_b64(data) else {
        return false;
    };
    let Some(img) = decode_image(&bytes) else {
        return false;
    };
    let mut out = std::io::Cursor::new(Vec::new());
    if img.write_to(&mut out, image::ImageFormat::Png).is_err() {
        return false;
    }
    *media_type = "image/png".to_string();
    *data = base64::engine::general_purpose::STANDARD.encode(out.into_inner());
    true
}

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
                || media_type_mismatch(media_type, data)
                || media_type_unsupported(media_type)
        });
    if !needs_change {
        return None;
    }

    let mut clamped = messages.to_vec();
    let mut changed = false;
    for message in &mut clamped {
        for block in &mut message.content {
            if let ContentBlock::Image { media_type, data } = block {
                // Correct a mismatched label first (e.g. JPEG bytes tagged as
                // `image/png`) so downstream re-encoding decisions and the wire
                // payload agree. Providers reject a request outright when the
                // declared media type disagrees with the image's magic bytes.
                if reconcile_media_type(media_type, data) {
                    changed = true;
                }
                // Then transcode anything providers cannot accept at all
                // (favicons, BMPs, ...) into PNG.
                if transcode_unsupported_format(media_type, data) {
                    changed = true;
                }
                if clamp_image_block(media_type, data, max_edge) {
                    changed = true;
                }
            }
        }
    }

    changed.then_some(clamped)
}

/// Decode an image, with a fallback for ICO files that wrap a PNG whose colour
/// type the `image` crate's ICO decoder rejects (a very common favicon shape).
/// In that case we decode the embedded PNG directly.
fn decode_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    if let Ok(img) = image::load_from_memory(bytes) {
        return Some(img);
    }
    embedded_ico_png(bytes).and_then(|png| image::load_from_memory(png).ok())
}

/// Return the embedded PNG payload of the first entry of an ICO file, if any.
fn embedded_ico_png(bytes: &[u8]) -> Option<&[u8]> {
    // ICONDIR: reserved(2) type(2) count(2), then 16-byte ICONDIRENTRYs.
    if bytes.len() < 22 || bytes[0..4] != [0x00, 0x00, 0x01, 0x00] {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    for i in 0..count {
        let entry = 6 + i * 16;
        if entry + 16 > bytes.len() {
            break;
        }
        let size = u32::from_le_bytes([
            bytes[entry + 8],
            bytes[entry + 9],
            bytes[entry + 10],
            bytes[entry + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            bytes[entry + 12],
            bytes[entry + 13],
            bytes[entry + 14],
            bytes[entry + 15],
        ]) as usize;
        let end = offset.checked_add(size)?;
        let payload = bytes.get(offset..end)?;
        if payload.len() >= 8 && payload[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
            return Some(payload);
        }
    }
    None
}

/// Detect an image's true media type from its leading magic bytes. Returns
/// `None` for formats we do not attempt to correct (the block is then left as
/// declared).
fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 4 && bytes[0..4] == [0x00, 0x00, 0x01, 0x00] {
        return Some("image/x-icon");
    }
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        return Some("image/bmp");
    }
    None
}

/// True when `media_type` disagrees with what the base64 payload's magic bytes
/// say the image actually is. Unknown formats and undecodable payloads are
/// treated as "no mismatch" so we never touch data we cannot reason about.
fn media_type_mismatch(media_type: &str, data_b64: &str) -> bool {
    let Some(bytes) = decode_b64(data_b64) else {
        return false;
    };
    match sniff_media_type(&bytes) {
        Some(actual) => !media_type.eq_ignore_ascii_case(actual),
        None => false,
    }
}

/// Rewrite `media_type` to match the payload's magic bytes when they disagree.
/// Returns `true` only when the label was actually corrected.
fn reconcile_media_type(media_type: &mut String, data_b64: &str) -> bool {
    let Some(bytes) = decode_b64(data_b64) else {
        return false;
    };
    match sniff_media_type(&bytes) {
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

    /// Reading a favicon used to send `image/x-icon` straight to the provider,
    /// which rejects it as "not a valid image". It must be transcoded to PNG.
    #[test]
    fn unsupported_format_is_transcoded_to_png() {
        let img = RgbImage::from_pixel(32, 32, image::Rgb([1, 2, 3]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Ico)
            .unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(out.into_inner());

        let messages = vec![image_message_with("image/x-icon", data)];
        let clamped = clamp_outbound_images(&messages).expect("favicon should be transcoded");
        match &clamped[0].content[0] {
            ContentBlock::Image { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(dims(data), (32, 32));
            }
            other => panic!("expected image block, got {other:?}"),
        }
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
            sniff_media_type(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(
            sniff_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_media_type(b"GIF89a....."), Some("image/gif"));
        assert_eq!(
            sniff_media_type(b"RIFF\0\0\0\0WEBP...."),
            Some("image/webp")
        );
        assert_eq!(sniff_media_type(b"not an image"), None);
    }
}
