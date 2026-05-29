use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub fn generated_image_side_panel_page_id(id: &str) -> String {
    let safe: String = id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        .take(74)
        .collect();
    if safe.is_empty() {
        "image.generated".to_string()
    } else {
        format!("image.{safe}")
    }
}

#[derive(Debug, Default, Clone)]
struct GeneratedImageMetadataSummary {
    provider: Option<String>,
    native_tool: Option<String>,
    id: Option<String>,
    status: Option<String>,
    created_at: Option<String>,
    byte_count: Option<u64>,
    revised_prompt: Option<String>,
    generation_options: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct GeneratedImagePanelInfo {
    path: String,
    file_label: String,
    metadata_path: Option<String>,
    output_format: String,
    dimensions: Option<(u32, u32)>,
    byte_count: Option<u64>,
    revised_prompt: Option<String>,
    metadata: GeneratedImageMetadataSummary,
}

impl GeneratedImagePanelInfo {
    fn from_inputs(
        path: &str,
        metadata_path: Option<&str>,
        output_format: &str,
        revised_prompt: Option<&str>,
    ) -> Self {
        let dimensions = ::image::image_dimensions(path).ok();
        let file_byte_count = std::fs::metadata(path).ok().map(|metadata| metadata.len());
        let metadata = metadata_path
            .filter(|value| !value.trim().is_empty())
            .and_then(read_generated_image_metadata_summary)
            .unwrap_or_default();
        let revised_prompt = revised_prompt
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| metadata.revised_prompt.clone());

        Self {
            path: path.to_string(),
            file_label: jcode_terminal_image::metadata::compact_path_label(path),
            metadata_path: metadata_path
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            output_format: output_format.to_string(),
            dimensions,
            byte_count: file_byte_count.or(metadata.byte_count),
            revised_prompt,
            metadata,
        }
    }

    fn title(&self) -> String {
        let mut title = format!("Image · {}", self.file_label);
        if let Some((width, height)) = self.dimensions {
            title.push_str(" · ");
            title.push_str(&jcode_terminal_image::metadata::format_dimensions(
                width, height,
            ));
        }
        title
    }

    fn summary_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some((width, height)) = self.dimensions {
            parts.push(jcode_terminal_image::metadata::format_dimensions(width, height));
        }
        parts.push(jcode_terminal_image::metadata::compact_image_format(
            &self.output_format,
        ));
        if let Some(byte_count) = self.byte_count {
            parts.push(jcode_terminal_image::metadata::format_byte_count(byte_count));
        }
        if let Some(source) = self.source_summary() {
            parts.push(source);
        }
        parts
    }

    fn source_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(provider) = self.metadata.provider.as_deref() {
            parts.push(provider.to_string());
        }
        if let Some(status) = self.metadata.status.as_deref() {
            parts.push(status.to_string());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    fn markdown(&self) -> String {
        let mut markdown = String::new();
        markdown.push_str("# ");
        markdown.push_str(&escape_markdown_text(&self.title()));
        markdown.push_str("\n\n");

        let summary = self.summary_parts();
        if !summary.is_empty() {
            markdown.push_str(&summary.join(" · "));
            markdown.push_str("\n\n");
        }

        markdown.push_str(&format!("![Generated image]({})\n\n", self.path));

        markdown.push_str("## Details\n\n");
        markdown.push_str(&format!("- File: {}\n", markdown_code(&self.path)));
        if let Some((width, height)) = self.dimensions {
            let mut dimensions = jcode_terminal_image::metadata::format_dimensions(width, height);
            if let Some(ratio) = jcode_terminal_image::metadata::aspect_ratio(width, height) {
                dimensions.push_str(&format!(" ({ratio})"));
            }
            markdown.push_str(&format!("- Dimensions: {}\n", markdown_code(&dimensions)));
        }
        markdown.push_str(&format!(
            "- Format: {}\n",
            markdown_code(&jcode_terminal_image::metadata::compact_image_format(
                &self.output_format,
            ))
        ));
        if let Some(byte_count) = self.byte_count {
            markdown.push_str(&format!(
                "- Bytes: {}\n",
                markdown_code(&jcode_terminal_image::metadata::format_byte_count(byte_count))
            ));
        }
        if let Some(metadata_path) = self.metadata_path.as_deref() {
            markdown.push_str(&format!("- Metadata: {}\n", markdown_code(metadata_path)));
        }
        if let Some(provider) = self.metadata.provider.as_deref() {
            markdown.push_str(&format!("- Provider: {}\n", markdown_code(provider)));
        }
        if let Some(native_tool) = self.metadata.native_tool.as_deref() {
            markdown.push_str(&format!("- Tool: {}\n", markdown_code(native_tool)));
        }
        if let Some(status) = self.metadata.status.as_deref() {
            markdown.push_str(&format!("- Status: {}\n", markdown_code(status)));
        }
        if let Some(id) = self.metadata.id.as_deref() {
            markdown.push_str(&format!("- ID: {}\n", markdown_code(id)));
        }
        if let Some(created_at) = self.metadata.created_at.as_deref() {
            markdown.push_str(&format!("- Created: {}\n", markdown_code(created_at)));
        }
        for (label, value) in &self.metadata.generation_options {
            markdown.push_str(&format!("- {label}: {}\n", markdown_code(value)));
        }

        if let Some(revised_prompt) = self.revised_prompt.as_deref() {
            markdown.push_str("\n## Revised prompt\n\n");
            markdown.push_str(revised_prompt.trim());
            markdown.push('\n');
        }

        markdown
    }
}

pub fn generated_image_side_panel_markdown(
    path: &str,
    metadata_path: Option<&str>,
    output_format: &str,
    revised_prompt: Option<&str>,
) -> String {
    GeneratedImagePanelInfo::from_inputs(path, metadata_path, output_format, revised_prompt)
        .markdown()
}

pub fn write_generated_image_side_panel_page(
    session_id: &str,
    id: &str,
    path: &str,
    metadata_path: Option<&str>,
    output_format: &str,
    revised_prompt: Option<&str>,
) -> Result<crate::side_panel::SidePanelSnapshot> {
    let page_id = generated_image_side_panel_page_id(id);
    let info =
        GeneratedImagePanelInfo::from_inputs(path, metadata_path, output_format, revised_prompt);
    let title = info.title();
    let content = info.markdown();
    crate::side_panel::write_markdown_page(session_id, &page_id, Some(&title), &content, true)
}

fn read_generated_image_metadata_summary(path: &str) -> Option<GeneratedImageMetadataSummary> {
    let raw = std::fs::read_to_string(Path::new(path)).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let response_item = value.get("response_item");
    let generation_options = response_item
        .into_iter()
        .flat_map(|item| {
            [
                ("size", "Size"),
                ("quality", "Quality"),
                ("background", "Background"),
                ("moderation", "Moderation"),
                ("output_compression", "Compression"),
            ]
            .into_iter()
            .filter_map(|(key, label)| {
                json_scalar_to_string(item.get(key)?).map(|value| (label.to_string(), value))
            })
        })
        .collect();

    Some(GeneratedImageMetadataSummary {
        provider: string_field(&value, "provider"),
        native_tool: string_field(&value, "native_tool"),
        id: string_field(&value, "id"),
        status: string_field(&value, "status"),
        created_at: value
            .get("created_at_unix_ms")
            .and_then(Value::as_u64)
            .and_then(format_unix_ms),
        byte_count: value.get("byte_count").and_then(Value::as_u64),
        revised_prompt: string_field(&value, "revised_prompt"),
        generation_options,
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(json_scalar_to_string)
}

fn json_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.trim().to_string()).filter(|value| !value.is_empty()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn format_unix_ms(ms: u64) -> Option<String> {
    let ms = i64::try_from(ms).ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|datetime| datetime.format("%Y-%m-%d %H:%M UTC").to_string())
}

fn escape_markdown_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
}

fn markdown_code(value: &str) -> String {
    format!("`{}`", value.replace('`', "′"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_image_side_panel_markdown_prefers_compact_useful_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "jcode-generated-image-side-panel-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp generated image dir");
        let path = dir.join("generated.png");
        ::image::RgbaImage::from_pixel(3, 2, ::image::Rgba([20, 40, 60, 255]))
            .save(&path)
            .expect("write temp generated png");
        let metadata_path = dir.join("generated.json");
        std::fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "provider": "openai",
                "native_tool": "image_generation",
                "id": "img_123",
                "status": "completed",
                "created_at_unix_ms": 1_765_000_000_000u64,
                "byte_count": 42,
                "revised_prompt": "A polished generated prompt",
                "response_item": {
                    "size": "1024x1024",
                    "quality": "high"
                }
            }))
            .expect("serialize metadata"),
        )
        .expect("write generated metadata");

        let markdown = generated_image_side_panel_markdown(
            path.to_str().expect("utf8 path"),
            Some(metadata_path.to_str().expect("utf8 metadata path")),
            "png",
            None,
        );

        assert!(markdown.contains("# Image · generated.png"), "{markdown}");
        assert!(markdown.contains("3×2"), "{markdown}");
        assert!(markdown.contains("PNG"), "{markdown}");
        assert!(markdown.contains("- Provider: `openai`"), "{markdown}");
        assert!(
            markdown.contains("- Tool: `image_generation`"),
            "{markdown}"
        );
        assert!(markdown.contains("- Status: `completed`"), "{markdown}");
        assert!(markdown.contains("- ID: `img_123`"), "{markdown}");
        assert!(markdown.contains("- Size: `1024x1024`"), "{markdown}");
        assert!(markdown.contains("- Quality: `high`"), "{markdown}");
        assert!(markdown.contains("## Revised prompt"), "{markdown}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_image_side_panel_title_includes_filename_and_dimensions() {
        let dir = std::env::temp_dir().join(format!(
            "jcode-generated-image-title-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp generated image dir");
        let path = dir.join("wide.png");
        ::image::RgbaImage::from_pixel(4, 2, ::image::Rgba([0, 0, 0, 255]))
            .save(&path)
            .expect("write temp generated png");

        let info = GeneratedImagePanelInfo::from_inputs(
            path.to_str().expect("utf8 path"),
            None,
            "png",
            None,
        );

        assert_eq!(info.title(), "Image · wide.png · 4×2");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
