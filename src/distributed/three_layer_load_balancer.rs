//! 三层负载均衡器 - 与缓存TTL严格对齐
//!
//! Layer 1: 租户隔离 (Tenant Isolation)
//! Layer 2: 模型路由 (Model Routing)
//! Layer 3: 会话粘性 (Session Sticky, TTL与Redis对齐)
//!
//! 设计原则:
//! - 会话粘性有效期必须与Redis缓存TTL严格对齐
//! - 否则将导致缓存失效,抵消缓存收益

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// 三层负载均衡配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreeLayerConfig {
    /// Layer 1: 租户隔离是否启用
    pub tenant_isolation_enabled: bool,

    /// Layer 2: 模型路由是否启用
    pub model_routing_enabled: bool,

    /// Layer 3: 会话粘性是否启用
    pub session_sticky_enabled: bool,

    /// 会话粘性TTL (秒) - 必须与Redis TTL对齐
    pub session_sticky_ttl_secs: u64,

    /// Redis连接URL
    pub redis_url: String,

    /// 默认模型路由策略
    pub default_model_route: String,
}

impl Default for ThreeLayerConfig {
    fn default() -> Self {
        Self {
            tenant_isolation_enabled: true,
            model_routing_enabled: true,
            session_sticky_enabled: true,
            session_sticky_ttl_secs: 3600, // 1小时,与Redis默认TTL对齐
            redis_url: "redis://localhost:6379".to_string(),
            default_model_route: "round_robin".to_string(),
        }
    }
}

/// 租户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantInfo {
    pub tenant_id: String,
    pub name: String,
    pub allowed_models: Vec<String>,
    pub max_concurrent_requests: usize,
    pub current_requests: usize,
    pub rate_limit_per_minute: usize,
}

/// 模型路由信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    pub model_name: String,
    pub backend_nodes: Vec<String>,
    pub routing_strategy: RoutingStrategy,
    pub cache_enabled: bool,
    pub cache_ttl_secs: u64,
}

/// 路由策略
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RoutingStrategy {
    RoundRobin,
    LeastLoaded,
    GpuMemoryAware,
    LatencyOptimized,
}

/// 会话粘性条目
#[derive(Debug, Clone)]
pub struct SessionStickyEntry {
    pub session_id: String,
    pub tenant_id: String,
    pub assigned_node: String,
    pub created_at: Instant,
    pub last_accessed: Instant,
    pub access_count: u64,
}

impl SessionStickyEntry {
    /// 检查会话是否过期
    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        self.last_accessed.elapsed() > Duration::from_secs(ttl_secs)
    }
}

/// 三层负载均衡器
pub struct ThreeLayerLoadBalancer {
    config: ThreeLayerConfig,

    /// Layer 1: 租户隔离表
    tenants: Arc<RwLock<HashMap<String, TenantInfo>>>,

    /// Layer 2: 模型路由表
    model_routes: Arc<RwLock<HashMap<String, ModelRoute>>>,

    /// Layer 3: 会话粘性映射 (session_id -> node)
    session_sticky_map: Arc<RwLock<HashMap<String, SessionStickyEntry>>>,

    /// 统计信息
    stats: Arc<RwLock<LoadBalancerStats>>,
}

impl ThreeLayerLoadBalancer {
    /// 创建新的三层负载均衡器
    pub fn new(config: ThreeLayerConfig) -> Self {
        info!(
            "Initializing Three-Layer Load Balancer:\n  - Tenant Isolation: {}\n  - Model Routing: {}\n  - Session Sticky: {} (TTL={}s)",
            if config.tenant_isolation_enabled { "enabled" } else { "disabled" },
            if config.model_routing_enabled { "enabled" } else { "disabled" },
            if config.session_sticky_enabled { "enabled" } else { "disabled" },
            config.session_sticky_ttl_secs
        );

        Self {
            config,
            tenants: Arc::new(RwLock::new(HashMap::new())),
            model_routes: Arc::new(RwLock::new(HashMap::new())),
            session_sticky_map: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(LoadBalancerStats::default())),
        }
    }

    /// 从环境变量加载配置
    pub fn from_env() -> Self {
        let config = ThreeLayerConfig {
            tenant_isolation_enabled: std::env::var("TENANT_ISOLATION_ENABLED")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true),

            model_routing_enabled: std::env::var("MODEL_ROUTING_ENABLED")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true),

            session_sticky_enabled: std::env::var("SESSION_STICKY_ENABLED")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true),

            session_sticky_ttl_secs: std::env::var("SESSION_STICKY_TTL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),

            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://localhost:6379".to_string()),

            default_model_route: std::env::var("DEFAULT_MODEL_ROUTE")
                .unwrap_or_else(|_| "round_robin".to_string()),
        };

        Self::new(config)
    }

    // ========================================================================
    // Layer 1: 租户隔离
    // ========================================================================

    /// 注册租户
    pub async fn register_tenant(&self, tenant: TenantInfo) {
        let mut tenants = self.tenants.write().await;
        info!("Registered tenant: {} (id={})", tenant.name, tenant.tenant_id);
        tenants.insert(tenant.tenant_id.clone(), tenant);
    }

    /// 验证租户并获取可用模型列表
    pub async fn validate_tenant(&self, tenant_id: &str) -> Option<Vec<String>> {
        let tenants = self.tenants.read().await;

        if let Some(tenant) = tenants.get(tenant_id) {
            // 检查并发限制
            if tenant.current_requests >= tenant.max_concurrent_requests {
                warn!(
                    "Tenant {} reached concurrent request limit ({}/{})",
                    tenant_id, tenant.current_requests, tenant.max_concurrent_requests
                );
                return None;
            }

            Some(tenant.allowed_models.clone())
        } else {
            warn!("Unknown tenant: {}", tenant_id);
            None
        }
    }

    /// 增加租户当前请求计数
    pub async fn increment_tenant_requests(&self, tenant_id: &str) {
        if let Some(tenant) = self.tenants.write().await.get_mut(tenant_id) {
            tenant.current_requests += 1;
        }
    }

    /// 减少租户当前请求计数
    pub async fn decrement_tenant_requests(&self, tenant_id: &str) {
        if let Some(tenant) = self.tenants.write().await.get_mut(tenant_id) {
            tenant.current_requests = tenant.current_requests.saturating_sub(1);
        }
    }

    // ========================================================================
    // Layer 2: 模型路由
    // ========================================================================

    /// 注册模型路由
    pub async fn register_model_route(&self, route: ModelRoute) {
        let mut routes = self.model_routes.write().await;
        info!(
            "Registered model route: {} -> {:?} nodes (strategy={:?}, cache_ttl={}s)",
            route.model_name,
            route.backend_nodes.len(),
            route.routing_strategy,
            route.cache_ttl_secs
        );
        routes.insert(route.model_name.clone(), route);
    }

    /// 根据模型名称选择后端节点
    pub async fn route_model_request(
        &self,
        model_name: &str,
        tenant_id: Option<&str>,
    ) -> Option<String> {
        let routes = self.model_routes.read().await;

        if let Some(route) = routes.get(model_name) {
            // 验证租户是否有权限访问该模型
            if let Some(tid) = tenant_id {
                if let Some(allowed_models) = self.validate_tenant(tid).await {
                    if !allowed_models.contains(&model_name.to_string()) {
                        warn!(
                            "Tenant {} not authorized to use model {}",
                            tid, model_name
                        );
                        return None;
                    }
                }
            }

            // 根据路由策略选择节点
            let selected_node = match route.routing_strategy {
                RoutingStrategy::RoundRobin => self.round_robin_select(&route.backend_nodes),
                RoutingStrategy::LeastLoaded => self.least_loaded_select(&route.backend_nodes).await,
                RoutingStrategy::GpuMemoryAware => self.gpu_memory_aware_select(&route.backend_nodes).await,
                RoutingStrategy::LatencyOptimized => self.latency_optimized_select(&route.backend_nodes).await,
            };

            if let Some(node) = &selected_node {
                debug!("Routed model {} to node {}", model_name, node);
                self.stats.write().await.total_routed_requests += 1;
            }

            selected_node
        } else {
            warn!("No route configured for model: {}", model_name);
            None
        }
    }

    // ========================================================================
    // Layer 3: 会话粘性 (TTL与Redis严格对齐)
    // ========================================================================

    /// 为会话分配节点 (带粘性)
    pub async fn assign_session_to_node(
        &self,
        session_id: &str,
        tenant_id: &str,
        model_name: &str,
    ) -> Option<String> {
        if !self.config.session_sticky_enabled {
            // 如果禁用会话粘性,直接路由
            return self.route_model_request(model_name, Some(tenant_id)).await;
        }

        let mut sticky_map = self.session_sticky_map.write().await;

        // 检查是否存在有效的会话粘性映射
        if let Some(entry) = sticky_map.get(session_id) {
            if !entry.is_expired(self.config.session_sticky_ttl_secs) {
                // 会话仍然有效,返回已分配的节点
                debug!(
                    "Session {} sticky to node {} (age={:?}, accesses={})",
                    session_id,
                    entry.assigned_node,
                    entry.last_accessed.elapsed(),
                    entry.access_count
                );

                // 更新访问统计
                let entry = sticky_map.get_mut(session_id).unwrap();
                entry.last_accessed = Instant::now();
                entry.access_count += 1;

                self.stats.write().await.sticky_hits += 1;

                return Some(entry.assigned_node.clone());
            } else {
                // 会话已过期,移除
                debug!("Session {} expired, removing", session_id);
                sticky_map.remove(session_id);
                self.stats.write().await.sticky_expirations += 1;
            }
        }

        // 没有有效的会话映射,分配新节点
        if let Some(node) = self.route_model_request(model_name, Some(tenant_id)).await {
            let entry = SessionStickyEntry {
                session_id: session_id.to_string(),
                tenant_id: tenant_id.to_string(),
                assigned_node: node.clone(),
                created_at: Instant::now(),
                last_accessed: Instant::now(),
                access_count: 1,
            };

            sticky_map.insert(session_id.to_string(), entry);

            info!(
                "Assigned session {} to node {} (TTL={}s)",
                session_id,
                node,
                self.config.session_sticky_ttl_secs
            );

            self.stats.write().await.sticky_assignments += 1;

            Some(node)
        } else {
            None
        }
    }

    /// 清理过期的会话粘性映射
    pub async fn cleanup_expired_sessions(&self) {
        let mut sticky_map = self.session_sticky_map.write().await;
        let initial_count = sticky_map.len();

        sticky_map.retain(|_, entry| {
            !entry.is_expired(self.config.session_sticky_ttl_secs)
        });

        let removed_count = initial_count - sticky_map.len();
        if removed_count > 0 {
            info!("Cleaned up {} expired session sticky entries", removed_count);
            self.stats.write().await.sticky_cleanups += 1;
        }
    }

    // ========================================================================
    // 完整请求处理流程 (三层联动)
    // ========================================================================

    /// 处理完整请求 (三层负载均衡)
    pub async fn handle_request(
        &self,
        tenant_id: &str,
        session_id: &str,
        model_name: &str,
    ) -> Option<String> {
        // Layer 1: 租户隔离验证
        if self.config.tenant_isolation_enabled {
            if self.validate_tenant(tenant_id).await.is_none() {
                return None;
            }
            self.increment_tenant_requests(tenant_id).await;
        }

        // Layer 2 + Layer 3: 模型路由 + 会话粘性
        let result = self
            .assign_session_to_node(session_id, tenant_id, model_name)
            .await;

        // 更新统计
        if result.is_some() {
            self.stats.write().await.successful_requests += 1;
        } else {
            self.stats.write().await.failed_requests += 1;
        }

        // 递减租户请求计数 (在请求完成后调用)
        if self.config.tenant_isolation_enabled && result.is_some() {
            // 注意:实际应在请求完成时递减,这里简化处理
            self.decrement_tenant_requests(tenant_id).await;
        }

        result
    }

    // ========================================================================
    // 辅助方法 - 节点选择策略
    // ========================================================================

    /// Round Robin选择
    fn round_robin_select(&self, nodes: &[String]) -> Option<String> {
        if nodes.is_empty() {
            return None;
        }

        // 简化实现:随机选择 (生产环境应维护全局计数器)
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_nanos() as usize;

        nodes.get(seed % nodes.len()).cloned()
    }

    /// Least Loaded选择 (需要查询节点负载)
    async fn least_loaded_select(&self, _nodes: &[String]) -> Option<String> {
        // TODO: 集成节点负载监控
        // 目前简化为Round Robin
        self.round_robin_select(_nodes)
    }

    /// GPU显存感知选择
    async fn gpu_memory_aware_select(&self, _nodes: &[String]) -> Option<String> {
        // TODO: 查询节点GPU显存使用情况
        // 选择显存最充足的节点
        self.round_robin_select(_nodes)
    }

    /// 延迟优化选择
    async fn latency_optimized_select(&self, _nodes: &[String]) -> Option<String> {
        // TODO: 基于历史延迟数据选择最优节点
        self.round_robin_select(_nodes)
    }

    // ========================================================================
    // 统计信息
    // ========================================================================

    /// 获取负载均衡统计
    pub async fn get_stats(&self) -> LoadBalancerStats {
        let mut stats = self.stats.read().await.clone();

        // 计算实时数据
        let sticky_map = self.session_sticky_map.read().await;
        stats.active_sessions = sticky_map.len();

        let tenants = self.tenants.read().await;
        stats.active_tenants = tenants.len();

        let routes = self.model_routes.read().await;
        stats.registered_model_routes = routes.len();

        // 计算缓存命中率
        let total_sticky_requests = stats.sticky_hits + stats.sticky_assignments;
        stats.sticky_hit_rate = if total_sticky_requests == 0 {
            0.0
        } else {
            stats.sticky_hits as f64 / total_sticky_requests as f64
        };

        stats
    }
}

/// 负载均衡统计
#[derive(Debug, Clone, Default, Serialize)]
pub struct LoadBalancerStats {
    pub active_tenants: usize,
    pub active_sessions: usize,
    pub registered_model_routes: usize,
    pub total_routed_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub sticky_hits: u64,
    pub sticky_assignments: u64,
    pub sticky_expirations: u64,
    pub sticky_cleanups: u64,
    pub sticky_hit_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_three_layer_load_balancer() {
        let config = ThreeLayerConfig {
            session_sticky_ttl_secs: 60, // 测试用短TTL
            ..ThreeLayerConfig::default()
        };

        let balancer = ThreeLayerLoadBalancer::new(config);

        // Layer 1: 注册租户
        let tenant = TenantInfo {
            tenant_id: "tenant-1".to_string(),
            name: "Test Tenant".to_string(),
            allowed_models: vec!["gpt-4".to_string(), "gpt-3.5".to_string()],
            max_concurrent_requests: 100,
            current_requests: 0,
            rate_limit_per_minute: 60,
        };
        balancer.register_tenant(tenant).await;

        // Layer 2: 注册模型路由
        let route = ModelRoute {
            model_name: "gpt-4".to_string(),
            backend_nodes: vec![
                "node-1".to_string(),
                "node-2".to_string(),
                "node-3".to_string(),
            ],
            routing_strategy: RoutingStrategy::RoundRobin,
            cache_enabled: true,
            cache_ttl_secs: 3600,
        };
        balancer.register_model_route(route).await;

        // Layer 3: 测试会话粘性
        let session_id = "session-123";
        let tenant_id = "tenant-1";
        let model_name = "gpt-4";

        // 第一次请求 - 应该分配节点
        let node1 = balancer
            .handle_request(tenant_id, session_id, model_name)
            .await;
        assert!(node1.is_some());

        // 第二次请求 - 应该命中粘性会话
        let node2 = balancer
            .handle_request(tenant_id, session_id, model_name)
            .await;
        assert_eq!(node1, node2); // 应该分配到同一个节点

        // 检查统计
        let stats = balancer.get_stats().await;
        assert_eq!(stats.active_tenants, 1);
        assert_eq!(stats.active_sessions, 1);
        assert!(stats.sticky_hit_rate > 0.0);

        println!("Load balancer stats: {:?}", stats);
    }

    #[tokio::test]
    async fn test_session_expiry() {
        let config = ThreeLayerConfig {
            session_sticky_ttl_secs: 1, // 1秒TTL用于测试
            ..ThreeLayerConfig::default()
        };

        let balancer = ThreeLayerLoadBalancer::new(config);

        // 注册租户和路由
        balancer
            .register_tenant(TenantInfo {
                tenant_id: "t1".to_string(),
                name: "T1".to_string(),
                allowed_models: vec!["m1".to_string()],
                max_concurrent_requests: 100,
                current_requests: 0,
                rate_limit_per_minute: 60,
            })
            .await;

        balancer
            .register_model_route(ModelRoute {
                model_name: "m1".to_string(),
                backend_nodes: vec!["n1".to_string()],
                routing_strategy: RoutingStrategy::RoundRobin,
                cache_enabled: true,
                cache_ttl_secs: 3600,
            })
            .await;

        // 第一次请求
        let node1 = balancer.handle_request("t1", "s1", "m1").await;
        assert!(node1.is_some());

        // 等待TTL过期
        tokio::time::sleep(Duration::from_secs(2)).await;

        // 清理过期会话
        balancer.cleanup_expired_sessions().await;

        // 再次请求 - 应该重新分配
        let node2 = balancer.handle_request("t1", "s1", "m1").await;
        assert!(node2.is_some());

        let stats = balancer.get_stats().await;
        assert_eq!(stats.sticky_expirations, 1);
    }
}
