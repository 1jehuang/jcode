//! First-run guidance printed after configuring a local OpenAI-compatible
//! endpoint.
//!
//! Extracted from `login.rs` so the copy is testable and the already-oversized
//! parent file keeps shrinking rather than growing.

/// The "what do I run now" line for a provider that needs no API key.
///
/// Ollama and LM Studio both require a separate model-loading step, so a generic
/// hint sends people to a run command that fails with an unknown model.
pub(super) fn local_endpoint_hint(provider_id: &str) -> String {
    match provider_id {
        "ollama" => "Next step: install a model with `ollama pull llama3.2`, then run \
             `jcode --provider ollama --model llama3.2 run 'hello'`."
            .to_string(),
        "lmstudio" => "Next step: load a chat model in LM Studio's Local Server, then run jcode \
             with that exact model id, for example \
             `jcode --provider lmstudio --model <model-id> run 'hello'`."
            .to_string(),
        other => format!(
            "Next step: run jcode with a model available on this endpoint, for example \
             `jcode --provider {} --model <model-id> run 'hello'`.",
            other
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_and_lmstudio_get_their_own_model_loading_step() {
        let ollama = local_endpoint_hint("ollama");
        assert!(ollama.contains("ollama pull"), "{ollama}");
        let lmstudio = local_endpoint_hint("lmstudio");
        assert!(lmstudio.contains("Local Server"), "{lmstudio}");
    }

    // Verify the extracted next-step hints are byte-identical to what login.rs
    // printed before extraction. A `\` line continuation swallows the newline *and*
    // the following indentation, so a mis-indented continuation silently changes
    // user-facing text while still compiling.
    #[test]
    fn extracted_hints_match_the_strings_login_printed_before_extraction() {
        assert_eq!(
            local_endpoint_hint("ollama"),
            "Next step: install a model with `ollama pull llama3.2`, then run `jcode --provider ollama --model llama3.2 run 'hello'`."
        );
        assert_eq!(
            local_endpoint_hint("lmstudio"),
            "Next step: load a chat model in LM Studio's Local Server, then run jcode with that exact model id, for example `jcode --provider lmstudio --model <model-id> run 'hello'`."
        );
        assert_eq!(
            local_endpoint_hint("ollama-turbo"),
            "Next step: run jcode with a model available on this endpoint, for example `jcode --provider ollama-turbo --model <model-id> run 'hello'`."
        );
    }

    #[test]
    fn unknown_providers_get_a_generic_hint_naming_themselves() {
        let hint = local_endpoint_hint("my-endpoint");
        assert!(hint.contains("--provider my-endpoint"), "{hint}");
        // A generic hint must not send people to another vendor's CLI.
        assert!(!hint.contains("ollama pull"), "{hint}");
    }
}
