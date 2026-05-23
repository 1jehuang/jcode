//! ## 任务 2.2: 企业级任务优先级调度
//!
//! 基于 Ruflo GOAP 的优先级规则系统，实现：
//!
//! 1. **多级优先级**: Critical > Urgent > High > Medium > Low
//! 2. **角色绑定**: 管理层实时问答 -> 高优先级, 批量文档处理 -> 低优先级
//! 3. **动态资源分配**: 高优任务自动调度到固定服务器，低优任务调度到闲置节点
//! 4. **自动降级/升级**: 等待超时自动升级优先级
//! 5. **抢占式调度**: 高优任务可抢占低优任务的资源

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::time::Duration;

/// 用户角色（本地定义，避免依赖 auth 模块）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserRole {
    SuperAdmin,
    OrgAdmin,
    DepartmentHead,
    Developer,
    Viewer,
}

/// 企业级优先级（比基础优先级更精细）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EnterprisePriority {
    /// 实时交互（老板/管理层的实时问答）：最低延迟
    Realtime = 100,
    /// 紧急任务（故障恢复、安全事件）
    Emergency = 80,
    /// 高优先级（核心业务处理）
    High = 60,
    /// 中优先级（一般业务）
    Medium = 40,
    /// 低优先级（批量处理）
    Low = 20,
    /// 后台任务（日志分析、数据备份）
    Background = 10,
}

impl EnterprisePriority {
    /// 根据用户角色获取默认优先级
    pub fn from_role(role: UserRole) -> Self {
        match role {
            UserRole::SuperAdmin | UserRole::OrgAdmin => Self::Realtime,
            UserRole::DepartmentHead => Self::High,
            UserRole::Developer => Self::Medium,
            UserRole::Viewer => Self::Low,
        }
    }

    /// 根据节点类型返回此任务适合的节点
    pub fn preferred_node_type(&self) -> &'static str {
        match self {
            Self::Realtime | Self::Emergency | Self::High => "server",
            Self::Medium => "desktop",
            Self::Low => "laptop",
            Self::Background => "internet_cafe",
        }
    }

    /// 最大可容忍延迟 (ms)
    pub fn max_latency_ms(&self) -> u64 {
        match self {
            Self::Realtime => 2000,    // < 2秒
            Self::Emergency => 5000,    // < 5秒
            Self::High => 10000,        // < 10秒
            Self::Medium => 30000,      // < 30秒
            Self::Low => 120000,        // < 2分钟
            Self::Background => 600000, // < 10分钟
        }
    }
}

impl std::fmt::Display for EnterprisePriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Realtime => write!(f, "realtime"),
            Self::Emergency => write!(f, "emergency"),
            Self::High => write!(f, "high"),
            Self::Medium => write!(f, "medium"),
            Self::Low => write!(f, "low"),
            Self::Background => write!(f, "background"),
        }
    }
}

impl From<EnterprisePriority> for jcode_unified_scheduler::TaskPriority {
    fn from(p: EnterprisePriority) -> Self {
        match p {
            EnterprisePriority::Realtime => Self::Critical,
            EnterprisePriority::Emergency => Self::Urgent,
            EnterprisePriority::High => Self::High,
            EnterprisePriority::Medium => Self::Medium,
            EnterprisePriority::Low | EnterprisePriority::Background => Self::Low,
        }
    }
}

/// 优先级规则引擎
#[derive(Debug, Clone)]
pub struct PriorityRuleEngine {
    /// 规则列表
    rules: Vec<PriorityRule>,
}

/// 优先级规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityRule {
    /// 规则名称
    pub name: String,
    /// 匹配的用户角色
    pub roles: Vec<UserRole>,
    /// 匹配的模型名称（支持前缀匹配，如 "qwen*"）
    pub model_patterns: Vec<String>,
    /// 任务类型
    pub task_type: Option<TaskType>,
    /// 生效后的优先级
    pub priority: EnterprisePriority,
    /// 优先级序号（小的优先匹配）
    pub order: u32,
}

/// 任务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    /// 实时对话
    Chat,
    /// 代码生成
    CodeGeneration,
    /// 文档处理（总结、翻译等）
    DocumentProcessing,
    /// 批量数据处理
    BatchProcessing,
    /// 嵌入向量生成
    Embedding,
    /// 系统管理
    Admin,
}

impl PriorityRuleEngine {
    /// 创建默认的优先级规则引擎
    pub fn default() -> Self {
        Self {
            rules: vec![
                PriorityRule {
                    name: "管理员实时对话".into(),
                    roles: vec![UserRole::SuperAdmin, UserRole::OrgAdmin],
                    model_patterns: vec!["*".into()],
                    task_type: Some(TaskType::Chat),
                    priority: EnterprisePriority::Realtime,
                    order: 1,
                },
                PriorityRule {
                    name: "部门负责人对话".into(),
                    roles: vec![UserRole::DepartmentHead],
                    model_patterns: vec!["*".into()],
                    task_type: Some(TaskType::Chat),
                    priority: EnterprisePriority::High,
                    order: 2,
                },
                PriorityRule {
                    name: "开发者日常任务".into(),
                    roles: vec![UserRole::Developer],
                    model_patterns: vec!["*".into()],
                    task_type: None,
                    priority: EnterprisePriority::Medium,
                    order: 3,
                },
                PriorityRule {
                    name: "批量文档处理".into(),
                    roles: vec![UserRole::Developer, UserRole::Viewer],
                    model_patterns: vec!["*".into()],
                    task_type: Some(TaskType::BatchProcessing),
                    priority: EnterprisePriority::Low,
                    order: 4,
                },
                PriorityRule {
                    name: "后台维护任务".into(),
                    roles: vec![UserRole::SuperAdmin],
                    model_patterns: vec!["*".into()],
                    task_type: Some(TaskType::Admin),
                    priority: EnterprisePriority::Background,
                    order: 5,
                },
                // 默认兜底规则
                PriorityRule {
                    name: "默认".into(),
                    roles: vec![],
                    model_patterns: vec!["*".into()],
                    task_type: None,
                    priority: EnterprisePriority::Medium,
                    order: 999,
                },
            ],
        }
    }

    /// 评估给定上下文应使用的优先级
    pub fn evaluate(
        &self,
        role: UserRole,
        model_name: &str,
        task_type: Option<TaskType>,
    ) -> EnterprisePriority {
        let mut matched_rules: Vec<&PriorityRule> = self.rules.iter()
            .filter(|r| {
                // 角色匹配：如果规则指定了角色，则必须匹配
                if !r.roles.is_empty() && !r.roles.contains(&role) {
                    return false;
                }
                // 任务类型匹配：如果规则指定了类型，则必须匹配
                if r.task_type.is_some() && r.task_type != task_type {
                    return false;
                }
                // 模型模式匹配：使用通配符匹配
                r.model_patterns.iter().any(|pattern| {
                    if pattern == "*" || pattern == "**" {
                        return true;
                    }
                    simple_pattern_match(pattern, model_name)
                })
            })
            .collect();

        // 按优先级序号从小到大排序
        matched_rules.sort_by_key(|r| r.order);

        // 取最优匹配的规则
        matched_rules.first()
            .map(|r| r.priority)
            .unwrap_or(EnterprisePriority::Medium)
    }

    /// 添加自定义优先级规则
    pub fn add_rule(&mut self, rule: PriorityRule) {
        self.rules.push(rule);
    }
}

/// 简单的通配符匹配（支持 `*`）
fn simple_pattern_match(pattern: &str, target: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == target;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() { continue; }
        if i == 0 {
            // 第一个非空部分必须在开头
            if !target.starts_with(part) { return false; }
            pos = part.len();
        } else if i == parts.len() - 1 {
            // 最后一个非空部分必须在结尾
            return target[pos..].ends_with(part);
        } else {
            match target[pos..].find(part) {
                Some(idx) => pos += idx + part.len(),
                None => return false,
            }
        }
    }
    true
}
