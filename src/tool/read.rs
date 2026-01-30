use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, FileOp, FileTouch};
use crate::tui::image::{display_image, ImageDisplayParams, ImageProtocol};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const DEFAULT_LIMIT: usize = 2000;
const MAX_LINE_LEN: usize = 2000;

pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ReadInput {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns lines with line numbers. \
         Supports reading specific ranges with offset and limit parameters. \
         Can read text files, and will indicate if a file is binary."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["file_path"],
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The path to the file to read (absolute or relative)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default 2000)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ReadInput = serde_json::from_value(input)?;

        let path = Path::new(&params.file_path);

        // Check if file exists
        if !path.exists() {
            // Try to find similar files
            let suggestions = find_similar_files(&params.file_path);
            if suggestions.is_empty() {
                return Err(anyhow::anyhow!("File not found: {}", params.file_path));
            } else {
                return Err(anyhow::anyhow!(
                    "File not found: {}\nDid you mean: {}",
                    params.file_path,
                    suggestions.join(", ")
                ));
            }
        }

        // Check for image files and display in terminal if supported
        if is_image_file(path) {
            return handle_image_file(path, &params.file_path);
        }

        // Check for PDF files and extract text
        if is_pdf_file(path) {
            return handle_pdf_file(path, &params.file_path);
        }

        // Check for binary files
        if is_binary_file(path) {
            return Ok(ToolOutput::new(format!(
                "Binary file detected: {}\nUse appropriate tools to handle binary files.",
                params.file_path
            )));
        }

        // Read file
        let content = tokio::fs::read_to_string(path).await?;
        let lines: Vec<&str> = content.lines().collect();

        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT);

        let total_lines = lines.len();
        let end = (offset + limit).min(total_lines);

        // Publish file touch event for swarm coordination
        Bus::global().publish(BusEvent::FileTouch(FileTouch {
            session_id: ctx.session_id.clone(),
            path: path.to_path_buf(),
            op: FileOp::Read,
            summary: Some(format!(
                "read lines {}-{} of {}",
                offset + 1,
                end,
                total_lines
            )),
        }));

        let mut output = String::new();

        for (i, line) in lines.iter().enumerate().skip(offset).take(limit) {
            let line_num = i + 1; // 1-based line numbers
            let truncated = if line.len() > MAX_LINE_LEN {
                format!("{}...", &line[..MAX_LINE_LEN])
            } else {
                line.to_string()
            };
            output.push_str(&format!("{:>5}\t{}\n", line_num, truncated));
        }

        // Add metadata
        if end < total_lines {
            output.push_str(&format!(
                "\n... {} more lines (use offset={} to continue)\n",
                total_lines - end,
                end
            ));
        }

        if output.is_empty() {
            Ok(ToolOutput::new("(empty file)"))
        } else {
            Ok(ToolOutput::new(output))
        }
    }
}

fn is_binary_file(path: &Path) -> bool {
    // Check by extension first
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_lowercase();
        let binary_exts = [
            "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "zip", "tar", "gz", "bz2", "xz",
            "7z", "rar", "exe", "dll", "so", "dylib", "o", "a", "class", "pyc", "wasm", "mp3",
            "mp4", "avi", "mov", "mkv", "flac", "ogg", "wav",
        ];
        if binary_exts.contains(&ext.as_str()) {
            return true;
        }
    }

    // Check first bytes for binary content
    if let Ok(bytes) = std::fs::read(path) {
        let check_len = bytes.len().min(8192);
        let null_count = bytes[..check_len].iter().filter(|&&b| b == 0).count();
        return null_count > check_len / 10; // More than 10% null bytes = binary
    }

    false
}

fn find_similar_files(path: &str) -> Vec<String> {
    let path = Path::new(path);
    let parent = path.parent().unwrap_or(Path::new("."));
    let filename = path.file_name().map(|s| s.to_string_lossy().to_lowercase());

    let mut suggestions = Vec::new();

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if let Some(ref target) = filename {
                // Simple similarity check
                let target_str: &str = target.as_ref();
                if name.contains(target_str) || target_str.contains(&name as &str) {
                    suggestions.push(entry.path().display().to_string());
                    if suggestions.len() >= 3 {
                        break;
                    }
                }
            }
        }
    }

    suggestions
}

/// Check if a file is an image based on extension
fn is_image_file(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_lowercase();
        matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
        )
    } else {
        false
    }
}

/// Handle reading an image file - display in terminal if supported
fn handle_image_file(path: &Path, file_path: &str) -> Result<ToolOutput> {
    let protocol = ImageProtocol::detect();

    // Get file size for metadata
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len();

    // Try to get image dimensions
    let dimensions = if let Ok(data) = std::fs::read(path) {
        get_image_dimensions_from_data(&data)
    } else {
        None
    };

    let dim_str = dimensions
        .map(|(w, h)| format!("{}x{}", w, h))
        .unwrap_or_else(|| "unknown".to_string());

    let size_str = if file_size < 1024 {
        format!("{} bytes", file_size)
    } else if file_size < 1024 * 1024 {
        format!("{:.1} KB", file_size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", file_size as f64 / 1024.0 / 1024.0)
    };

    // Display image in terminal if supported
    if protocol.is_supported() {
        let params = ImageDisplayParams::from_terminal();
        match display_image(path, &params) {
            Ok(true) => {
                return Ok(ToolOutput::new(format!(
                    "Image: {} ({})\nDimensions: {}\nDisplayed in terminal using {:?} protocol",
                    file_path, size_str, dim_str, protocol
                )));
            }
            Ok(false) => {
                // Fall through to text output
            }
            Err(e) => {
                crate::logging::info(&format!("Warning: Failed to display image: {}", e));
                // Fall through to text output
            }
        }
    }

    // Fallback: return metadata about the image
    Ok(ToolOutput::new(format!(
        "Image: {} ({})\nDimensions: {}\nTerminal does not support inline images (try Kitty or iTerm2)",
        file_path, size_str, dim_str
    )))
}

/// Get image dimensions from raw data (duplicated from tui::image for convenience)
fn get_image_dimensions_from_data(data: &[u8]) -> Option<(u32, u32)> {
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

    None
}

/// Check if a file is a PDF based on extension
fn is_pdf_file(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        ext.to_string_lossy().to_lowercase() == "pdf"
    } else {
        false
    }
}

/// Handle reading a PDF file - extract text content
fn handle_pdf_file(path: &Path, file_path: &str) -> Result<ToolOutput> {
    // Get file metadata
    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len();

    let size_str = if file_size < 1024 {
        format!("{} bytes", file_size)
    } else if file_size < 1024 * 1024 {
        format!("{:.1} KB", file_size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", file_size as f64 / 1024.0 / 1024.0)
    };

    // Extract text from PDF
    match pdf_extract::extract_text(path) {
        Ok(text) => {
            let mut output = String::new();
            output.push_str(&format!("PDF: {} ({})\n", file_path, size_str));
            output.push_str(&format!("{}\n", "=".repeat(60)));

            // Split into pages (pdf_extract uses form feed \x0c as page separator)
            let pages: Vec<&str> = text.split('\x0c').collect();
            let page_count = pages.len();

            output.push_str(&format!("Pages: {}\n\n", page_count));

            for (i, page) in pages.iter().enumerate() {
                let page_text = page.trim();
                if !page_text.is_empty() {
                    output.push_str(&format!("--- Page {} ---\n", i + 1));
                    // Limit each page to reasonable length
                    if page_text.len() > 10000 {
                        output.push_str(crate::util::truncate_str(page_text, 10000));
                        output.push_str("\n... (page truncated)\n");
                    } else {
                        output.push_str(page_text);
                    }
                    output.push_str("\n\n");
                }
            }

            Ok(ToolOutput::new(output))
        }
        Err(e) => {
            // Fall back to metadata only if text extraction fails
            Ok(ToolOutput::new(format!(
                "PDF: {} ({})\nCould not extract text: {}\nThis may be a scanned/image-based PDF.",
                file_path, size_str, e
            )))
        }
    }
}
