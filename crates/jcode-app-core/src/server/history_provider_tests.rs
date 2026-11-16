use super::*;

fn session_with_provider_key(key: Option<&str>) -> crate::session::Session {
    let mut session = crate::session::Session::create_with_id(
        "test_history_provider_name".to_string(),
        None,
        None,
    );
    session.provider_key = key.map(str::to_string);
    session
}

#[test]
fn history_provider_name_prefers_persisted_openai_key() {
    let session = session_with_provider_key(Some("openai"));
    assert_eq!(
        history_provider_name_from_session(&session).as_deref(),
        Some("OpenAI")
    );
}

#[test]
fn history_provider_name_preserves_unknown_runtime_profile() {
    let session = session_with_provider_key(Some("remote-catalog"));
    assert_eq!(
        history_provider_name_from_session(&session).as_deref(),
        Some("remote-catalog")
    );
}

/// A direct OpenAI-compatible profile is persisted as its source key.
/// Clients must receive a display label, not the raw key: passing the key
/// through would make the spend ledger bill it to `openai:api-key` (#1286).
#[test]
fn history_provider_name_maps_a_compatible_profile_key_to_its_label() {
    let session = session_with_provider_key(Some("openai-compatible:deepseek"));
    assert_eq!(
        history_provider_name_from_session(&session).as_deref(),
        Some("DeepSeek")
    );

    let session = session_with_provider_key(Some("openai-compatible:nvidia-nim"));
    assert_eq!(
        history_provider_name_from_session(&session).as_deref(),
        Some("NVIDIA NIM")
    );
}

/// The same profile is also persisted bare (`deepseek`, the session
/// vocabulary). Both shapes must report the profile's display name instead
/// of the raw key (#1286).
#[test]
fn history_provider_name_maps_a_bare_compatible_profile_key() {
    for (key, label) in [
        ("deepseek", "DeepSeek"),
        ("openai-compatible", "OpenAI-compatible"),
        ("opencode-go", "OpenCode Go"),
        ("nvidia-nim", "NVIDIA NIM"),
    ] {
        let session = session_with_provider_key(Some(key));
        assert_eq!(
            history_provider_name_from_session(&session).as_deref(),
            Some(label),
            "provider key {key} must report its profile label"
        );
    }
}

/// Import-source codes live in the same field but are not profile routes:
/// `opencode` collides with the `OpenCode Zen` profile id, and a session
/// imported from that CLI must not be relabelled as the gateway.
#[test]
fn history_provider_name_keeps_import_source_codes_verbatim() {
    for key in ["opencode", "claude-code", "openai-codex"] {
        let session = session_with_provider_key(Some(key));
        assert_eq!(
            history_provider_name_from_session(&session).as_deref(),
            Some(key),
            "import source {key} must stay verbatim"
        );
    }
}

/// With no persisted key, the fallback must be the profile-aware label.
/// `Provider::name()` is the multiplexing slot (`openrouter`) that serves
/// every direct OpenAI-compatible profile, so it tagged DeepSeek sessions
/// as OpenRouter (#1286).
struct SlotOnlyProvider;

#[async_trait::async_trait]
impl Provider for SlotOnlyProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        Err(anyhow::anyhow!(
            "the history fallback test never sends a request"
        ))
    }

    fn name(&self) -> &str {
        "OpenRouter"
    }

    fn display_name(&self) -> String {
        "DeepSeek".to_string()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(SlotOnlyProvider)
    }
}

#[test]
fn history_provider_name_falls_back_to_the_profile_label() {
    let session = session_with_provider_key(None);
    assert_eq!(
        history_provider_name(&session, &SlotOnlyProvider).as_deref(),
        Some("DeepSeek")
    );
}
