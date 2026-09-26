//! Runtime image-input capability overrides.
//!
//! Static capability knowledge (`Provider::supports_image_input`) can be wrong
//! for direct OpenAI-compatible endpoints: the default there is optimistic
//! (any `/v1/chat/completions` server "might" accept `image_url` parts), and a
//! text-only model behind such an endpoint advertises image support it does not
//! have. When that happens the request fails with a modality error only after
//! the image is already in the request, and every later request repeats the
//! failure because the stale `true` answer keeps the images flowing.
//!
//! This module records per `(runtime-scoped provider key, model)` that the
//! endpoint *actually* rejected image input, with a TTL. The key is the
//! concrete runtime identity (e.g. `openrouter::profile:<id>`) where the
//! provider slot multiplexes several endpoints, so a rejection on one endpoint
//! never suppresses images for another endpoint serving the same model name.
//! `MultiProvider::supports_image_input` consults the override first, so:
//!
//! - `complete_with_failover` replaces image blocks with text markers for the
//!   very next request (the existing #755 filter), and
//! - turn loops stop attaching new tool-result/generated images while the
//!   override is fresh.
//!
//! The override is scoped to the model: switching to a vision-capable model on
//! the same endpoint clears the penalty (and a manual `/model` switch clears
//! every entry for the runtime anyway via `clear_image_input_overrides_for_provider`).
//!
//! Unlike `ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS`, these entries never steer
//! provider failover: the endpoint is healthy, only the image modality is
//! unsupported, so we degrade the request instead of switching providers.
//!
//! The OpenAI-compatible runtime surfaces HTTP errors asynchronously inside the
//! response stream (the stream handle is returned before the request is sent),
//! so the streaming retry loop also consults this module: when a request that
//! carried `image_url` parts fails with a modality error, the retry loop can
//! rewrite the request JSON (`filter_image_url_parts_from_request`) and replay
//! the same request once instead of failing the turn.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// How long a recorded "this model rejected images" observation stays
/// authoritative. Long enough to cover a long session, short enough that a
/// server-side upgrade (or a wrongly classified error) heals itself.
const IMAGE_INPUT_OVERRIDE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone)]
struct ImageInputRejection {
    recorded_at: Instant,
    #[allow(dead_code)] // kept for parity with other runtime records; useful in future dumps
    observed_at: SystemTime,
    #[allow(dead_code)] // debug aid when inspecting why a request was filtered
    matched: String,
}

static IMAGE_INPUT_REJECTIONS: LazyLock<RwLock<HashMap<String, ImageInputRejection>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Error fragments that mean "this model/endpoint cannot accept image input".
///
/// Kept deliberately narrow: generic schema errors (e.g. a missing tool field)
/// must not be treated as a modality problem. All needles are matched
/// case-insensitively against the full error text.
const IMAGE_REJECTION_NEEDLES: &[&str] = &[
    // Anthropic-style
    "does not support image",
    "images are not supported",
    "image input is not supported",
    "unsupported content type: image",
    "does not accept image",
    "cannot process images",
    "no image support",
    // OpenAI / OpenAI-compatible style
    "does not support image input",
    "image input not supported",
    "not support image_url",
    "invalid content type: image_url",
    "unsupported content type: image_url",
    "image_url is not supported",
    "image content is not supported",
    "not multimodal",
    "is not multimodal",
    "does not support multimodal",
    "text-only model",
    "only supports text",
    "only supports text input",
    "image is not a supported modality",
    "modality not supported",
    "unsupported modality",
    "input modalities",
];

/// True when `error_text` looks like the endpoint rejected image input.
pub fn error_indicates_image_rejection(error_text: &str) -> bool {
    let lower = error_text.to_ascii_lowercase();
    IMAGE_REJECTION_NEEDLES
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Record that `model` on `provider_key` rejected image input.
///
/// `provider_key` must already be runtime-scoped, because the same provider
/// slot can serve different accounts with different capabilities: pass the
/// concrete profile/endpoint identity (e.g. `openrouter::profile:<id>` or
/// `openrouter::endpoint:<url>`), not the bare slot key, when the caller knows
/// it. Two OpenAI-compatible profiles sharing a model name must not share a
/// rejection record: a text-only endpoint's rejection would otherwise make a
/// vision-capable endpoint omit valid images for the TTL window. Pass `None`
/// only when the caller genuinely cannot determine a concrete endpoint
/// identity; the record is then kept under the un-scoped key so lookups that
/// use the same key shape still find it.
pub fn record_image_input_rejection(provider_key: Option<&str>, model: &str, error_text: &str) {
    // Keys are normalized (trim + lowercase) so every caller agrees even when
    // they hold different spellings of the same model id (raw runtime string,
    // stripped/lowercased lookups, session-restored forms).
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() {
        return;
    }
    let Some(provider_key) = provider_key.map(str::trim).filter(|k| !k.is_empty()) else {
        return;
    };
    let matched = IMAGE_REJECTION_NEEDLES
        .iter()
        .find(|needle| error_text.to_ascii_lowercase().contains(*needle))
        .map(|needle| needle.to_string())
        .unwrap_or_else(|| "unclassified".to_string());
    let entry_key = format!("{provider_key}::{model}");
    if let Ok(mut rejections) = IMAGE_INPUT_REJECTIONS.write() {
        rejections.insert(
            entry_key,
            ImageInputRejection {
                recorded_at: Instant::now(),
                observed_at: SystemTime::now(),
                matched: matched.clone(),
            },
        );
    }
    jcode_logging::info(&format!(
        "Recorded image-input rejection for {provider_key}/{model} (matched: {}). \
         Outbound images will be filtered to text markers until the model changes.",
        matched
    ));
}

/// True when a fresh "rejected image input" record exists for `model` on
/// `provider_key`. Expired entries are dropped on read.
pub fn image_input_rejected(provider_key: &str, model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    if model.is_empty() || provider_key.trim().is_empty() {
        return false;
    }
    let entry_key = format!("{provider_key}::{model}");
    let Ok(mut rejections) = IMAGE_INPUT_REJECTIONS.write() else {
        return false;
    };
    match rejections.get(&entry_key) {
        Some(entry) if entry.recorded_at.elapsed() <= IMAGE_INPUT_OVERRIDE_TTL => true,
        Some(_) => {
            rejections.remove(&entry_key);
            false
        }
        None => false,
    }
}

/// Drop every override for `provider_key` (called when the model is switched
/// on that runtime, so a fresh model gets a fresh capability answer).
pub fn clear_image_input_overrides_for_provider(provider_key: &str) {
    let prefix = format!("{provider_key}::");
    if let Ok(mut rejections) = IMAGE_INPUT_REJECTIONS.write() {
        rejections.retain(|key, _| !key.starts_with(&prefix));
    }
}

/// True when a serialized chat-completions request body contains `image_url`
/// content parts (the OpenAI wire shape for user-supplied images).
pub fn request_contains_image_url_parts(request: &serde_json::Value) -> bool {
    request
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(|t| t.as_str()) == Some("image_url")
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Replace every `image_url` content part in a serialized chat-completions
/// request body with a text marker describing the omitted image. Returns a
/// new request value; the input is untouched. Non-array message content
/// (plain strings) carries no images, so it is preserved as-is.
pub fn filter_image_url_parts_from_request(request: &serde_json::Value) -> serde_json::Value {
    let mut filtered = request.clone();
    let Some(messages) = filtered.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return filtered;
    };
    for message in messages.iter_mut() {
        let Some(parts) = message.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            if part.get("type").and_then(|t| t.as_str()) != Some("image_url") {
                continue;
            }
            // Keep the media type in the marker so the model still knows what
            // kind of image was omitted; the pixel data itself is dropped.
            let media_type = part
                .get("image_url")
                .and_then(|iu| iu.get("url"))
                .and_then(|u| u.as_str())
                .and_then(|url| url.strip_prefix("data:"))
                .and_then(|rest| rest.split(';').next())
                .unwrap_or("unknown");
            *part = serde_json::json!({
                "type": "text",
                "text": format!(
                    "[Image omitted: this model does not support image input; media_type={media_type}]"
                )
            });
        }
    }
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn unique_provider() -> String {
        format!(
            "test-provider-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn modality_errors_are_classified() {
        assert!(error_indicates_image_rejection(
            "Error: model does not support image input"
        ));
        assert!(error_indicates_image_rejection(
            "400: this is a text-only model"
        ));
        assert!(error_indicates_image_rejection(
            "Request failed: image_url is not supported by this model"
        ));
        assert!(error_indicates_image_rejection(
            "Invalid request: unsupported modality: image"
        ));
        assert!(error_indicates_image_rejection(
            "model is not multimodal, cannot accept image parts"
        ));
        assert!(error_indicates_image_rejection(
            "model does not support multimodal requests"
        ));
    }

    #[test]
    fn unrelated_errors_are_not_classified() {
        assert!(!error_indicates_image_rejection("rate limit exceeded"));
        assert!(!error_indicates_image_rejection(
            "tool_use ids were found without tool_result"
        ));
        assert!(!error_indicates_image_rejection(
            "max_tokens must be greater than 0"
        ));
        // Positive-sounding multimodal text must not flip a request to the
        // text-only path.
        assert!(!error_indicates_image_rejection(
            "gpt-4o is multimodal and can see images"
        ));
        assert!(!error_indicates_image_rejection(""));
    }

    #[test]
    fn rejection_roundtrip_and_clear() {
        let provider = unique_provider();
        assert!(!image_input_rejected(&provider, "text-model"));

        record_image_input_rejection(
            Some(&provider),
            "text-model",
            "does not support image input",
        );
        assert!(image_input_rejected(&provider, "text-model"));
        // Scoped per model.
        assert!(!image_input_rejected(&provider, "other-model"));

        // Mixed-case spellings of the same model id hit the same entry:
        // the record side may hold the raw runtime string while lookups
        // strip/lowercase (or vice versa).
        record_image_input_rejection(
            Some(&provider),
            "Vision-Model-X",
            "does not support image input",
        );
        assert!(image_input_rejected(&provider, "vision-model-x"));
        assert!(image_input_rejected(&provider, "  VISION-MODEL-X  "));

        clear_image_input_overrides_for_provider(&provider);
        assert!(!image_input_rejected(&provider, "text-model"));
        assert!(!image_input_rejected(&provider, "vision-model-x"));
    }

    #[test]
    fn empty_inputs_are_ignored() {
        let provider = unique_provider();
        record_image_input_rejection(Some(&provider), "  ", "does not support image input");
        record_image_input_rejection(None, "model", "does not support image input");
        record_image_input_rejection(Some(""), "model", "does not support image input");
        assert!(!image_input_rejected(&provider, "  "));
        assert!(!image_input_rejected("", "model"));
    }

    #[test]
    fn request_image_url_detection_and_filtering() {
        let request = json!({
            "model": "text-only-test-model",
            "messages": [
                {"role": "system", "content": "you are helpful"},
                {"role": "user", "content": [
                    {"type": "text", "text": "look at this"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,Zm9v"}}
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": "plain string content"}
            ]
        });
        assert!(request_contains_image_url_parts(&request));

        let filtered = filter_image_url_parts_from_request(&request);
        assert!(!request_contains_image_url_parts(&filtered));
        // Text parts preserved.
        let user_parts = filtered["messages"][1]["content"].as_array().unwrap();
        assert_eq!(user_parts.len(), 2);
        assert_eq!(user_parts[0]["type"], "text");
        assert_eq!(user_parts[0]["text"], "look at this");
        assert_eq!(user_parts[1]["type"], "text");
        let marker = user_parts[1]["text"].as_str().unwrap();
        assert!(marker.contains("[Image omitted:"));
        assert!(marker.contains("media_type=image/png"));
        // String-content messages untouched.
        assert_eq!(
            filtered["messages"][2]["content"],
            json!("plain string content")
        );
        // Original request not mutated.
        assert!(request_contains_image_url_parts(&request));
    }

    #[test]
    fn request_without_images_is_untouched() {
        let request = json!({
            "model": "text-only-test-model",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello"}]},
                {"role": "user", "content": "plain string"}
            ]
        });
        assert!(!request_contains_image_url_parts(&request));
        let filtered = filter_image_url_parts_from_request(&request);
        assert_eq!(filtered, request);
    }
}
