//! 文件监听器 - 配置热更新
//!
//! 移植自 Claude Code `config.ts`:
//! ```typescript
//! const CONFIG_FRESHNESS_POLL_MS = 1000
//! startGlobalConfigFreshnessWatcher():
//!   watchFile(file, { interval: 1000 }, curr => {
//!     if (curr.mtimeMs <= globalConfigCache.mtime) return
//!     // 其他进程写入 → 热加载新内容
//!     content = fs.readFile(file)
//!     parsed = safeParseJSON(content)
//!     globalConfigCache = { config: parsed, mtime: curr.mtimeMs }
//!   })
//! ```

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// 配置缓存 (带 mtime 时间戳)
#[derive(Debug, Clone)]
pub struct ConfigCache {
    /// 缓存的配置内容
    pub config: JsonValue,
    
    /// 文件最后修改时间
    pub mtime: DateTime<Utc>,
}

impl Default for ConfigCache {
    fn default() -> Self {
        Self {
            config: JsonValue::Object(serde_json::Map::new()),
            mtime: DateTime::from(std::time::UNIX_EPOCH),
        }
    }
}

/// 配置文件监听器
///
/// 通过定期轮询文件修改时间来检测跨进程的配置变更
/// 当检测到变更时自动热加载新内容到内存缓存
pub struct ConfigFileWatcher {
    /// 监听的配置文件路径
    config_path: PathBuf,
    
    /// 内存缓存 (Arc<RwLock<> 支持并发读写)
    cache: Arc<RwLock<ConfigCache>>,
    
    /// 轮询间隔
    poll_interval_ms: u64,
    
    /// 是否已启动
    is_running: Arc<RwLock<bool>>,
}

impl ConfigFileWatcher {
    /// 创建新的文件监听器
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
            cache: Arc::new(RwLock::new(ConfigCache::default())),
            poll_interval_ms: 1000, // Claude Code 默认 1000ms
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    /// 设置轮询间隔
    pub fn with_poll_interval(mut self, millis: u64) -> Self {
        self.poll_interval_ms = millis;
        self
    }

    /// 启动文件监控任务
    ///
    /// 返回 JoinHandle 用于等待任务完成 (通常不需要等待)
    pub async fn spawn(&self) -> Result<tokio::task::JoinHandle<()>> {
        {
            let mut running = self.is_running.write().await;
            if *running {
                return Err(anyhow::anyhow!("Watcher already running"));
            }
            *running = true;
        }

        info!(
            "ConfigFileWatcher started on {:?} (interval={}ms)",
            self.config_path,
            self.poll_interval_ms
        );

        let path = self.config_path.clone();
        let cache = self.cache.clone();
        let is_running = self.is_running.clone();
        let interval_ms = self.poll_interval_ms;

        let handle = tokio::spawn(async move {
            use tokio::time::{interval, Duration};
            
            let mut ticker = interval(Duration::from_millis(interval_ms));
            
            loop {
                ticker.tick().await;

                // 检查是否仍在运行
                {
                    let running = is_running.read().await;
                    if !*running {
                        break;
                    }
                }

                // 检查文件是否存在
                if !path.exists() {
                    continue;
                }

                // 获取文件元数据
                match fs::metadata(&path).await {
                    Ok(meta) => {
                        let modified: DateTime<Utc> = meta.modified().ok()
                            .and_then(|t| t.into())
                            .unwrap_or_else(Utc::now);

                        // 检查 mtime 是否更新
                        {
                            let guard = cache.read().await;
                            if modified <= guard.mtime {
                                continue; // 未变化
                            }
                        }

                        // mtime 变更 → 重新读取并解析
                        match fs::read_to_string(&path).await {
                            Ok(content) => {
                                match serde_json::from_str::<JsonValue>(&content) {
                                    Ok(parsed) => {
                                        let mut guard = cache.write().await;
                                        guard.config = parsed;
                                        guard.mtime = modified;
                                        
                                        debug!("Config hot-reloaded from {:?}", path);
                                    }
                                    Err(e) => {
                                        warn!("Failed to parse updated config {:?}: {}", path, e);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to read updated config {:?}: {}", path, e);
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Cannot stat config file {:?}: {}", path, e);
                    }
                }
            }

            info!("ConfigFileWatcher stopped");
        });

        Ok(handle)
    }

    /// 手动停止监听任务
    pub async fn stop(&self) {
        let mut running = self.is_running.write().await;
        *running = false;
        info!("ConfigFileWatcher stop requested");
    }

    /// 检查是否正在运行
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// 读取当前缓存的配置
    pub async fn get_config(&self) -> JsonValue {
        self.cache.read().await.config.clone()
    }

    /// 强制刷新缓存 (从磁盘重新读取)
    pub async fn force_refresh(&self) -> Result<JsonValue> {
        let content = fs::read_to_string(&self.config_path).await?;
        let parsed: JsonValue = serde_json::from_str(&content)?;
        
        let meta = fs::metadata(&self.config_path).await?;
        let modified: DateTime<Utc> = meta.modified().ok()
            .and_then(|t| t.into())
            .unwrap_or_else(Utc::now);

        let mut guard = self.cache.write().await;
        guard.config = parsed;
        guard.mtime = modified;

        Ok(guard.config.clone())
    }

    /// 获取缓存的 Arc 引用 (供其他组件共享访问)
    pub fn cache_arc(&self) -> Arc<RwLock<ConfigCache>> {
        self.cache.clone()
    }
}
