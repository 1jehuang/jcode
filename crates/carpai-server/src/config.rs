//! Server configuration (Layer 2a)

use carpai_core::CoreConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    #[serde(default = "default_redis_pool")]
    pub pool_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // === Network ===
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default = "default_port")]
    pub port: u16,

    // === TLS ===
    pub tls: Option<TlsConfig>,

    // === Database ===
    pub database: DatabaseConfig,
    pub redis: Option<RedisConfig>,

    // === Authentication ===
    pub jwt_secret: String,
    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry_hours: u64,

    // === Multi-tenant ===
    #[serde(default)]
    pub multi_tenant: bool,
    #[serde(default = "default_tenant")]
    pub default_tenant_id: String,

    // === Enterprise features ===
    #[serde(default)]
    pub audit_log_enabled: bool,
    #[serde(default)]
    pub rate_limit_enabled: bool,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_rpm: u64,
}

fn default_listen_addr() -> String { "0.0.0.0".into() }
fn default_port() -> u16 { 8080 }
fn default_max_connections() -> u32 { 20 }
fn default_redis_pool() -> u32 { 10 }
fn default_jwt_expiry() -> u64 { 24 }
fn default_tenant() -> String { "org-default".into() }
fn default_rate_limit() -> u64 { 60 }

impl ServerConfig {
    /// Load configuration with three-level override: defaults → file → env vars
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if path.exists() {
            let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
            config = toml::from_str(&content).map_err(ConfigError::Parse)?;
        }

        // Environment variable overrides
        if let Ok(v) = std::env::var("CARPAI_SERVER__PORT") {
            config.port = v.parse().map_err(|_| ConfigError::Env("CARPAI_SERVER__PORT"))?;
        }
        if let Ok(v) = std::env::var("CARPAI_SERVER__LISTEN_ADDR") {
            config.listen_addr = v;
        }
        if let Ok(v) = std::env::var("CARPAI_SERVER__JWT_SECRET") {
            config.jwt_secret = v;
        }
        if let Ok(v) = std::env::var("CARPAI_SERVER__DATABASE_URL") {
            config.database.url = v;
        }

        Ok(config)
    }

    pub fn full_listen_addr(&self) -> String {
        format!("{}:{}", self.listen_addr, self.port)
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            core: CoreConfig::default(),
            listen_addr: default_listen_addr(),
            port: default_port(),
            tls: None,
            database: DatabaseConfig {
                url: "postgres://localhost/carpai".into(),
                max_connections: default_max_connections(),
            },
            redis: None,
            jwt_secret: "dev-secret-change-in-production".into(),
            jwt_expiry_hours: default_jwt_expiry(),
            multi_tenant: false,
            default_tenant_id: default_tenant(),
            audit_log_enabled: false,
            rate_limit_enabled: false,
            rate_limit_rpm: default_rate_limit(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Invalid environment variable: {0}")]
    Env(&'static str),
}
