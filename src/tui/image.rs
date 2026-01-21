//! Terminal image display support
//!
//! Supports Kitty graphics protocol (Kitty, Ghostty) and iTerm2 inline images.
//! Falls back to a simple placeholder if no image protocol is available.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::io::{self, Write};
use std::path::Path;

/// Terminal image protocol support
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageProtocol {
    /// Kitty graphics protocol (most feature-rich)
    Kitty,
    /// iTerm2 inline images
    ITerm2,
    /// No image support
    None,
}

impl ImageProtocol {
    /// Detect the best available image protocol for the current terminal
    pub fn detect() -> Self {
        // Check for Kitty first (most capable)
        if std::env::var("KITTY_WINDOW_ID").is_ok() {
            return Self::Kitty;
        }

        // Check TERM for kitty
        if let Ok(term) = std::env::var("TERM") {
            if term.contains("kitty") {
                return Self::Kitty;
            }
        }

        // Check for iTerm2
        if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
            if term_program == "iTerm.app" {
                return Self::ITerm2;
            }
        }

        // Check LC_TERMINAL for iTerm2
        if let Ok(lc_terminal) = std::env::var("LC_TERMINAL") {
            if lc_terminal == "iTerm2" {
                return Self::ITerm2;
            }
        }

        Self::None
    }

    /// Check if image display is supported
    pub fn is_supported(&self) -> bool {
        *self != Self::None
    }
}

/// Display parameters for terminal images
#[derive(Debug, Clone)]
pub struct ImageDisplayParams {
    /// Maximum width in terminal columns
    pub max_cols: u16,
    /// Maximum height in terminal rows
    pub max_rows: u16,
}

impl Default for ImageDisplayParams {
    fn default() -> Self {
        Self {
            max_cols: 80,
            max_rows: 24,
        }
    }
}

impl ImageDisplayParams {
    /// Create display params based on terminal size
    pub fn from_terminal() -> Self {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 40));

        // Use about 2/3 of terminal width, capped at 100 columns
        // Use about 1/2 of terminal height, capped at 30 rows
        Self {
            max_cols: (cols * 2 / 3).min(100).max(40),
            max_rows: (rows / 2).min(30).max(10),
        }
    }
}

/// Display an image in the terminal
///
/// Returns Ok(true) if the image was displayed, Ok(false) if not supported,
/// or an error if something went wrong.
pub fn display_image(path: &Path, params: &ImageDisplayParams) -> io::Result<bool> {
    let protocol = ImageProtocol::detect();

    if !protocol.is_supported() {
        return Ok(false);
    }

    // Read the image file
    let data = std::fs::read(path)?;

    // Get image dimensions to calculate aspect ratio
    let (img_width, img_height) = get_image_dimensions(&data).unwrap_or((0, 0));

    match protocol {
        ImageProtocol::Kitty => display_kitty(&data, params, img_width, img_height),
        ImageProtocol::ITerm2 => display_iterm2(&data, path, params, img_width, img_height),
        ImageProtocol::None => Ok(false),
    }
}

/// Get image dimensions from raw data
fn get_image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG: check signature and parse IHDR chunk
    if data.len() > 24 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((width, height));
    }

    // JPEG: look for SOF0/SOF2 markers
    if data.len() > 2 && data[0] == 0xFF && data[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            // SOF0 (baseline) or SOF2 (progressive)
            if marker == 0xC0 || marker == 0xC2 {
                let height = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let width = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((width, height));
            }
            // Skip to next marker
            if i + 3 < data.len() {
                let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                i += 2 + len;
            } else {
                break;
            }
        }
    }

    // GIF: parse header
    if data.len() > 10 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
        let width = u16::from_le_bytes([data[6], data[7]]) as u32;
        let height = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((width, height));
    }

    // WebP: parse RIFF header
    if data.len() > 30 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        // VP8 chunk
        if &data[12..16] == b"VP8 " && data.len() > 30 {
            // VP8 bitstream starts at offset 23, dimensions at offset 26
            if data[23] == 0x9D && data[24] == 0x01 && data[25] == 0x2A {
                let width = (u16::from_le_bytes([data[26], data[27]]) & 0x3FFF) as u32;
                let height = (u16::from_le_bytes([data[28], data[29]]) & 0x3FFF) as u32;
                return Some((width, height));
            }
        }
        // VP8L (lossless)
        if &data[12..16] == b"VP8L" && data.len() > 25 {
            let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            return Some((width, height));
        }
    }

    None
}

/// Calculate display size maintaining aspect ratio
fn calculate_display_size(
    img_width: u32,
    img_height: u32,
    max_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    if img_width == 0 || img_height == 0 {
        return (max_cols.min(40), max_rows.min(20));
    }

    // Terminal cells are typically ~2:1 aspect ratio (taller than wide)
    // So we need to account for that when calculating display size
    let cell_aspect = 2.0; // height/width ratio of a terminal cell

    let img_aspect = img_width as f64 / img_height as f64;
    let max_width = max_cols as f64;
    let max_height = max_rows as f64 * cell_aspect; // Convert rows to "width units"

    let (display_width, display_height) = if img_aspect > max_width / max_height {
        // Image is wider than available space
        (max_width, max_width / img_aspect)
    } else {
        // Image is taller than available space
        (max_height * img_aspect, max_height)
    };

    (
        (display_width as u16).max(10),
        (display_height / cell_aspect) as u16, // Convert back to rows
    )
}

/// Display image using Kitty graphics protocol
fn display_kitty(
    data: &[u8],
    params: &ImageDisplayParams,
    img_width: u32,
    img_height: u32,
) -> io::Result<bool> {
    let (cols, rows) =
        calculate_display_size(img_width, img_height, params.max_cols, params.max_rows);

    // Encode image data as base64
    let encoded = BASE64.encode(data);

    let mut stdout = io::stdout().lock();

    // Kitty graphics protocol:
    // \x1b_G<key>=<value>,...;<payload>\x1b\\
    //
    // Keys:
    //   a=T - action: transmit and display
    //   f=100 - format: auto-detect
    //   c=<cols> - display width in cells
    //   r=<rows> - display height in cells
    //   m=1 - more data follows (chunked)
    //   m=0 - final chunk

    // Send in chunks (max 4096 bytes per chunk for safety)
    const CHUNK_SIZE: usize = 4096;
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(CHUNK_SIZE)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();

    for (i, chunk) in chunks.iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == chunks.len() - 1;
        let more = if is_last { 0 } else { 1 };

        if is_first {
            // First chunk includes all parameters
            write!(
                stdout,
                "\x1b_Ga=T,f=100,c={},r={},m={};{}\x1b\\",
                cols, rows, more, chunk
            )?;
        } else {
            // Subsequent chunks only have m flag
            write!(stdout, "\x1b_Gm={};{}\x1b\\", more, chunk)?;
        }
    }

    // Newline after image
    writeln!(stdout)?;
    stdout.flush()?;

    Ok(true)
}

/// Display image using iTerm2 inline image protocol
fn display_iterm2(
    data: &[u8],
    path: &Path,
    params: &ImageDisplayParams,
    img_width: u32,
    img_height: u32,
) -> io::Result<bool> {
    let (cols, _rows) =
        calculate_display_size(img_width, img_height, params.max_cols, params.max_rows);

    // Encode image data as base64
    let encoded = BASE64.encode(data);

    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());
    let filename_b64 = BASE64.encode(filename.as_bytes());

    let mut stdout = io::stdout().lock();

    // iTerm2 inline image protocol:
    // \x1b]1337;File=name=<base64name>;size=<size>;inline=1;width=<cols>:<base64data>\x07
    write!(
        stdout,
        "\x1b]1337;File=name={};size={};inline=1;width={}:{}\x07",
        filename_b64,
        data.len(),
        cols,
        encoded
    )?;

    // Newline after image
    writeln!(stdout)?;
    stdout.flush()?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_detection() {
        // This test just verifies the detection doesn't panic
        let protocol = ImageProtocol::detect();
        println!("Detected protocol: {:?}", protocol);
    }

    #[test]
    fn test_calculate_display_size() {
        // Wide image
        let (w, h) = calculate_display_size(1920, 1080, 80, 24);
        assert!(w <= 80);
        assert!(h <= 24);

        // Tall image
        let (w, h) = calculate_display_size(1080, 1920, 80, 24);
        assert!(w <= 80);
        assert!(h <= 24);

        // Square image
        let (w, h) = calculate_display_size(500, 500, 80, 24);
        assert!(w <= 80);
        assert!(h <= 24);
    }
}
