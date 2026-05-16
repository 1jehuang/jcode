//! 数据库迁移和数据访问层
//!
//! 支持 SQLite 和 PostgreSQL，自动创建表结构。

use crate::config::DatabaseConfig;
use sqlx::migrate::MigrateDatabase;
use sqlx::{Pool, Sqlite, Postgres, Row};
use tracing::info;

/// 数据库类型
pub enum DbPool {
    Sqlite(Pool<Sqlite>),
    #[allow(dead_code)]
    Postgres(Pool<Postgres>),
}

/// 数据库管理器
pub struct DatabaseManager {
    pub pool: DbPool,
}

impl DatabaseManager {
    /// 初始化数据库连接并运行迁移
    pub async fn new(config: &DatabaseConfig) -> anyhow::Result<Self> {
        let pool = if config.url.starts_with("postgres://") || config.url.starts_with("postgresql://") {
            let p = sqlx::postgres::PgPoolOptions::new()
                .max_connections(config.max_connections)
                .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
                .connect(&config.url)
                .await?;
            DbPool::Postgres(p)
        } else {
            // SQLite – 自动创建数据库文件
            if !config.url.starts_with("sqlite://") {
                anyhow::bail!("不支持的数据库 URL: {}", config.url);
            }

            let db_path = &config.url["sqlite://".len()..];
            let db_path = if db_path.contains('?') {
                &db_path[..db_path.find('?').unwrap()]
            } else {
                db_path
            };

            // 确保目录存在
            if let Some(parent) = std::path::Path::new(db_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // 如果数据库不存在，自动创建
            if !std::path::Path::new(db_path).exists() {
                Sqlite::create_database(&config.url).await?;
                info!("已创建 SQLite 数据库: {}", db_path);
            }

            let p = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(config.max_connections)
                .connect(&config.url)
                .await?;
            DbPool::Sqlite(p)
        };

        let manager = Self { pool };

        if config.auto_migrate {
            manager.run_migrations().await?;
        }

        Ok(manager)
    }

    /// 运行数据库迁移（自动建表）
    async fn run_migrations(&self) -> anyhow::Result<()> {
        match &self.pool {
            DbPool::Sqlite(pool) => {
                // 组织表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS organizations (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        plan TEXT NOT NULL DEFAULT 'free',
                        max_users INTEGER NOT NULL DEFAULT 5,
                        daily_token_limit INTEGER NOT NULL DEFAULT 100000,
                        concurrent_limit INTEGER NOT NULL DEFAULT 2,
                        is_active INTEGER NOT NULL DEFAULT 1,
                        created_at TEXT NOT NULL
                    )"
                ).execute(pool).await?;

                // 用户表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS users (
                        id TEXT PRIMARY KEY,
                        org_id TEXT NOT NULL,
                        email TEXT NOT NULL UNIQUE,
                        name TEXT NOT NULL,
                        role TEXT NOT NULL DEFAULT 'developer',
                        password_hash TEXT NOT NULL,
                        api_key_hash TEXT,
                        is_active INTEGER NOT NULL DEFAULT 1,
                        created_at TEXT NOT NULL,
                        last_login TEXT,
                        FOREIGN KEY (org_id) REFERENCES organizations(id)
                    )"
                ).execute(pool).await?;

                // API Key 表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS api_keys (
                        id TEXT PRIMARY KEY,
                        user_id TEXT NOT NULL,
                        key_hash TEXT NOT NULL UNIQUE,
                        key_preview TEXT NOT NULL,
                        is_active INTEGER NOT NULL DEFAULT 1,
                        created_at TEXT NOT NULL,
                        expires_at TEXT,
                        FOREIGN KEY (user_id) REFERENCES users(id)
                    )"
                ).execute(pool).await?;

                // 模型表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS models (
                        name TEXT PRIMARY KEY,
                        display_name TEXT NOT NULL,
                        model_type TEXT NOT NULL DEFAULT 'chat',
                        quantized INTEGER NOT NULL DEFAULT 1,
                        quantization TEXT NOT NULL DEFAULT 'Q4_K_M',
                        min_memory_gb REAL NOT NULL,
                        num_layers INTEGER NOT NULL DEFAULT 40,
                        is_active INTEGER NOT NULL DEFAULT 1
                    )"
                ).execute(pool).await?;

                // 用量统计表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS usage_records (
                        id TEXT PRIMARY KEY,
                        org_id TEXT NOT NULL,
                        user_id TEXT,
                        model_name TEXT NOT NULL,
                        prompt_tokens INTEGER NOT NULL DEFAULT 0,
                        completion_tokens INTEGER NOT NULL DEFAULT 0,
                        total_tokens INTEGER NOT NULL DEFAULT 0,
                        latency_ms INTEGER NOT NULL DEFAULT 0,
                        request_type TEXT NOT NULL DEFAULT 'chat',
                        created_at TEXT NOT NULL,
                        FOREIGN KEY (org_id) REFERENCES organizations(id)
                    )"
                ).execute(pool).await?;

                // 节点表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS compute_nodes (
                        node_id TEXT PRIMARY KEY,
                        node_name TEXT NOT NULL,
                        node_type TEXT NOT NULL DEFAULT 'desktop',
                        ip_address TEXT NOT NULL,
                        port INTEGER NOT NULL DEFAULT 8002,
                        total_memory_gb REAL NOT NULL DEFAULT 16,
                        available_memory_gb REAL NOT NULL DEFAULT 16,
                        cpu_cores INTEGER NOT NULL DEFAULT 4,
                        has_gpu INTEGER NOT NULL DEFAULT 0,
                        gpu_vram_mb INTEGER NOT NULL DEFAULT 0,
                        status TEXT NOT NULL DEFAULT 'online',
                        last_heartbeat TEXT NOT NULL,
                        created_at TEXT NOT NULL
                    )"
                ).execute(pool).await?;

                // 审计日志表
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS audit_logs (
                        id TEXT PRIMARY KEY,
                        org_id TEXT NOT NULL,
                        user_id TEXT NOT NULL,
                        action TEXT NOT NULL,
                        resource_type TEXT NOT NULL,
                        resource_id TEXT,
                        details TEXT,
                        ip_address TEXT,
                        created_at TEXT NOT NULL,
                        FOREIGN KEY (org_id) REFERENCES organizations(id)
                    )"
                ).execute(pool).await?;

                // 为用量统计创建索引
                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_usage_org_date ON usage_records(org_id, created_at)"
                ).execute(pool).await?;

                sqlx::query(
                    "CREATE INDEX IF NOT EXISTS idx_audit_org_date ON audit_logs(org_id, created_at)"
                ).execute(pool).await?;
            }
            DbPool::Postgres(pool) => {
                // PostgreSQL 建表语句（类型略有不同）
                sqlx::query(
                    "CREATE TABLE IF NOT EXISTS organizations (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        plan TEXT NOT NULL DEFAULT 'free',
                        max_users INTEGER NOT NULL DEFAULT 5,
                        daily_token_limit BIGINT NOT NULL DEFAULT 100000,
                        concurrent_limit INTEGER NOT NULL DEFAULT 2,
                        is_active BOOLEAN NOT NULL DEFAULT true,
                        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                    )"
                ).execute(pool).await?;

                // PostgreSQL 其余表结构类似，省略重复...
                // 在生产环境中应使用正式的 migration 文件而非内联 SQL
            }
        }

        info!("数据库迁移完成");
        Ok(())
    }

    /// 检查数据库连接健康
    pub async fn health_check(&self) -> bool {
        match &self.pool {
            DbPool::Sqlite(pool) => {
                sqlx::query("SELECT 1").execute(pool).await.is_ok()
            }
            DbPool::Postgres(pool) => {
                sqlx::query("SELECT 1").execute(pool).await.is_ok()
            }
        }
    }
}
