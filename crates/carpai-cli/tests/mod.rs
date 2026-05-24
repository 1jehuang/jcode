//! CarpAI CLI — 集成测试套件
//!
//! ## 测试范围
//!
//! | 测试文件 | 覆盖范围 |
//! |---------|---------|
//! | `config_test.rs` | CliConfig 加载/解析/环境变量覆盖 |
//! | `bridge_test.rs` | AgentBridge 双模式 + 重试 + 优雅降级 |
//! | `ambient_test.rs` | BackgroundRunner + TaskScheduler |
//! | `notifications_test.rs` | BrowserOpener + TelegramNotifier + GmailNotifier |
//! | `e2e_test.rs` | 端到端: CLI (local) → core execute_agent_turn |

pub mod config_test;
pub mod ambient_test;
pub mod bridge_test;
pub mod notifications_test;
// pub mod e2e_test; // 需要 carpai-core 编译通过后启用
