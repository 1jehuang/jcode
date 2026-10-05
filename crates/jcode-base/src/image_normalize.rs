//! Normalize image bytes into a format every vision provider accepts.
//!
//! Anthropic, OpenAI and Gemini all accept `image/png`, `image/jpeg`,
//! `image/gif` and `image/webp`. Anything else (BMP, ICO, TIFF, HEIC, ...)
//! is rejected with a 400. Once such a block is persisted in a session it is
//! replayed on every later request, so a single unsupported image used to brick
//! the whole conversation (#1712).
//!
//! Every place that turns bytes into an image block (the `read` tool,
//! drag-and-drop paste, generated images) and the outbound request chokepoint
//! route through [`normalize_image_bytes`]. The format is detected from magic
//! bytes, never from the file extension. Decodable non-provider formats are
//! re-encoded to PNG. Everything else is refused with a human-readable reason
//! so callers can fall back to text.

/// Media types every vision-capable provider jcode talks to accepts.
pub const PROVIDER_IMAGE_MEDIA_TYPES: [&str; 4] =
    ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// True when `media_type` is one of [`PROVIDER_IMAGE_MEDIA_TYPES`].
pub fn is_provider_media_type(media_type: &str) -> bool {
    PROVIDER_IMAGE_MEDIA_TYPES
        .iter()
        .any(|accepted| accepted.eq_ignore_ascii_case(media_type))
}

/// Detect a provider-accepted media type from the leading magic bytes.
/// Returns `None` for every other format.
pub fn sniff_provider_media_type(bytes: &[u8]) -> Option<&'static str> {
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
    None
}

/// Name of a recognised non-provider image format, for conversion and for
/// user-facing explanations.
fn sniff_other_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        return Some("BMP");
    }
    if bytes.len() >= 4 && bytes[0..4] == [0, 0, 1, 0] {
        return Some("ICO");
    }
    if bytes.len() >= 4 && (&bytes[0..4] == b"II*\0" || &bytes[0..4] == b"MM\0*") {
        return Some("TIFF");
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" {
            return Some("AVIF");
        }
        if [
            b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1",
        ]
        .iter()
        .any(|b| brand == *b)
        {
            return Some("HEIC");
        }
    }
    let head = &bytes[..bytes.len().min(256)];
    let head = String::from_utf8_lossy(head);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg")) {
        return Some("SVG");
    }
    None
}

/// An image ready to be attached to a provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderImage {
    /// One of [`PROVIDER_IMAGE_MEDIA_TYPES`], matching the bytes.
    pub media_type: &'static str,
    pub data: Vec<u8>,
    /// Source format name when the bytes were re-encoded (for example "BMP").
    pub converted_from: Option<&'static str>,
}

/// Turn arbitrary image bytes into a provider-safe image.
///
/// - PNG/JPEG/GIF/WebP pass through untouched, labelled by their magic bytes.
/// - BMP/ICO/TIFF are decoded and re-encoded as PNG.
/// - Anything else (HEIC, AVIF, SVG, corrupt data) returns `Err` with a short
///   explanation suitable for showing to the model or the user.
pub fn normalize_image_bytes(data: Vec<u8>) -> Result<ProviderImage, String> {
    if let Some(media_type) = sniff_provider_media_type(&data) {
        return Ok(ProviderImage {
            media_type,
            data,
            converted_from: None,
        });
    }
    match sniff_other_format(&data) {
        Some(format @ ("BMP" | "ICO" | "TIFF")) => match reencode_png(&data) {
            Some(png) => Ok(ProviderImage {
                media_type: "image/png",
                data: png,
                converted_from: Some(format),
            }),
            None => Err(format!(
                "{format} data could not be decoded, so it cannot be sent to the model"
            )),
        },
        Some(format) => Err(format!(
            "{format} is not a format the model can view directly; convert it to PNG or JPEG and read the result"
        )),
        None => Err("unrecognised or corrupt image data, so it cannot be sent to the model".into()),
    }
}

fn reencode_png(data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_bmp() -> Vec<u8> {
        // 2x2 24-bit BMP, rows padded to 4 bytes.
        let row: [u8; 8] = [0, 0, 255, 0, 255, 0, 0, 0];
        let pixels: Vec<u8> = row.iter().chain(row.iter()).copied().collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&2i32.to_le_bytes());
        out.extend_from_slice(&2i32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&pixels);
        out
    }

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn encode_as(format: image::ImageFormat) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(16, 16, image::Rgba([9, 8, 7, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn provider_formats_pass_through_untouched() {
        let png = tiny_png();
        let out = normalize_image_bytes(png.clone()).unwrap();
        assert_eq!(out.media_type, "image/png");
        assert_eq!(out.data, png);
        assert_eq!(out.converted_from, None);
    }

    #[test]
    fn bmp_ico_and_tiff_are_converted_to_png() {
        for (bytes, name) in [
            (tiny_bmp(), "BMP"),
            (encode_as(image::ImageFormat::Ico), "ICO"),
            (encode_as(image::ImageFormat::Tiff), "TIFF"),
        ] {
            let out = normalize_image_bytes(bytes).unwrap();
            assert_eq!(out.media_type, "image/png", "{name}");
            assert_eq!(out.converted_from, Some(name));
            assert_eq!(sniff_provider_media_type(&out.data), Some("image/png"));
            image::load_from_memory(&out.data).expect("converted PNG decodes");
        }
    }

    #[test]
    fn undecodable_formats_are_refused_with_a_reason() {
        let mut heic = vec![0, 0, 0, 24];
        heic.extend_from_slice(b"ftypheic");
        heic.extend_from_slice(&[0; 16]);
        let err = normalize_image_bytes(heic).unwrap_err();
        assert!(err.contains("HEIC"), "{err}");

        let err = normalize_image_bytes(b"<svg xmlns='x'></svg>".to_vec()).unwrap_err();
        assert!(err.contains("SVG"), "{err}");

        let err = normalize_image_bytes(b"definitely not an image".to_vec()).unwrap_err();
        assert!(err.contains("unrecognised"), "{err}");

        let err = normalize_image_bytes(b"BMgarbage".to_vec()).unwrap_err();
        assert!(err.contains("BMP"), "{err}");
    }
}
