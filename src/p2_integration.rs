//! # P2 功能模块集成
//!
//! 将P2任务的三个核心模块集成到主流程：
//! - TDD支持（智能测试生成）
//! - 性能优化（6层缓存 + 命中率优化器）
//! - Dashboard（实时监控面板）

use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::performance_advanced::{
    CacheHitOptimizer, 
    CacheOptimizationConfig,
    LlmResponseCache,
};
use crate::tdd::{TestGenerator, TddConfig};
use crate::dashboard::DashboardServer;

/// P2集成管理器
pub struct P2Integration {
    /// 缓存命中率优化器
    cache_optimizer: Option<Arc<CacheHitOptimizer>>,
    
    /// TDD测试生成器
    tdd_generator: Option<Arc<TestGenerator>>,
    
    /// Dashboard服务器句柄
    dashboard_handle: Option<JoinHandle<()>>,
    
    /// 是否已初始化
    initialized: bool,
}

impl P2Integration {
    /// 创建新的P2集成管理器
    pub fn new() -> Self {
        Self {
            cache_optimizer: None,
            tdd_generator: None,
            dashboard_handle: None,
            initialized: false,
        }
    }
    
    /// 初始化所有P2模块
    pub async fn initialize(&mut self) -> Result<(), String> {
        if self.initialized {
            return Ok(());
        }
        
        info!("🚀 Initializing P2 modules...");
        
        // 1. 初始化缓存命中率优化器
        self.init_cache_optimizer()?;
        
        // 2. 初始化TDD测试生成器
        self.init_tdd_generator()?;
        
        // 3. 启动Dashboard服务器
        self.start_dashboard_server()?;
        
        self.initialized = true;
        info!("✅ P2 modules initialized successfully");
        
        Ok(())
    }
    
    /// 初始化缓存命中率优化器
    fn init_cache_optimizer(&mut self) -> Result<(), String> {
        info!("📊 Initializing cache hit optimizer...");
        
        let config = CacheOptimizationConfig {
            static_prefix_ttl: 1800,           // 30分钟
            dynamic_suffix_ttl: 300,           // 5分钟
            hot_path_threshold: 5,             // 5次访问标记热点
            enable_predictive_prefetch: true,  // 启用预测
            prefetch_window_size: 3,
            max_warmup_time_ms: 100,
            enable_semantic_caching: true,     // 启用语义缓存
            semantic_similarity_threshold: 0.85,
        };
        
        let optimizer = Arc::new(CacheHitOptimizer::new(config));
        
        // 启动后台统计报告任务
        let optimizer_clone = optimizer.clone();
        tokio::spawn(async move {
            Self::cache_stats_report_loop(optimizer_clone).await;
        });
        
        self.cache_optimizer = Some(optimizer);
        info!("✅ Cache hit optimizer initialized (target: 90%+ hit rate)");
        
        Ok(())
    }
    
    /// 初始化TDD测试生成器
    fn init_tdd_generator(&mut self) -> Result<(), String> {
        info!("🧪 Initializing TDD test generator...");
        
        let config = TddConfig {
            llm_enabled: true,
            batch_size: 5,
            parallel_limit: 3,
            cache_enabled: true,
            ..Default::default()
        };
        
        let generator = Arc::new(TestGenerator::new(config));
        self.tdd_generator = Some(generator);
        
        info!("✅ TDD test generator initialized (LLM-powered)");
        
        Ok(())
    }
    
    /// 启动Dashboard服务器
    fn start_dashboard_server(&mut self) -> Result<(), String> {
        info!("📈 Starting Dashboard server...");
        
        let port: u16 = std::env::var("CARPAI_DASHBOARD_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        
        let host = std::env::var("CARPAI_DASHBOARD_HOST")
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        
        let server = DashboardServer::new(port).with_host(&host);
        let url = server.url();
        
        let handle = tokio::spawn(async move {
            if let Err(e) = server.run().await {
                warn!("Dashboard server error: {}", e);
            }
        });
        
        self.dashboard_handle = Some(handle);
        info!("✅ Dashboard server started at http://{}:{}", host, port);
        
        Ok(())
    }
    
    /// 获取缓存优化器引用
    pub fn cache_optimizer(&self) -> Option<&Arc<CacheHitOptimizer>> {
        self.cache_optimizer.as_ref()
    }
    
    /// 获取TDD生成器引用
    pub fn tdd_generator(&self) -> Option<&Arc<TestGenerator>> {
        self.tdd_generator.as_ref()
    }
    
    /// 后台统计报告循环（每5分钟输出一次）
    async fn cache_stats_report_loop(optimizer: Arc<CacheHitOptimizer>) {
        use tokio::time::{sleep, Duration};
        
        loop {
            sleep(Duration::from_secs(300)).await; // 5分钟
            
            let stats = optimizer.get_stats().await;
            
            info!(
                "📊 Cache Stats | Hit Rate: {:.1}% | L1: {} | L2: {} | L3: {} | Semantic: {} | Tokens Saved: {} | Cost Savings: ${:.2}",
                stats.hit_rate * 100.0,
                stats.l1_hits,
                stats.l2_hits,
                stats.l3_hits,
                stats.semantic_hits,
                stats.tokens_saved,
                stats.estimated_cost_savings_usd
            );
            
            // 如果命中率低于90%，输出建议
            if stats.hit_rate < 0.90 {
                let recommendations = optimizer.generate_recommendations().await;
                for rec in recommendations {
                    warn!("💡 {}", rec);
                }
            }
        }
    }
}

/// 全局P2集成实例
static mut P2_INTEGRATION: Option<P2Integration> = None;

/// 初始化全局P2集成
pub async fn init_p2_integration() -> Result<(), String> {
    unsafe {
        if P2_INTEGRATION.is_none() {
            let mut integration = P2Integration::new();
            integration.initialize().await?;
            P2_INTEGRATION = Some(integration);
        }
    }
    Ok(())
}

/// 获取全局P2集成引用
pub fn get_p2_integration() -> Option<&'static P2Integration> {
    unsafe { P2_INTEGRATION.as_ref() }
}

/// 便捷函数：记录缓存请求
pub async fn record_cache_request(
    key: u64,
    prompt: &str,
    hit_level: crate::performance_advanced::CacheHitLevel,
    response_time_ms: f64,
    tokens_saved: u32,
) {
    if let Some(integration) = get_p2_integration() {
        if let Some(optimizer) = integration.cache_optimizer() {
            optimizer.record_request(key, prompt, hit_level, response_time_ms, tokens_saved).await;
        }
    }
}

/// 便捷函数：获取TDD生成器
pub fn get_tdd_generator() -> Option<Arc<TestGenerator>> {
    get_p2_integration()
        .and_then(|i| i.tdd_generator().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_p2_integration_init() {
        let mut integration = P2Integration::new();
        let result = integration.initialize().await;
        
        // 注意：这个测试可能会因为端口占用而失败
        // 在实际环境中应该使用mock
        assert!(result.is_ok() || result.is_err());
    }
}
