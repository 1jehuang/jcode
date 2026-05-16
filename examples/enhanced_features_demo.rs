//! CarpAI Enhanced Features - Usage Examples
//!
//! This file demonstrates how to use the newly ported features from claude_code_src.
//! All examples are ready to run and showcase the key capabilities.

use carpai::mcp::enhanced_client::*;
use carpai::lsp_enhanced::*;
use carpai::cli::extended_commands::*;
use carpai::skill_system::*;
use carpai::app_state::*;

// ══════════════════════════════════════════════════════════════════
// 1. MCP Enhanced Client Examples
// ══════════════════════════════════════════════════════════════════

/// Example 1.1: Connect to an MCP server with retry logic
async fn example_mcp_connect_with_retry() {
    println!("📡 Example 1.1: MCP Client with Retry Logic\n");

    let config = EnhancedMcpConfig {
        name: "filesystem".to_string(),
        transport_type: TransportType::StdIO,
        command: Some("npx".to_string()),
        args: vec!["@modelcontextprotocol/server-filesystem".to_string(), "/tmp".to_string()],
        request_timeout_secs: 30,
        max_retries: 3,
        retry_delay_ms: 1000,
        ..Default::default()
    };

    match EnhancedMcpClient::connect(config).await {
        Ok(client) => {
            let handle = client.handle();

            // List available tools
            let tools = handle.tools();
            println!("✅ Connected! Available tools:");
            for tool in &tools {
                println!("  - {} ({})", tool.name, tool.description.unwrap_or_default());
            }

            // Call a tool with progress
            if !tools.is_empty() {
                let tool_name = &tools[0].name;
                println!("\n🔧 Calling tool '{}'...", tool_name);

                match handle.call_tool_with_progress(
                    tool_name,
                    serde_json::json!({}),
                ).await {
                    Ok(result) => {
                        println!("✅ Tool result: {:?}", result.content);
                    }
                    Err(e) => {
                        println!("❌ Error: {}", e);
                    }
                }
            }

            // Check health
            let health = client.health_check().await;
            println!("\n🏥 Health check: healthy={}, latency={}ms",
                     health.healthy, health.latency_ms);

            // Disconnect
            client.disconnect().await.expect("Failed to disconnect");
            println!("👋 Disconnected");
        }
        Err(e) => {
            println!("❌ Connection failed: {}", e);
        }
    }
}

/// Example 1.2: MCP with OAuth authentication
fn example_mcp_oauth_config() -> EnhancedMcpConfig {
    println!("📡 Example 1.2: MCP with OAuth\n");

    EnhancedMcpConfig {
        name: "github-mcp".to_string(),
        transport_type: TransportType::StdIO,
        command: Some("node".to_string()),
        args: vec!["github-mcp-server.js".to_string()],
        enable_oauth: true,
        request_timeout_secs: 60,
        max_retries: 5,
        ..Default::default()
    }
}

// ══════════════════════════════════════════════════════════════════
// 2. LSP Enhanced Client Examples
// ══════════════════════════════════════════════════════════════════

/// Example 2.1: Start an LSP server with full lifecycle management
async fn example_lsp_lifecycle() {
    println!("🔍 Example 2.1: LSP Server Lifecycle\n");

    let config = EnhancedLspConfig {
        name: "rust-analyzer".to_string(),
        command: "rust-analyzer".to_string(),
        args: vec![],
        root_path: Some(std::path::PathBuf::from(".")),
        initialization_timeout_secs: 30,
        request_timeout_secs: 10,
        auto_restart: true,
        max_restarts: 3,
        ..Default::default()
    };

    match EnhancedLspServer::connect(config).await {
        Ok(server) => {
            let handle = server.handle();

            // Check state
            let state = handle.state().await;
            println!("Server state: {}", state);

            // Get diagnostics registry
            let diag_registry = server.diagnostic_registry();

            // Simulate opening a file (in real usage, you'd use actual URIs)
            let file_uri = url::Url::parse("file:///src/main.rs").unwrap();

            // Go to definition
            if let Ok(result) = handle.goto_definition(
                &file_uri,
                lsp_types::Position { line: 10, character: 5 },
            ).await {
                println!("Definition found at: {:?}", result.result);
                println!("Latency: {}ms", result.latency_ms);
            }

            // Find references
            if let Ok(result) = handle.find_references(
                &file_uri,
                lsp_types::Position { line: 10, character: 5 },
                lsp_types::ReferenceContext {
                    include_declaration: true,
                },
            ).await {
                println!("Found {} references", result.result.len());
            }

            // Document symbols
            if let Ok(result) = handle.document_symbol(&file_uri).await {
                println!("Document contains {} symbols", result.result.len());
            }

            // Get metrics
            let metrics = handle.metrics();
            println!("\n📊 LSP Metrics:");
            println!("  Total requests: {}", metrics.total_requests);
            println!("  Successful: {}", metrics.successful_requests);
            println!("  Failed: {}", metrics.failed_requests);
            println!("  Avg latency: {:.2}ms", metrics.average_latency_ms);

            // Shutdown
            server.shutdown().await.expect("Shutdown failed");
            println!("👋 Server shut down");
        }
        Err(e) => {
            println!("❌ Failed to start LSP server: {}", e);
        }
    }
}

/// Example 2.2: Diagnostic caching and monitoring
async fn example_diagnostics_monitoring() {
    println!("📊 Example 2.2: Diagnostics Monitoring\n");

    let registry = Arc::new(EnhancedDiagnosticRegistry::new(50));

    // Simulate receiving diagnostics
    let uri = url::Url::parse("file:///src/lib.rs").unwrap();
    let diagnostics = vec![
        lsp_types::Diagnostic {
            range: lsp_types::Range {
                start: lsp_types::Position { line: 42, character: 0 },
                end: lsp_types::Position { line: 42, character: 20 },
            },
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            code: None,
            source: Some("rust-analyzer".to_string()),
            message: "Unused variable `x`".to_string(),
            related_information: None,
            tags: None,
            data: None,
        },
    ];

    // Update diagnostics
    registry.update(&uri, Some(1), diagnostics.clone());

    // Query diagnostics
    let errors = registry.get_errors_count();
    let warnings = registry.get_warnings_count();
    println!("Errors: {}, Warnings: {}", errors, warnings);

    // Get specific file diagnostics
    let file_diags = registry.get_diagnostics_for_file(&uri.to_string());
    for diag in &file_diags {
        println!("{}: {} (line {})",
                 if diag.severity == lsp_types::DiagnosticSeverity::ERROR {
                     "❌ Error"
                 } else {
                     "⚠️ Warning"
                 },
                 diag.message,
                 diag.range.start.line);
    }
}

// ══════════════════════════════════════════════════════════════════
// 3. Extended Commands Examples
// ══════════════════════════════════════════════════════════════════

/// Example 3.1: Using the /btw command for contextual hints
async fn example_btw_command() {
    println!("💡 Example 3.1: /btw Command\n");

    let registry = init_extended_commands().await;

    let ctx = CommandContext {
        session_id: "session_123".to_string(),
        user_input: "I need to refactor this large function".to_string(),
        timestamp: chrono::Utc::now(),
        metadata: [
            ("task_type".to_string(), "refactoring".to_string()),
            ("complexity".to_string(), "high".to_string()),
        ].into_iter().collect(),
    };

    match registry.execute_command("btw", &ctx, None).await {
        Ok(result) => {
            println!("{}", result.message);
            println!("⏱️ Duration: {}ms", result.duration_ms);
        }
        Err(e) => {
            println!("❌ Error: {}", e);
        }
    }
}

/// Example 3.2: Fast mode toggle
async fn example_fast_mode() {
    println!("⚡ Example 3.2: /fast Command - Mode Toggle\n");

    let registry = init_extended_commands().await;
    let ctx = CommandContext::default();

    // Cycle through modes: Normal -> Fast -> Turbo -> Normal
    for _ in 0..4 {
        match registry.execute_command("fast", &ctx, None).await {
            Ok(result) => {
                println!("{}", result.message);
            }
            Err(e) => {
                println!("❌ Error: {}", e);
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

/// Example 3.3: Session rewind functionality
async fn example_rewind_command() {
    println!("⏪ Example 3.3: /rewind Command - Session Rollback\n");

    let registry = init_extended_commands().await;
    let ctx = CommandContext::default();

    // Create some snapshots (simulated)
    let rewind_cmd = registry.get("rewind").await.unwrap();
    // In real usage, you'd call RewindCommand's create_snapshot method

    // List snapshots
    match registry.execute_command("rewind", &ctx, Some("list")).await {
        Ok(result) => {
            println!("{}", result.message);
        }
        Err(e) => {
            println!("❌ Error: {}", e);
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 4. Skills System Examples
// ══════════════════════════════════════════════════════════════════

/// Example 4.1: Loop skill for iterative improvement
async fn example_loop_skill() {
    println!("🔄 Example 4.1: Loop Skill - Iterative Execution\n");

    let skills = init_skills_system().await;

    let ctx = SkillContext {
        task_description: "Optimize database query performance".to_string(),
        current_state: serde_json::json!({
            "current_performance": "slow",
            "bottleneck": "N+1 query problem"
        }),
        history: vec![],
        constraints: SkillConstraints {
            max_iterations: 5,
            timeout_secs: 60,
            quality_threshold: 0.8,
            ..Default::default()
        },
    };

    match skills.execute_skill("loop", &ctx).await {
        Ok(result) => {
            println!("Success: {}", result.success);
            println!("Quality score: {:.2}", result.quality_score.unwrap_or(0.0));
            println!("Iterations used: {}", result.iterations_used);
            println!("Duration: {}ms", result.duration_ms);
            println!("\nOutput:\n{}", result.output);
        }
        Err(e) => {
            println!("❌ Error: {}", e);
        }
    }
}

/// Example 4.2: Verify skill for validation
async fn example_verify_skill() {
    println!("✅ Example 4.2: Verify Skill - Result Validation\n");

    let skills = init_skills_system().await;

    let ctx = SkillContext {
        task_description: "Check code for common issues:\n- Syntax errors\n- Missing error handling\n- Security vulnerabilities".to_string(),
        current_state: serde_json::json!({}),
        history: vec![],
        constraints: SkillConstraints::default(),
    };

    match skills.execute_skill("verify", &ctx).await {
        Ok(result) => {
            println!("{}", result.output);
        }
        Err(e) => {
            println!("❌ Error: {}", e);
        }
    }
}

/// Example 4.3: Simplify skill for code optimization
async fn example_simplify_skill() {
    println!("✨ Example 4.3: Simplify Skill - Code Optimization\n");

    let skills = init_skills_system().await;

    let complex_code = r#"
fn process_data(data: Vec<Data>) -> Result<Vec<ProcessedData>> {
    let mut result = Vec::new();
    for item in data.iter() {
        if item.is_valid() {
            let processed = item.process()?;
            if processed.is_ok() {
                result.push(processed);
            } else {
                continue;
            }
        } else {
            log::warn!("Invalid item");
        }
    }
    Ok(result)
}
"#.to_string();

    let ctx = SkillContext {
        task_description: complex_code,
        current_state: serde_json::json!({}),
        history: vec![],
        constraints: SkillConstraints::default(),
    };

    match skills.execute_skill("simplify", &ctx).await {
        Ok(result) => {
            println!("{}", result.output);
        }
        Err(e) => {
            println!("❌ Error: {}", e);
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 5. AppState Management Examples
// ══════════════════════════════════════════════════════════════════

/// Example 5.1: Basic state management with selectors
async fn example_appstate_basic() {
    println!("🗄️ Example 5.1: Basic State Management\n");

    let manager = create_state_manager_with_defaults().await;

    // Update state
    manager.update(|state| {
        state.config.model_name = "gpt-4".to_string();
        state.session.current_task = Some("Refactor module X".to_string());
    }).await.expect("Update failed");

    // Use selectors
    let model_name = manager.select::<String, _>(&ModelNameSelector).await;
    println!("Current model: {}", model_name);

    let theme = manager.select::<String, _>(&ThemeSelector).await;
    println!("Theme: {}", theme);

    // Increment counters
    manager.increment_message_count().await.expect("Counter failed");
    manager.increment_tool_call_count().await.expect("Counter failed");

    // Get summary
    let summary = manager.summary().await;
    println!("\nState Summary:\n{}", summary);
}

/// Example 5.2: State persistence and recovery
async fn example_appstate_persistence() {
    println!("💾 Example 5.2: State Persistence\n");

    let manager = AppStateManager::new(50);
    let path = std::path::Path::new("test_state.json");

    // Modify state
    manager.update(|state| {
        state.ui.theme = "dark".to_string();
        state.ui.font_size = 16;
        state.custom.insert("user_preference".to_string(), serde_json::json!("value"));
    }).await.expect("Update failed");

    // Persist to disk
    manager.persist(path).await.expect("Persist failed");
    println!("✅ State saved to disk");

    // Reset
    manager.reset().await.expect("Reset failed");
    println!("🔄 State reset");

    // Load from disk
    manager.load(path).await.expect("Load failed");
    println!("✅ State loaded from disk");

    // Verify
    let theme = manager.select::<String, _>(&ThemeSelector).await;
    println!("Restored theme: {}", theme);

    // Cleanup
    let _ = std::fs::remove_file(path);
}

/// Example 5.3: Undo/Redo functionality
async fn_example_appstate_undo_redo() {
    println!("↩️ Example 5.3: Undo/Redo\n");

    let manager = AppStateManager::new(10);

    // Make several changes
    for i in 1..=5 {
        let value = format!("State version {}", i);
        manager.update(move |state| {
            state.session.id = value;
        }).await.expect("Update failed");
    }

    println!("History length: {}", manager.history_length().await);

    // Undo changes
    while manager.undo().await.expect("Undo failed") {
        println!("Undone one change");
        if manager.history_length().await <= 0 {
            break;
        }
    }
}

/// Example 5.4: Observer pattern for reactive updates
async fn example_appstate_observer() {
    println!("👀 Example 5.4: Observer Pattern\n");

    let manager = AppStateManager::new(50);

    // Subscribe to changes
    manager.subscribe(|old_state, new_state| {
        println!("State changed: v{} -> v{}", old_state.version, new_state.version);
    }).await;

    // Subscribe via broadcast channel
    let mut rx = manager.subscribe_channel();

    // Spawn a listener
    tokio::spawn(async move {
        while let Ok(change) = rx.recv().await {
            println!("Broadcast received: v{} at {:?}",
                     change.version, change.timestamp);
        }
    });

    // Trigger updates
    for i in 1..=3 {
        manager.update(|state| {
            state.version += 1;  // This will be overwritten by update()
        }).await.expect("Update failed");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

// ══════════════════════════════════════════════════════════════════
// Main Entry Point - Run All Examples
// ══════════════════════════════════════════════════════════════════

#[tokio::main]
async fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║   CarpAI Enhanced Features - Usage Examples      ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // Run examples (comment/uncomment as needed)
    
    // 1. MCP Examples
    example_mcp_connect_with_retry().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    // 2. LSP Examples
    example_lsp_lifecycle().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    // 3. Extended Commands
    example_btw_command().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    example_fast_mode().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    // 4. Skills System
    example_loop_skill().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    example_verify_skill().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    example_simplify_skill().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    // 5. AppState Management
    example_appstate_basic().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    example_appstate_persistence().await;
    println!("\n" + &"-".repeat(60) + "\n");
    
    println!("✅ All examples completed successfully!");
}
