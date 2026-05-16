//! JWT Proactive Refresh Scheduler
//!
//! 移植自 Claude Code `remoteBridgeCore.ts` TokenRefreshScheduler:
//! ```typescript
//! // 过期前 5 分钟主动刷新
//! const refreshScheduler = new TokenRefreshScheduler(jwt, expires_in)
//! scheduler.start() -> 每 30s 检查一次, 剩余 <5min 时刷新
//! scheduler.force_refresh() -> 401 响应时手动触发
//! scheduler.stop() -> 关闭调度器
//! ```

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

/// JWT 刷新配置
#[derive(Debug, Clone)]
pub struct JwtRefreshConfig {
    /// 提前刷新的时间窗口 (默认 5 分钟)
    pub advance_refresh_seconds: u64,
    
    /// 定期检查间隔 (默认 30 秒)
    pub check_interval_secs: u64,
    
    /// 刷新 API 端点 URL
    pub refresh_url: String,
    
    /// 组织 UUID
    pub org_uuid: String,
}

impl Default for JwtRefreshConfig {
    fn default() -> Self {
        Self {
            advance_refresh_seconds: 300, // 5 分钟
            check_interval_secs: 30,     // 30 秒
            refresh_url: String::new(),
            org_uuid: String::new(),
        }
    }
}

/// 解码后的 JWT payload (简化版)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtPayload {
    /// 签发时间
    pub iat: i64,
    
    /// 过期时间
    pub exp: i64,
    
    /// 签发者
    pub iss: Option<String>,
    
    /// 主题
    pub sub: Option<String>,
}

impl JwtPayload {
    /// 从 base64 编码的 JWT payload 部分解码
    /// JWT 格式: header.payload.signature
    pub fn from_jwt(token: &str) -> Result<Self> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 2 && parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid JWT format"));
        }

        // Base64url decode + padding fix
        let payload_b64 = parts[1];
        let padded = format!("{}{}", payload_b64, "=".repeat((4 - payload_b64.len() % 4) % 4));
        
        let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(padded.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to decode JWT payload: {}", e))?;
        
        let json_str = String::from_utf8(decoded)
            .map_err(|e| anyhow::anyhow!("JWT payload is not valid UTF-8: {}", e))?;
        
        let payload: JwtPayload = serde_json::from_str(&json_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse JWT payload: {}", e))?;

        Ok(payload)
    }

    /// 计算距离过期还有多少秒
    pub fn seconds_until_expiry(&self) -> i64 {
        let now = Utc::now().timestamp();
        let remaining = self.exp - now;
        remaining.max(0) // 不返回负数
    }

    /// 检查是否已过期
    pub fn is_expired(&self) -> bool {
        self.seconds_until_expiry() <= 0
    }

    /// 检查是否需要刷新 (剩余时间小于阈值)
    pub fn needs_refresh(&self, threshold_seconds: u64) -> bool {
        self.seconds_until_expiry() < threshold_seconds as i64
    }
}

/// JWT 主动刷新调度器
pub struct JwtRefreshScheduler {
    /// 当前 JWT 令牌 (Arc<RwLock<> 允许多读者+单写者)
    jwt: Arc<RwLock<String>>,
    
    /// 配置
    config: JwtRefreshConfig,
    
    /// 是否已启动
    is_running: Arc<RwLock<bool>>,
}

impl JwtRefreshScheduler {
    /// 创建新的刷新调度器
    pub fn new(jwt: String, config: JwtRefreshConfig) -> Self {
        Self {
            jwt: Arc::new(RwLock::new(jwt)),
            config,
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    /// 启动定时刷新调度器
    ///
    /// 行为:
    /// - 每 `check_interval_secs` 秒检查一次 JWT 过期时间
    /// - 如果剩余时间 < `advance_refresh_seconds`, 自动执行刷新
    /// - 刷新失败时记录警告但不崩溃 (下次会重试)
    pub async fn start(&self) -> Result<()> {
        {
            let mut running = self.is_running.write().await;
            if *running {
                return Ok(()); // 已经在运行
            }
            *running = true;
        }

        info!(
            "JWT Refresh Scheduler started (advance={}s, interval={}s)",
            self.config.advance_refresh_seconds,
            self.config.check_interval_secs
        );

        let jwt_clone = self.jwt.clone();
        let config = self.config.clone();
        let is_running_clone = self.is_running.clone();

        // 启动后台任务
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(config.check_interval_secs));
            
            loop {
                ticker.tick().await;

                // 检查是否仍在运行
                {
                    let running = is_running_clone.read().await;
                    if !*running {
                        break;
                    }
                }

                // 读取当前 JWT 并检查
                {
                    let current_jwt = jwt_clone.read().await;
                    
                    match JwtPayload::from_jwt(&current_jwt) {
                        Ok(payload) => {
                            if payload.needs_refresh(config.advance_refresh_seconds) {
                                debug!(
                                    "JWT expiring in {}s (<{}s threshold), refreshing...",
                                    payload.seconds_until_expiry(),
                                    config.advance_refresh_bytes
                                );
                                
                                // 执行实际刷新
                                // 注意: 这里需要写回, 但 RwLock 读锁不能升级为写锁
                                drop(current_jwt);
                                
                                // 实际实现中应该通过 channel 或其他方式通知外部执行刷新
                                warn!("JWT refresh triggered (auto-refresh not yet fully implemented)");
                            } else {
                                debug!(
                                    "JWT OK, expires in {}s",
                                    payload.seconds_until_expiry()
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse JWT for expiry check: {}", e);
                        }
                    }
                }
            }

            info!("JWT Refresh Scheduler stopped");
        });

        Ok(())
    }

    /// 手动强制刷新 (401 响应时调用)
    ///
    /// 对应 Claude Code 的 `401 Recovery -> rebuildTransport()` 流程
    pub async fn force_refresh(&self) -> Result<String> {
        info!("Force JWT refresh requested");

        // TODO: 实际调用刷新 API
        // POST /bridge -> {worker_jwt, expires_in}
        
        let current = self.jwt.read().await;
        debug!("Current JWT length: {}", current.len());
        
        // 模拟: 解析当前 token 信息
        match JwtPayload::from_jwt(&current) {
            Ok(payload) => {
                info!(
                    "Force refresh: current JWT exp={}, issued={}",
                    payload.exp,
                    payload.iat
                );
            }
            Err(e) => {
                warn!("Cannot parse current JWT: {}", e);
            }
        }

        // TODO: 替换为新的 JWT
        Err(anyhow::anyhow!("force_refresh not yet implemented"))
    }

    /// 获取当前 JWT 的副本
    pub async fn get_current_jwt(&self) -> String {
        self.jwt.read().await.clone()
    }

    /// 更新 JWT (由外部调用, 例如从 401 恢复流程中)
    pub async fn update_jwt(&self, new_jwt: String) {
        let mut jwt = self.jwt.write().await;
        *jwt = new_jwt;
        info!("JWT updated successfully");
    }

    /// 停止调度器
    pub async fn stop(&self) {
        let mut running = self.is_running.write().await;
        *running = false;
        info!("JWT Refresh Scheduler stop requested");
    }

    /// 检查是否正在运行
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }
}
