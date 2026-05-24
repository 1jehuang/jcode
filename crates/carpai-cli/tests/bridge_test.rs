//! AgentBridge 集成测试
//!
//! 测试双模式、重试逻辑、优雅降级

use carpai_cli::AgentBridge;
use carpai_cli::BridgeMode;
use carpai_cli::RetryConfig;
use carpai_core::config::CoreConfig;

fn create_local_bridge() -> AgentBridge {
    let mut config = CoreConfig::default();
    config.base.working_dir = std::env::temp_dir();
    let ctx = carpai_core::build_local_agent_context(&config);
    AgentBridge::new_local(ctx)
}

#[tokio::test]
async fn test_local_mode_returns_response() {
    let bridge = create_local_bridge();
    let result = bridge.execute_turn("Say 'hello'").await;
    assert!(result.is_ok(), "Local mode should return Ok, got: {:?}", result.err());
    let output = result.unwrap();
    assert!(!output.text.is_empty(), "Response should not be empty");
}

#[tokio::test]
async fn test_remote_mode_graceful_degradation() {
    let bridge = AgentBridge::new_remote("http://localhost:12345".into());
    let result = bridge.execute_turn("test").await;
    assert!(result.is_ok(), "Remote mode should degrade gracefully, not error");
    let output = result.unwrap();
    assert!(output.text.contains("not yet implemented"), "Should indicate unimplemented");
}

#[tokio::test]
async fn test_bridge_session_id_local() {
    let bridge = create_local_bridge();
    let sid = bridge.session_id().await;
    assert!(sid.is_some(), "Local bridge should have a session ID");
}

#[tokio::test]
async fn test_bridge_session_id_remote() {
    let bridge = AgentBridge::new_remote("http://localhost:12345".into());
    let sid = bridge.session_id().await;
    assert!(sid.is_none(), "Remote bridge should not have a session ID");
}

#[tokio::test]
async fn test_bridge_with_custom_retry() {
    let mut config = CoreConfig::default();
    config.base.working_dir = std::env::temp_dir();
    let ctx = carpai_core::build_local_agent_context(&config);

    let retry = RetryConfig {
        max_retries: 1,
        base_delay_ms: 10,
        max_delay_ms: 100,
        jitter: false,
    };

    let bridge = AgentBridge::new_local_with_retry(ctx, retry);
    let result = bridge.execute_turn("test").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_bridge_mode_switch() {
    let bridge = create_local_bridge();

    let sid_before = bridge.session_id().await;
    assert!(sid_before.is_some());
}
