//! # 迁移指南：从 Arc<RwLock<T>> 到 Shared<T>
//! 
//! 本指南展示如何将项目中的 Arc<RwLock<>> 替换为 Shared<T>。
//! 
//! ## 迁移步骤
//! 
//! ### 步骤 1：添加依赖
//! 
//! 在 Cargo.toml 中添加：
//! ```toml
//! jcode-lock-manager = { path = "crates/jcode-lock-manager" }
//! ```
//! 
//! ### 步骤 2：替换类型定义
//! 
//! #### 旧代码：
//! ```rust
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//! 
//! struct Registry {
//!     tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
//!     skills: Arc<RwLock<SkillRegistry>>,
//! }
//! ```
//! 
//! #### 新代码：
//! ```rust
//! use jcode_lock_manager::Shared;
//! 
//! struct Registry {
//!     tools: Shared<HashMap<String, Arc<dyn Tool>>>,
//!     skills: Shared<SkillRegistry>,
//! }
//! ```
//! 
//! ### 步骤 3：替换初始化代码
//! 
//! #### 旧代码：
//! ```rust
//! Self {
//!     tools: Arc::new(RwLock::new(HashMap::new())),
//!     skills: Arc::new(RwLock::new(SkillRegistry::default())),
//! }
//! ```
//! 
//! #### 新代码：
//! ```rust
//! Self {
//!     tools: Shared::with_name(HashMap::new(), "tool_registry"),
//!     skills: Shared::with_name(SkillRegistry::default(), "skill_registry"),
//! }
//! ```
//! 
//! ### 步骤 4：替换方法返回类型
//! 
//! #### 旧代码：
//! ```rust
//! pub fn skills(&self) -> Arc<RwLock<SkillRegistry>> {
//!     self.skills.clone()
//! }
//! ```
//! 
//! #### 新代码：
//! ```rust
//! pub fn skills(&self) -> Shared<SkillRegistry> {
//!     self.skills.clone()
//! }
//! ```
//! 
//! ### 步骤 5：使用方式保持不变
//! 
//! ```rust
//! // 读取操作（完全相同）
//! let guard = self.tools.read().await;
//! let tool = guard.get("tool_name");
//! 
//! // 写入操作（完全相同）
//! let mut guard = self.tools.write().await;
//! guard.insert("tool_name", tool);
//! ```
//! 
//! ## 迁移优势
//! 
//! 1. **自动化注册**：所有 Shared<T> 自动注册到全局 LockManager
//! 2. **智能监控**：自动检测锁竞争并发出警告
//! 3. **统计分析**：实时获取锁使用统计报告
//! 4. **类型安全**：完全兼容原有 API，无需修改使用代码
//! 
//! ## 启用监控
//! 
//! ```rust
//! // 在应用启动时开启监控
//! tokio::spawn(async {
//!     loop {
//!         let report = LockManager::generate_report().await;
//!         println!("{}", report);
//!         tokio::time::sleep(Duration::from_minutes(5)).await;
//!     }
//! });
//! ```
//! 
//! ## 迁移检查清单
//! 
//! - [ ] 添加 jcode-lock-manager 依赖
//! - [ ] 替换 Arc<RwLock<T>> 为 Shared<T>
//! - [ ] 更新初始化代码使用 Shared::new() 或 Shared::with_name()
//! - [ ] 更新方法返回类型
//! - [ ] 启用锁监控
//! - [ ] 定期检查竞争报告并优化热点
