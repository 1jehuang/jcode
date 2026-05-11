//! RAG 工具链闭环系统 - 完整使用示例
//!
//! 本示例演示如何使用五层防御体系完成一次完整的代码修改手术
//!
//! ## 使用场景
//!
//! 假设我们有一个超大型项目 (30万行代码)，需要：
//! 1. 找到所有处理用户认证的函数
//! 2. 在认证逻辑中添加日志记录
//! 3. 确保修改不会破坏现有功能
//! 4. 添加调试断点以便后续观察

use std::path::PathBuf;
use std::sync::Arc;
use jcode_rag::{
    // 核心编排器
    RagToolchainOrchestrator, OrchestratorConfig, SurgicalRequest, TargetScope,
    Priority, SafetyMode,
    
    // Layer 1: 感知层
    GlobalSymbolIndexer, IndexingConfig,
    
    // Layer 2: 检索层
    MultiEngineRetriever, RetrievalConfig,
    
    // Layer 3: 编辑层
    SafeEditor, EditingConfig,
    
    // Layer 4: 验证层
    MultiLanguageValidator, ValidationConfig,
    
    // Layer 5: 调试层
    ObservabilityManager, DebuggingConfig,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🏥 RAG Toolchain Closed-Loop System Demo");
    println!("=".repeat(60));

    // ============== 第1步: 初始化各层 ==============
    
    println!("\n📋 Step 1: Initializing all layers...");

    // Layer 1: 感知层 - 全局符号索引
    let indexer = Arc::new(GlobalSymbolIndexer::new(IndexingConfig {
        project_root: PathBuf::from("."), // 当前项目
        concurrency_limit: 4,
        enable_lsp_indexing: true,
        enable_ctags_indexing: true,
        enable_dependency_analysis: true,
        ..Default::default()
    }));

    println!("  ✅ Indexing layer initialized");

    // Layer 2: 检索层 - 多引擎融合检索
    let retriever = Arc::new(MultiEngineRetriever::new(
        RetrievalConfig::default(),
        indexer.clone(),
        // TODO: 注入实际的字符串搜索实现
        Arc::new(DummyStringSearcher),
    ));

    println!("  ✅ Retrieval layer initialized");

    // Layer 3: 编辑层 - 安全编辑器
    let editor = Arc::new(SafeEditor::new(EditingConfig {
        auto_backup: true,
        backup_dir: PathBuf::from(".jcode/backups"),
        enable_conflict_detection: true,
        conflict_resolution_strategy: jcode_rag::editing_layer::ConflictStrategy::Abort,
        ..Default::default()
    }));

    println!("  ✅ Editing layer initialized");

    // Layer 4: 验证层 - 多语言验证器
    let validator = Arc::new(MultiLanguageValidator::new(ValidationConfig {
        enable_compilation_check: true,
        enable_test_execution: false, // 演示中不实际运行测试
        enable_linting: true,
        error_handling: jcode_rag::validation_layer::ErrorHandlingPolicy::CollectAll,
        ..Default::default()
    }));

    println!("  ✅ Validation layer initialized");

    // Layer 5: 调试层 - 可观测性管理器
    let debugger = Arc::new(ObservabilityManager::new(DebuggingConfig {
        enable_log_injection: true,
        enable_breakpoint_management: true,
        enable_execution_tracing: true,
        default_log_level: jcode_rag::LogLevel::Debug,
        max_injections_per_file: 10,
        ..Default::default()
    }));

    println!("  ✅ Debugging layer initialized");

    // ============== 第2步: 构建索引 ==============
    
    println!("\n🔍 Step 2: Building global index...");
    
    let index_stats = indexer.build_full_index().await?;
    
    println!(
        "  📊 Index built: {} symbols in {} files",
        index_stats.total_symbols,
        index_stats.total_files
    );
    println!(
        "  🌐 Languages detected: {:?}",
        index_stats.languages_detected
    );

    // ============== 第3步: 创建手术请求 ==============
    
    println!("\n🎯 Step 3: Creating surgical request...");
    
    let request = SurgicalRequest {
        request_id: "demo_001".to_string(),
        intent: "Add logging to all authentication functions to track user login attempts".to_string(),
        target: TargetScope::EntireProject { 
            root: PathBuf::from(".") 
        },
        priority: Priority::High,
        safety_mode: SafetyMode::Safe, // 安全模式，需要确认
        created_at: chrono::Utc::now(),
        requested_by: "demo_user".to_string(),
    };

    println!("  📝 Request created:");
    println!("     Intent: {}", request.intent);
    println!("     Scope: Entire Project");
    println!("     Safety Mode: {:?}", request.safety_mode);

    // ============== 第4步: 创建编排器并执行手术 ==============
    
    println!("\n⚙️  Step 4: Creating orchestrator and executing surgery...");
    
    let orchestrator = RagToolchainOrchestrator::new(
        OrchestratorConfig {
            max_context_window_tokens: 8000,
            default_safety_mode: SafetyMode::Safe,
            auto_commit: false, // 不自动提交，需人工审核
            ..Default::default()
        },
        indexer.clone(),       // Layer 1
        retriever.clone(),     // Layer 2
        editor.clone(),        // Layer 3
        validator.clone(),     // Layer 4
        debugger.clone(),      // Layer 5
    );

    println!("  ✅ Orchestrator created");
    println!("  🔪 Starting surgical procedure...");
    println!();

    // 执行手术
    match orchestrator.execute_surgery(&request).await {
        Ok(result) => {
            print_surgical_result(&result);
        }
        Err(e) => {
            eprintln!("❌ Surgery failed with error: {}", e);
        }
    }

    println!("\n🎉 Demo completed!");

    Ok(())
}

/// 打印手术结果详情
fn print_surgical_result(result: &jcode_rag::SurgicalResult) {
    println!("╔══════════════════════════════════════════════════════╗");
    println!("║           SURGICAL RESULT REPORT                    ║");
    println!("╠══════════════════════════════════════════════════════╣");
    
    println!("║ Request ID: {:^44} ║", result.request_id);
    println!("║ Status: {:<6} {:^38} ║", 
        if result.success { "✅ PASS" } else { "❌ FAIL" },
        ""
    );
    
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║ EXECUTION PHASES:                                   ║");
    println!("┌──────────────────────────────────────────────────────┐");
    
    for (i, phase) in result.phases.iter().enumerate() {
        let phase_name = format!("{:?}", phase.phase);
        let status = if phase.passed { "✅" } else { "❌" };
        
        println!("│ Phase {}: {:12} | {} | {:>6}ms | {:>3} warnings │",
            i + 1,
            phase_name,
            status,
            phase.duration_ms,
            phase.warnings.len()
        );
        
        // 显示关键信息
        match &phase.output {
            jcode_rag::PhaseOutput::IndexingOutput { symbols_found, files_indexed, .. } => {
                println!("│         └─ Symbols: {}, Files: {}", symbols_found, files_indexed);
            }
            jcode_rag::PhaseOutput::RetrievalOutput { context_windows, .. } => {
                for ctx in context_windows {
                    println!("│         └─ Context: {} segments, {} tokens", 
                        ctx.segments.len(),
                        ctx.total_tokens
                    );
                }
            }
            jcode_rag::PhaseOutput::EditingOutput { diffs_generated, files_modified, .. } => {
                println!("│         └─ Diffs: {}, Files: {}", 
                    diffs_generated.len(),
                    files_modified.len()
                );
            }
            jcode_rag::PhaseOutput::ValidationOutput { compilation_results, test_results, .. } => {
                let errors = compilation_results.iter().filter(|r| !r.success).count();
                println!("│         └─ Compilation errors: {}, Tests: {}", 
                    errors,
                    test_results.len()
                );
            }
            jcode_rag::PhaseOutput::DebuggingOutput { logs_injected, breakpoints_set, traces_captured, .. } => {
                println!("│         └─ Logs: {}, Breakpoints: {}, Traces: {}", 
                    logs_injected.len(),
                    breakpoints_set.len(),
                    traces_captured.len()
                );
            }
        }
    }
    
    println!("└──────────────────────────────────────────────────────┘");
    
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║ STATISTICS:                                          ║");
    println!("║   Total Duration: {:>8}ms                          ║", result.stats.total_duration_ms);
    println!("║   File I/O Ops:   {:>8}                            ║", result.stats.file_io_operations);
    println!("║   Process Launch: {:>8}                            ║", result.stats.process_launches);
    
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║ IMPACT ANALYSIS:                                     ║");
    println!("║   Risk Level: {:<42} ║", 
        format!("{:?}", result.impact_analysis.risk_level)
    );
    println!("║   Directly Affected: {:>3} files                     ║", 
        result.impact_analysis.directly_affected_files.len()
    );
    println!("║   Suggested Tests: {:>3}                             ║", 
        result.impact_analysis.suggested_regression_tests.len()
    );
    
    println!("╚══════════════════════════════════════════════════════╝");
}

// ============== Dummy 实现 (用于演示) ==============

/// 字符串搜索器的虚拟实现
struct DummyStringSearcher;

#[async_trait::async_trait]
impl jcode_rag::retrieval_layer::StringSearchProvider for DummyStringSearcher {
    async fn search(
        &self, 
        _pattern: &str, 
        _options: &jcode_rag::retrieval_layer::GrepConfig
    ) -> Result<Vec<jcode_rag::retrieval_layer::RawSearchResult>, anyhow::Error> {
        // 返回空结果用于演示
        Ok(Vec::new())
    }
}
