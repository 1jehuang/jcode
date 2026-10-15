//! Explicit native OpenAI conversation compaction.

use super::*;

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
            openai_access_token(self.oauth_account_label(), &self.credentials).await?;
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
            })
            .ok_or_else(|| anyhow::anyhow!("OpenAI compact response missing compaction item"))?;

        if !jcode_base::provider::openai_request::openai_encrypted_content_is_sendable(
            &encrypted_content,
        ) {
            anyhow::bail!(
                "OpenAI compact response returned oversized encrypted_content ({} chars > safe limit {} chars)",
                encrypted_content.len(),
                jcode_base::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
            );
        }

        Ok(jcode_provider_core::NativeCompactionResult {
            summary_text: None,
            openai_encrypted_content: Some(encrypted_content),
        })
    }
}
