//! # Auto Mode - 智能自动模式引擎
//!
//! 提供基于机器学习的智能决策系统，支持：
//! - **置信度评估** - 基于历史数据计算操作安全性
//! - **模式学习** - 记录用户决策，动态调整策略
//! - **敏感词检测** - 自动识别危险操作
//! - **安全白名单** - 低风险操作自动批准
//! - **统计监控** - 追踪自动/手动决策比例
//!
//! ## 决策流程
//!
//! ```
//! 用户请求 → should_auto_approve()
//!     │
//!     ├─ 模式未启用 → ManualReview (完全人工)
//!     │
//!     ├─ 包含敏感词 → RequiresConfirmation (必须确认)
//!     │   └─ delete/rm/force/push/deploy
//!     │
//!     ├─ 匹配学习模式
//!     │   ├─ 置信度 ≥ 阈值 → AutoApprove (自动批准)
//!     │   └─ 置信度 < 阈值 → SuggestApprove (建议但需审核)
//!     │
//!     └─ 安全操作 + auto_accept_safe → AutoApprove
//!         └─ FileEdit / FileCreate
//! ```
//!
//! ## 配置示例
//!
//! ```rust,no_run
//! use carpai::auto_mode::{AutoModeConfig, AutoModeEngine};
//!
//! let config = AutoModeConfig {
//!     enabled: true,
//!     approval_threshold: 0.85,        // 85%置信度阈值
//!     auto_accept_safe: true,          // 自动接受安全操作
//!     max_auto_actions: 50,            // 最大自动操作数
//!     require_confirmation_for: vec![
//!         "delete".to_string(),
//!         "rm".to_string(),
//!         "deploy".to_string(),
//!     ],
//!     ..Default::default()
//! };
//!
//! let mut engine = AutoModeEngine::new(config);
//!
//! // 决策示例
//! match engine.should_auto_approve(&ActionType::FileEdit, "update config") {
//!     AutoApprovalDecision::AutoApprove(reason) => {
//!         println!("✅ 自动批准: {}", reason);
//!     }
//!     AutoApprovalDecision::RequiresConfirmation(msg) => {
//!         println!("⚠️  需要确认: {}", msg);
//!     }
//!     AutoApprovalDecision::ManualReview => {
//!         println!("👤 完全人工审核");
//!     }
//! }
//! ```

use serde::{Deserialize, Serialize};