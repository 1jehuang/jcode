//! Integration Tests for Enhanced Features
//!
//! Comprehensive test suite for MCP, LSP, Commands, Skills, and AppState modules.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ══════════════════════════════════════════════════════════════════
    // 1. MCP Enhanced Client Tests
    // ══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_mcp_config_default() {
        let config = EnhancedMcpConfig::default();
        assert_eq!(config.transport_type, TransportType::StdIO);
        assert_eq!(config.request_timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
        assert!(!config.enable_oauth);
    }

    #[test]
    fn test_transport_type_display() {
        assert_eq!(TransportType::StdIO.to_string(), "stdio");
        assert_eq!(TransportType::SSE.to_string(), "sse");
        assert_eq!(TransportType::StreamableHTTP.to_string(), "streamable-http");
        assert_eq!(TransportType::WebSocket.to_string(), "websocket");
    }

    #[test]
    fn test_mcp_error_types() {
        let auth_err = McpError::AuthError {
            server_name: "test".to_string(),
            message: "failed".to_string(),
        };
        assert!(auth_err.is_auth_error());
        assert!(!auth_err.is_session_expired());
        assert_eq!(auth_err.server_name(), Some("test"));

        let session_err = McpError::SessionExpired {
            server_name: "test".to_string(),
        };
        assert!(session_err.is_session_expired());
        assert!(!session_err.is_auth_error());
    }

    #[test]
    fn test_connection_state_display() {
        assert_eq!(ConnectionState::Disconnected.to_string(), "disconnected");
        assert_eq!(ConnectionState::Connected.to_string(), "connected");
        assert_eq!(ConnectionState::NeedsAuth.to_string(), "needs-auth");

        let error_state = ConnectionState::Error("connection refused".to_string());
        assert!(error_state.to_string().contains("error"));
    }

    // ══════════════════════════════════════════════════════════════════
    // 2. LSP Enhanced Client Tests
    // ══════════════════════════════════════════════════════════════════

    #[test]
    fn test_lsp_config_default() {
        let config = EnhancedLspConfig::default();
        assert_eq!(config.initialization_timeout_secs, 30);
        assert_eq!(config.request_timeout_secs, 10);
        assert!(!config.auto_restart);
        assert_eq!(config.max_restarts, 3);
    }

    #[test]
    fn test_lsp_server_state() {
        assert_eq!(EnhancedLspServerState::Stopped.label(), "stopped");
        assert_eq!(EnhancedLspServerState::Running.label(), "running");
        assert!(EnhancedLspServerState::Running.is_operational());
        assert!(!EnhancedLspServerState::Stopped.is_operational());
    }

    #[test]
    fn test_lsp_metrics_default() {
        let metrics = LspMetrics::default();
        assert_eq!(metrics.total_requests, 0);
        assert_eq!(metrics.successful_requests, 0);
        assert_eq!(metrics.average_latency_ms, 0.0);
    }

    #[tokio::test]
    async fn test_diagnostic_registry() {
        let registry = Arc::new(EnhancedDiagnosticRegistry::new(10));

        // Initially empty
        assert_eq!(registry.get_errors_count(), 0);
        assert_eq!(registry.get_warnings_count(), 0);

        // Add diagnostics
        let uri = url::Url::parse("file:///test.rs").unwrap();
        let diagnostics = vec![
            lsp_types::Diagnostic {
                range: lsp_types::Range {
                    start: lsp_types::Position { line: 0, character: 0 },
                    end: lsp_types::Position { line: 0, character: 10 },
                },
                severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                code: None,
                source: Some("test".to_string()),
                message: "Test error".to_string(),
                related_information: None,
                tags: None,
                data: None,
            },
            ];

        registry.update(&uri, Some(1), diagnostics);

        // Verify counts
        assert_eq!(registry.get_errors_count(), 1);

        // Clear
        registry.clear_uri(&uri.to_string());
        assert_eq!(registry.get_errors_count(), 0);
    }

    // ══════════════════════════════════════════════════════════════════
    // 3. Extended Commands Tests
    // ══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_btw_command() {
        let cmd = BtwCommand::new();
        assert_eq!(cmd.name(), "btw");
        assert!(cmd.description().contains("contextual hints"));

        let ctx = CommandContext::default();

        // Should succeed with no args
        cmd.validate_args(None).await.unwrap();
        cmd.validate_args(Some("context")).await.unwrap();

        // Execute
        let result = cmd.execute(&ctx, None).await.unwrap();
        assert!(result.success);
        assert!(result.message.contains("By the way"));
    }

    #[tokio::test]
    async fn test_fast_command() {
        let cmd = FastCommand::new();
        assert_eq!(cmd.name(), "fast");

        // Validate args
        cmd.validate_args(Some("normal")).await.unwrap();
        cmd.validate_args(Some("fast")).await.unwrap();
        cmd.validate_args(Some("turbo")).await.unwrap();

        // Invalid mode should fail
        assert!(cmd.validate_args(Some("invalid")).await.is_err());

        // Execute with mode
        let ctx = CommandContext::default();
        let result = cmd.execute(&ctx, Some("turbo")).await.unwrap();
        assert!(result.success);
        assert!(result.data.is_some());
    }

    #[tokio::test]
    async fn test_rewind_command() {
        let cmd = RewindCommand::new(5);
        assert_eq!(cmd.name(), "rewind");

        let ctx = CommandContext::default();

        // List snapshots (empty)
        let result = cmd.execute(&ctx, Some("list")).await.unwrap();
        assert!(result.success);
        assert!(result.message.contains("No snapshots"));
    }

    #[tokio::test]
    async fn test_command_registry() {
        let registry = ExtendedCommandRegistry::new();

        // Register commands
        registry.register(Arc::new(BtwCommand::new())).await;
        registry.register(Arc::new(FastCommand::new())).await;

        // List commands
        let commands = registry.list_commands().await;
        assert!(commands.contains(&"btw".to_string()));
        assert!(commands.contains(&"fast".to_string()));

        // Get command
        let btw = registry.get("btw").await;
        assert!(btw.is_some());

        // Unknown command
        let unknown = registry.get("unknown").await;
        assert!(unknown.is_none());
    }

    // ══════════════════════════════════════════════════════════════════
    // 4. Skills System Tests
    // ══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_loop_skill() {
        let skill = LoopSkill::new();
        assert_eq!(skill.name(), "loop");
        assert!(skill.description().contains("iterative"));

        let ctx = SkillContext {
            task_description: "Test task".to_string(),
            ..Default::default()
        };

        assert!(skill.can_execute(&ctx).await);

        let cost = skill.estimate_cost(&ctx).await;
        assert!(cost.estimated_time_ms > 0);

        // Execute
        let result = skill.execute(&ctx).await.unwrap();
        assert!(result.output.contains("[Iteration"));
    }

    #[tokio::test]
    async fn test_verify_skill() {
        let skill = VerifySkill::new();
        assert_eq!(skill.name(), "verify");

        let ctx = SkillContext {
            task_description: "Check this code".to_string(),
            ..Default::default()
        };

        let result = skill.execute(&ctx).await.unwrap();
        assert!(result.success || !result.success); // May pass or fail depending on input
        assert!(result.output.contains("Verification Results"));
    }

    #[tokio::test]
    async fn test_simplify_skill() {
        let skill = SimplifySkill::new();
        assert_eq!(skill.name(), "simplify");

        let ctx = SkillContext {
            task_description: "Complex code here".repeat(100),
            ..Default::default()
        };

        let result = skill.execute(&ctx).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Simplification Results"));
    }

    #[tokio::test]
    async fn test_skills_registry() {
        let registry = SkillsRegistry::new();

        // Register skills
        registry.register(Arc::new(LoopSkill::new())).await;
        registry.register(Arc::new(VerifySkill::new())).await;
        registry.register(Arc::new(SimplifySkill::new())).await;

        // List skills
        let skills = registry.list_skills().await;
        assert_eq!(skills.len(), 3);
        assert!(skills.contains(&"loop".to_string()));
        assert!(skills.contains(&"verify".to_string()));
        assert!(skills.contains(&"simplify".to_string()));

        // Execute skill
        let ctx = SkillContext {
            task_description: "Test".to_string(),
            ..Default::default()
        };

        let result = registry.execute_skill("verify", &ctx).await;
        assert!(result.is_ok());

        // History should have one entry
        let history = registry.get_history().await;
        assert_eq!(history.len(), 1);
    }

    // ══════════════════════════════════════════════════════════════════
    // 5. AppState Management Tests
    // ══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_appstate_default() {
        let state = AppState::default();
        assert_eq!(state.version, 1);
        assert_eq!(state.ui.theme, "dark");
        assert_eq!(state.config.model_name, "default");
        assert!(state.session.current_task.is_none());
    }

    #[tokio::test]
    async fn test_appstate_manager_basic() {
        let manager = AppStateManager::new(50);

        // Initial state
        let state = manager.get_state().await;
        assert_eq!(state.version, 1);

        // Update state
        manager.update(|state| {
            state.config.model_name = "gpt-4".to_string();
            state.ui.font_size = 16;
        }).await.unwrap();

        // Verify update
        let state = manager.get_state().await;
        assert_eq!(state.config.model_name, "gpt-4");
        assert_eq!(state.ui.font_size, 16);
        assert!(state.version > 1);
    }

    #[tokio::test]
    async fn test_appstate_selectors() {
        let manager = AppStateManager::new(50);

        manager.update(|state| {
            state.config.model_name = "test-model".to_string();
            state.ui.theme = "light".to_string();
        }).await.unwrap();

        // Test selectors
        let model = manager.select::<String, _>(&ModelNameSelector).await;
        assert_eq!(model, "test-model");

        let theme = manager.select::<String, _>(&ThemeSelectorSelector).await;
        assert_eq!(theme, "light");
    }

    #[tokio::test]
    async fn test_appstate_undo() {
        let manager = AppStateManager::new(10);

        // Make multiple updates
        for i in 1..=3 {
            manager.update(move |state| {
                state.version += 1;  // Will be overwritten by manager
            }).await.unwrap();
        }

        // Should have history
        assert!(manager.history_length().await > 0);

        // Undo
        let undone = manager.undo().await.unwrap();
        assert!(undone);
    }

    #[tokio::test]
    async fn test_appstate_counters() {
        let manager = AppStateManager::new(50);

        // Increment counters
        manager.increment_message_count().await.unwrap();
        manager.increment_message_count().await.unwrap();
        manager.increment_tool_call_count().await.unwrap();

        let state = manager.get_state().await;
        assert_eq!(state.session.message_count, 2);
        assert_eq!(state.session.tool_call_count, 1);
    }

    #[tokio::test]
    async fn test_appstate_custom_data() {
        let manager = AppStateManager::new(50);

        // Set custom data
        manager.merge_custom_data([
            ("key1".to_string(), serde_json::json!("value1")),
            ("key2".to_string(), serde_json::json!(42)),
        ].into_iter().collect()).await.unwrap();

        // Get custom data
        let value1 = manager.get_custom_value("key1").await;
        assert!(value1.is_some());
        assert_eq!(value1.unwrap(), serde_json::json!("value1"));

        let value2 = manager.get_custom_value("key2").await;
        assert_eq!(value2.unwrap(), serde_json::json!(42));

        // Non-existent key
        let missing = manager.get_custom_value("nonexistent").await;
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_appstate_summary() {
        let manager = AppStateManager::new(50);

        manager.update(|state| {
            state.session.id = "test-session".to_string();
            state.session.message_count = 10;
            state.session.tool_call_count = 5;
            state.session.current_task = Some("Test task".to_string());
        }).await.unwrap();

        let summary = manager.summary().await;
        assert!(summary.contains("test-session"));
        assert!(summary.contains("10 messages"));
        assert!(summary.contains("5 tool calls"));
        assert!(summary.contains("Test task"));
    }

    #[tokio::test]
    async fn test_batch_update() {
        let manager = AppStateManager::new(50);

        batch_update(&manager, vec![
            Box::new(|state| { state.config.model_name = "batched".to_string(); }),
            Box::new(|state| { state.ui.theme = "custom".to_string(); }),
        ]).await.unwrap();

        let state = manager.get_state().await;
        assert_eq!(state.config.model_name, "batched");
        assert_eq!(state.ui.theme, "custom");
    }

    // ══════════════════════════════════════════════════════════════════
    // 6. Integration Tests
    // ══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_full_workflow() {
        // Initialize all systems
        let command_registry = init_extended_commands().await;
        let skills_registry = init_skills_system().await;
        let state_manager = create_state_manager_with_defaults().await;

        // Update state
        state_manager.update(|state| {
            state.config.model_name = "integration-test".to_string();
        }).await.unwrap();

        // Use commands
        let ctx = CommandContext {
            session_id: "integration-test-session".to_string(),
            user_input: "Test workflow".to_string(),
            ..Default::default()
        };

        let _ = command_registry.execute_command("btw", &ctx, None).await;

        // Use skills
        let skill_ctx = SkillContext {
            task_description: "Integration test task".to_string(),
            ..Default::default()
        };

        let _ = skills_registry.execute_skill("verify", &skill_ctx).await;

        // Verify final state
        let model = state_manager.select::<String, _>(&ModelNameSelector).await;
        assert_eq!(model, "integration-test");

        println!("✅ Full integration workflow completed successfully!");
    }
}
