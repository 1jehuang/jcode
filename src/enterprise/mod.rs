//! # CarpAI Server 基础设施模块
//!
//! 多租户/分布式推理/节点发现/管理API等服务器核心能力。
//! 所有的 CarpAI Server 启动都会加载这些模块。

pub mod config;
pub mod usage;
pub mod discovery;
pub mod priority;
pub mod quota;
pub mod metrics;
pub mod audit;
pub mod compliance;
pub mod gdpr;
pub mod cross_region;
pub mod model_quant;
pub mod virtual_memory;
pub mod kv_cache_storage;
pub mod cpu_inference;
pub mod milvus_adapter;

// 以下模块依赖可选的 enterprise feature crate
#[cfg(feature = "enterprise")]
pub mod cache;
#[cfg(feature = "enterprise")]
pub mod auth;
#[cfg(feature = "enterprise")]
pub mod db;
#[cfg(feature = "enterprise")]
pub mod distributed;
#[cfg(feature = "enterprise")]
pub mod hipaa;
#[cfg(feature = "enterprise")]
pub mod admin_api;
#[cfg(feature = "enterprise")]
pub mod enterprise;
