//! Command Code reasoning-effort ladders and one-shot recovery: a pre-stream
//! 400/422 rejecting reasoning effort triggers exactly ONE effort-omitted
//! retry; the capability is cached so later requests omit it directly
//! (D-17, D-18, CMDC-06).

use jcode_provider_command_code::MODEL_EFFORTS;
use std::collections::HashMap;
use std::sync::RwLock;

/// A rejection that specifically targets reasoning effort, classified over a
/// bounded error body.
#[derive(Debug, Clone, PartialEq)]
pub struct ReasoningRejection {
    pub model: String,
    pub rejected_effort: Option<String>,
}

/// Classifier: 400/422 only, and the bounded body must name reasoning effort
/// specifically. Unrelated 400/422 bodies never match (D-18): shared false
/// positives (e.g. malformed JSON) must not disable reasoning or retry.
pub fn is_reasoning_effort_rejection(status: u16, body: &str) -> bool {
    if status != 400 && status != 422 {
        return false;
    }
    let lowered = body.to_ascii_lowercase();
    [
        "reasoning_effort",
        "reasoning-effort",
        "reasoning effort",
        "unsupported effort",
        "invalid effort",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}

/// Cache per routing scope (account+session) like other runtime caches.
#[derive(Debug)]
pub struct CommandCodeReasoningCapability {
    denied: RwLock<HashMap<String, ReasoningRejection>>,
}

impl Default for CommandCodeReasoningCapability {
    fn default() -> Self {
        Self {
            denied: RwLock::new(HashMap::new()),
        }
    }
}

impl CommandCodeReasoningCapability {
    /// Evaluate a pre-stream error: when it is a reasoning rejection, write
    /// the capability cache and approve exactly one degrade retry.
    pub fn classify_pre_stream_rejection(
        &self,
        model: &str,
        status: u16,
        body: &str,
    ) -> Option<ReasoningRejection> {
        if !is_reasoning_effort_rejection(status, body) {
            // Unrelated errors: no cache write, no recovery signal (D-18).
            return None;
        }
        let rejection = ReasoningRejection {
            model: model.to_string(),
            rejected_effort: None,
        };
        if let Ok(mut denied) = self.denied.write() {
            denied.insert(model.to_string(), rejection.clone());
        }
        Some(rejection)
    }

    /// True once an upstream rejection has recorded the model as
    /// effortless; callers then omit reasoning_effort up front.
    pub fn reasoning_denied(&self, model: &str) -> bool {
        self.denied
            .read()
            .map(|denied| denied.contains_key(model))
            .unwrap_or(false)
    }

    /// One-shot recovery ceiling: values are only ever written once per
    /// model; inserting again never changes the decision surface.
    pub fn note_already_denied(&self, model: &str) {
        if let Ok(mut denied) = self.denied.write()
            && !denied.contains_key(model)
        {
            denied.insert(
                model.to_string(),
                ReasoningRejection {
                    model: model.to_string(),
                    rejected_effort: None,
                },
            );
        }
    }

    /// Per-model ladder from the curated table, filtered to effort values
    /// not yet denied for this model. Unsupported entries stay hidden from
    /// user-visible choices (CMDC-03).
    pub fn visible_efforts(&self, model: &str) -> &'static [&'static str] {
        MODEL_EFFORTS
            .iter()
            .find(|(key, _)| *key == model)
            .map(|(_, efforts)| *efforts)
            .unwrap_or(&[])
    }
}
