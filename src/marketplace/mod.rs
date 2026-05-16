//! # Plugin Marketplace - 插件市场
//!
//! 提供完整的在线插件分发和管理能力，包括：
//! - **插件注册** - 开发者发布插件
//! - **插件发现** - 搜索、分类、推荐
//! - **自动安装** - 一键安装依赖
//! - **版本管理** - 更新和回滚
//! - **评价系统** - 用户评分和评论
//!
//! ## 架构设计
//!
//! ```
//! Plugin Marketplace
//! +-- Registry (注册中心)
//! |   +-- Plugin Metadata Store
//! |   +-- Version History
//! |   +-- User Reviews
//! +-- Client (客户端)
//! |   +-- Search & Browse
//! |   +-- Install/Uninstall
//! |   +-- Update Checker
//! +-- API Server
//!     +-- RESTful Endpoints
//!     +-- Authentication
//! ```

pub mod registry;
pub mod client;
pub mod types;
pub mod api;

pub use registry::MarketplaceRegistry;
pub use client::MarketplaceClient;
pub use types::{MarketplacePlugin, PluginVersion, Review, Category};
pub use api::MarketplaceApi;