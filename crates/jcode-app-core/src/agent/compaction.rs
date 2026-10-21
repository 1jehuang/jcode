use super::*;

impl Agent {
    pub(super) fn note_compaction_applied(&mut self) {
        self.cache_tracker.reset();
        self.kv_cache_monitor.reset();
        self.locked_tools = None;
        self.provider_session_id = None;
        self.session.provider_session_id = None;
    }

    pub fn poll_compaction_completion_event(&mut self) -> Option<CompactionEvent> {
        let provider_messages = self.session.messages_for_provider();
        let compaction = self.registry.compaction();
        let event = match compaction.try_write() {
            Ok(mut manager) => {
                let event = manager.poll_compaction_event_with(&provider_messages);
                if event.is_some() {
                    self.sync_session_compaction_state_from_manager(&manager);
                }
                event
            }
            Err(_) => return None,
        };

        if event.is_some() {
            self.note_compaction_applied();
            self.persist_session_best_effort("compaction completion");
        }

        event
    }

    pub fn request_manual_compaction(&mut self) -> (String, bool) {
        if !self.provider.supports_compaction() {
            return (
                "Manual compaction is not available for this provider.".to_string(),
                false,
            );
        }

        let provider = self.provider.fork();
        let messages = self.session.messages_for_provider();
        let compaction = self.registry.compaction();

        match compaction.try_write() {
            Ok(mut manager) => {
                let stats = manager.stats_with(&messages);
                let status_msg = format!(
                    "**Context Status:**\n\
                    • Messages: {} (active), {} (total history)\n\
                    • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                    • Has summary: {}\n\
                    • Compacting: {}",
                    stats.active_messages,
                    stats.total_turns,
                    stats.effective_tokens / 1000,
                    stats.token_estimate / 1000,
                    manager.token_budget() / 1000,
                    stats.context_usage * 100.0,
                    if stats.has_summary { "yes" } else { "no" },
                    if stats.is_compacting {
                        "in progress..."
                    } else {
                        "no"
                    }
                );

                match manager.force_compact_with(&messages, provider) {
                    Ok(()) => (
                        format!(
                            "{}\n\n📦 **Compacting context** (manual) — summarizing older messages in the background to stay within the context window.\n\
                            The summary will be applied automatically when ready.",
                            status_msg
                        ),
                        true,
                    ),
                    Err(reason) => (
                        format!("{status_msg}\n\n⚠ **Cannot compact:** {reason}"),
                        false,
                    ),
                }
            }
            Err(_) => (
                "⚠ Cannot access compaction manager (lock held)".to_string(),
                false,
            ),
        }
    }

    fn is_context_limit_error(error: &str) -> bool {
        let lower = error.to_lowercase();
        lower.contains("context length")
            || lower.contains("context window")
            || lower.contains("maximum context")
            || lower.contains("max context")
            || lower.contains("token limit")
            || lower.contains("too many tokens")
            || lower.contains("prompt is too long")
            || lower.contains("input is too long")
            || lower.contains("request too large")
            || lower.contains("length limit")
            || lower.contains("maximum tokens")
            || (lower.contains("exceeded") && lower.contains("tokens"))
    }

    /// Best-effort emergency recovery after a context-limit error.
    ///
    /// Performs a synchronous hard compaction and resets provider session state,
    /// allowing the caller to retry the same turn immediately.
    pub(super) fn try_auto_compact_after_context_limit(&mut self, error: &str) -> bool {
        if crate::provider::openai_request::is_openai_encrypted_content_too_large_error(error)
            && self.try_recover_oversized_openai_native_compaction()
        {
            return true;
        }
        // A provider HTTP 413 ("request too large") is a *byte-size* failure
        // driven by inline base64 images, not a token-context overflow. Token
        // accounting deliberately undercounts images, so ordinary compaction
        // would not shrink the payload and the retry would 413 again. Strip
        // oversized images first.
        if self.try_recover_after_payload_too_large(error) {
            return true;
        }
        if self.try_recover_after_image_rejection(error) {
            return true;
        }
        if !Self::is_context_limit_error(error) {
            return false;
        }
        if !self.provider.supports_compaction() {
            return false;
        }

        let context_limit = self.provider.input_token_limit() as u64;
        let compaction = self.registry.compaction();

        let (dropped, usage_pct) = match compaction.try_write() {
            Ok(mut manager) => {
                let (dropped, usage_pct) = {
                    let all_messages = self.session.provider_messages();
                    manager.update_observed_input_tokens(context_limit);
                    let usage_pct = manager.context_usage_with(all_messages) * 100.0;
                    let dropped = match manager.hard_compact_with(all_messages) {
                        Ok(dropped) => dropped,
                        Err(reason) => {
                            logging::warn(&format!(
                                "Context-limit auto-recovery failed: hard compact failed ({})",
                                reason
                            ));
                            return false;
                        }
                    };
                    (dropped, usage_pct)
                };
                self.sync_session_compaction_state_from_manager(&manager);
                (dropped, usage_pct)
            }
            Err(_) => {
                logging::warn("Context-limit auto-recovery skipped: compaction manager lock busy");
                return false;
            }
        };

        self.cache_tracker.reset();
        self.kv_cache_monitor.reset();
        self.locked_tools = None;
        self.provider_session_id = None;
        self.session.provider_session_id = None;

        logging::warn(&format!(
            "Context limit exceeded; auto-compacted and retrying (dropped {} messages, usage was {:.1}%)",
            dropped, usage_pct
        ));
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "auto_compaction_applied",
                "context_limit_auto_compaction",
            )
            .with_session_id(self.session.id.clone())
            .with_detail(format!(
                "dropped_messages={dropped},usage_pct={usage_pct:.1}"
            ))
            .force_attribution(),
        );

        true
    }

    /// Best-effort recovery after a provider HTTP 413 "request too large" error.
    ///
    /// This failure is caused by the serialized request body (dominated by inline
    /// base64 images) exceeding the provider's size cap, which is independent of
    /// the token context window. We strip oversized images from the persisted
    /// transcript, oldest-first, down to a conservative byte budget and reset the
    /// provider session/cache so the caller can retry the same turn immediately.
    fn try_recover_after_payload_too_large(&mut self, error: &str) -> bool {
        if !crate::compaction::is_request_payload_too_large_error(error) {
            return false;
        }

        let stripped = self
            .session
            .strip_oversized_images(crate::compaction::PAYLOAD_IMAGE_CHAR_BUDGET);
        if stripped == 0 {
            logging::warn(
                "Request-too-large recovery skipped: no oversized inline images to strip",
            );
            return false;
        }
        self.reset_after_transcript_image_rewrite();

        logging::warn(&format!(
            "Request body exceeded provider size limit; stripped {} oversized inline image(s) and retrying",
            stripped
        ));
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "payload_too_large_recovered",
                "request_payload_too_large",
            )
            .with_session_id(self.session.id.clone())
            .with_detail(format!("images_stripped={stripped}"))
            .force_attribution(),
        );

        true
    }

    /// Repair only a uniquely identifiable malformed/unsupported image. Provider
    /// error indices refer to the encoded request, not necessarily stored history
    /// (compaction, memory injection and provider encoding can change positions).
    /// Never guess from those indices or remove otherwise valid images.
    fn try_recover_after_image_rejection(&mut self, error: &str) -> bool {
        use base64::Engine as _;

        if !crate::compaction::is_image_rejection_error(error) {
            return false;
        }
        let mut candidate = None;
        for (message_index, message) in self.session.messages.iter().enumerate() {
            for (block_index, block) in message.content.iter().enumerate() {
                let ContentBlock::Image { media_type, data } = block else {
                    continue;
                };
                let normalized = base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .map_err(|_| "invalid base64 image data".to_string())
                    .and_then(crate::image_normalize::normalize_image_bytes);
                let replacement = match normalized {
                    Ok(image)
                        if image.converted_from.is_none()
                            && image.media_type.eq_ignore_ascii_case(media_type) =>
                    {
                        continue;
                    }
                    Ok(image) => ContentBlock::Image {
                        media_type: image.media_type.into(),
                        data: base64::engine::general_purpose::STANDARD.encode(image.data),
                    },
                    Err(reason) => ContentBlock::Text {
                        text: format!("[Image omitted: {media_type}: {reason}]"),
                        cache_control: None,
                    },
                };
                if candidate.is_some() {
                    logging::warn("Image-rejection recovery skipped: ambiguous offending image");
                    return false;
                }
                candidate = Some((message_index, block_index, replacement));
            }
        }
        let Some((message_index, block_index, replacement)) = candidate else {
            logging::warn("Image-rejection recovery skipped: no identifiable invalid image");
            return false;
        };
        // Use the session mutation API to invalidate its provider-message cache.
        let mut messages = self.session.messages.clone();
        let content = &mut messages[message_index].content;
        // Keep parallel tool results contiguous when an unrepairable attachment
        // must become a note, matching outbound image-clamp behavior.
        if let ContentBlock::Text { text, .. } = &replacement
            && block_index > 0
            && let ContentBlock::ToolResult {
                content: result, ..
            } = &mut content[block_index - 1]
        {
            result.push('\n');
            result.push_str(text);
            content.remove(block_index);
            if let Some(ContentBlock::Text { text, .. }) = content.get(block_index)
                && text.starts_with("[Attached image associated with the preceding tool result: ")
            {
                let label = text.clone();
                content.remove(block_index);
                if let ContentBlock::ToolResult {
                    content: result, ..
                } = &mut content[block_index - 1]
                {
                    result.push('\n');
                    result.push_str(&label);
                }
            }
        } else {
            content[block_index] = replacement;
        }
        self.session.replace_messages(messages);
        self.reset_after_transcript_image_rewrite();
        if let Err(err) = self.session.save() {
            logging::warn(&format!(
                "Image-rejection recovery: failed to persist selective repair: {err}"
            ));
        }
        logging::warn(
            "Provider rejected an inline image; repaired one identified image and retrying",
        );
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "image_rejection_recovered",
                "provider_image_rejected",
            )
            .with_session_id(self.session.id.clone())
            .with_detail("images_repaired=1".to_string())
            .force_attribution(),
        );
        true
    }

    /// The transcript's images changed; reseed compaction bookkeeping and
    /// reset provider session/cache state so the retry sends the new payload.
    fn reset_after_transcript_image_rewrite(&mut self) {
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            let provider_messages = self.session.messages_for_provider();
            manager.reset();
            manager.set_budget(self.provider.input_token_limit());
            if let Some(state) = self.session.compaction.as_ref() {
                manager.restore_persisted_state_with(state, &provider_messages);
            } else {
                manager.seed_restored_messages_with(&provider_messages);
            }
            self.sync_session_compaction_state_from_manager(&manager);
        }

        self.cache_tracker.reset();
        self.kv_cache_monitor.reset();
        self.locked_tools = None;
        self.provider_session_id = None;
        self.session.provider_session_id = None;
    }

    fn try_recover_oversized_openai_native_compaction(&mut self) -> bool {
        let compaction = self.registry.compaction();
        let recovered = match compaction.try_write() {
            Ok(mut manager) => {
                if !manager.discard_oversized_openai_native_compaction() {
                    return false;
                }
                self.sync_session_compaction_state_from_manager(&manager);
                true
            }
            Err(_) => {
                logging::warn(
                    "OpenAI native compaction recovery skipped: compaction manager lock busy",
                );
                false
            }
        };

        if !recovered {
            return false;
        }

        self.cache_tracker.reset();
        self.kv_cache_monitor.reset();
        self.locked_tools = None;
        self.provider_session_id = None;
        self.session.provider_session_id = None;

        logging::warn(
            "OpenAI native compaction payload exceeded provider size limit; discarded native state and retrying with text fallback",
        );
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "native_compaction_payload_recovered",
                "openai_encrypted_content_too_large",
            )
            .with_session_id(self.session.id.clone())
            .force_attribution(),
        );

        true
    }

    pub(super) fn effective_context_tokens_from_usage(
        &self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> u64 {
        // Shared heuristic (jcode-compaction-core): keeps the compaction
        // manager's observed-token feed consistent with the client-side
        // context display.
        crate::compaction::effective_context_tokens_from_usage(
            self.provider.name(),
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        )
    }

    pub(super) fn update_compaction_usage_from_stream(
        &mut self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) {
        if !self.provider.uses_jcode_compaction() || input_tokens == 0 {
            return;
        }
        let observed = self.effective_context_tokens_from_usage(
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        );
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.update_observed_input_tokens(observed);
            manager.push_token_snapshot(observed);
        };
    }

    /// Push an embedding snapshot for the semantic compaction mode.
    /// Called after each assistant turn with a short text snippet.
    /// No-op if the embedding model is unavailable or mode is not semantic.
    pub(super) fn push_embedding_snapshot_if_semantic(&mut self, text: &str) {
        use crate::config::CompactionMode;
        let is_semantic = {
            let compaction = self.registry.compaction();
            compaction
                .try_read()
                .map(|m| m.mode() == CompactionMode::Semantic)
                .unwrap_or(false)
        };
        if !is_semantic {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.push_embedding_snapshot(text);
        };
    }
}

#[cfg(test)]
mod image_recovery_tests {
    use super::*;
    use async_trait::async_trait;
    use base64::Engine as _;

    const REJECTION: &str = "400 invalid_request_error: unsupported image media type";

    fn bmp_bytes() -> Vec<u8> {
        // A complete 1x1, 24-bit BMP with one padded pixel row.
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&58u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&1i32.to_le_bytes());
        bmp.extend_from_slice(&1i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(&[0, 0, 255, 0]);
        bmp
    }

    fn png_data() -> String {
        // Generate a truly decodable PNG rather than a signature-only fixture.
        let png = crate::image_normalize::normalize_image_bytes(bmp_bytes()).unwrap();
        assert_eq!(png.media_type, "image/png");
        assert_eq!(png.converted_from, Some("BMP"));
        base64::engine::general_purpose::STANDARD.encode(png.data)
    }

    #[derive(Clone, Default)]
    struct RecordingProvider {
        requests: Arc<StdMutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl Provider for RecordingProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages.to_vec());
            if requests.len() == 1 {
                anyhow::bail!(REJECTION);
            }
            assert_eq!(requests.len(), 2, "recovery must not loop");
            Ok(Box::pin(futures::stream::iter([
                Ok(StreamEvent::TextDelta("recovered".into())),
                Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".into()),
                }),
            ])))
        }

        fn name(&self) -> &str {
            "image-recovery-test"
        }

        fn supports_compaction(&self) -> bool {
            false
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(self.clone())
        }
    }

    fn image(media_type: &str, data: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: media_type.into(),
            data: data.into(),
        }
    }

    fn images(messages: &[Message]) -> Vec<(String, String)> {
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::Image { media_type, data } => {
                    Some((media_type.clone(), data.clone()))
                }
                _ => None,
            })
            .collect()
    }

    async fn exercise_recovery(
        rejected: Vec<ContentBlock>,
        expected: Option<usize>,
        streaming: bool,
    ) {
        let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let provider = Arc::new(RecordingProvider::default());
        let mut agent = Agent::new(provider.clone(), Registry::empty());
        agent.memory_enabled = false;
        agent.session.system_prompt = Some("Image recovery regression".into());
        // Separate older, valid history from the offending attachment. Seed the
        // provider cache and disk before entering the real public turn API.
        let png = png_data();
        agent.add_message(Role::User, vec![image("image/png", &png)]);
        agent.add_message(Role::User, rejected);
        let before = images(&agent.session.messages_for_provider());
        agent.session.save().unwrap();
        let id = agent.session.id.clone();
        agent.session = Session::load(&id).unwrap();
        let result = if streaming {
            let (tx, _rx) = mpsc::unbounded_channel();
            agent
                .run_once_streaming_mpsc("Continue", vec![], None, tx)
                .await
        } else {
            agent.run_once_capture("Continue").await.map(|_| ())
        };
        let requests = provider.requests.lock().unwrap();
        assert_eq!(images(&requests[0]), before);
        let mut reopened = Session::load(&id).unwrap();
        let persisted = images(&reopened.messages_for_provider());
        assert_eq!(persisted.first(), Some(&("image/png".into(), png)));
        if let Some(count) = expected {
            result.unwrap();
            assert_eq!(requests.len(), 2);
            let retried = images(&requests[1]);
            assert_eq!(retried.len(), count);
            assert_eq!(retried, persisted);
            assert!(
                retried
                    .iter()
                    .all(|(media_type, _)| media_type == "image/png")
            );
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported image")
            );
            assert_eq!(
                requests.len(),
                1,
                "ambiguous rejection must fail without retry"
            );
            assert_eq!(
                persisted, before,
                "ambiguous recovery must not rewrite history"
            );
        }
    }

    #[tokio::test]
    async fn public_recovery_preserves_earlier_png_in_retry_and_reopened_history() {
        for streaming in [false, true] {
            exercise_recovery(
                vec![image("image/bmp", "Qk0=")], // BMP signature, truncated data
                Some(1),
                streaming,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn public_recovery_repairs_decodable_bmp_without_discarding_it() {
        let data = base64::engine::general_purpose::STANDARD.encode(bmp_bytes());
        for streaming in [false, true] {
            exercise_recovery(vec![image("image/bmp", &data)], Some(2), streaming).await;
        }
    }

    #[tokio::test]
    async fn public_recovery_fails_safely_when_image_is_ambiguous() {
        for streaming in [false, true] {
            // No locally identifiable defect, despite a deterministic rejection.
            exercise_recovery(vec![image("image/png", &png_data())], None, streaming).await;
            // Multiple malformed attachments cannot identify one offender either.
            exercise_recovery(
                vec![
                    image("image/bmp", "Qk0="),
                    image("image/png", "not base64!"),
                ],
                None,
                streaming,
            )
            .await;
        }
    }
}
