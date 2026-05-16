//! # CarpAI Enterprise Server — 主入口
//!
//! 运行企业级 AI 服务版。
//!
//! ## 启动方式
//!
//! ```bash
//! # 默认配置启动（使用 SQLite）
//! cargo run --bin carpai-enterprise-server
//!
//! # 指定配置文件
//! CARPAI_CONFIG=./config/enterprise.toml cargo run --bin carpai-enterprise-server
//!
//! # 自定义端口
//! CARPAI_API_PORT=8000 CARPAI_ADMIN_PORT=8001 cargo run --bin carpai-enterprise-server
//! ```
//!
//! ## 环境变量
//!
//! - `CARPAI_CONFIG` — 配置文件路径
//! - `CARPAI_API_PORT` — API 端口（默认 8000）
//! - `CARPAI_ADMIN_PORT` — Admin 端口（默认 8001）
//! - `CARPAI_DATABASE_URL` — 数据库连接 URL
//! - `CARPAI_LOG_LEVEL` — 日志级别 (trace/debug/info/warn/error)
//! - `CARPAI_MODEL_DIR` — 模型文件目录
//! - `CARPAI_JWT_SECRET` — JWT 签名密钥
//! - `CARPAI_LLAMACPP_PATH` — llama-server 可执行文件路径

use jcode_enterprise_server::enterprise::EnterpriseServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = std::env::var("CARPAI_CONFIG").ok();

    let server = EnterpriseServer::new(config_path).await?;
    server.serve().await?;

    Ok(())
}
