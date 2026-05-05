use crate::bus::{BackgroundTaskCompleted, BackgroundTaskProgressEvent, BackgroundTaskStatus};
use base64::Engine as _;
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

pub use jcode_message_types::{
    CacheControl, ConnectionPhase, ContentBlock, InputShellResult, Message, Role, StreamEvent,
    ToolCall, ToolDefinition,
};

mod notifications;

pub use notifications::{
    ParsedBackgroundTaskNotification, ParsedBackgroundTaskProgressNotification,
    background_task_display_label, background_task_status_notice,
    format_background_task_notification_markdown, format_background_task_progress_markdown,
    format_input_shell_result_markdown, input_shell_status_notice,
    parse_background_task_notification_markdown,
    parse_background_task_progress_notification_markdown,
};

fn compile_static_regex(pattern: &str) -> Option<Regex> {
    match Regex::new(pattern) {
        Ok(regex) => Some(regex),
        Err(err) => {
            eprintln!("jcode: failed to compile static regex: {err}");
            None
        }
    }
}

fn compile_static_regexes(patterns: &[&str]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|pattern| compile_static_regex(pattern))
        .collect()
}

pub const TOOL_OUTPUT_MISSING_TEXT: &str =
    "Tool output missing (session interrupted before tool execution completed)";

const STABLE_HASH_SEED: u64 = 0xcbf29ce484222325;
const STABLE_HASH_PRIME: u64 = 0x100000001b3;

fn stable_hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = STABLE_HASH_SEED;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(STABLE_HASH_PRIME);
    }
    hash
}

pub(crate) fn extend_stable_hash(acc: u64, next: u64) -> u64 {
    stable_hash_bytes(&[acc.to_le_bytes().as_slice(), next.to_le_bytes().as_slice()].concat())
}

pub(crate) fn stable_message_hash(message: &Message) -> u64 {
    match serde_json::to_vec(message) {
        Ok(bytes) => stable_hash_bytes(&bytes),
        Err(_) => stable_hash_bytes(format!("{:?}", message).as_bytes()),
    }
}

pub fn ends_with_fresh_user_turn(messages: &[Message]) -> bool {
    for msg in messages.iter().rev() {
        if msg.role != Role::User {
            return false;
        }

        if msg
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        {
            return false;
        }

        if msg.content.is_empty() {
            return false;
        }

        let mut saw_user_text = false;
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with("<system-reminder>") {
                        saw_user_text = true;
                    }
                }
                _ => return false,
            }
        }

        if saw_user_text {
            return true;
        }

        if msg.is_internal_system_reminder() {
            continue;
        }

        return false;
    }

    false
}

/// Sanitize a tool ID so it matches the pattern `^[a-zA-Z0-9_-]+$`.
///
/// Different providers generate tool IDs in different formats. When switching
/// from one provider to another mid-conversation, the historical tool IDs may
/// contain characters that the new provider rejects (e.g., dots in Copilot IDs
/// sent to Anthropic). This function replaces any invalid characters with
/// underscores.
pub fn sanitize_tool_id(id: &str) -> String {
    if id.is_empty() {
        return "unknown".to_string();
    }
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Redact likely secrets from persisted tool output.
///
/// This is a best-effort safeguard for local session history files. It targets
/// high-confidence token/key patterns and common `KEY=VALUE` assignments used by
/// auth flows.
pub fn redact_secrets(text: &str) -> String {
    // Fast path to avoid regex work for most tool outputs.
    let lower = text.to_ascii_lowercase();

    if !text.contains("sk-")
        && !text.contains("ghp_")
        && !text.contains("github_pat_")
        && !text.contains("AIza")
        && !text.contains("ya29.")
        && !text.contains("xox")
        && !lower.contains("api_key")
        && !lower.contains("token")
    {
        return text.to_string();
    }

    static DIRECT_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    static ASSIGNMENT_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

    let direct_patterns = DIRECT_PATTERNS.get_or_init(|| {
        compile_static_regexes(&[
            r"sk-ant-(?:oat|ort)01-[A-Za-z0-9_-]{20,}",
            r"sk-or-v1-[A-Za-z0-9_-]{20,}",
            r"ghp_[A-Za-z0-9]{20,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"ya29\.[A-Za-z0-9._-]{20,}",
            r"AIza[0-9A-Za-z_-]{20,}",
            r"xox[baprs]-[A-Za-z0-9-]{10,}",
        ])
    });

    let assignment_patterns = ASSIGNMENT_PATTERNS.get_or_init(|| {
        compile_static_regexes(&[
            r"(?m)^\s*(OPENROUTER_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(OPENCODE_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(OPENCODE_GO_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(ZHIPU_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(ZAI_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(302AI_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(BASETEN_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(CORTECS_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(DEEPSEEK_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(FIRMWARE_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(HF_TOKEN\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(MOONSHOT_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(NEBIUS_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(SCALEWAY_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(STACKIT_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(GROQ_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(MISTRAL_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(PERPLEXITY_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(TOGETHER_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(DEEPINFRA_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(XAI_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(LMSTUDIO_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(OLLAMA_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(CHUTES_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(CEREBRAS_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(OPENAI_COMPAT_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(ANTHROPIC_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(OPENAI_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(AZURE_OPENAI_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(CURSOR_API_KEY\s*=\s*)[^\r\n]+",
            r"(?m)^\s*(GITHUB_TOKEN\s*=\s*)[^\r\n]+",
        ])
    });

    let mut redacted = text.to_string();
    let mut redacted_keys: HashSet<String> = [
        "OPENROUTER_API_KEY",
        "OPENCODE_API_KEY",
        "OPENCODE_GO_API_KEY",
        "ZHIPU_API_KEY",
        "ZAI_API_KEY",
        "302AI_API_KEY",
        "BASETEN_API_KEY",
        "CORTECS_API_KEY",
        "DEEPSEEK_API_KEY",
        "FIRMWARE_API_KEY",
        "HF_TOKEN",
        "MOONSHOT_API_KEY",
        "NEBIUS_API_KEY",
        "SCALEWAY_API_KEY",
        "STACKIT_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "PERPLEXITY_API_KEY",
        "TOGETHER_API_KEY",
        "DEEPINFRA_API_KEY",
        "XAI_API_KEY",
        "LMSTUDIO_API_KEY",
        "OLLAMA_API_KEY",
        "CHUTES_API_KEY",
        "CEREBRAS_API_KEY",
        "OPENAI_COMPAT_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "CURSOR_API_KEY",
        "GITHUB_TOKEN",
    ]
    .iter()
    .map(|k| (*k).to_string())
    .collect();

    for re in direct_patterns {
        redacted = re.replace_all(&redacted, "[REDACTED_SECRET]").into_owned();
    }

    for re in assignment_patterns {
        redacted = re
            .replace_all(&redacted, "${1}[REDACTED_SECRET]")
            .into_owned();
    }

    // Also redact custom API key variable names configured at runtime.
    for source in [
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
    ] {
        let Some(key_name) = std::env::var(source)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        else {
            continue;
        };

        if !key_name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        if !redacted_keys.insert(key_name.clone()) {
            continue;
        }

        let pattern = format!(r"(?m)^\s*({}\s*=\s*)[^\r\n]+", regex::escape(&key_name));
        if let Ok(re) = Regex::new(&pattern) {
            redacted = re
                .replace_all(&redacted, "${1}[REDACTED_SECRET]")
                .into_owned();
        }
    }

    redacted
}

pub const GENERATED_IMAGE_TOOL_NAME: &str = "image_generation";
pub const GENERATED_IMAGE_MAX_AUTO_VISION_BYTES: u64 = 20 * 1024 * 1024;

pub fn generated_image_tool_input(
    path: &str,
    metadata_path: Option<&str>,
    output_format: &str,
    revised_prompt: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "path": path,
        "metadata_path": metadata_path,
        "output_format": output_format,
        "revised_prompt": revised_prompt,
    })
}

pub fn generated_image_summary(
    path: &str,
    metadata_path: Option<&str>,
    output_format: &str,
    revised_prompt: Option<&str>,
) -> String {
    let mut summary = format!("Generated image ({}) saved to `{}`.", output_format, path);
    if let Some(metadata_path) = metadata_path {
        summary.push_str(&format!("\nMetadata saved to `{}`.", metadata_path));
    }
    if let Some(revised_prompt) = revised_prompt.filter(|prompt| !prompt.trim().is_empty()) {
        summary.push_str("\n\nRevised prompt:\n");
        summary.push_str(revised_prompt.trim());
    }
    summary
}

pub fn generated_image_visual_context_blocks(
    path: &str,
    metadata_path: Option<&str>,
    output_format: &str,
    revised_prompt: Option<&str>,
) -> Option<Vec<ContentBlock>> {
    let path_ref = Path::new(path);
    let metadata = std::fs::metadata(path_ref).ok()?;
    if !metadata.is_file() || metadata.len() > GENERATED_IMAGE_MAX_AUTO_VISION_BYTES {
        return None;
    }

    let data = std::fs::read(path_ref).ok()?;
    let media_type = generated_image_media_type(path_ref, output_format).to_string();
    let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let mut reminder = format!(
        "<system-reminder>\nA provider-native image generation call created `{}`. Jcode attached the image pixels as visual context for future turns because the active provider supports image input and the file is under the safe {} MB limit.\nFormat: {}",
        path,
        GENERATED_IMAGE_MAX_AUTO_VISION_BYTES / 1024 / 1024,
        output_format,
    );
    if let Some(metadata_path) = metadata_path.filter(|value| !value.trim().is_empty()) {
        reminder.push_str(&format!("\nMetadata: {}", metadata_path));
    }
    if let Some(revised_prompt) = revised_prompt.filter(|value| !value.trim().is_empty()) {
        reminder.push_str("\nRevised prompt:\n");
        reminder.push_str(revised_prompt.trim());
    }
    reminder.push_str("\n</system-reminder>");

    Some(vec![
        ContentBlock::Text {
            text: reminder,
            cache_control: None,
        },
        ContentBlock::Image {
            media_type,
            data: data_b64,
        },
    ])
}

fn generated_image_media_type(path: &Path, output_format: &str) -> &'static str {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or(output_format)
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests;
