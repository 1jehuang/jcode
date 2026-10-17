//! Explicit native OpenAI conversation compaction.

use super::*;

fn compaction_usage(body: &Value) -> Option<jcode_provider_core::SimpleCompletionUsage> {
    let StreamEvent::TokenUsage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
    } = jcode_provider_openai::stream::extract_usage_from_response(body)?
    else {
        return None;
    };
    Some(jcode_provider_core::SimpleCompletionUsage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
    })
}

impl OpenAIProvider {
    pub(super) async fn native_compact_request(
        &self,
        messages: &[ChatMessage],
        existing_summary_text: Option<&str>,
        existing_openai_encrypted_content: Option<&str>,
    ) -> Result<jcode_provider_core::NativeCompactionResult> {
        if self.native_compaction_mode != OpenAINativeCompactionMode::Explicit {
            anyhow::bail!(
                "OpenAI native explicit compaction is disabled (mode={})",
                self.native_compaction_mode.as_str()
            );
        }

        let access_token =
            openai_access_token(self.refresh_account_label(), &self.credentials).await?;
        let creds = self.credentials.read().await;
        let is_chatgpt_mode = Self::is_chatgpt_mode(&creds);
        let account_id = creds.account_id.clone();
        let url = Self::responses_compact_url(&creds);
        drop(creds);

        let mut input = Vec::new();
        if let Some(encrypted_content) = existing_openai_encrypted_content {
            if !jcode_base::provider::openai_request::openai_encrypted_content_is_sendable(
                encrypted_content,
            ) {
                anyhow::bail!(
                    "OpenAI native compaction payload is too large to replay ({} chars > safe limit {} chars)",
                    encrypted_content.len(),
                    jcode_base::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
                );
            }
            input.push(serde_json::json!({
                "type": "compaction",
                "encrypted_content": encrypted_content,
            }));
        } else if let Some(summary_text) = existing_summary_text {
            input.push(serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": format!("## Previous Conversation Summary\n\n{}\n", summary_text),
                }]
            }));
        }
        input.extend(build_responses_input(messages));
        // The compact request declares no tools, so hosted-search history
        // goes as its text summary.
        native_web_search::downgrade_web_search_calls(&mut input);

        let mut builder = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json");

        if is_chatgpt_mode {
            builder = builder.header("originator", ORIGINATOR);
            if let Some(account_id) = account_id.as_ref() {
                builder = builder.header("chatgpt-account-id", account_id);
            }
        }

        let response = builder
            .json(&serde_json::json!({
                "model": self.model_id().await,
                "input": input,
                "store": false,
            }))
            .send()
            .await
            .context("Failed to send OpenAI compact request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = jcode_base::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!("OpenAI compact error {}: {}", status, body);
        }

        let body: Value = response
            .json()
            .await
            .context("Failed to parse OpenAI compact response")?;
        parse_compaction_response(&body)
    }
}

fn parse_compaction_response(body: &Value) -> Result<jcode_provider_core::NativeCompactionResult> {
    let encrypted_content = body
        .get("output")
        .and_then(|v| v.as_array())
        .and_then(|items| {
            items.iter().find_map(|item| {
                if item.get("type").and_then(|v| v.as_str()) == Some("compaction") {
                    item.get("encrypted_content")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
        });
    let usage = compaction_usage(body);
    if encrypted_content.is_none() && usage.is_none() {
        anyhow::bail!("OpenAI compact response missing compaction item and usage");
    }

    // The caller validates replay size after recording usage. A billed
    // response must not lose its counters just because its artifact is
    // too large and the caller needs to fall back to a text summary.
    Ok(jcode_provider_core::NativeCompactionResult {
        summary_text: None,
        openai_encrypted_content: encrypted_content,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_compaction_missing_artifact_preserves_observed_usage() {
        let result = parse_compaction_response(&serde_json::json!({
            "output": [{"type": "compaction", "encrypted_content": 123}],
            "usage": {"input_tokens": 120, "output_tokens": 8}
        }))
        .unwrap();
        assert_eq!(result.usage.unwrap().input_tokens, Some(120));
        assert!(result.openai_encrypted_content.is_none());
        assert!(result.summary_text.is_none());
        assert!(parse_compaction_response(&serde_json::json!({"output": []})).is_err());
    }

    #[test]
    fn native_compaction_returns_usage_even_for_oversized_artifacts() {
        let encrypted = "x".repeat(
            jcode_base::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS + 1,
        );
        let result = parse_compaction_response(&serde_json::json!({
            "output": [{"type": "compaction", "encrypted_content": encrypted}],
            "usage": {"input_tokens": 120, "output_tokens": 8}
        }))
        .unwrap();
        assert_eq!(result.usage.unwrap().input_tokens, Some(120));
        assert!(
            !jcode_base::provider::openai_request::openai_encrypted_content_is_sendable(
                result.openai_encrypted_content.as_deref().unwrap()
            )
        );
    }

    #[test]
    fn native_compaction_preserves_inclusive_input_and_sparse_usage() {
        let usage = compaction_usage(&serde_json::json!({"usage": {
            "input_tokens": 120, "output_tokens": 8,
            "input_tokens_details": {"cached_tokens": 100, "cache_write_tokens": 5}
        }}))
        .unwrap();
        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.cache_read_input_tokens, Some(100));
        assert_eq!(usage.cache_creation_input_tokens, Some(5));
        let sparse = compaction_usage(&serde_json::json!({"usage": {
            "input_tokens_details": {"cached_tokens": 0}
        }}))
        .unwrap();
        assert_eq!(sparse.input_tokens, None);
        assert_eq!(sparse.output_tokens, None);
        assert_eq!(sparse.cache_read_input_tokens, Some(0));
        assert_eq!(sparse.cache_creation_input_tokens, None);
        for body in [
            serde_json::json!({}),
            serde_json::json!({"usage": null}),
            serde_json::json!({"usage": {}}),
        ] {
            assert_eq!(compaction_usage(&body), None);
        }
    }
}
