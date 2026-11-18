//! Process-local opt-in for experimental voice replies. Never inherited by daemons.
use crate::protocol::{SessionToolConfig, SessionToolDefinition};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable before constructing the interactive client.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Construct the configured client-side speech backend. Explicit HTTP service
/// configuration fails closed rather than changing the user's selected voice.
pub fn create_speech_player()
-> Result<jcode_voice::speech::SpeechPlayer, jcode_voice::speech::SpeechError> {
    let config = &crate::config::config().jarvis.speech;
    match config.endpoint.as_deref() {
        Some(endpoint) => {
            let key = config
                .api_key_env
                .as_ref()
                .map(|name| {
                    std::env::var(name)
                        .map_err(|_| jcode_voice::speech::SpeechError::InvalidConfiguration)
                })
                .transpose()?;
            jcode_voice::speech::SpeechPlayer::with_http(
                endpoint,
                &config.model,
                &config.voice,
                key,
            )
        }
        None => jcode_voice::speech::SpeechPlayer::new(),
    }
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub(crate) const REMINDER: &str = "Jarvis voice mode is active. Use the speak tool for concise conversational replies and important progress updates. Keep code, logs, and detailed explanations on screen, not in speech. Do not duplicate spoken replies in ordinary assistant text unless adding useful visual detail. Queued speech does not mean the user heard it. Normal approval and safety rules still apply.";

pub(crate) fn with_speech(mut config: SessionToolConfig) -> SessionToolConfig {
    config.custom.retain(|tool| tool.name != "speak");
    config.custom.push(SessionToolDefinition {
        name: "speak".into(),
        description: "Speak a short plain-text reply aloud on the user's local Jarvis client. Use for conversational answers and important updates, not code or logs. Returns queued, not confirmation of playback. The text is also retained in the conversation. Can be interrupted by the user.".into(),
        parameters: json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "text": {"type": "string", "minLength": 1, "maxLength": 2000, "description": "Short plain-text utterance, at most 4096 UTF-8 bytes."},
                "intent": {"type": "string"}
            },
            "required": ["text", "intent"]
        }),
    });
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_configuration_preserves_other_tools_and_restrictions() {
        let original = SessionToolConfig {
            enabled: Some(vec!["bash".into(), "speak".into()]),
            disabled: vec!["edit".into()],
            custom: vec![SessionToolDefinition {
                name: "cloud_environment".into(),
                description: "resource bridge".into(),
                parameters: json!({"type":"object"}),
            }],
        };
        let result = with_speech(with_speech(original));
        assert_eq!(result.custom.len(), 2);
        assert_eq!(result.custom[0].name, "cloud_environment");
        assert_eq!(result.custom[1].name, "speak");
        assert_eq!(result.disabled, ["edit"]);
        assert_eq!(result.enabled.unwrap(), ["bash", "speak"]);
    }
}
