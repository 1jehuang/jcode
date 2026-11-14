use super::*;
use crate::message::{ContentBlock, Message, Role, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use async_trait::async_trait;

#[test]
fn context_limit_error_recognizes_ntl_and_canonical_markers() {
    for error in [
        r#"HTTP 400: {"error":"The chat context is too large. Start a new chat or summarize the current one.","code":"prompt_too_large"}"#,
        "The chat context is too large. Start a new chat or summarize the current one.",
        "prompt_too_large",
        "context_length_exceeded",
        "CONTEXT_LENGTH_EXCEEDED",
        "Context length exceeded: The chat context is too large.",
        "This model's maximum context length is 200000 tokens",
    ] {
        assert!(Agent::is_context_limit_error(error), "{error}");
    }
}

#[test]
fn context_limit_error_keeps_unrelated_size_failures_separate() {
    for error in [
        "HTTP 413: Payload Too Large",
        r#"HTTP 413: {"error":{"type":"request_too_large","message":"Request exceeds the maximum size"}}"#,
        "HTTP 400: image is too large",
        "HTTP 400: invalid image data",
        "HTTP 400: message exceeds the 50,000 character limit",
        "HTTP 400: invalid_request_error",
        "rate limit exceeded, retry after 20s",
    ] {
        assert!(!Agent::is_context_limit_error(error), "{error}");
    }
}

struct ContextLimitProvider;

#[async_trait]
impl Provider for ContextLimitProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        panic!("context-limit recovery must not call the upstream provider")
    }

    fn name(&self) -> &str {
        "context-limit-test"
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        200_000
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[tokio::test]
async fn ntl_context_limit_recovery_preserves_durable_transcript_and_resets_resume_state() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::config::Config::invalidate_cache();

    for error in [
        r#"HTTP 400: {"error":"The chat context is too large. Start a new chat or summarize the current one.","code":"prompt_too_large"}"#,
        "The chat context is too large. Start a new chat or summarize the current one.",
        "prompt_too_large",
        "context_length_exceeded",
    ] {
        let mut agent = Agent::new(Arc::new(ContextLimitProvider), Registry::empty());
        agent.session.system_prompt = Some("Keep every original caller instruction.".to_string());
        for index in 0..16 {
            agent.add_message(
                if index % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                vec![ContentBlock::Text {
                    text: format!("Original caller history {index}: preserve this text verbatim."),
                    cache_control: None,
                }],
            );
        }
        agent.provider_session_id = Some("stale-runtime-resume".to_string());
        agent.session.provider_session_id = Some("stale-stored-resume".to_string());
        agent.locked_tools = Some(Vec::new());
        let original_messages = serde_json::to_value(&agent.session.messages).unwrap();
        let original_system = agent.session.system_prompt.clone();

        assert!(agent.try_auto_compact_after_context_limit(error), "{error}");
        assert!(agent.provider_session_id.is_none());
        assert!(agent.session.provider_session_id.is_none());
        assert!(agent.locked_tools.is_none());
        assert_eq!(
            serde_json::to_value(&agent.session.messages).unwrap(),
            original_messages,
            "compaction must retain the original transcript"
        );
        assert_eq!(agent.session.system_prompt, original_system);
        let state = agent.session.compaction.clone().expect("compaction state");
        assert!(state.compacted_count > 0);
        assert!(state.compacted_count < agent.session.messages.len());
        assert!(!state.summary_text.is_empty());

        // Recovery persists a summary/cutoff, not a rewritten original transcript.
        let restored = Session::load(&agent.session.id).expect("persisted recovery");
        assert_eq!(
            serde_json::to_value(&restored.messages).unwrap(),
            original_messages
        );
        assert_eq!(restored.system_prompt, original_system);
        assert_eq!(restored.compaction, agent.session.compaction);

        let compaction = agent.registry.compaction();
        let mut manager = compaction.try_write().unwrap();
        let active = manager.messages_for_api_with(agent.session.provider_messages());
        assert!(active.len() < agent.session.messages.len());
        assert!(active[0].content.iter().any(|block| {
            matches!(block, ContentBlock::Text { text, .. } if text.contains(&state.summary_text))
        }));
    }
}
