use anyhow::Result;

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

pub fn generated_image_side_panel_markdown(
    path: &str,
    metadata_path: Option<&str>,
    output_format: &str,
    revised_prompt: Option<&str>,
) -> String {
    let mut markdown = String::from("# Generated image\n\n");
    markdown.push_str(&format!("![Generated image]({path})\n\n"));
    markdown.push_str(&format!("- Image: `{path}`\n"));
    markdown.push_str(&format!("- Format: `{output_format}`\n"));
    if let Some(metadata_path) = metadata_path {
        markdown.push_str(&format!("- Metadata: `{metadata_path}`\n"));
    }
    if let Some(revised_prompt) = revised_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        markdown.push_str("\n## Revised prompt\n\n");
        markdown.push_str(revised_prompt.trim());
        markdown.push('\n');
    }
    markdown
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
    let content =
        generated_image_side_panel_markdown(path, metadata_path, output_format, revised_prompt);
    crate::side_panel::write_markdown_page(
        session_id,
        &page_id,
        Some("Generated image"),
        &content,
        true,
    )
}
