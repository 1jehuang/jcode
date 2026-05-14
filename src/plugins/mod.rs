//! # Plugin System - 插件系统
//!
//! 提供完整的插件生命周期管理能力，包括：
//! - **插件注册表** (PluginRegistry) - 跟踪已安装的插件
//! - **插件加载器** (PluginLoader) - 加载和验证插件清单
//! - **插件管理器** (PluginManager) - 高级API封装
//! - **CLI命令** (PluginCommand) - 命令行接口
//!
//! ## 快速开始
//!
//! ```rust,no_run
//! use carpai::plugins::{PluginManager, PluginCommand};
//!
//! // 创建管理器
//! let mut manager = PluginManager::new(".carpai/plugins");
//!
//! // 安装插件
//! manager.add(std::path::Path::new("my-plugin")).unwrap();
//!
//! // 列出插件
//! for plugin in manager.list() {
//!     println!("{} v{}", plugin.manifest.name, plugin.manifest.version);
//! }
//! ```
//!
//! ## 插件清单格式 (plugin.json)
//!
//! ```json
//! {
//!   "name": "my-plugin",
//!   "version": "1.0.0",
//!   "description": "Plugin description",
//!   "entry_point": "lib.rs",
//!   "capabilities": ["Commands", "Tools"],
//!   "dependencies": []
//! }
//! ```
//!
//! ## 架构设计
//!
//! ```
//! PluginManager (高级API)
//!     └── PluginRegistry (数据存储)
//!           ├── plugins: HashMap<String, InstalledPlugin>
//!           └── plugins_dir: PathBuf
//!
//! PluginLoader (工具函数)
//!     ├── load_from_manifest() - 从JSON加载
//!     ├── validate_manifest() - 验证字段
//!     ├── install_from_local() - 本地安装
//!     └── uninstall() - 卸载清理
//! ```

pub mod types;
pub mod registry;
pub mod loader;
pub mod manager;
pub mod command;

#[cfg(test)]
mod tests;

pub use types::{PluginManifest, PluginCapability, InstalledPlugin};
pub use registry::PluginRegistry;
pub use loader::PluginLoader;
pub use manager::PluginManager;
pub use command::PluginCommand;