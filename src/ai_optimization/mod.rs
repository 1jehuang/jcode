//! # AI Self-Optimization Engine - AI自优化引擎
//!
//! 提供基于使用数据的智能自我改进能力，包括：
//! - **数据收集** - 用户行为和性能指标采集
//! - **模式识别** - 机器学习驱动的行为分析
//! - **自动调优** - 参数自适应优化
//! - **A/B测试** - 特性实验框架
//! - **反馈循环** - 持续学习改进
//!
//! ## 优化维度
//!
//! 1. **性能优化** - 响应时间、吞吐量、资源利用率
//! 2. **体验优化** - UI交互、命令推荐、错误处理
//! 3. **准确性优化** - 代码生成质量、任务完成率
//! 4. **安全性优化** - 风险检测、自动审批策略

pub mod collector;
pub mod analyzer;
pub mod optimizer;
pub mod ab_test;
pub mod feedback;

pub use collector::DataCollector;
pub use analyzer::BehaviorAnalyzer;
pub use optimizer::AutoOptimizer;
pub use ab_test::ABTestFramework;
pub use feedback::FeedbackLoop;