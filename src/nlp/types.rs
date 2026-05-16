//! NLP (自然语言处理) 能力模块
//!
//! 从Claude Code深度移植的自然语言处理能力，使CarpAI能够：
//! 1. 理解和分析自然语言需求
//! 2. 从自然语言描述生成代码
//! 3. 文档分析和摘要生成
//! 4. 实体识别和关系提取
//! 5. 查询理解和转换
//!
//! ## 核心能力
//!
//! ### 文本分析
//! - 文本分类 (问题类型识别)
//! - 意图理解 (用户想要什么)
//! - 实体提取 (关键信息识别)
//! - 关系抽取 (依赖和关联)
//!
//! ### 代码生成
//! - NL2Code (自然语言->代码)
//! - Code2NL (代码->自然语言文档)
//! - API设计从描述
//! - 数据模型推导
//!
//! ### 文档处理
//! - 自动摘要生成
//! - 关键点提取
//! - FAQ生成
//! - 变更日志生成

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// --- Types ---------------------------------

/// NLP分析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NlpAnalysisResult {
    /// 原始文本
    pub original_text: String,
    
    /// 文本分类
    pub classification: TextClassification,
    
    /// 提取的意图
    pub intent: UserIntent,
    
    /// 识别的实体
    pub entities: Vec<Entity>,
    
    /// 关键概念
    pub key_concepts: Vec<String>,
    
    /// 技术栈推断
    pub inferred_tech_stack: Vec<TechStackItem>,
    
    /// 复杂度评估
    pub complexity: ComplexityAssessment,
    
    /// 生成的任务分解
    pub task_breakdown: Vec<TaskItem>,
    
    /// 置信度 (0.0-1.0)
    pub confidence: f64,
}

/// 文本分类
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TextClassification {
    /// 问题/疑问
    Question,
    
    /// 任务/指令
    Task,
    
    /// 需求描述
    Requirement,
    
    /// Bug报告
    BugReport,
    
    /// 功能请求
    FeatureRequest,
    
    /// 架构设计
    ArchitectureDesign,
    
    /// 代码审查请求
    CodeReview,
    
    /// 文档请求
    Documentation,
    
    /// 迁移/重构请求
    MigrationRefactoring,
    
    /// 性能优化请求
    PerformanceOptimization,
    
    /// 安全相关
    SecurityRelated,
    
    /// 其他
    Other(String),
}

impl std::fmt::Display for TextClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Question => write!(f, "❓ 问题"),
            Self::Task => write!(f, "✅ 任务"),
            Self::Requirement => write!(f, "📋 需求"),
            Self::BugReport => write!(f, "🐛 Bug报告"),
            Self::FeatureRequest => write!(f, "✨ 功能请求"),
            Self::ArchitectureDesign => write!(f, "🏗️ 架构设计"),
            Self::CodeReview => write!(f, "🔍 代码审查"),
            Self::Documentation => write!(f, "📖 文档"),
            Self::MigrationRefactoring => write!(f, "🔄 迁移重构"),
            Self::PerformanceOptimization => write!(f, "⚡ 性能优化"),
            Self::SecurityRelated => write!(f, "🔒 安全"),
            Self::Other(s) => write!(f, "📝 {}", s),
        }
    }
}

/// 用户意图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIntent {
    /// 主要意图
    pub primary: IntentType,
    
    /// 次要意图 (如果有)
    pub secondary: Option<IntentType>,
    
    /// 意图置信度
    pub confidence: f64,
    
    /// 动作动词
    pub action_verbs: Vec<String>,
    
    /// 目标对象
    pub target_objects: Vec<String>,
    
    /// 约束条件
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum IntentType {
    Create,
    Modify,
    Delete,
    Query,
    Analyze,
    Refactor,
    Migrate,
    Optimize,
    Debug,
    Test,
    Document,
    Explain,
    Compare,
    Integrate,
    Validate,
    Generate,
    Other(String),
}

impl std::fmt::Display for IntentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Create => write!(f, "创建"),
            Self::Modify => write!(f, "修改"),
            Self::Delete => write!(f, "删除"),
            Self::Query => write!(f, "查询"),
            Self::Analyze => write!(f, "分析"),
            Self::Refactor => write!(f, "重构"),
            Self::Migrate => write!(f, "迁移"),
            Self::Optimize => write!(f, "优化"),
            Self::Debug => write!(f, "调试"),
            Self::Test => write!(f, "测试"),
            Self::Document => write!(f, "文档化"),
            Self::Explain => write!(f, "解释"),
            Self::Compare => write!(f, "比较"),
            Self::Integrate => write!(f, "集成"),
            Self::Validate => write!(f, "验证"),
            Self::Generate => write!(f, "生成"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

/// 实体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// 实体文本
    pub text: String,
    
    /// 实体类型
    pub entity_type: EntityType,
    
    /// 在原文中的位置
    pub position: (usize, usize),
    
    /// 置信度
    pub confidence: f64,
    
    /// 属性 (如版本号、框架名等)
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntityType {
    // 技术实体
    ProgrammingLanguage,
    Framework,
    Library,
    Database,
    API,
    Protocol,
    Platform,
    Tool,
    
    // 项目实体
    ProjectName,
    ModuleName,
    ClassName,
    FunctionName,
    VariableName,
    FileName,
    
    // 版本和配置
    Version,
    Configuration,
    Setting,
    
    // 业务实体
    Feature,
    Requirement,
    Bug,
    PerformanceMetric,
    SecurityVulnerability,
    
    // 其他
    Other(String),
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProgrammingLanguage => write!(f, "编程语言"),
            Self::Framework => write!(f, "框架"),
            Self::Library => write!(f, "库"),
            Self::Database => write!(f, "数据库"),
            Self::API => write!(f, "API"),
            Self::Protocol => write!(f, "协议"),
            Self::Platform => write!(f, "平台"),
            Self::Tool => write!(f, "工具"),
            Self::ProjectName => write!(f, "项目名"),
            Self::ModuleName => write!(f, "模块名"),
            Self::ClassName => write!(f, "类名"),
            Self::FunctionName => write!(f, "函数名"),
            Self::VariableName => write!(f, "变量名"),
            Self::FileName => write!(f, "文件名"),
            Self::Version => write!(f, "版本"),
            Self::Configuration => write!(f, "配置"),
            Self::Setting => write!(f, "设置"),
            Self::Feature => write!(f, "功能"),
            Self::Requirement => write!(f, "需求"),
            Self::Bug => write!(f, "Bug"),
            Self::PerformanceMetric => write!(f, "性能指标"),
            Self::SecurityVulnerability => write!(f, "安全漏洞"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

/// 技术栈项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechStackItem {
    /// 类别
    pub category: TechCategory,
    
    /// 名称
    pub name: String,
    
    /// 版本 (如果指定)
    pub version: Option<String>,
    
    /// 置信度
    pub confidence: f64,
    
    /// 推理依据
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TechCategory {
    Language,
    Framework,
    Database,
    Runtime,
    DevOps,
    Testing,
    Other(String),
}

impl std::fmt::Display for TechCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TechCategory::Language => write!(f, "语言"),
            TechCategory::Framework => write!(f, "框架"),
            TechCategory::Database => write!(f, "数据库"),
            TechCategory::Runtime => write!(f, "运行时"),
            TechCategory::DevOps => write!(f, "DevOps"),
            TechCategory::Testing => write!(f, "测试"),
            TechCategory::Other(s) => write!(f, "{}", s),
        }
    }
}

/// 复杂度评估
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityAssessment {
    /// 总体复杂度 (1-10)
    pub overall_score: u8,
    
    /// 复杂度级别
    pub level: ComplexityLevel,
    
    /// 各维度评分
    pub dimensions: ComplexityDimensions,
    
    /// 影响因素
    pub factors: Vec<ComplexityFactor>,
    
    /// 建议工时估算 (人天)
    pub estimated_effort_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComplexityLevel {
    Trivial,      // 1-2
    Simple,       // 3-4
    Moderate,     // 5-6
    Complex,      // 7-8
    VeryComplex,  // 9
    Extreme,      // 10
}

impl std::fmt::Display for ComplexityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trivial => write!(f, "🟢 简单 (1-2)"),
            Self::Simple => write!(f, "🟢 较简单 (3-4)"),
            Self::Moderate => write!(f, "🟡 中等 (5-6)"),
            Self::Complex => write!(f, "🟠 复杂 (7-8)"),
            Self::VeryComplex => write!(f, "🔴 很复杂 (9)"),
            Self::Extreme => write!(f, "🔴 极复杂 (10)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityDimensions {
    /// 功能复杂度
    pub functional: u8,
    
    /// 技术复杂度
    pub technical: u8,
    
    /// 集成复杂度
    pub integration: u8,
    
    /// 数据复杂度
    pub data: u8,
    
    /// 业务逻辑复杂度
    pub business_logic: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityFactor {
    /// 因素描述
    pub description: String,
    
    /// 影响程度 (+/-)
    pub impact: i8,
    
    /// 权重
    pub weight: f64,
}

/// 任务项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskItem {
    /// 任务ID
    pub id: usize,
    
    /// 任务标题
    pub title: String,
    
    /// 任务描述
    pub description: String,
    
    /// 任务类型
    pub task_type: TaskType,
    
    /// 优先级
    pub priority: Priority,
    
    /// 估计时间 (分钟)
    pub estimated_minutes: Option<u32>,
    
    /// 依赖的任务ID
    pub dependencies: Vec<usize>,
    
    /// 子任务
    pub sub_tasks: Vec<TaskItem>,
    
    /// 生成的代码片段 (可选)
    pub generated_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskType {
    Analysis,
    Design,
    Implementation,
    Testing,
    Documentation,
    Deployment,
    Configuration,
    Migration,
    Refactoring,
    Optimization,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd, Ord, Eq)]
pub enum Priority {
    Critical,
    High,
    Medium,
    Low,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Critical => write!(f, "🔴 P0-紧急"),
            Self::High => write!(f, "🟠 P1-高"),
            Self::Medium => write!(f, "🟡 P2-中"),
            Self::Low => write!(f, "🟢 P3-低"),
        }
    }
}

// --- Core Engine ------------------------------

/// NLP引擎
pub struct NlpEngine {
    /// 配置
    pub config: NlpConfig,
    
    /// Rust最佳实践知识库 (用于代码质量保证)
    pub knowledge_base: Option<crate::knowledge::RustKnowledgeBase>,
}

#[derive(Debug, Clone)]
pub struct NlpConfig {
    /// 是否启用深度分析
    pub deep_analysis: bool,
    
    /// 最大实体数
    pub max_entities: usize,
    
    /// 支持的语言
    pub languages: Vec<String>,
    
    /// 领域特定词典
    pub domain_dictionaries: HashMap<String, Vec<String>>,
}

impl Default for NlpConfig {
    fn default() -> Self {
        let mut domain_dictionaries = HashMap::new();
        
        // 编程语言关键词
        domain_dictionaries.insert(
            "programming_languages".to_string(),
            vec![
                "rust".to_string(), "c++".to_string(), "typescript".to_string(),
                "java".to_string(), "c#".to_string(), ".net".to_string(),
                "go".to_string(), "python".to_string(), "javascript".to_string(),
                "kotlin".to_string(), "swift".to_string(), "ruby".to_string(),
            ]
        );
        
        // 框架关键词
        domain_dictionaries.insert(
            "frameworks".to_string(),
            vec![
                "spring".to_string(), "asp.net".to_string(), "express".to_string(),
                "django".to_string(), "rails".to_string(), "actix".to_string(),
                "react".to_string(), "vue".to_string(), "angular".to_string(),
                "entity framework".to_string(), "nhibernate".to_string(),
            ]
        );
        
        // 数据库关键词
        domain_dictionaries.insert(
            "databases".to_string(),
            vec![
                "sql server".to_string(), "postgresql".to_string(), "mysql".to_string(),
                "mongodb".to_string(), "redis".to_string(), "elasticsearch".to_string(),
                "oracle".to_string(), "sqlite".to_string(), "dynamodb".to_string(),
            ]
        );
        
        Self {
            deep_analysis: true,
            max_entities: 50,
            languages: vec!["zh-CN".to_string(), "en-US".to_string()],
            domain_dictionaries,
        }
    }
}
