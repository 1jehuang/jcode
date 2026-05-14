//! # Version Manager - 版本管理系统
//!
//! 提供完整的版本控制和回滚能力，包括：
//! - **版本安装** - 自动创建回滚点
//! - **回滚管理** - 支持按ID/版本/latest回滚
//! - **变更日志** - 追踪版本演进历史
//! - **数据持久化** - JSON格式存储版本信息
//!
//! ## 核心概念
//!
//! ### VersionInfo (版本信息)
//! ```rust,no_run
//! pub struct VersionInfo {
//!     pub version: String,           // 语义化版本号 (1.2.3)
//!     pub build_date: DateTime<Utc>, // 构建时间
//!     pub commit_hash: Option<String>, // Git提交哈希
//!     pub changelog: Vec<String>,    // 变更列表
//! }
//! ```
//!
//! ### RollbackPoint (回滚点)
//! ```rust,no_run
//! pub struct RollbackPoint {
//!     pub id: String,                // 唯一标识 (rb-YYYYMMDD-HHMMSS)
//!     pub timestamp: DateTime<Utc>,  // 创建时间
//!     pub description: String,       // 回滚点描述
//!     pub version: String,           // 对应版本号
//!     pub backup_path: PathBuf,      // 备份路径
//! }
//! ```
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use carpai::version_manager::VersionManager;
//!
//! let mut vm = VersionManager::new(".carpai/versions");
//!
//! // 安装新版本（自动创建回滚点）
//! let result = vm.install_version("2.0.0", vec![
//!     "新增插件市场功能".to_string(),
//!     "优化性能30%".to_string(),
//!     "修复安全漏洞".to_string(),
//! ]);
//!
//! // 手动创建回滚点（重大变更前）
//! vm.create_rollback_point("数据库迁移前").ok();
//!
//! // 查看所有回滚点
//! println!("{}", vm.list_rollback_points());
//!
//! // 回滚到上一版本
//! let rollback_result = vm.rollback("latest");
//!
//! // 查看变更日志
//! println!("{}", vm.get_changelog(10));
//! ```

use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};