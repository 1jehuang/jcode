use super::*;

struct UsageOnlyNativeProvider {
    accounting_attributions: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for UsageOnlyNativeProvider {
    fn context_window(&self) -> usize {
        128_000
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        unreachable!("fallback uses simple completion")
    }

    fn name(&self) -> &str {
        "mock-native"
    }

    fn model(&self) -> String {
        // generate_compaction_artifact asks for model attribution only at its
        // accounting boundaries, once for each separately billed attempt.
        self.accounting_attributions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        "mock-model".to_string()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            accounting_attributions: self.accounting_attributions.clone(),
        })
    }

    async fn native_compact(
        &self,
        _messages: &[Message],
        _existing_summary_text: Option<&str>,
        _existing_openai_encrypted_content: Option<&str>,
    ) -> Result<jcode_provider_core::NativeCompactionResult> {
        Ok(jcode_provider_core::NativeCompactionResult {
            summary_text: None,
            openai_encrypted_content: None,
            usage: Some(jcode_provider_core::SimpleCompletionUsage {
                input_tokens: Some(120),
                output_tokens: Some(8),
                ..Default::default()
            }),
        })
    }

    async fn complete_simple_with_usage(
        &self,
        _prompt: &str,
        _system: &str,
    ) -> Result<(String, jcode_provider_core::SimpleCompletionUsage)> {
        assert_eq!(
            self.accounting_attributions
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "native attempt must be accounted before starting fallback"
        );
        Ok((
            "fallback summary".to_string(),
            jcode_provider_core::SimpleCompletionUsage {
                input_tokens: Some(20),
                output_tokens: Some(4),
                ..Default::default()
            },
        ))
    }
}

#[tokio::test]
async fn usage_only_native_compaction_accounts_attempt_then_falls_back() {
    let attributions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = Arc::new(UsageOnlyNativeProvider {
        accounting_attributions: attributions.clone(),
    });
    let result =
        generate_compaction_artifact(provider, vec![make_text_message(Role::User, "hello")], None)
            .await
            .unwrap();
    assert_eq!(result.summary_text, "fallback summary");
    assert!(result.openai_encrypted_content.is_none());
    assert_eq!(attributions.load(std::sync::atomic::Ordering::SeqCst), 2);
}
