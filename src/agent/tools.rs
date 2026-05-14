use crate::message::{ContentBlock, ToolCall};
use crate::tool::ToolOutput;
use chrono::Utc;
use std::fs;
use std::path::PathBuf;

const TOOL_OUTPUT_ELIDE_THRESHOLD_TOKENS: usize = 500;
const TOOL_OUTPUT_ELIDE_HEAD_TOKENS: usize = 250;
const TOOL_OUTPUT_ELIDE_TAIL_TOKENS: usize = 250;

pub(super) fn tool_output_to_content_blocks(
    tool_use_id: String,
    output: ToolOutput,
) -> Vec<ContentBlock> {
    let content = maybe_elide_and_cache_tool_output(&tool_use_id, output.output);
    let mut blocks = vec![ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error: None,
    }];
    for img in output.images {
        blocks.push(ContentBlock::Image {
            media_type: img.media_type,
            data: img.data,
        });
        if let Some(label) = img.label.filter(|label| !label.trim().is_empty()) {
            blocks.push(ContentBlock::Text {
                text: format!(
                    "[Attached image associated with the preceding tool result: {}]",
                    label
                ),
                cache_control: None,
            });
        }
    }
    blocks
}

pub(super) fn maybe_elide_and_cache_tool_output(tool_use_id: &str, output: String) -> String {
    if output.contains("[... elided ") && output.contains("jcode-tool-output-cache") {
        return output;
    }

    let tokens: Vec<&str> = output.split_whitespace().collect();
    if tokens.len() <= TOOL_OUTPUT_ELIDE_THRESHOLD_TOKENS {
        return output;
    }

    match cache_full_tool_output(tool_use_id, &output) {
        Ok(path) => {
            let head = tokens
                .iter()
                .take(TOOL_OUTPUT_ELIDE_HEAD_TOKENS)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            let tail = tokens
                .iter()
                .rev()
                .take(TOOL_OUTPUT_ELIDE_TAIL_TOKENS)
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" ");
            let omitted = tokens
                .len()
                .saturating_sub(TOOL_OUTPUT_ELIDE_HEAD_TOKENS + TOOL_OUTPUT_ELIDE_TAIL_TOKENS);
            format!(
                "{}\n\n[... elided {} middle tokens from tool output; full output cached at {} ...]\n\n{}",
                head,
                omitted,
                path.display(),
                tail
            )
        }
        Err(err) => {
            let head = tokens
                .iter()
                .take(TOOL_OUTPUT_ELIDE_HEAD_TOKENS)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            let tail = tokens
                .iter()
                .rev()
                .take(TOOL_OUTPUT_ELIDE_TAIL_TOKENS)
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "{}\n\n[... elided middle of large tool output; failed to cache full output: {} ...]\n\n{}",
                head, err, tail
            )
        }
    }
}

fn cache_full_tool_output(tool_use_id: &str, output: &str) -> std::io::Result<PathBuf> {
    let now = Utc::now();
    let base = std::env::temp_dir()
        .join("jcode-tool-output-cache")
        .join(now.format("%Y-%m-%d").to_string())
        .join(now.format("%H").to_string());
    fs::create_dir_all(&base)?;
    let safe_id: String = tool_use_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let file = base.join(format!(
        "{}-{}-{}.txt",
        now.format("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id(),
        safe_id
    ));
    fs::write(&file, output)?;
    Ok(file)
}

pub(super) fn print_tool_summary(tool: &ToolCall) {
    match tool.name.as_str() {
        "bash" => {
            if let Some(cmd) = tool.input.get("command").and_then(|v| v.as_str()) {
                let short = if cmd.len() > 60 {
                    format!("{}...", crate::util::truncate_str(cmd, 60))
                } else {
                    cmd.to_string()
                };
                println!("$ {}", short);
            }
        }
        "read" | "write" | "edit" => {
            if let Some(path) = tool.input.get("file_path").and_then(|v| v.as_str()) {
                println!("{}", path);
            }
        }
        "glob" | "grep" => {
            if let Some(pattern) = tool.input.get("pattern").and_then(|v| v.as_str()) {
                println!("'{}'", pattern);
            }
        }
        "ls" => {
            let path = tool
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            println!("{}", path);
        }
        _ => {}
    }
}
