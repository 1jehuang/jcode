use super::*;

pub(super) fn stored_messages_to_messages(messages: &[StoredMessage]) -> Vec<Message> {
    messages.iter().map(StoredMessage::to_message).collect()
}

pub(super) fn message_has_tool_image(content: &[ContentBlock]) -> bool {
    content
        .iter()
        .any(|block| matches!(block, ContentBlock::Image { .. }))
        && content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

pub(super) fn is_internal_system_reminder_message(message: &StoredMessage) -> bool {
    message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.trim_start()),
            _ => None,
        })
        .is_some_and(|text| text.starts_with("<system-reminder>"))
}

pub(super) fn is_visible_conversation_message(message: &StoredMessage) -> bool {
    message.display_role.is_none()
        && !is_internal_system_reminder_message(message)
        && !is_scheduled_task_message(message)
}

/// Recognize scheduler prompts persisted before they received an explicit
/// system display role. This keeps old sessions from treating them as user
/// prompts after resume.
pub fn is_scheduled_task_message(message: &StoredMessage) -> bool {
    message.role == Role::User
        && message.content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text, .. } if text.trim_start().starts_with("[Scheduled task]\n"))
        })
}

pub fn derive_session_provider_key(provider_name: &str) -> Option<String> {
    let normalized_name = provider_name.trim().to_ascii_lowercase();
    if normalized_name == "jcode" {
        return Some("jcode".to_string());
    }

    let runtime_provider = std::env::var("JCODE_RUNTIME_PROVIDER")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    // The shared server keeps one process env for every session, and a session
    // that switches to native Claude or OpenAI writes its pin there. That pin
    // must not relabel a session running another provider: its swarm workers
    // would inherit the wrong route (an OpenAI-compatible model sent to Claude).
    let name_family = match normalized_name.as_str() {
        "anthropic" | "claude" | "claude cli" => {
            Some(jcode_provider_core::DualAuthProvider::Anthropic)
        }
        "openai" => Some(jcode_provider_core::DualAuthProvider::OpenAI),
        _ => None,
    };
    let runtime_pin_is_foreign = jcode_provider_core::AuthRoute::parse(&runtime_provider)
        .is_some_and(|route| Some(route.provider) != name_family);

    if !runtime_pin_is_foreign
        && !runtime_provider.is_empty()
        && runtime_provider != "openai-compatible"
    {
        return Some(runtime_provider);
    }

    // OpenAI-compatible runtimes identify themselves through these; a native
    // runtime with a foreign pin goes straight to its own name.
    if !runtime_pin_is_foreign || normalized_name == "openrouter" {
        if let Ok(namespace) = std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE") {
            let namespace = namespace.trim().to_ascii_lowercase();
            if !namespace.is_empty() {
                return Some(namespace);
            }
        }

        if let Ok(active) = std::env::var("JCODE_ACTIVE_PROVIDER") {
            let active = active.trim().to_ascii_lowercase();
            if !active.is_empty() {
                return Some(active);
            }
        }
    }

    let fallback = match normalized_name.as_str() {
        "anthropic" | "claude" | "claude cli" => "claude",
        // Claude Code mode (`MultiProvider::name()` = "Claude Code") is its
        // own runtime, never native Claude.
        "claude code" | "claude-code" => "claude-code",
        "openai" => "openai",
        "github copilot" | "copilot" => "copilot",
        "openrouter" => "openrouter",
        "cursor" => "cursor",
        "gemini" => "gemini",
        "antigravity" => "antigravity",
        "" => return None,
        other => other,
    };

    Some(fallback.to_string())
}
