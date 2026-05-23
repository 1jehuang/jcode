//! # CarpAI Enterprise Server
//!
//! 企业级 AI 服务版，专为"低投入 + 异构闲置硬件部署本地大模型"场景设计。
//!
//! ## 架构概览
//!
//! ```text
//! +---------------------------------------------------------+
//! |              CarpAI Enterprise Server                    |
//! |                                                          |
//! |  +--------------------------------------------------+   |
//! |  |              Admin API (Axum)                     |   |
//! |  |  POST /v1/chat/completions   (OpenAI 兼容)        |   |
//! |  |  POST /v1/embeddings                              |   |
//! |  |  GET  /v1/models                                   |   |
//! |  |  POST /admin/orgs           (组织管理)            |   |
//! |  |  POST /admin/users         (用户管理)            |   |
//! |  |  GET  /admin/usage          (用量统计)            |   |
//! |  |  GET  /admin/audit          (审计日志)            |   |
//! |  +----------+---------------------------------------+   |
//! |             |                                            |
//! |  +----------▼---------------------------------------+   |
//! |  |        UnifiedScheduler (Parallax + Ruflo)        |   |
//! |  |    - Model layer allocation (Parallax Phase 1)    |   |
//! |  |    - Request routing      (Parallax Phase 2)      |   |
//! |  |    - Task GOAP planning   (Ruflo)                 |   |
//! |  |    - Priority scheduling                          |   |
//! |  +----------+---------------------------------------+   |
//! |             |                                            |
//! |  +----------▼---------------------------------------+   |
//! |  |         Node Discovery & Heartbeat Manager        |   |
//! |  |    - mDNS auto-discovery                         |   |
//! |  |    - Heartbeat timeout detection                 |   |
//! |  |    - Dynamic node registration/unregistration    |   |
//! |  |    - Virtual memory capacity monitoring          |   |
//! |  +----------+---------------------------------------+   |
//! |             |                                            |
//! |  +----------▼---------------------------------------+   |
//! |  |         Enterprise Auth & Multi-Tenancy           |   |
//! |  |    - Organization management                     |   |
//! |  |    - API key authentication                       |   |
//! |  |    - Role-based access control (RBAC)            |   |
//! |  |    - Usage tracking & quota limits               |   |
//! |  |    - Audit logging                               |   |
//! |  +--------------------------------------------------+   |
//! +---------------------------------------------------------+
//! ```
//!
//! ## 核心技术优势（零额外硬件投入）
//!
//! 1. **低比特量化推理**: GGUF Q4_K_M/INT4 量化，128G内存台式机跑72B模型
//! 2. **纯CPU推理优化**: 无需GPU，利用llama.cpp的CPU推理能力
//! 3. **分布式推理**: 将大模型拆分到多台机器，解决单节点显存不足
//! 4. **动态节点调度**: 自动利用网吧闲置、员工下班等碎片化资源
//! 5. **虚拟内存推理**: 利用512G虚拟内存存储KV Cache
//! 6. **多租户管理**: 企业级权限、用量统计、审计日志

pub mod config;
pub mod model_quant;
pub mod cpu_inference;
pub mod distributed;
pub mod discovery;
pub mod priority;
pub mod virtual_memory;
pub mod admin_api;
pub mod auth;
pub mod db;
pub mod usage;
pub mod enterprise;
pub mod audit;
pub mod quota;
pub mod metrics;
pub mod kv_cache_storage;
pub mod milvus_adapter;
pub mod cache;
pub mod compliance;
pub mod gdpr;
pub mod hipaa;
pub mod cross_region;

pub use config::*;
pub use enterprise::EnterpriseServer;
