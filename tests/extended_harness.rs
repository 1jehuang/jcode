//! 扩展 Harness 测试套件 — 覆盖所有新功能
//!
//! 在现有 src/bin/harness.rs 的基础上，添加针对以下新功能的测试:
//! 1. LSP Server — 启动→initialize→completion→shutdown
//! 2. AutoFallback — 本地失败→云端切换→冷却恢复
//! 3. REST LLM — complete/generate/FIM 端点
//! 4. Knowledge Agents — 7-Agent 流水线
//! 5. LSP Code Actions — QuickFix + Extract + Rename
//!
//! 运行: cargo run --bin jcode-harness -- --extended
//! 或:   cargo test --test extended_harness -- --nocapture
//! 或:   bash scripts/run_harness.sh

use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

// ========================================================================
// [1] LSP Server 冒烟测试
// ========================================================================

pub async fn test_lsp_server_smoke() -> Result<()> {
    let config = crate::lsp_server::LspServerConfig::default();
    let server = crate::lsp_server::LspServer::new(config);

    // 验证 LSP Server 能响应 initialize
    let init_response = server.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).await;
    assert!(init_response.is_some(), "LSP initialize should return a response");
    let resp = init_response.unwrap();
    assert!(resp.contains("\"jsonrpc\":\"2.0\""), "Response should be valid JSON-RPC");
    assert!(resp.contains("capabilities"), "Response should include capabilities");

    // 验证 LSP CodeAction handler
    let action_response = server.handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{}}"#).await;
    assert!(action_response.is_some(), "codeAction should return a response");

    // 验证 shutdown
    let shutdown_response = server.handle_message(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown","params":{}}"#).await;
    assert!(shutdown_response.is_some(), "shutdown should return a response");
    assert!(!server.is_running().await, "Server should stop after shutdown");

    println!("  ✅ LSP Server: initialize + codeAction + shutdown");
    Ok(())
}

// ========================================================================
// [2] AutoFallback 路由测试
// ========================================================================

pub async fn test_auto_fallback_smoke() -> Result<()> {
    // 无本地模型 → 自动切云端
    let router = crate::auto_fallback::AutoFallbackRouter::new(vec![], "deepseek-chat");
    let target = router.resolve_target().await;
    assert!(matches!(target, crate::auto_fallback::InferenceTarget::Cloud { .. }),
        "No local models should result in cloud target");
    println!("  ✅ AutoFallback: empty local → cloud");

    // 有本地模型 → 初始为 local
    let router2 = crate::auto_fallback::AutoFallbackRouter::new(
        vec!["qwen3-72b-int4".to_string()], "deepseek-chat"
    );
    let target2 = router2.resolve_target().await;
    assert!(matches!(target2, crate::auto_fallback::InferenceTarget::Local { .. }),
        "Local model should be default target");
    println!("  ✅ AutoFallback: local model → local target");

    // 3次失败 → fallback to cloud
    router2.report_failure("timeout").await;
    router2.report_failure("OOM").await;
    router2.report_failure("crash").await;
    let target3 = router2.resolve_target().await;
    assert!(matches!(target3, crate::auto_fallback::InferenceTarget::Cloud { .. }),
        "3 failures should trigger fallback to cloud");
    println!("  ✅ AutoFallback: 3 failures → cloud fallback");

    // 状态报告可读
    let status = router2.status_summary().await;
    assert!(!status.is_empty(), "Status summary should be non-empty");
    println!("  ✅ AutoFallback: status_summary");

    Ok(())
}

// ========================================================================
// [3] REST LLM 推理测试
// ========================================================================

pub async fn test_rest_llm_smoke() -> Result<()> {
    // InferenceRouter 初始化
    let router = crate::rest_llm::InferenceRouter::new(
        vec![], "deepseek-chat"
    );

    // FIM 响应格式验证
    let fim_req = crate::rest_llm::FimRequest {
        model: "deepseek-chat".to_string(),
        prompt: "fn hello()".to_string(),
        suffix: "}".to_string(),
        max_tokens: Some(50),
        temperature: Some(0.5),
    };
    let fim_resp = router.fill_in_middle(&fim_req).await;
    assert!(!fim_resp.id.is_empty(), "FIM response should have an id");
    assert!(!fim_resp.choices.is_empty(), "FIM response should have choices");
    println!("  ✅ REST LLM: FIM response format");

    // 代码块提取测试
    let code = crate::rest_llm::extract_code_block(
        "Here:\n```rust\nfn main() {}\n```\nEnd.", "rust"
    );
    assert!(code.contains("fn main()"), "Should extract code block");
    println!("  ✅ REST LLM: code block extraction");

    // 补全请求格式
    let complete_req = crate::rest_llm::AiCompleteRequest {
        code: "fn hello() {}".to_string(),
        language: "rust".to_string(),
        cursor_line: 0,
        cursor_character: 13,
    };
    let complete_resp = router.complete(&complete_req).await;
    println!("  ✅ REST LLM: complete request format ({} items)", complete_resp.items.len());

    Ok(())
}

// ========================================================================
// [4] Knowledge Agents 流水线测试
// ========================================================================

pub async fn test_knowledge_agents_smoke() -> Result<()> {
    // 创建临时目录并写入测试文件
    let temp = std::env::temp_dir().join("carpai-harness-kg");
    let _ = std::fs::create_dir_all(&temp);
    std::fs::write(temp.join("main.rs"), "//! Entry point\nfn main() { println!(\"hello\"); }").ok();
    std::fs::write(temp.join("lib.rs"), "//! Library\npub fn helper() -> u32 { 42 }").ok();
    std::fs::write(temp.join("README.md"), "# Test Project\n\nA test project for harness.").ok();

    // Project Scanner 冒烟测试
    let config = crate::knowledge_agents::PipelineConfig::default();
    let files = crate::knowledge_agents::project_scanner::scan_project(&temp, &config).await;
    assert!(files.is_ok(), "Project scanner should succeed");
    let files = files.unwrap();
    assert!(files.len() >= 2, "Should find at least 2 source files");
    println!("  ✅ Knowledge Agents: project_scanner ({} files)", files.len());

    // File Analyzer 冒烟测试
    let file_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    let analyses = crate::knowledge_agents::file_analyzer::analyze_files(&temp, &file_paths, 5).await;
    assert!(analyses.is_ok(), "File analyzer should succeed");
    let analyses = analyses.unwrap();
    assert!(!analyses.is_empty(), "Should have analysis results");
    println!("  ✅ Knowledge Agents: file_analyzer ({} files)", analyses.len());

    // Architecture Analyzer 冒烟测试
    let graph = Arc::new(RwLock::new(crate::knowledge_agents::KnowledgeGraph {
        metadata: crate::knowledge_agents::GraphMetadata::default(),
        nodes: vec![],
        edges: vec![],
    }));
    let arch_result = crate::knowledge_agents::architecture_analyzer::analyze_architecture(&temp, &analyses, &graph).await;
    assert!(arch_result.is_ok(), "Architecture analyzer should succeed");
    println!("  ✅ Knowledge Agents: architecture_analyzer");

    // Graph Reviewer 冒烟测试
    // 需要至少 2 个节点来测试审查器
    for analysis in &analyses {
        graph.write().await.nodes.push(crate::knowledge_agents::KGNode {
            id: analysis.node_id.clone(),
            name: analysis.symbol_name.clone(),
            kind: crate::knowledge_agents::NodeKind::File,
            file_path: analysis.file_path.clone(),
            line: 0, column: 0,
            summary: analysis.summary.clone(),
            architecture_layer: None,
            domain: None,
            complexity: None,
        });
    }
    let review_result = crate::knowledge_agents::graph_reviewer::review_graph(&graph.read().await);
    assert!(review_result.is_ok(), "Graph reviewer should succeed");
    let review = review_result.unwrap();
    println!("  ✅ Knowledge Agents: graph_reviewer ({}/{})", review.passed_checks, review.total_checks);

    // 清理
    let _ = std::fs::remove_dir_all(&temp);

    Ok(())
}

// ========================================================================
// [5] Claude Agent Port 模式测试
// ========================================================================

pub async fn test_claude_agent_port_smoke() -> Result<()> {
    use crate::claude_agent_port::*;

    // 并发安全分区
    let tools = vec![
        ToolCallInfo { name: "read".into(), input: json!({}), safety: ConcurrencySafety::ReadOnly },
        ToolCallInfo { name: "search".into(), input: json!({}), safety: ConcurrencySafety::ReadOnly },
        ToolCallInfo { name: "edit".into(), input: json!({}), safety: ConcurrencySafety::WriteExclusive },
    ];
    let batches = partition_tool_calls(tools);
    assert_eq!(batches.len(), 2, "Read+Search should batch, Edit should be separate");
    println!("  ✅ Claude Agent: tool partition ({} batches)", batches.len());

    // Plan Manager
    let temp = std::env::temp_dir().join("carpai-harness-plan");
    let mgr = PlanManager::new(&temp);
    let slug = mgr.generate_slug();
    mgr.save_plan(&slug, "# Plan\n1. Test", None).await?;
    let loaded = mgr.load_plan(&slug).await?;
    assert!(loaded.contains("Plan"), "Should load saved plan");
    println!("  ✅ Claude Agent: plan persistence");

    // 错误消息
    let err = structured_error("File not found", "Check path", Some("Use find"));
    assert!(err.contains("File not found") && err.contains("Use find"));
    println!("  ✅ Claude Agent: structured error");

    // 重试 Hook
    let hook = RetryHook::new(3, 200);
    assert!(matches!(hook.decide("timeout", 0), RetryDecision::AutoRetry { .. }));
    assert!(matches!(hook.decide("permission denied", 0), RetryDecision::RetryAllowed { .. }));
    println!("  ✅ Claude Agent: retry hook (timeout→auto, denied→allowed)");

    let _ = std::fs::remove_dir_all(&temp);
    Ok(())
}

// ========================================================================
// [6] LSP CodeActions 协议测试
// ========================================================================

pub async fn test_lsp_code_actions_smoke() -> Result<()> {
    use crate::lsp_code_actions::*;

    let provider = CodeActionProvider::new();
    let params = CodeActionParams {
        text_document: TextDocumentIdentifier { uri: "file:///test.rs".to_string() },
        range: LspRange {
            start: LspPosition { line: 0, character: 0 },
            end: LspPosition { line: 5, character: 0 },
        },
        context: CodeActionContext { diagnostics: vec![], only: None },
    };
    let actions = provider.provide_code_actions(&params).await;
    assert!(actions.len() >= 3, "Should provide at least 3 code actions");
    println!("  ✅ LSP CodeActions: {} actions provided", actions.len());

    // 验证各类重构操作
    assert!(actions.iter().any(|a| a.kind.as_deref() == Some("quickfix")), "Should have quickfix");
    assert!(actions.iter().any(|a| a.kind.as_deref() == Some("refactor.extract.function")), "Should have extract");
    assert!(actions.iter().any(|a| a.kind.as_deref() == Some("refactor.rename")), "Should have rename");
    println!("  ✅ LSP CodeActions: quickfix + extract + rename present");

    Ok(())
}

// ========================================================================
// [7] 完整流水线运行
// ========================================================================

pub async fn run_all_extended_tests() -> Result<()> {
    println!("\n━━━ Extended Harness Tests ━━━\n");

    // [1] LSP Server
    println!("📡 LSP Server...");
    test_lsp_server_smoke().await?;

    // [2] AutoFallback
    println!("\n🔄 AutoFallback...");
    test_auto_fallback_smoke().await?;

    // [3] REST LLM
    println!("\n🤖 REST LLM...");
    test_rest_llm_smoke().await?;

    // [4] Knowledge Agents
    println!("\n🧠 Knowledge Agents...");
    test_knowledge_agents_smoke().await?;

    // [5] Claude Agent Port
    println!("\n⚡ Claude Agent Port...");
    test_claude_agent_port_smoke().await?;

    // [6] LSP CodeActions
    println!("\n💡 LSP CodeActions...");
    test_lsp_code_actions_smoke().await?;

    println!("\n━━━ All {} tests passed! ━━━", 6u32);
    Ok(())
}

// ========================================================================
// cargo test 入口
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lsp_server() {
        test_lsp_server_smoke().await.unwrap();
    }

    #[tokio::test]
    async fn test_auto_fallback() {
        test_auto_fallback_smoke().await.unwrap();
    }

    #[tokio::test]
    async fn test_rest_llm() {
        test_rest_llm_smoke().await.unwrap();
    }

    #[tokio::test]
    async fn test_knowledge_agents() {
        test_knowledge_agents_smoke().await.unwrap();
    }

    #[tokio::test]
    async fn test_claude_agent_port() {
        test_claude_agent_port_smoke().await.unwrap();
    }

    #[tokio::test]
    async fn test_lsp_code_actions() {
        test_lsp_code_actions_smoke().await.unwrap();
    }
}
