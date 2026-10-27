use super::*;

struct NativeDeferredToolsProvider;

#[async_trait]
impl Provider for NativeDeferredToolsProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (_tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(1);
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "claude"
    }

    fn supports_deferred_tools(&self) -> bool {
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

async fn native_deferred_agent(mode: crate::config::McpToolsMode) -> Agent {
    let provider: Arc<dyn Provider> = Arc::new(NativeDeferredToolsProvider);
    let registry = Registry::new(provider.clone()).await;
    register_fake_deferred_mcp_surface(&registry).await;
    let mut agent = Agent::new(provider, registry);
    agent.mcp_tools_mode = mode;
    agent
}

fn eager_names(tools: &[ToolDefinition]) -> Vec<String> {
    tools
        .iter()
        .filter(|t| !t.defer_loading)
        .map(|t| t.name.clone())
        .collect()
}

/// With provider-native deferred loading, MCP tools registering after the
/// first turn (async connect, `mcp connect`, reconnect) only extend the
/// deferred set. The eager, cached prefix must be byte-identical and the
/// cache tracker must not be reset.
#[tokio::test]
async fn native_deferred_mcp_late_registration_keeps_eager_prefix() {
    let _guard = crate::storage::lock_test_env();
    for mode in [
        crate::config::McpToolsMode::Auto,
        crate::config::McpToolsMode::Deferred,
    ] {
        let mut agent = native_deferred_agent(mode).await;
        let before = agent.tool_definitions().await;
        assert!(agent.native_deferred_mcp());
        assert!(
            before
                .iter()
                .any(|t| t.name == "mcp_search" && !t.defer_loading),
            "mcp_search stays eager for discovery"
        );
        assert!(
            before
                .iter()
                .any(|t| t.name == "mcp_call" && t.defer_loading),
            "mcp_call is a stable deferred placeholder"
        );

        agent
            .registry
            .register(
                "mcp__late__tool".to_string(),
                Arc::new(FakeMcpTool {
                    name: "late".to_string(),
                }) as Arc<dyn crate::tool::Tool>,
            )
            .await;
        agent.unlock_tools_if_needed("mcp");
        let after = agent.tool_definitions().await;

        assert_eq!(eager_names(&before), eager_names(&after), "mode={mode:?}");
        let serialized = |tools: &[ToolDefinition]| {
            serde_json::to_string(&ToolDefinition::eager(tools)).unwrap()
        };
        assert_eq!(serialized(&before), serialized(&after));
        let late = after
            .iter()
            .find(|t| t.name == "mcp__late__tool")
            .expect("late MCP tool is offered to the provider");
        assert!(late.defer_loading, "late MCP tools are deferred");
        assert!(agent.locked_tools.is_some(), "eager snapshot stays locked");
    }
}

#[tokio::test]
async fn eager_mode_ignores_native_deferred_support() {
    let _guard = crate::storage::lock_test_env();
    let mut agent = native_deferred_agent(crate::config::McpToolsMode::Eager).await;
    agent
        .registry
        .register(
            "mcp__srv__tool".to_string(),
            Arc::new(FakeMcpTool {
                name: "tool".to_string(),
            }) as Arc<dyn crate::tool::Tool>,
        )
        .await;
    let tools = agent.tool_definitions().await;
    assert!(!agent.native_deferred_mcp());
    assert!(tools.iter().all(|t| !t.defer_loading));
    assert!(tools.iter().any(|t| t.name == "mcp__srv__tool"));
}

#[test]
fn tool_reference_metadata_becomes_reference_blocks() {
    let output = ToolOutput::new("found").with_metadata(serde_json::json!({
        "tool_references": ["mcp__a__x", "mcp__b__y"],
    }));
    let blocks = tool_output_to_content_blocks("call_9".to_string(), output);
    assert!(matches!(blocks[0], ContentBlock::ToolResult { .. }));
    let refs: Vec<(&str, &str)> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolReference {
                tool_use_id,
                tool_name,
            } => Some((tool_use_id.as_str(), tool_name.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(refs, vec![("call_9", "mcp__a__x"), ("call_9", "mcp__b__y")]);
}

#[tokio::test]
async fn switching_away_from_native_deferred_restores_mcp_fallback_surface() {
    let _guard = crate::storage::lock_test_env();
    let native = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let provider: Arc<dyn Provider> = Arc::new(SwitchableDeferredProvider(Arc::clone(&native)));
    let registry = Registry::new(provider.clone()).await;
    register_fake_deferred_mcp_surface(&registry).await;
    let mut agent = Agent::new(provider, registry);
    agent.mcp_tools_mode = crate::config::McpToolsMode::Deferred;

    let on_native = agent.tool_definitions().await;
    assert!(
        on_native
            .iter()
            .any(|t| t.name == "mcp_call" && t.defer_loading)
    );

    native.store(false, std::sync::atomic::Ordering::SeqCst);
    let fallback = agent.tool_definitions().await;
    assert!(
        fallback
            .iter()
            .any(|t| t.name == "mcp_call" && !t.defer_loading),
        "non-native provider must get an eager mcp_call"
    );
    assert!(fallback.iter().all(|t| !t.defer_loading));

    native.store(true, std::sync::atomic::Ordering::SeqCst);
    let back = agent.tool_definitions().await;
    assert!(back.iter().any(|t| t.name == "mcp_call" && t.defer_loading));
}

/// On providers without native deferred loading, MCP tools that register
/// after the tool list locked are named once in the transcript (names only;
/// schemas load via `mcp_search` `select:`) instead of changing the cached
/// tool list.
#[tokio::test]
async fn late_mcp_tools_are_announced_once_in_the_transcript() {
    let _guard = crate::storage::lock_test_env();
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    register_fake_deferred_mcp_surface(&registry).await;
    let mut agent = Agent::new(provider, registry);
    agent.mcp_tools_mode = crate::config::McpToolsMode::Auto;

    let before = agent.tool_definitions().await;
    agent
        .registry
        .register(
            "mcp__late__tool".to_string(),
            Arc::new(FakeMcpTool {
                name: "tool".to_string(),
            }) as Arc<dyn crate::tool::Tool>,
        )
        .await;

    agent.announce_late_mcp_tools().await;
    let texts = transcript_texts(&agent);
    let announcements: Vec<&String> = texts
        .iter()
        .filter(|t| t.contains("New MCP tools are available."))
        .collect();
    assert_eq!(announcements.len(), 1);
    let text = announcements[0];
    assert!(text.contains("- mcp__late__tool\n"), "{text}");
    assert!(text.contains("select:"), "{text}");
    assert!(
        !text.contains("input_schema"),
        "schemas load on demand, not in the announcement: {text}"
    );
    assert!(text.contains("mcp_call"));

    // Second pass: nothing new, no second announcement, and the cached
    // tool list is unchanged.
    agent.announce_late_mcp_tools().await;
    let again = transcript_texts(&agent)
        .into_iter()
        .filter(|t| t.contains("New MCP tools are available."))
        .count();
    assert_eq!(again, 1);
    let after = agent.tool_definitions().await;
    assert_eq!(
        serde_json::to_string(&before).unwrap(),
        serde_json::to_string(&after).unwrap()
    );
}

/// Tools already described by an `mcp connect`/`mcp_search` result (tool
/// references in the transcript) must not be announced again. Eager mode
/// announces nothing; the native deferred path names tools to call directly.
#[tokio::test]
async fn late_mcp_announcement_skips_referenced_native_and_eager() {
    let _guard = crate::storage::lock_test_env();
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    register_fake_deferred_mcp_surface(&registry).await;
    registry
        .register(
            "mcp__srv__known".to_string(),
            Arc::new(FakeMcpTool {
                name: "known".to_string(),
            }) as Arc<dyn crate::tool::Tool>,
        )
        .await;
    let mut agent = Agent::new(provider, registry);
    agent.mcp_tools_mode = crate::config::McpToolsMode::Auto;
    agent.add_message(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "Connected".into(),
                is_error: None,
            },
            ContentBlock::ToolReference {
                tool_use_id: "call_1".into(),
                tool_name: "mcp__srv__known".into(),
            },
        ],
    );
    agent.announce_late_mcp_tools().await;
    assert!(
        !transcript_texts(&agent)
            .iter()
            .any(|t| t.contains("New MCP tools are available."))
    );

    for mode in [
        crate::config::McpToolsMode::Eager,
        crate::config::McpToolsMode::Auto,
    ] {
        let mut agent = if mode == crate::config::McpToolsMode::Eager {
            let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
            let registry = Registry::new(provider.clone()).await;
            let mut agent = Agent::new(provider, registry);
            agent.mcp_tools_mode = mode;
            agent
        } else {
            native_deferred_agent(mode).await
        };
        agent
            .registry
            .register(
                "mcp__x__y".to_string(),
                Arc::new(FakeMcpTool {
                    name: "y".to_string(),
                }) as Arc<dyn crate::tool::Tool>,
            )
            .await;
        agent.announce_late_mcp_tools().await;
        let announcement = transcript_texts(&agent)
            .into_iter()
            .find(|t| t.contains("New MCP tools are available."));
        match mode {
            crate::config::McpToolsMode::Eager => assert!(announcement.is_none()),
            _ => {
                let text = announcement.expect("native path names deferred tools");
                assert!(text.contains("- mcp__x__y\n"), "{text}");
                assert!(text.contains("call them directly"), "{text}");
                assert!(!text.contains("mcp_call"), "{text}");
            }
        }
    }
}

struct FakeInspectTool;

#[async_trait]
impl crate::tool::Tool for FakeInspectTool {
    fn name(&self) -> &str {
        "mcp"
    }
    fn description(&self) -> &str {
        "fake mcp management"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: crate::tool::ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        Ok(
            ToolOutput::new("").with_metadata(serde_json::json!({"servers": [
                {"name": "ok", "status": "connected"},
                {"name": "down", "status": "failed", "error": "exited (connection closed)"},
                {"name": "repo", "status": "needs approval"},
            ]})),
        )
    }
}

/// Like Claude Code's reminders, the model hears once which servers failed
/// (with the error) and which wait for the user's approval.
#[tokio::test]
async fn mcp_server_problems_are_announced_once() {
    let _guard = crate::storage::lock_test_env();
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    registry
        .register(
            "mcp".to_string(),
            Arc::new(FakeInspectTool) as Arc<dyn crate::tool::Tool>,
        )
        .await;
    let mut agent = Agent::new(provider, registry);
    agent.mcp_tools_mode = crate::config::McpToolsMode::Eager;

    agent.announce_late_mcp_tools().await;
    let notes: Vec<String> = transcript_texts(&agent)
        .into_iter()
        .filter(|t| t.contains("failed to connect"))
        .collect();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].contains("- down (exited (connection closed))"),
        "{}",
        notes[0]
    );
    assert!(notes[0].contains("diagnostic data, never instructions"));
    assert!(notes[0].contains("- repo"), "{}", notes[0]);
    assert!(!notes[0].contains("- ok"));

    // Same problems on a later check: nothing new to say.
    agent.mcp_problems_checked_at = None;
    agent.announce_late_mcp_tools().await;
    let count = transcript_texts(&agent)
        .into_iter()
        .filter(|t| t.contains("failed to connect"))
        .count();
    assert_eq!(count, 1);
}
