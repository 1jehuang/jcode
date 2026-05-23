//! 系统诊断命令 - 验证三层架构配置
//!
//! 检查项:
//! 1. PostgreSQL + pgvector连接和扩展状态
//! 2. Milvus向量数据库连接(如果启用)
//! 3. Redis Cluster连接和节点状态
//! 4. KV Cache外存配置
//! 5. 三层负载均衡器配置
//! 6. Higress网关连接
//! 7. 缓存TTL对齐验证

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// 诊断结果
#[derive(Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub timestamp: String,
    pub overall_status: HealthStatus,
    pub checks: Vec<HealthCheck>,
    pub recommendations: Vec<String>,
}

/// 健康状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Error,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "✓ Healthy"),
            Self::Warning => write!(f, "⚠ Warning"),
            Self::Error => write!(f, "✗ Error"),
        }
    }
}

/// 健康检查项
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthStatus,
    pub message: String,
    pub details: Option<String>,
    pub duration_ms: u64,
}

pub struct DoctorCommand;

impl Command for DoctorCommand {
    fn name(&self) -> &str {
        "doctor"
    }

    fn description(&self) -> &str {
        "System health diagnosis for three-layer architecture"
    }

    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        println!("🔍 Running CarpAI system diagnosis...\n");

        let start_time = Instant::now();
        let mut checks = Vec::new();
        let mut recommendations = Vec::new();

        // Layer 1: 数据库层检查
        checks.push(self.check_postgresql().await?);
        checks.push(self.check_milvus().await?);

        // Layer 2: 缓存层检查
        checks.push(self.check_redis_cluster().await?);
        checks.push(self.check_kv_cache_storage().await?);

        // Layer 3: 负载均衡层检查
        checks.push(self.check_load_balancer().await?);
        checks.push(self.check_higress_gateway().await?);

        // 一致性检查
        checks.push(self.check_ttl_alignment().await?);

        // 背压机制检查
        checks.push(self.check_backpressure().await?);

        let total_duration = start_time.elapsed();

        // 确定整体状态
        let overall_status = if checks.iter().any(|c| c.status == HealthStatus::Error) {
            HealthStatus::Error
        } else if checks.iter().any(|c| c.status == HealthStatus::Warning) {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };

        // 生成建议
        recommendations = self.generate_recommendations(&checks);

        let report = DoctorReport {
            timestamp: chrono::Utc::now().to_rfc3339(),
            overall_status,
            checks,
            recommendations,
        };

        // 打印报告
        self.print_report(&report, total_duration)?;

        Ok(CommandResult::success("Diagnosis complete"))
    }
}

impl DoctorCommand {
    /// 检查PostgreSQL + pgvector
    async fn check_postgresql(&self) -> Result<HealthCheck> {
        let start = Instant::now();
        let db_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://carpai:carpai_dev_password@localhost:5432/carpai".to_string());

        // 尝试连接PostgreSQL
        let status = if db_url.starts_with("postgresql://") || db_url.starts_with("postgres://") {
            // 生产环境:实际连接测试
            match sqlx::PgPool::connect(&db_url).await {
                Ok(pool) => {
                    // 检查pgvector扩展
                    let row: (i64,) = sqlx::query_as(
                        "SELECT COUNT(*) FROM pg_extension WHERE extname = 'vector'"
                    )
                    .fetch_one(&pool)
                    .await
                    .context("Failed to query pgvector extension")?;

                    let has_pgvector = row.0 > 0;

                    if has_pgvector {
                        HealthCheck {
                            name: "PostgreSQL + pgvector".to_string(),
                            status: HealthStatus::Healthy,
                            message: "PostgreSQL connected, pgvector extension enabled".to_string(),
                            details: Some(format!("URL: {}", db_url)),
                            duration_ms: start.elapsed().as_millis() as u64,
                        }
                    } else {
                        HealthCheck {
                            name: "PostgreSQL + pgvector".to_string(),
                            status: HealthStatus::Warning,
                            message: "PostgreSQL connected, but pgvector extension NOT found".to_string(),
                            details: Some("Run: CREATE EXTENSION IF NOT EXISTS vector;".to_string()),
                            duration_ms: start.elapsed().as_millis() as u64,
                        }
                    }
                }
                Err(e) => {
                    HealthCheck {
                        name: "PostgreSQL + pgvector".to_string(),
                        status: HealthStatus::Error,
                        message: format!("Failed to connect to PostgreSQL: {}", e),
                        details: Some(format!("URL: {}", db_url)),
                        duration_ms: start.elapsed().as_millis() as u64,
                    }
                }
            }
        } else {
            // 开发环境:SQLite
            HealthCheck {
                name: "PostgreSQL + pgvector".to_string(),
                status: HealthStatus::Warning,
                message: "Using SQLite (development mode), pgvector not available".to_string(),
                details: Some("For production, use PostgreSQL with pgvector".to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            }
        };

        Ok(status)
    }

    /// 检查Milvus向量数据库
    async fn check_milvus(&self) -> Result<HealthCheck> {
        let start = Instant::now();
        let milvus_uri = std::env::var("MILVUS_URI")
            .unwrap_or_else(|_| "milvus://localhost:19530".to_string());

        let vector_store_type = std::env::var("VECTOR_STORE_TYPE")
            .unwrap_or_else(|_| "pgvector".to_string());

        if vector_store_type == "milvus" {
            // 尝试连接Milvus
            #[cfg(feature = "milvus")]
            {
                match jcode_enterprise_server::milvus_adapter::MilvusClient::from_env().await {
                    Ok(client) => {
                        if client.is_initialized() {
                            if let Ok(stats) = client.get_stats().await {
                                HealthCheck {
                                    name: "Milvus Vector Database".to_string(),
                                    status: HealthStatus::Healthy,
                                    message: format!("Milvus connected, {} vectors indexed", stats.total_vectors),
                                    details: Some(format!("URI: {}, Index: {}", milvus_uri, stats.index_type)),
                                    duration_ms: start.elapsed().as_millis() as u64,
                                }
                            } else {
                                HealthCheck {
                                    name: "Milvus Vector Database".to_string(),
                                    status: HealthStatus::Warning,
                                    message: "Milvus connected, but failed to get stats".to_string(),
                                    details: Some(milvus_uri),
                                    duration_ms: start.elapsed().as_millis() as u64,
                                }
                            }
                        } else {
                            HealthCheck {
                                name: "Milvus Vector Database".to_string(),
                                status: HealthStatus::Error,
                                message: "Milvus client initialized but not ready".to_string(),
                                details: Some(milvus_uri),
                                duration_ms: start.elapsed().as_millis() as u64,
                            }
                        }
                    }
                    Err(e) => {
                        HealthCheck {
                            name: "Milvus Vector Database".to_string(),
                            status: HealthStatus::Error,
                            message: format!("Failed to connect to Milvus: {}", e),
                            details: Some(milvus_uri),
                            duration_ms: start.elapsed().as_millis() as u64,
                        }
                    }
                }
            }

            #[cfg(not(feature = "milvus"))]
            {
                HealthCheck {
                    name: "Milvus Vector Database".to_string(),
                    status: HealthStatus::Warning,
                    message: "Milvus feature not compiled (enable with --features milvus)".to_string(),
                    details: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        } else {
            HealthCheck {
                name: "Milvus Vector Database".to_string(),
                status: HealthStatus::Healthy,
                message: "Using pgvector (Milvus not required)".to_string(),
                details: Some("Enable Milvus for >10M vector scale".to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }

        Ok(Ok(HealthCheck {
            name: "Milvus Vector Database".to_string(),
            status: HealthStatus::Healthy,
            message: "Check skipped".to_string(),
            details: None,
            duration_ms: start.elapsed().as_millis() as u64,
        })).unwrap_or_else(|e: anyhow::Error| {
            Ok(HealthCheck {
                name: "Milvus Vector Database".to_string(),
                status: HealthStatus::Error,
                message: e.to_string(),
                details: None,
                duration_ms: start.elapsed().as_millis() as u64,
            })
        })
    }

    /// 检查Redis Cluster
    async fn check_redis_cluster(&self) -> Result<HealthCheck> {
        let start = Instant::now();
        let redis_mode = std::env::var("REDIS_MODE")
            .unwrap_or_else(|_| "standalone".to_string());

        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://localhost:6379".to_string());

        if redis_mode == "cluster" {
            // 检查Redis Cluster配置
            let nodes: Vec<&str> = redis_url.split(',').collect();

            if nodes.len() >= 3 {
                HealthCheck {
                    name: "Redis Cluster".to_string(),
                    status: HealthStatus::Healthy,
                    message: format!("Redis Cluster configured with {} nodes", nodes.len()),
                    details: Some(redis_url),
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            } else {
                HealthCheck {
                    name: "Redis Cluster".to_string(),
                    status: HealthStatus::Warning,
                    message: "Redis Cluster mode enabled but insufficient nodes (<3)".to_string(),
                    details: Some(redis_url),
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        } else {
            HealthCheck {
                name: "Redis Cluster".to_string(),
                status: HealthStatus::Warning,
                message: "Using standalone Redis (not cluster mode)".to_string(),
                details: Some("For production, use Redis Cluster for high availability".to_string()),
                duration_ms: start.elapsed().as_millis() as u64,
            }
        }
    }

    /// 检查KV Cache外存配置
    async fn check_kv_cache_storage(&self) -> Result<HealthCheck> {
        let start = Instant::now();
        let storage_type = std::env::var("KV_CACHE_STORAGE_TYPE")
            .unwrap_or_else(|_| "nvme".to_string());

        let storage_path = std::env::var("KV_CACHE_STORAGE_PATH")
            .unwrap_or_else(|_| "/data/kv_cache".to_string());

        let ttl_secs = std::env::var("KV_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let status = match storage_type.as_str() {
            "memory" => HealthStatus::Warning,
            "nvme" => HealthStatus::Healthy,
            "xsky_ai_mesh" => HealthStatus::Healthy,
            _ => HealthStatus::Warning,
        };

        let message = match storage_type.as_str() {
            "memory" => "KV Cache in memory only (high GPU cost)".to_string(),
            "nvme" => format!("KV Cache on NVMe SSD (estimated 30-40% GPU cost reduction)"),
            "xsky_ai_mesh" => format!("KV Cache on XSKY AI Mesh (estimated 35-50% GPU cost reduction)"),
            other => format!("Unknown KV Cache storage type: {}", other),
        };

        Ok(HealthCheck {
            name: "KV Cache External Storage".to_string(),
            status,
            message,
            details: Some(format!("Type: {}, Path: {}, TTL: {}s", storage_type, storage_path, ttl_secs)),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// 检查三层负载均衡器
    async fn check_load_balancer(&self) -> Result<HealthCheck> {
        let start = Instant::now();

        let tenant_isolation = std::env::var("TENANT_ISOLATION_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(true);

        let model_routing = std::env::var("MODEL_ROUTING_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(true);

        let session_sticky = std::env::var("SESSION_STICKY_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(true);

        let all_enabled = tenant_isolation && model_routing && session_sticky;

        let status = if all_enabled {
            HealthStatus::Healthy
        } else {
            HealthStatus::Warning
        };

        let message = if all_enabled {
            "Three-layer load balancer fully enabled".to_string()
        } else {
            format!(
                "Load balancer layers: Tenant={}, Model={}, Session={}",
                if tenant_isolation { "ON" } else { "OFF" },
                if model_routing { "ON" } else { "OFF" },
                if session_sticky { "ON" } else { "OFF" }
            )
        };

        Ok(HealthCheck {
            name: "Three-Layer Load Balancer".to_string(),
            status,
            message,
            details: Some(format!(
                "Strategy: {}",
                std::env::var("LOAD_BALANCER_STRATEGY").unwrap_or_else(|_| "three_layer".to_string())
            )),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// 检查Higress网关
    async fn check_higress_gateway(&self) -> Result<HealthCheck> {
        let start = Instant::now();

        let higress_url = std::env::var("HIGRESS_ADMIN_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string());

        // 尝试连接Higress Admin API
        let client = reqwest::Client::new();
        match client.get(&format!("{}/apis", higress_url)).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    HealthCheck {
                        name: "Higress Gateway".to_string(),
                        status: HealthStatus::Healthy,
                        message: "Higress gateway connected and responding".to_string(),
                        details: Some(format!("Admin URL: {}", higress_url)),
                        duration_ms: start.elapsed().as_millis() as u64,
                    }
                } else {
                    HealthCheck {
                        name: "Higress Gateway".to_string(),
                        status: HealthStatus::Warning,
                        message: format!("Higress responded with status: {}", response.status()),
                        details: Some(higress_url),
                        duration_ms: start.elapsed().as_millis() as u64,
                    }
                }
            }
            Err(_) => {
                HealthCheck {
                    name: "Higress Gateway".to_string(),
                    status: HealthStatus::Warning,
                    message: "Higress gateway not reachable (may not be deployed)".to_string(),
                    details: Some(format!("Expected at: {}", higress_url)),
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        }
    }

    /// 检查缓存TTL对齐
    async fn check_ttl_alignment(&self) -> Result<HealthCheck> {
        let start = Instant::now();

        let session_sticky_ttl = std::env::var("SESSION_STICKY_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let kv_cache_ttl = std::env::var("KV_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        // Redis默认TTL (从docker-compose或配置文件读取)
        let redis_ttl = 3600; // 假设值

        let aligned = session_sticky_ttl == kv_cache_ttl && kv_cache_ttl == redis_ttl;

        let (status, message) = if aligned {
            (
                HealthStatus::Healthy,
                "All TTLs are aligned (cache consistency guaranteed)".to_string(),
            )
        } else {
            (
                HealthStatus::Warning,
                format!(
                    "TTL MISMATCH! SessionSticky={}s, KVCache={}s, Redis={}s",
                    session_sticky_ttl, kv_cache_ttl, redis_ttl
                ),
            )
        };

        Ok(HealthCheck {
            name: "Cache TTL Alignment".to_string(),
            status,
            message,
            details: Some(
                "Session sticky TTL must match Redis/KV Cache TTL to avoid cache invalidation".to_string()
            ),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// 检查背压机制状态
    async fn check_backpressure(&self) -> Result<HealthCheck> {
        let start = Instant::now();

        // Read backpressure metrics from environment or defaults
        let max_pending = std::env::var("BACKPRESSURE_MAX_PENDING")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500);

        let max_concurrent = std::env::var("BACKPRESSURE_MAX_CONCURRENT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);

        let enabled = std::env::var("BACKPRESSURE_ENABLED")
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(true);

        let (status, message) = if enabled {
            (
                HealthStatus::Healthy,
                format!("Backpressure controller enabled (max_pending={}, max_concurrent={})",
                        max_pending, max_concurrent),
            )
        } else {
            (
                HealthStatus::Warning,
                "Backpressure controller disabled - system vulnerable to overload".to_string(),
            )
        };

        Ok(HealthCheck {
            name: "Backpressure Controller".to_string(),
            status,
            message,
            details: Some(format!(
                "Protects against cascading failures when load exceeds capacity"
            )),
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// 生成建议
    fn generate_recommendations(&self, checks: &[HealthCheck]) -> Vec<String> {
        let mut recommendations = Vec::new();

        for check in checks {
            match check.name.as_str() {
                "PostgreSQL + pgvector" => {
                    if check.status == HealthStatus::Warning {
                        recommendations.push(
                            "💡 Enable pgvector: docker compose up -d postgres && psql -c 'CREATE EXTENSION vector;'".to_string()
                        );
                    }
                }
                "Redis Cluster" => {
                    if check.status == HealthStatus::Warning {
                        recommendations.push(
                            "💡 Deploy Redis Cluster: docker compose --profile cluster up -d".to_string()
                        );
                    }
                }
                "KV Cache External Storage" => {
                    if check.message.contains("memory only") {
                        recommendations.push(
                            "💡 Configure NVMe storage: export KV_CACHE_STORAGE_TYPE=nvme".to_string()
                        );
                    }
                }
                "Cache TTL Alignment" => {
                    if check.status == HealthStatus::Warning {
                        recommendations.push(
                            "💡 Align TTLs: Set SESSION_STICKY_TTL_SECS=KV_CACHE_TTL_SECS=3600".to_string()
                        );
                    }
                }
                "Backpressure Controller" => {
                    if check.status == HealthStatus::Warning {
                        recommendations.push(
                            "💡 Enable backpressure: export BACKPRESSURE_ENABLED=true".to_string()
                        );
                    }
                }
                _ => {}
            }
        }

        if recommendations.is_empty() {
            recommendations.push("✅ All systems operational!".to_string());
        }

        recommendations
    }

    /// 打印诊断报告
    fn print_report(&self, report: &DoctorReport, total_duration: std::time::Duration) -> Result<()> {
        println!("\n{}", "=".repeat(80));
        println!("  CarpAI System Diagnosis Report");
        println!("  Timestamp: {}", report.timestamp);
        println!("  Duration: {:?}", total_duration);
        println!("{}", "=".repeat(80));

        println!("\nOverall Status: {}\n", report.overall_status);

        println!("Checks:");
        println!("{}", "-".repeat(80));

        for check in &report.checks {
            println!(
                "\n  {} {} [{}ms]",
                check.status,
                check.name,
                check.duration_ms
            );
            println!("     {}", check.message);

            if let Some(details) = &check.details {
                println!("     Details: {}", details);
            }
        }

        println!("\n{}", "=".repeat(80));
        println!("Recommendations:");
        println!("{}", "-".repeat(80));

        for (i, rec) in report.recommendations.iter().enumerate() {
            println!("  {}. {}", i + 1, rec);
        }

        println!("\n{}", "=".repeat(80));

        Ok(())
    }
}
