use super::*;

/// Only derived, reproducible allocations are included here. Canonical
/// transcript, frozen prompts/tools, rewind undo, provider and hook state stay
/// resident. Reloading a Session is not a transparent wake operation.
#[derive(Debug)]
pub(crate) struct IdleCacheRelease {
    pub provider_cache_bytes: usize,
    pub tool_index_entries: usize,
}

impl Agent {
    /// Caller must hold the idle Agent reservation. Never checkpoint here:
    /// save_prepared can succeed without writing under a migration lease.
    /// Keeping the source history means waking is simply lazy cache rebuilding,
    /// with no disk dependency, lifecycle hooks or environment resnapshotting.
    pub(crate) fn hibernate_idle_caches(&mut self) -> Option<IdleCacheRelease> {
        if crate::turn_cancel_registry::has_active_turn(self.session_id())
            || self.session.migration_lease_block().is_some()
            || !self.pending_alerts.is_empty()
        {
            return None;
        }
        let queue = self.soft_interrupt_queue.try_lock().ok()?;
        if !queue.is_empty() {
            return None;
        }
        let compaction = self.registry.compaction();
        let manager = compaction.try_read().ok()?;
        if manager.is_compacting() {
            return None;
        }
        // A backgrounded tool can outlive the Agent lock. Do not disturb even
        // derived state until every tool belonging to this history has settled.
        if self.session.messages.iter().any(|message| {
            message.content.iter().any(|block| match block {
                ContentBlock::ToolUse { id, .. } => crate::tool::inflight::is_tool_in_flight(id),
                _ => false,
            })
        }) {
            return None;
        }

        let released = IdleCacheRelease {
            provider_cache_bytes: self
                .session
                .memory_profile_snapshot()
                .provider_cache_json_bytes,
            tool_index_entries: self.tool_call_ids.len() + self.tool_result_ids.len(),
        };
        self.session.release_provider_messages_cache();
        self.tool_call_ids = HashSet::new();
        self.tool_result_ids = HashSet::new();
        self.tool_output_scan_index = 0;
        Some(released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHome(Option<std::ffi::OsString>, tempfile::TempDir);
    impl TestHome {
        fn new() -> Self {
            let previous = std::env::var_os("JCODE_HOME");
            let home = tempfile::tempdir().unwrap();
            crate::env::set_var("JCODE_HOME", home.path());
            Self(previous, home)
        }
    }
    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    struct TestProvider;
    #[async_trait::async_trait]
    impl Provider for TestProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            anyhow::bail!("cache hibernation must not invoke the provider")
        }
        fn name(&self) -> &str {
            "hibernation-test"
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }

    fn agent() -> Agent {
        Agent::build_base(
            Arc::new(TestProvider),
            Registry::empty(),
            Session::create_with_id(id::new_id("session_idle_test"), None, None),
            None,
            HashSet::new(),
        )
    }

    fn paired_tool_history(agent: &mut Agent, tool_id: &str, payload: String) {
        agent.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: tool_id.into(),
                name: "read".into(),
                input: serde_json::json!({"file_path": "test.txt"}),
                thought_signature: None,
            }],
        );
        agent.add_message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_id.into(),
                content: payload,
                is_error: None,
            }],
        );
    }

    #[test]
    fn idle_hibernation_releases_cache_and_rebuilds_exact_history_and_indexes() {
        let _env = crate::storage::lock_test_env();
        let mut agent = agent();
        paired_tool_history(&mut agent, "tool_old", "large result ".repeat(100_000));
        assert_eq!(agent.repair_missing_tool_outputs(), 0);
        let expected = serde_json::to_value(&agent.session).unwrap();
        let provider_messages = agent.session.messages_for_provider().to_vec();
        let frozen_tools = vec![ToolDefinition {
            name: "frozen".into(),
            description: "keep original tool surface".into(),
            input_schema: serde_json::json!({"type": "object"}),
            defer_loading: false,
        }];
        agent.locked_tools = Some(frozen_tools);
        agent.agents_md_snapshot.0 = Some("Frozen instructions, not recaptured on wake".into());
        agent.provider_session_id = Some("upstream-live-session".into());
        agent.rewind_undo_snapshot = Some(RewindUndoSnapshot {
            messages: agent.session.messages.clone(),
            provider_session_id: Some("undo-provider".into()),
            session_provider_session_id: None,
            visible_message_count: 2,
        });
        let released = agent.hibernate_idle_caches().unwrap();
        assert!(released.provider_cache_bytes > 1_000_000);
        assert_eq!(released.tool_index_entries, 2);
        assert_eq!(
            agent
                .session
                .memory_profile_snapshot()
                .provider_cache_message_count,
            0
        );
        assert_eq!(agent.tool_call_ids.capacity(), 0);
        assert_eq!(agent.tool_result_ids.capacity(), 0);
        assert_eq!(serde_json::to_value(&agent.session).unwrap(), expected);
        assert_eq!(
            agent.provider_session_id.as_deref(),
            Some("upstream-live-session")
        );
        assert_eq!(agent.locked_tools.as_ref().unwrap()[0].name, "frozen");
        assert_eq!(
            agent.agents_md_snapshot.0.as_deref(),
            Some("Frozen instructions, not recaptured on wake")
        );
        assert_eq!(
            agent.rewind_undo_snapshot.as_ref().unwrap().messages.len(),
            2
        );
        assert_eq!(
            serde_json::to_value(agent.session.messages_for_provider()).unwrap(),
            serde_json::to_value(provider_messages).unwrap()
        );
        // The next turn's repair scan rebuilds from retained history. Previously
        // paired results must not gain synthetic duplicates after eviction.
        paired_tool_history(&mut agent, "tool_new", "next result".into());
        assert_eq!(agent.repair_missing_tool_outputs(), 0);
        assert_eq!(agent.session.messages.len(), 4);
        assert!(agent.tool_result_ids.contains("tool_old"));
        assert!(agent.tool_result_ids.contains("tool_new"));
        assert_eq!(agent.tool_output_scan_index, 4);
    }

    #[test]
    fn idle_hibernation_refuses_queued_alerts_busy_compaction_and_live_tools() {
        let _env = crate::storage::lock_test_env();
        let mut agent = agent();
        paired_tool_history(&mut agent, "tool_hibernation_inflight", "result".into());
        agent.session.messages_for_provider();
        agent.pending_alerts.push("pending delivery".into());
        assert!(agent.hibernate_idle_caches().is_none());
        agent.pending_alerts.clear();
        let queue = agent.soft_interrupt_queue();
        queue.lock().unwrap().push(SoftInterruptMessage {
            content: "scheduled wake".into(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::User,
            client_id: None,
        });
        assert!(agent.hibernate_idle_caches().is_none());
        queue.lock().unwrap().clear();
        let queue_guard = queue.lock().unwrap();
        assert!(agent.hibernate_idle_caches().is_none());
        drop(queue_guard);
        let compaction = agent.registry.compaction();
        let compaction_guard = compaction.try_write().unwrap();
        assert!(agent.hibernate_idle_caches().is_none());
        drop(compaction_guard);
        let tool = crate::tool::inflight::mark_tool_in_flight("tool_hibernation_inflight").unwrap();
        assert!(agent.hibernate_idle_caches().is_none());
        assert_eq!(
            agent
                .session
                .memory_profile_snapshot()
                .provider_cache_message_count,
            2
        );
        drop(tool);
        let turn = crate::turn_cancel_registry::register_active_turn(
            agent.session_id(),
            InterruptSignal::new(),
        );
        assert!(agent.hibernate_idle_caches().is_none());
        drop(turn);
        assert!(agent.hibernate_idle_caches().is_some());
    }

    #[test]
    fn idle_hibernation_does_not_write_session_and_refuses_migration_lease() {
        let _env = crate::storage::lock_test_env();
        let home = TestHome::new();
        let mut agent = agent();
        paired_tool_history(&mut agent, "tool_saved", "saved result".into());
        agent.session.save_prepared().unwrap();
        let path = crate::session::session_path(agent.session_id()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        agent.session.messages_for_provider();
        assert!(agent.hibernate_idle_caches().is_some());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );

        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "unsaved local state must survive a no-op save".into(),
                cache_control: None,
            }],
        );
        agent.session.messages_for_provider();
        crate::storage::write_session_lease(&crate::storage::SessionLease {
            session_id: agent.session_id().into(),
            epoch: 1,
            away_host: Some("elsewhere".into()),
            repo_root: None,
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap();
        // Regression for the misleading successful-but-no-op save API.
        assert!(agent.session.save_prepared().is_ok());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(agent.hibernate_idle_caches().is_none());
        assert_eq!(agent.session.messages.len(), 3);
        assert_eq!(
            agent
                .session
                .memory_profile_snapshot()
                .provider_cache_message_count,
            3
        );
        assert!(home.1.path().exists());
    }

    #[tokio::test]
    async fn idle_hibernation_next_turn_sends_original_tool_pairs_without_duplicates() {
        #[derive(Clone)]
        struct CaptureProvider(Arc<std::sync::Mutex<Vec<Vec<Message>>>>);
        #[async_trait::async_trait]
        impl Provider for CaptureProvider {
            async fn complete(
                &self,
                messages: &[Message],
                _: &[ToolDefinition],
                _: &str,
                _: Option<&str>,
            ) -> Result<crate::provider::EventStream> {
                self.0.lock().unwrap().push(messages.to_vec());
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(StreamEvent::TextDelta("awake".into())),
                    Ok(StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".into()),
                    }),
                ])))
            }
            fn name(&self) -> &str {
                "capture-idle-test"
            }
            fn fork(&self) -> Arc<dyn Provider> {
                Arc::new(self.clone())
            }
        }
        let _env = crate::storage::lock_test_env();
        let _home = TestHome::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut agent = Agent::build_base(
            Arc::new(CaptureProvider(Arc::clone(&calls))),
            Registry::empty(),
            Session::create_with_id(id::new_id("session_idle_turn_test"), None, None),
            None,
            HashSet::new(),
        );
        agent.memory_enabled = false;
        paired_tool_history(
            &mut agent,
            "pre_hibernation_tool",
            "retained exact result".into(),
        );
        assert_eq!(agent.repair_missing_tool_outputs(), 0);
        agent.session.messages_for_provider();
        assert!(agent.hibernate_idle_caches().is_some());
        assert_eq!(agent.run_once_capture("wake now").await.unwrap(), "awake");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let blocks: Vec<_> = calls[0]
            .iter()
            .flat_map(|message| &message.content)
            .collect();
        assert_eq!(blocks.iter().filter(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "pre_hibernation_tool")).count(), 1);
        // The live request may prefix result text with a configured timestamp.
        assert_eq!(blocks.iter().filter(|block| matches!(block, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "pre_hibernation_tool")).count(), 1);
        assert!(blocks.iter().any(|block| matches!(block, ContentBlock::ToolResult { tool_use_id, content, .. } if tool_use_id == "pre_hibernation_tool" && content.contains("retained exact result"))));
        assert!(!blocks.iter().any(|block| matches!(block, ContentBlock::ToolResult { content, .. } if content == TOOL_OUTPUT_MISSING_TEXT)));
    }

    #[test]
    #[ignore = "manual isolated RSS measurement; allocator and other tests affect RSS"]
    fn idle_hibernation_measured_rss_delta() {
        let _env = crate::storage::lock_test_env();
        let mut agent = agent();
        paired_tool_history(&mut agent, "tool_rss_measure", "x".repeat(64 * 1024 * 1024));
        agent.session.messages_for_provider();
        let before = crate::process_memory::snapshot();
        let released = agent.hibernate_idle_caches().unwrap();
        crate::process_memory::release_retained_heap("idle_hibernation_measurement");
        let after = crate::process_memory::snapshot();
        println!(
            "idle_hibernation_measurement cache_json_bytes={} rss_before={:?} rss_after={:?} delta={:?}",
            released.provider_cache_bytes,
            before.rss_bytes,
            after.rss_bytes,
            before
                .rss_bytes
                .zip(after.rss_bytes)
                .map(|(before, after)| before as i128 - after as i128)
        );
        assert_eq!(agent.session.messages.len(), 2);
        assert_eq!(
            agent
                .session
                .memory_profile_snapshot()
                .provider_cache_message_count,
            0
        );
    }
}
