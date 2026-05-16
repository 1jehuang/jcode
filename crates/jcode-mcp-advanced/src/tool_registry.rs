// ════════════════════════════════════════════════════════════════
// MCP 工具注册表 + 缓存管理
// ════════════════════════════════════════════════════════════════

use crate::types::{McpTool};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 缓存的工具条目
pub struct ToolCacheEntry {
    pub tool: McpTool,
    pub cached_at: Instant,
    /// 来源 server 的标识 (用于多 server 场景)
    pub source_server: String,
}

/// MCP 工具注册表 — 管理从 MCP Server 获取的工具定义
pub struct MCPToolRegistry {
    /// tool_name -> ToolCacheEntry
    tools: HashMap<String, ToolCacheEntry>,
    
    /// TTL for cache invalidation
    ttl: Duration,
}

impl Default for MCPToolRegistry {
    fn default() -> Self {
        Self {
            tools: HashMap::new(),
            ttl: Duration::from_secs(crate::TOOL_CACHE_TTL_SECS),
        }
    }
}

impl MCPToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ttl(ttl_secs: u64) -> Self {
        Self { ttl: Duration::from_secs(ttl_secs), ..Default::default() }
    }

    /// 更新工具列表 (全量替换)
    pub fn update_tools(&mut self, tools: Vec<McpTool>) {
        let now = Instant::now();
        // 先收集名称再遍历，避免move后借用问题
        let tool_names: std::collections::HashSet<String> = tools.iter().map(|t| t.name.clone()).collect();

        for tool in tools {
            self.tools.insert(tool.name.clone(), ToolCacheEntry {
                cached_at: now,
                source_server: "unknown".into(),
                tool,
            });
        }

        // Remove tools that were not in this update
        self.tools.retain(|name, _| tool_names.contains(name));
    }

    /// 获取单个工具定义
    pub fn get(&self, name: &str) -> Option<&McpTool> {
        self.tools.get(name).map(|e| &e.tool)
    }

    /// 获取所有工具
    pub fn get_all(&self) -> Vec<McpTool> {
        self.tools.values().map(|e| e.tool.clone()).collect()
    }

    /// 检查缓存是否过期
    pub fn is_expired(&self) -> bool {
        if self.tools.is_empty() {
            return true;
        }
        
        // If any entry is older than TTL, consider expired
        self.tools.values().any(|e| e.cached_at.elapsed() > self.ttl)
    }

    /// 清空所有缓存
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// 获取工具数量
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
