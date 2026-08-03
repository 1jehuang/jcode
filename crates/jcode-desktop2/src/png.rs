//! Minimal PNG encoding, shared by offscreen capture and clipboard images.
//!
//! One encoder rather than two: the capture path needs to write a file and the
//! paste path needs the same bytes in memory (arboard hands back raw RGBA, and
//! the harness API only carries encoded images), so the byte-level format work
//! lives here where both can reach it. Stored (uncompressed) zlib blocks keep
//! it dependency-free; these images are debug captures and one-off pastes, not
//! a hot path.

/// Encode tight RGBA8 rows as a PNG.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    // Raw scanlines with filter byte 0, stored (uncompressed) zlib blocks.
    let mut raw = Vec::with_capacity((height * (1 + width * 4)) as usize);
    for row in 0..height {
        raw.push(0);
        let start = (row * width * 4) as usize;
        raw.extend_from_slice(&rgba[start..start + (width * 4) as usize]);
    }
    let mut idat = vec![0x78, 0x01];
    let mut adler_a: u32 = 1;
    let mut adler_b: u32 = 0;
    for &byte in &raw {
        adler_a = (adler_a + u32::from(byte)) % 65521;
        adler_b = (adler_b + adler_a) % 65521;
    }
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        idat.push(u8::from(last));
        idat.extend_from_slice(&(block.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        idat.extend_from_slice(block);
    }
    idat.extend_from_slice(&((adler_b << 16) | adler_a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &idat);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Pixel dimensions of an encoded image, read from its header.
///
/// Only the containers the clipboard actually hands us are decoded, and only
/// far enough to answer "how big is it": the app reports the size to the user
/// so a paste is verifiable, and a wrong number would be worse than none.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG: IHDR is always the first chunk, width and height at a fixed offset.
    if data.len() > 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") {
        let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((width, height));
    }
    // JPEG: walk the segment chain to the start-of-frame, which carries them.
    if data.len() > 4 && data.starts_with(b"\xff\xd8") {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            // SOF0..SOF3 and SOF5..SOF15; DHT/DAC/RST are not frame headers.
            if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                let height = u16::from_be_bytes([data[i + 5], data[i + 6]]);
                let width = u16::from_be_bytes([data[i + 7], data[i + 8]]);
                return Some((u32::from(width), u32::from(height)));
            }
            i += 2 + length;
        }
    }
    None
}

/// Standard base64, no line breaks: how the harness API carries image bytes.
///
/// Written out rather than depended on. It is twenty lines, it sits beside the
/// encoder whose output it wraps, and a paste must not be able to stop working
/// because a dependency line went missing.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let packed =
            (u32::from(triple[0]) << 16) | (u32::from(triple[1]) << 8) | u32::from(triple[2]);
        for position in 0..4 {
            // Positions past the end of a short final chunk are padding, not
            // data: emitting the zero bytes instead would decode to a longer
            // image than the one that was copied.
            if position <= chunk.len() {
                out.push(char::from(
                    ALPHABET[(packed >> (18 - 6 * position)) as usize & 0x3F],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoder and the header reader must agree, or the size reported for a
    /// pasted image would be a guess about our own output.
    #[test]
    fn encoded_png_reports_its_own_size() {
        let png = encode_rgba(3, 2, &[0u8; 3 * 2 * 4]);
        assert_eq!(dimensions(&png), Some((3, 2)));
    }

    #[test]
    fn non_image_bytes_have_no_dimensions() {
        assert_eq!(dimensions(b"not an image at all, just text"), None);
    }

    /// A minimal JPEG frame header: the clipboard hands JPEG through
    /// untouched, so its size has to come from the container, not from us.
    #[test]
    fn jpeg_dimensions_come_from_the_frame_header() {
        let mut jpeg = vec![0xFF, 0xD8];
        // APP0-ish filler segment, skipped by length.
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        // SOF0: length, precision, height 0x0010, width 0x0020, components.
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x20, 0x03]);
        jpeg.extend_from_slice(&[0u8; 16]);
        assert_eq!(dimensions(&jpeg), Some((32, 16)));
    }

    /// The encoding has to be everyone else's base64, or the daemon receives
    /// bytes that only fail much later, as a model confused by the image.
    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"hello world"), "aGVsbG8gd29ybGQ=");
        // Every byte value, so no sign or shift error hides in the top bits.
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(base64(&all).len(), all.len().div_ceil(3) * 4);
        assert!(
            base64(&all).ends_with("/P3+/w=="),
            "the top byte values encode wrong"
        );
    }

    /// A real PNG through both halves of this module: encode, then base64, is
    /// exactly the path a pasted screenshot takes.
    #[test]
    fn a_png_survives_encoding_and_base64_together() {
        let png = encode_rgba(2, 2, &[7u8; 2 * 2 * 4]);
        assert!(
            base64(&png).starts_with("iVBORw0KGgo"),
            "not a base64 PNG header"
        );
        assert_eq!(dimensions(&png), Some((2, 2)));
    }
}
