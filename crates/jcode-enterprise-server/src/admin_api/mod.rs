//! ## 任务 3.1 + 3.2: 企业管理 REST API
//!
//! 提供 OpenAI 兼容 API + 企业管理后台 API 路由

pub mod openai_routes;
pub mod admin_routes;
pub mod auth_middleware;
pub mod api_models;

pub use openai_routes::*;
pub use admin_routes::*;
pub use auth_middleware::*;
