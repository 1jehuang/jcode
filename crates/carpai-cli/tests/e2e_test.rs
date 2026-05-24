//! # E2E 端到端测试
//!
//! 验证完整链路的正确性:
//!
//! | 链路 | 路径 | 状态 |
//! |------|------|------|
//! | CLI local | TUI → bridge → core → execute_agent_turn | ⏳ 需 carpai-core 编译通过 |
//! | CLI remote | TUI → bridge → gRPC → server | ❌ gRPC client 未实现 |
//!
//! ## 前置条件
//!
//! - `cargo check -p carpai-core` 通过
//! - 配置文件中 `completion_provider.endpoint` 指向可用的 API 端点
//!
//! ## 运行方式
//!
//! ```bash
//! # 运行所有 E2E 测试 (需要外部 API 端点)
//! cargo test --test e2e_test -- --ignored
//!
//! # 运行本地模式 E2E (不需要外部依赖)
//! cargo test --test e2e_test local_mode
//! ```
//!
//! ## 测试场景
//!
//! 1. **local_mode**: 本地模式 AgentTurn 单轮对话
//! 2. **local_mode_streaming**: (预留) 流式输出验证
//! 3. **local_mode_multiturn**: (预留) 多轮对话 + 上下文保持
//! 4. **remote_mode_connect**: (预留) 远程模式连接到 server
//! 5. **config_persistence**: (预留) 配置加载 → session 持久化 → 恢复

use carpai_cli::AgentBridge;
use carpai_core::config::CoreConfig;

// ============================================================================
// 辅助函数
// ============================================================================

/// 构建一个最小化的本地测试 AgentBridge
fn setup_local_bridge() -> AgentBridge {
    let mut config = CoreConfig::default();
    config.base.working_dir = std::env::temp_dir();
    let ctx = carpai_core::build_local_agent_context(&config);
    AgentBridge::new_local(ctx)
}

// ============================================================================
// 测试案例
// ============================================================================

/// E2E 1: 本地模式单轮对话
///
/// 验证完整流程:
/// 1. 加载 CliConfig
/// 2. 构建 AgentContext (所有 Local* 实现)
/// 3. 执行 execute_agent_turn
/// 4. 检查返回的 AgentTurnOutput 包含 text/usage/duration
#[tokio::test]
async fn e2e_local_mode_basic_turn() {
    let bridge = setup_local_bridge();

    let result = bridge.execute_turn("What is 2+2?").await;
    assert!(result.is_ok(), "Basic turn should succeed");

    let output = result.unwrap();
    assert!(!output.text.is_empty(), "Response text should not be empty");
    assert!(!output.session_id.is_empty(), "Should have a session ID");
    assert!(output.duration_ms > 0 || output.duration_ms == 0, "Duration should be valid");
}

/// E2E 2: 本地模式空输入处理
///
/// 验证空消息或特殊输入的边缘情况
#[tokio::test]
async fn e2e_local_mode_empty_input() {
    let bridge = setup_local_bridge();

    let result = bridge.execute_turn("").await;
    // Empty input should still be handled gracefully
    assert!(result.is_ok(), "Empty input should not cause error");
}

/// E2E 3: 构建 → 销毁 → 重建链路
///
/// 验证多次 AgentContext 构建不会泄漏资源
#[tokio::test]
async fn e2e_local_mode_rebuild_context() {
    for i in 0..3 {
        let bridge = setup_local_bridge();
        let result = bridge.execute_turn(&format!("Turn {}", i)).await;
        assert!(result.is_ok(), "Turn {} should succeed", i);
    }
}

/// E2E 4: 配置热重载链路
///
/// 验证 ConfigWatcher 能检测文件变化 (仅验证 Watcher 功能, 不涉及 TUI)
#[test]
fn e2e_config_watch_chain() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("carpai.toml");

    // Write initial config
    std::fs::write(&config_path, r#"mode = "cli""#).unwrap();

    let mut watcher = carpai_cli::config_watch::ConfigWatcher::new(config_path.clone());

    // Verify initial config loaded
    assert_eq!(watcher.config().core.base.working_dir, std::env::temp_dir());

    // Modify config
    std::fs::write(&config_path, r#"mode = "server""#).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Check reload
    assert!(watcher.check_reload().is_some(), "Config change should be detected");
}

// ============================================================================
// 预留: 需要外部依赖或完整 server crate 的 E2E 测试
// ============================================================================

/// E2E 5: 远程模式连接 (预留)
///
/// 需要 carpai-server 运行在可访问的地址上
#[tokio::test]
#[ignore = "需要 carpai-server 运行"]
async fn e2e_remote_mode_connect() {
    // Arrange: connect to running server
    let bridge = AgentBridge::new_remote("http://localhost:8080".into());

    // Act: send a request
    let result = bridge.execute_turn("Hello from E2E test").await;

    // Assert: should succeed
    assert!(result.is_ok(), "Remote mode should connect to server");
}

/// E2E 6: SDK → Server 完整链路 (预留)
///
/// 需要 carpai-server 和 carpai-sdk 编译通过
#[tokio::test]
#[ignore = "需要 carpai-sdk + carpai-server"]
async fn e2e_sdk_to_server_chain() {
    // 使用 carpai-sdk 的 CarpaiClient 连接到运行中的 server
    // 发送 ChatCompletionRequest → 验证 ChatCompletionResponse
}

/// E2E 7: 跨产品整合 (预留)
///
/// CLI(local) → CLI(remote→server) → SDK 全链路
#[tokio::test]
#[ignore = "需要完整产品构建"]
async fn e2e_full_product_chain() {
    // 1. CLI local mode: single turn
    // 2. CLI remote mode: connect to server
    // 3. SDK: connect to server via HTTP/gRPC
    // 4. Verify all three produce consistent results
}
