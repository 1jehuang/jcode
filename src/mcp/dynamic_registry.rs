//! Dynamic Tool Registry - 运行时工具注册与管理
//!
//! 提供 MCP 协议的动态工具注册能力：
//! - 运行时注册/注销工具
//! - 工具元数据与版本管理
//! - 工具发现与分类索引
//! - 生命周期钩子 (pre/post register/unregister)
//! - 变更通知 (tools/listChanged)
//!
//! 特性：
//! - 线程安全 (RwLock + Arc)
//! - 支持热加载插件
//! - 自动冲突检测与解决
//! - 完整的审计日志

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

// --- Types --------------------------------------------------

/// 动态工具定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicTool {
    /// 唯一名称 (e.g., "custom_deploy")
    pub name: String,
    
    /// 人类可读描述
    pub description: String,
    
    /// JSON Schema 输入参数定义
    pub input_schema: serde_json::Value,
    
    /// 工具分类 (用于分组显示)
    #[serde(default)]
    pub category: ToolCategory,
    
    /// 版本号 (语义化版本)
    #[serde(default = "default_version")]
    pub version: String,
    
    /// 作者信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    
    /// 标签列表 (用于搜索)
    #[serde(default)]
    pub tags: Vec<String>,
    
    /// 是否启用
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    
    /// 创建时间戳
    #[serde(skip)]
    pub created_at: std::time::Instant,
    
    /// 最后更新时间戳
    #[serde(skip)]
    pub updated_at: std::time::SystemTime,
    
    /// 自定义元数据 (扩展字段)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_enabled() -> bool {
    true
}

impl Default for DynamicTool {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            input_schema: serde_json::Value::Null,
            category: ToolCategory::default(),
            version: default_version(),
            author: None,
            tags: Vec::new(),
            enabled: default_enabled(),
            created_at: std::time::Instant::now(),
            updated_at: std::time::SystemTime::now(),
            metadata: HashMap::new(),
        }
    }
}

/// 工具分类枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    /// 文件操作
    File,
    /// Git 操作
    Git,
    /// Shell 命令
    Shell,
    /// 网络/Web
    Network,
    /// AI/ML 相关
    AiMl,
    /// 部署/DevOps
    Deployment,
    /// 数据库
    Database,
    /// 自定义/其他
    Custom(String),
}

impl Default for ToolCategory {
    fn default() -> Self {
        ToolCategory::Custom("general".to_string())
    }
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File => write!(f, "file"),
            Self::Git => write!(f, "git"),
            Self::Shell => write!(f, "shell"),
            Self::Network => write!(f, "network"),
            Self::AiMl => write!(f, "ai_ml"),
            Self::Deployment => write!(f, "deployment"),
            Self::Database => write!(f, "database"),
            Self::Custom(name) => write!(f, "{}", name),
        }
    }
}

/// 注册结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RegisterResult {
    /// 成功注册
    Success { tool_name: String, version: String },
    /// 已存在 (覆盖)
    Updated { tool_name: String, old_version: String, new_version: String },
    /// 冲突 (同名但不同schema)
    Conflict { tool_name: String, reason: String },
    /// 验证失败
    ValidationFailed { tool_name: String, errors: Vec<String> },
}

/// 注销结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnregisterResult {
    /// 成功注销
    Success { tool_name: String },
    /// 未找到
    NotFound { tool_name: String },
    /// 受保护无法删除
    Protected { tool_name: String, reason: String },
}

/// 工具变更事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolChangeEvent {
    /// 新工具注册
    Registered { name: String, category: ToolCategory },
    /// 工具更新
    Updated { name: String, old_version: String, new_version: String },
    /// 工具注销
    Unregistered { name: String },
    /// 批量变更
    BatchChanged { added: usize, removed: usize, updated: usize },
}

// --- Hooks ------------------------------------------------

/// 注册前钩子 (可拒绝或修改工具)
pub type PreRegisterHook =
    dyn Fn(&DynamicTool) -> std::result::Result<DynamicTool, String> + Send + Sync;

/// 注册后钩子 (用于通知、日志等)
pub type PostRegisterHook = dyn Fn(&DynamicTool) + Send + Sync;

/// 注销前钩子 (可阻止删除)
pub type PreUnregisterHook =
    dyn Fn(&str) -> std::result::Result<(), String> + Send + Sync;

/// 注销后钩子
pub type PostUnregisterHook = dyn Fn(&str) + Send + Sync;

// --- Configuration ---------------------------------------

/// 动态注册配置
#[derive(Debug, Clone)]
pub struct DynamicRegistryConfig {
    /// 最大允许的工具数量
    pub max_tools: usize,
    
    /// 是否允许覆盖已存在的工具
    pub allow_overwrite: bool,
    
    /// 是否启用严格模式 (验证所有schema)
    pub strict_mode: bool,
    
    /// 是否自动发送 tools/listChanged 通知
    pub notify_on_change: bool,
    
    /// 保护的基础工具列表 (不可注销)
    protected_tools: Vec<String>,
}

impl Default for DynamicRegistryConfig {
    fn default() -> Self {
        Self {
            max_tools: 1000,
            allow_overwrite: true,
            strict_mode: false,
            notify_on_change: true,
            protected_tools: vec![
                "read".to_string(),
                "write".to_string(),
                "edit".to_string(),
                "bash".to_string(),
                "grep".to_string(),
            ],
        }
    }
}

// --- Core Registry --------------------------------------

/// 动态工具注册表
pub struct DynamicToolRegistry {
    /// 工具存储 (name -> tool)
    tools: Arc<RwLock<HashMap<String, DynamicTool>>>,
    
    /// 分类索引 (category -> [name])
    category_index: Arc<RwLock<HashMap<ToolCategory, Vec<String>>>>,
    
    /// 标签索引 (tag -> [name])
    tag_index: Arc<RwLock<HashMap<String, Vec<String>>>>,
    
    /// 配置
    config: DynamicRegistryConfig,
    
    /// 变更事件广播通道
    change_tx: broadcast::Sender<ToolChangeEvent>,
    
    /// 钩子
    pre_register_hooks: Vec<Box<PreRegisterHook>>,
    post_register_hooks: Vec<Box<PostRegisterHook>>,
    pre_unregister_hooks: Vec<Box<PreUnregisterHook>>,
    post_unregister_hooks: Vec<Box<PostUnregisterHook>>,
    
    /// 统计信息
    stats: Arc<RwLock<RegistryStats>>,
}

/// 统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryStats {
    /// 总工具数
    pub total_tools: usize,
    /// 启用数
    pub enabled_count: usize,
    /// 按分类统计
    pub by_category: HashMap<String, usize>,
    /// 注册次数
    pub total_registrations: u64,
    /// 注销次数
    pub total_unregistrations: u64,
    /// 最后变更时间
    pub last_changed_at: Option<std::time::SystemTime>,
}

impl DynamicToolRegistry {
    /// 创建新的动态注册表
    pub fn new(config: DynamicRegistryConfig) -> Self {
        let (change_tx, _) = broadcast::channel(256);
        
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            category_index: Arc::new(RwLock::new(HashMap::new())),
            tag_index: Arc::new(RwLock::new(HashMap::new())),
            config,
            change_tx,
            pre_register_hooks: Vec::new(),
            post_register_hooks: Vec::new(),
            pre_unregister_hooks: Vec::new(),
            post_unregister_hooks: Vec::new(),
            stats: Arc::new(RwLock::new(RegistryStats::default())),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(DynamicRegistryConfig::default())
    }

    // --- Registration ----------------------------------

    /// 注册新工具 (或更新已存在的)
    pub async fn register_tool(&self, mut tool: DynamicTool) -> Result<RegisterResult> {
        // 验证工具数量限制
        {
            let tools = self.tools.read().await;
            if tools.len() >= self.config.max_tools && !tools.contains_key(&tool.name) {
                anyhow::bail!("Registry full (max={})", self.config.max_tools);
            }
        }

        // 执行预注册钩子
        for hook in &self.pre_register_hooks {
            match hook(&tool) {
                Ok(modified) => tool = modified,
                Err(reason) => {
                    return Ok(RegisterResult::ValidationFailed {
                        tool_name: tool.name.clone(),
                        errors: vec![reason],
                    });
                }
            }
        }

        // 验证 schema (如果 strict mode开启)
        if self.config.strict_mode {
            if let Err(e) = self.validate_schema(&tool.input_schema) {
                return Ok(RegisterResult::ValidationFailed {
                    tool_name: tool.name.clone(),
                    errors: vec![e.to_string()],
                });
            }
        }

        // 检查是否已存在
        let result = {
            let mut tools = self.tools.write().await;
            
            if let Some(existing) = tools.get(&tool.name) {
                if !self.config.allow_overwrite {
                    return Ok(RegisterResult::Conflict {
                        tool_name: tool.name.clone(),
                        reason: "Tool already exists and overwrite disabled".to_string(),
                    });
                }
                
                // 更新时间戳
                tool.updated_at = std::time::SystemTime::now();
                
                let old_version = existing.version.clone();
                tools.insert(tool.name.clone(), tool.clone());
                
                RegisterResult::Updated {
                    tool_name: tool.name.clone(),
                    old_version,
                    new_version: tool.version.clone(),
                }
            } else {
                // 新工具
                tool.created_at = std::time::Instant::now();
                tool.updated_at = std::time::SystemTime::now();
                
                tools.insert(tool.name.clone(), tool.clone());
                
                RegisterResult::Success {
                    tool_name: tool.name.clone(),
                    version: tool.version.clone(),
                }
            }
        };

        // 更新索引
        self.update_indexes(&tool).await;
        
        // 更新统计
        self.update_stats_registration(&tool).await;
        
        // 发送事件
        if self.config.notify_on_change {
            let event = match &result {
                RegisterResult::Success { .. } => {
                    ToolChangeEvent::Registered {
                        name: tool.name.clone(),
                        category: tool.category.clone(),
                    }
                }
                RegisterResult::Updated { tool_name, old_version: _, new_version: _ } => {
                    ToolChangeEvent::Updated {
                        name: tool_name.clone(),
                        old_version: "0.0.0".to_string(),
                        new_version: tool.version.clone(),
                    }
                }
                _ => return Ok(result),
            };
            
            let _ = self.change_tx.send(event);
        }

        // 执行后置钩子
        for hook in &self.post_register_hooks {
            hook(&tool);
        }

        info!(
            tool = %tool.name,
            version = %tool.version,
            "Dynamic tool registered"
        );

        Ok(result)
    }

    /// 批量注册工具
    pub async fn register_batch(
        &self,
        tools: Vec<DynamicTool>,
    ) -> Result<Vec<RegisterResult>> {
        let mut results = Vec::with_capacity(tools.len());
        let mut added = 0usize;
        let mut updated = 0usize;

        for tool in tools {
            match self.register_tool(tool).await? {
                RegisterResult::Success { .. } => {
                    added += 1;
                    results.push(RegisterResult::Success {
                        tool_name: "batch_item".to_string(),
                        version: "1.0.0".to_string(),
                    });
                }
                RegisterResult::Updated { tool_name, old_version, new_version } => {
                    updated += 1;
                    results.push(RegisterResult::Updated {
                        tool_name,
                        old_version,
                        new_version,
                    });
                }
                other => results.push(other),
            }
        }

        // 发送批量变更事件
        if added > 0 || updated > 0 {
            let _ = self.change_tx.send(ToolChangeEvent::BatchChanged {
                added,
                removed: 0,
                updated,
            });
        }

        Ok(results)
    }

    // --- Unregistration --------------------------------

    /// 注销工具
    pub async fn unregister_tool(&self, name: &str) -> Result<UnregisterResult> {
        // 检查是否受保护
        if self.config.protected_tools.contains(&name.to_string()) {
            return Ok(UnregisterResult::Protected {
                tool_name: name.to_string(),
                reason: "Tool is protected and cannot be unregistered".to_string(),
            });
        }

        // 执行前置钩子
        for hook in &self.pre_unregister_hooks {
            if let Err(reason) = hook(name) {
                return Ok(UnregisterResult::Protected {
                    tool_name: name.to_string(),
                    reason,
                });
            }
        }

        // 从存储中移除
        let removed = {
            let mut tools = self.tools.write().await;
            tools.remove(name).is_some()
        };

        if !removed {
            return Ok(UnregisterResult::NotFound {
                tool_name: name.to_string(),
            });
        }

        // 清理索引
        self.cleanup_indexes(name).await;
        
        // 更新统计
        self.update_stats_unregistration().await;
        
        // 发送事件
        if self.config.notify_on_change {
            let _ = self.change_tx.send(ToolChangeEvent::Unregistered {
                name: name.to_string(),
            });
        }

        // 执行后置钩子
        for hook in &self.post_unregister_hooks {
            hook(name);
        }

        info!(tool = %name, "Dynamic tool unregistered");

        Ok(UnregisterResult::Success {
            tool_name: name.to_string(),
        })
    }

    /// 注销所有非保护工具 (按类别)
    pub async fn unregister_by_category(&self, category: &ToolCategory) -> Result<usize> {
        let names_to_remove = {
            let idx = self.category_index.read().await;
            idx.get(category).cloned().unwrap_or_default()
        };

        let mut count = 0;
        for name in names_to_remove {
            if matches!(
                self.unregister_tool(&name).await?,
                UnregisterResult::Success { .. }
            ) {
                count += 1;
            }
        }

        Ok(count)
    }

    // --- Query -----------------------------------------

    /// 获取所有工具列表
    pub async fn list_tools(&self) -> Vec<DynamicTool> {
        let tools = self.tools.read().await;
        tools.values().cloned().collect()
    }

    /// 仅获取启用的工具
    pub async fn list_enabled_tools(&self) -> Vec<DynamicTool> {
        let tools = self.tools.read().await;
        tools.values().filter(|t| t.enabled).cloned().collect()
    }

    /// 按类别获取工具
    pub async fn list_by_category(&self, category: &ToolCategory) -> Vec<DynamicTool> {
        let names = {
            let idx = self.category_index.read().await;
            idx.get(category).cloned().unwrap_or_default()
        };

        let tools = self.tools.read().await;
        names.iter()
            .filter_map(|name| tools.get(name).cloned())
            .collect()
    }

    /// 按标签搜索工具
    pub async fn search_by_tag(&self, tag: &str) -> Vec<DynamicTool> {
        let names = {
            let idx = self.tag_index.read().await;
            idx.get(tag).cloned().unwrap_or_default()
        };

        let tools = self.tools.read().await;
        names.iter()
            .filter_map(|name| tools.get(name).cloned())
            .collect()
    }

    /// 模糊搜索工具 (名称/描述/标签)
    pub async fn search_fuzzy(&self, query: &str) -> Vec<DynamicTool> {
        let query_lower = query.to_lowercase();
        
        let tools = self.tools.read().await;
        tools.values()
            .filter(|tool| {
                tool.name.to_lowercase().contains(&query_lower)
                    || tool.description.to_lowercase().contains(&query_lower)
                    || tool.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
            })
            .cloned()
            .collect()
    }

    /// 获取单个工具详情
    pub async fn get_tool(&self, name: &str) -> Option<DynamicTool> {
        let tools = self.tools.read().await;
        tools.get(name).cloned()
    }

    /// 检查工具是否存在
    pub async fn contains_tool(&self, name: &str) -> bool {
        let tools = self.tools.read().await;
        tools.contains_key(name)
    }

    // --- Metadata --------------------------------------

    /// 更新工具元数据
    pub async fn update_metadata(
        &self,
        name: &str,
        metadata: HashMap<String, String>,
    ) -> Result<bool> {
        let mut tools = self.tools.write().await;
        
        if let Some(tool) = tools.get_mut(name) {
            tool.metadata.extend(metadata);
            tool.updated_at = std::time::SystemTime::now();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 切换工具启用状态
    pub async fn toggle_enabled(&self, name: &str) -> Result<bool> {
        let mut tools = self.tools.write().await;
        
        if let Some(tool) = tools.get_mut(name) {
            tool.enabled = !tool.enabled;
            Ok(tool.enabled)
        } else {
            anyhow::bail!("Tool not found: {}", name)
        }
    }

    // --- Stats -----------------------------------------

    /// 获取统计信息
    pub async fn get_stats(&self) -> RegistryStats {
        self.stats.read().await.clone()
    }

    /// 获取工具总数
    pub async fn len(&self) -> usize {
        self.tools.read().await.len()
    }

    /// 是否为空
    pub async fn is_empty(&self) -> bool {
        self.tools.read().await.is_empty()
    }

    // --- Event Subscription ---------------------------

    /// 订阅工具变更事件
    pub fn subscribe_changes(&self) -> broadcast::Receiver<ToolChangeEvent> {
        self.change_tx.subscribe()
    }

    // --- Hook Management -----------------------------

    /// 添加预注册钩子
    pub fn add_pre_register_hook<F>(&mut self, hook: F)
    where
        F: Fn(&DynamicTool) -> std::result::Result<DynamicTool, String> + Send + Sync + 'static,
    {
        self.pre_register_hooks.push(Box::new(hook));
    }

    /// 添加后置注册钩子
    pub fn add_post_register_hook<F>(&mut self, hook: F)
    where
        F: Fn(&DynamicTool) + Send + Sync + 'static,
    {
        self.post_register_hooks.push(Box::new(hook));
    }

    /// 添加预注销钩子
    pub fn add_pre_unregister_hook<F>(&mut self, hook: F)
    where
        F: Fn(&str) -> std::result::Result<(), String> + Send + Sync + 'static,
    {
        self.pre_unregister_hooks.push(Box::new(hook));
    }

    /// 添加后置注销钩子
    pub fn add_post_unregister_hook<F>(&mut self, hook: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.post_unregister_hooks.push(Box::new(hook));
    }

    // --- Internal Helpers -----------------------------

    /// 更新分类和标签索引
    async fn update_indexes(&self, tool: &DynamicTool) {
        // 更新分类索引
        {
            let mut idx = self.category_index.write().await;
            idx.entry(tool.category.clone())
                .or_insert_with(Vec::new)
                .push(tool.name.clone());
        }

        // 更新标签索引
        {
            let mut idx = self.tag_index.write().await;
            for tag in &tool.tags {
                idx.entry(tag.clone())
                    .or_insert_with(Vec::new)
                    .push(tool.name.clone());
            }
        }
    }

    /// 清理指定工具的索引条目
    async fn cleanup_indexes(&self, name: &str) {
        // 获取工具的类别和标签
        let (category, tags) = {
            let tools = self.tools.read().await;
            match tools.get(name) {
                Some(t) => (Some(t.category.clone()), t.tags.clone()),
                None => (None, Vec::new()),
            }
        };

        // 从分类索引移除
        if let Some(cat) = category {
            let mut idx = self.category_index.write().await;
            if let Some(names) = idx.get_mut(&cat) {
                names.retain(|n| n != name);
            }
        }

        // 从标签索引移除
        {
            let mut idx = self.tag_index.write().await;
            for tag in &tags {
                if let Some(names) = idx.get_mut(tag) {
                    names.retain(|n| n != name);
                }
            }
        }
    }

    /// 更新注册统计
    async fn update_stats_registration(&self, tool: &DynamicTool) {
        let mut stats = self.stats.write().await;
        stats.total_registrations += 1;
        stats.last_changed_at = Some(std::time::SystemTime::now());

        if tool.enabled {
            stats.enabled_count += 1;
        }

        *stats.by_category.entry(format!("{}", tool.category)).or_insert(0) += 1;
        stats.total_tools = self.tools.read().await.len();
    }

    /// 更新注销统计
    async fn update_stats_unregistration(&self) {
        let mut stats = self.stats.write().await;
        stats.total_unregistrations += 1;
        stats.last_changed_at = Some(std::time::SystemTime::now());
        stats.total_tools = self.tools.read().await.len();

        // 重新计算启用数和分类统计
        let tools = self.tools.read().await;
        stats.enabled_count = tools.values().filter(|t| t.enabled).count();
        stats.by_category.clear();
        for tool in tools.values() {
            *stats.by_category.entry(format!("{}", tool.category)).or_insert(0) += 1;
        }
    }

    /// 验证 JSON Schema 格式
    fn validate_schema(&self, schema: &serde_json::Value) -> Result<()> {
        // 基本结构检查
        if let Some(obj) = schema.as_object() {
            // 必须有 type 字段
            if !obj.contains_key("type") {
                anyhow::bail!("Schema missing 'type' field");
            }

            // 如果是 object，应该有 properties
            if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
                if !obj.contains_key("properties") {
                    anyhow::bail!("Object schema missing 'properties' field");
                }
            }
        } else {
            anyhow::bail!("Schema must be a JSON object");
        }

        Ok(())
    }
}

// --- MCP Integration Helpers ----------------------------

impl DynamicToolRegistry {
    /// 将动态工具转换为 MCP tools/list 格式
    pub async fn to_mcp_tool_list(&self) -> Vec<serde_json::Value> {
        let tools = self.list_enabled_tools().await;
        
        tools.into_iter().map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema
            })
        }).collect()
    }

    /// 从 MCP ExtraToolDef 创建 DynamicTool
    pub fn from_extra_tool_def(def: &crate::mcp::server::ExtraToolDef) -> DynamicTool {
        DynamicTool {
            name: def.name.clone(),
            description: def.description.clone(),
            input_schema: def.input_schema.clone(),
            category: ToolCategory::Custom("external".to_string()),
            version: "1.0.0".to_string(),
            author: None,
            tags: Vec::new(),
            enabled: true,
            created_at: std::time::Instant::now(),
            updated_at: std::time::SystemTime::now(),
            metadata: HashMap::new(),
        }
    }
}

// --- Tests ---------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_list() {
        let registry = DynamicToolRegistry::with_defaults();
        
        let tool = DynamicTool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            }),
            ..Default::default()
        };

        let result = registry.register_tool(tool).await.unwrap();
        assert!(matches!(result, RegisterResult::Success { .. }));

        let tools = registry.list_tools().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
    }

    #[tokio::test]
    async fn test_update_existing_tool() {
        let registry = DynamicToolRegistry::with_defaults();
        
        let tool_v1 = DynamicTool {
            name: "test_tool".to_string(),
            description: "Version 1".to_string(),
            input_schema: json!({"type": "object"}),
            version: "1.0.0".to_string(),
            ..Default::default()
        };
        
        registry.register_tool(tool_v1).await.unwrap();

        let tool_v2 = DynamicTool {
            name: "test_tool".to_string(),
            description: "Version 2".to_string(),
            input_schema: json!({"type": "object"}),
            version: "2.0.0".to_string(),
            ..Default::default()
        };

        let result = registry.register_tool(tool_v2).await.unwrap();
        assert!(matches!(result, RegisterResult::Updated { .. }));
    }

    #[tokio::test]
    async fn test_unregister_protected_tool() {
        let config = DynamicRegistryConfig {
            protected_tools: vec!["protected_tool".to_string()],
            ..Default::default()
        };
        
        let registry = DynamicToolRegistry::new(config);
        
        let tool = DynamicTool {
            name: "protected_tool".to_string(),
            description: "Protected".to_string(),
            input_schema: json!({"type": "object"}),
            ..Default::default()
        };
        
        registry.register_tool(tool).await.unwrap();

        let result = registry.unregister_tool("protected_tool").await.unwrap();
        assert!(matches!(result, UnregisterResult::Protected { .. }));
    }

    #[tokio::test]
    async fn test_search_by_category() {
        let registry = DynamicToolRegistry::with_defaults();
        
        let file_tool = DynamicTool {
            name: "file_op".to_string(),
            description: "File operation".to_string(),
            input_schema: json!({"type": "object"}),
            category: ToolCategory::File,
            ..Default::default()
        };
        
        let git_tool = DynamicTool {
            name: "git_op".to_string(),
            description: "Git operation".to_string(),
            input_schema: json!({"type": "object"}),
            category: ToolCategory::Git,
            ..Default::default()
        };

        registry.register_tool(file_tool).await.unwrap();
        registry.register_tool(git_tool).await.unwrap();

        let file_tools = registry.list_by_category(&ToolCategory::File).await;
        assert_eq!(file_tools.len(), 1);
        assert_eq!(file_tools[0].name, "file_op");
    }

    #[tokio::test]
    async fn test_event_subscription() {
        let registry = DynamicToolRegistry::with_defaults();
        let mut rx = registry.subscribe_changes();
        
        let tool = DynamicTool {
            name: "event_test".to_string(),
            description: "Test events".to_string(),
            input_schema: json!({"type": "object"}),
            ..Default::default()
        };

        registry.register_tool(tool).await.unwrap();

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, ToolChangeEvent::Registered { .. }));

        registry.unregister_tool("event_test").await.unwrap();

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, ToolChangeEvent::Unregistered { .. }));
    }

    #[tokio::test]
    async fn test_max_tools_limit() {
        let config = DynamicRegistryConfig {
            max_tools: 2,
            ..Default::default()
        };
        
        let registry = DynamicToolRegistry::new(config);
        
        let make_tool = |i: u32| DynamicTool {
            name: format!("tool_{}", i),
            description: format!("Tool {}", i),
            input_schema: json!({"type": "object"}),
            ..Default::default()
        };

        registry.register_tool(make_tool(1)).await.unwrap();
        registry.register_tool(make_tool(2)).await.unwrap();
        
        let result = registry.register_tool(make_tool(3)).await;
        assert!(result.is_err());
    }
}
