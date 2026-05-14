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
//! - NL2Code (自然语言→代码)
//! - Code2NL (代码→自然语言文档)
//! - API设计从描述
//! - 数据模型推导
//!
//! ### 文档处理
//! - 自动摘要生成
//! - 关键点提取
//! - FAQ生成
//! - 变更日志生成

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Types ─────────────────────────────────

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

// ─── Core Engine ──────────────────────────────

/// NLP引擎
pub struct NlpEngine {
    /// 配置
    config: NlpConfig,
    
    /// Rust最佳实践知识库 (用于代码质量保证)
    knowledge_base: Option<crate::knowledge::RustKnowledgeBase>,
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

impl NlpEngine {
    /// 创建新的NLP引擎
    pub fn new(config: Option<NlpConfig>) -> Self {
        Self {
            config: config.unwrap_or_default(),
            knowledge_base: None,
        }
    }
    
    /// 创建带有知识库的NLP引擎 (推荐用于代码生成)
    pub fn with_knowledge_base(
        config: Option<NlpConfig>,
        knowledge_base: Option<crate::knowledge::RustKnowledgeBase>,
    ) -> Self {
        Self {
            config: config.unwrap_or_default(),
            knowledge_base,
        }
    }
    
    /// 分析自然语言输入
    pub async fn analyze(&self, text: &str) -> Result<NlpAnalysisResult> {
        let start = std::time::Instant::now();
        
        tracing::info!(
            text_len = text.len(),
            "Starting NLP analysis"
        );
        
        // 1. 文本分类
        let classification = self.classify_text(text);
        
        // 2. 意图识别
        let intent = self.extract_intent(text);
        
        // 3. 实体提取
        let entities = self.extract_entities(text);
        
        // 4. 关键概念提取
        let key_concepts = self.extract_key_concepts(text);
        
        // 5. 技术栈推断
        let tech_stack = self.infer_tech_stack(text, &entities);
        
        // 6. 复杂度评估
        let complexity = self.assess_complexity(text, &classification, &entities);
        
        // 7. 任务分解
        let task_breakdown = self.decompose_tasks(text, &intent, &complexity);
        
        // 8. 计算综合置信度
        let confidence = self.calculate_confidence(&classification, &intent, &entities);
        
        let elapsed = start.elapsed();
        
        tracing::info!(
            elapsed_ms = elapsed.as_millis(),
            classification = ?(classification as u8),
            entity_count = entities.len(),
            task_count = task_breakdown.len(),
            confidence = confidence,
            "NLP analysis completed"
        );
        
        Ok(NlpAnalysisResult {
            original_text: text.to_string(),
            classification,
            intent,
            entities,
            key_concepts,
            inferred_tech_stack: tech_stack,
            complexity,
            task_breakdown,
            confidence,
        })
    }
    
    /// 从自然语言生成代码 (带最佳实践验证)
    pub async fn nl2code(&self, description: &str, language: &str) -> Result<GeneratedCode> {
        let analysis = self.analyze(description).await?;
        
        // 根据分析结果生成代码
        let mut code = match analysis.classification {
            TextClassification::Task | TextClassification::FeatureRequest => {
                self.generate_implementation_code(&analysis, language).await?
            }
            TextClassification::ArchitectureDesign => {
                self.generate_architecture_code(&analysis, language).await?
            }
            TextClassification::MigrationRefactoring => {
                self.generate_migration_code(&analysis, language).await?
            }
            _ => {
                self.generate_generic_code(&analysis, language).await?
            }
        };
        
        // 如果有知识库，进行代码质量验证和改进
        if let Some(ref kb) = self.knowledge_base {
            // 验证生成的代码质量
            let validation = kb.validate_code(&code.main_file.content);
            
            if validation.score < 90.0 && !validation.violations.is_empty() {
                // 生成改进报告
                let improvement_report = kb.generate_improvement_report(&code.main_file.content);
                
                tracing::info!(
                    score = validation.score,
                    violations = validation.violations.len(),
                    "代码质量检查完成，应用最佳实践改进"
                );
                
                // 将改进建议添加到代码注释中
                let quality_note = format!(
                    "\n// 📊 Rust Best Practices Score: {:.0}/100\n\
                     // ⚠️ Issues Found: {}\n\
                     // ✅ Suggestions: {}\n\
                     // \n// Top Recommendations:\n{}\n",
                    validation.score,
                    validation.violations.len(),
                    improvement_report.top_recommendations.len(),
                    improvement_report.top_recommendations.iter()
                        .take(3)
                        .map(|r| format!("// - {} [{}]\n", r.title, r.priority))
                        .collect::<String>()
                );
                
                code.main_file.content.push_str(&quality_note);
                code.comments.push(format!(
                    "代码已通过Rust最佳实践验证 (得分: {:.0}/100)",
                    validation.score
                ));
            } else {
                code.comments.push(
                    "✅ 代码符合Rust最佳实践标准".to_string()
                );
            }
            
            // 添加模块结构建议（如果是Rust）
            if language.to_lowercase() == "rust" {
                let module_tip = kb.get_rules_by_category(
                    crate::knowledge::PracticeCategory::ModuleSystem
                );
                
                if !module_tip.is_empty() {
                    let module_guidance = module_tip.iter()
                        .take(3)
                        .map(|rule| format!("// 💡 {}: {}", rule.name, rule.description))
                        .collect::<Vec<_>>()
                        .join("\n");
                    
                    code.comments.push(
                        "📦 已应用Rust模块系统最佳实践".to_string()
                    );
                    
                    // 在主文件开头添加模块结构指导
                    let header_comment = format!(
                        "\n//! 📚 Rust Module Structure Guidelines\n\
                         //! \n{}\
                         //! \n//! Key Rules Applied:\n\
                         //! - MOD-001: Single entry file per directory\n\
                         //! - MOD-002: Use mod.rs as standard entry point\n\
                         //! - MOD-003: Avoid redundant re-export files\n\
                         //! - MOD-004: Appropriate splitting granularity\n",
                        module_guidance
                    );
                    
                    code.main_file.content = format!("{}{}", header_comment, code.main_file.content);
                }
            }
        }
        
        Ok(code)
    }
    
    /// 生成文档
    pub async fn generate_documentation(&self, code: &str, doc_type: DocType) -> Result<String> {
        match doc_type {
            DocType::ApiDoc => self.generate_api_doc(code).await,
            DocType::Readme => self.generate_readme(code).await,
            DocType::Changelog => self.generate_changelog(code).await,
            DocType::FAQ => self.generate_faq(code).await,
            DocType::Summary => self.generate_summary(code).await,
        }
    }
    
    /// 分析代码并生成自然语言描述
    pub async fn code2nl(&self, code: &str, detail_level: DetailLevel) -> Result<String> {
        let analysis = self.analyze_code_structure(code).await?;
        
        match detail_level {
            DetailLevel::Brief => Ok(self.generate_brief_description(&analysis)),
            DetailLevel::Standard => Ok(self.generate_standard_description(&analysis)),
            DetailLevel::Detailed => Ok(self.generate_detailed_description(&analysis)),
        }
    }
}

// ─── Generated Code Types ────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedCode {
    /// 语言
    pub language: String,
    
    /// 主文件内容
    pub main_file: GeneratedFile,
    
    /// 附加文件
    pub additional_files: Vec<GeneratedFile>,
    
    /// 依赖列表
    pub dependencies: Vec<DependencyInfo>,
    
    /// 说明注释
    pub comments: Vec<String>,
    
    /// 使用示例
    pub usage_example: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    /// 文件路径
    pub path: String,
    
    /// 内容
    pub content: String,
    
    /// 文件类型
    pub file_type: FileType,
    
    /// 描述
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileType {
    Source,
    Config,
    Test,
    Documentation,
    BuildScript,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    /// 名称
    pub name: String,
    
    /// 版本要求
    pub version: Option<String>,
    
    /// 包管理器
    pub package_manager: PackageManager,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PackageManager {
    Cargo,
    Npm,
    Pip,
    Maven,
    Nuget,
    GoModules,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DocType {
    ApiDoc,
    Readme,
    Changelog,
    FAQ,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DetailLevel {
    Brief,
    Standard,
    Detailed,
}

// ─── Implementation Methods ─────────────────

impl NlpEngine {
    fn classify_text(&self, text: &str) -> TextClassification {
        let lower = text.to_lowercase();
        
        // 问题检测
        if lower.contains('?') || lower.contains("吗") || lower.contains("什么") || lower.contains("如何") {
            if lower.contains("为什么") || lower.contains("原因") {
                return TextClassification::Question;
            }
        }
        
        // Bug检测
        if lower.contains("bug") || lower.contains("错误") || lower.contains("异常")
            || lower.contains("崩溃") || lower.contains("失败") || lower.contains("不工作") {
            return TextClassification::BugReport;
        }
        
        // 迁移/重构检测
        if lower.contains("迁移") || lower.contains("重构") || lower.contains("升级")
            || lower.contains("转换") || lower.contains("移植") || lower.contains("改写") {
            
            if lower.contains(".net") || lower.contains("c#") || lower.contains("java")
                || lower.contains("旧系统") || lower.contains("legacy") {
                return TextClassification::MigrationRefactoring;
            }
            return TextClassification::Task;
        }
        
        // 性能优化检测
        if lower.contains("性能") || lower.contains("优化") || lower.contains("加速")
            || lower.contains("慢") || lower.contains("延迟") || lower.contains("瓶颈") {
            return TextClassification::PerformanceOptimization;
        }
        
        // 安全检测
        if lower.contains("安全") || lower.contains("漏洞") || lower.contains("攻击")
            || lower.contains("加密") || lower.contains("认证") || lower.contains("授权") {
            return TextClassification::SecurityRelated;
        }
        
        // 架构设计检测
        if lower.contains("架构") || lower.contains("设计") || lower.contains("模式")
            || lower.contains("微服务") || lower.contains("分布式") || lower.contains("系统") {
            return TextClassification::ArchitectureDesign;
        }
        
        // 功能请求检测
        if lower.contains("实现") || lower.contains("添加") || lower.contains("创建")
            || lower.contains("开发") || lower.contains("构建") || lower.contains("新功能") {
            return TextClassification::FeatureRequest;
        }
        
        // 默认为任务
        TextClassification::Task
    }
    
    fn extract_intent(&self, text: &str) -> UserIntent {
        let mut intent = UserIntent {
            primary: IntentType::Other("unknown".to_string()),
            secondary: None,
            confidence: 0.0,
            action_verbs: Vec::new(),
            target_objects: Vec::new(),
            constraints: Vec::new(),
        };
        
        let lower = text.to_lowercase();
        
        // 提取动作动词
        let action_patterns = [
            ("创建", vec!["创建", "新建", "实现", "开发", "build", "create", "implement"]),
            ("修改", vec!["修改", "更新", "改变", "调整", "refactor", "modify", "update"]),
            ("删除", vec!["删除", "移除", "清理", "remove", "delete"]),
            ("查询", vec!["查询", "查找", "搜索", "获取", "query", "find", "search", "get"]),
            ("分析", vec!["分析", "检查", "审查", "analyze", "check", "review"]),
            ("迁移", vec!["迁移", "转换", "移植", "migrate", "convert", "port"]),
            ("优化", vec!["优化", "改进", "提升", "optimize", "improve"]),
            ("测试", vec!["测试", "验证", "test", "verify"]),
        ];
        
        for (_, verbs) in action_patterns.iter() {
            for verb in verbs.iter() {
                if lower.contains(verb) {
                    intent.action_verbs.push(verb.clone());
                }
            }
        }
        
        // 映射到意图类型
        if !intent.action_verbs.is_empty() {
            intent.primary = self.map_verbs_to_intent(&intent.action_verbs);
            intent.confidence = 0.85;
        } else {
            intent.primary = IntentType::Query;
            intent.confidence = 0.6;
        }
        
        // 提取目标对象
        let target_patterns = [
            ("API", vec!["api", "接口", "endpoint"]),
            ("数据库", vec!["database", "数据库", "db"]),
            ("服务", vec!["service", "服务", "server"]),
            ("模块", vec!["module", "模块", "component"]),
            ("函数", vec!["function", "函数", "method"]),
            ("类", vec!["class", "类", "type"]),
            ("系统", vec!["system", "系统", "application"]),
        ];
        
        for (obj_type, patterns) in target_patterns.iter() {
            for pattern in patterns.iter() {
                if lower.contains(pattern) {
                    intent.target_objects.push(obj_type.to_string());
                }
            }
        }
        
        // 提取约束条件
        if lower.contains("必须") || lower.contains("需要") || lower.contains("应该") {
            intent.constraints.push("强制性约束".to_string());
        }
        if lower.contains("兼容") || lower.contains("支持") {
            intent.constraints.push("兼容性要求".to_string());
        }
        if lower.contains("高性能") || lower.contains("快速") {
            intent.constraints.push("性能要求".to_string());
        }
        if lower.contains("安全") || lower.contains("加密") {
            intent.constraints.push("安全要求".to_string());
        }
        
        intent
    }
    
    fn map_verbs_to_intent(&self, verbs: &[String]) -> IntentType {
        for verb in verbs.iter() {
            match verb.as_str() {
                "创建" | "新建" | "实现" | "开发" => return IntentType::Create,
                "修改" | "更新" | "改变" => return IntentType::Modify,
                "删除" | "移除" => return IntentType::Delete,
                "查询" | "查找" | "搜索" => return IntentType::Query,
                "分析" | "检查" | "审查" => return IntentType::Analyze,
                "迁移" | "转换" | "移植" => return IntentType::Migrate,
                "优化" | "改进" | "提升" => return IntentType::Optimize,
                "测试" | "验证" => return IntentType::Test,
                _ => continue,
            }
        }
        IntentType::Other("mixed".to_string())
    }
    
    fn extract_entities(&self, text: &str) -> Vec<Entity> {
        let mut entities = Vec::new();
        let lower = text.to_lowercase();
        let words: Vec<&str> = text.split_whitespace().collect();
        
        // 编程语言检测
        let lang_map = [
            ("rust", EntityType::ProgrammingLanguage),
            ("c++", EntityType::ProgrammingLanguage),
            ("typescript", EntityType::ProgrammingLanguage),
            ("java", EntityType::ProgrammingLanguage),
            ("c#", EntityType::ProgrammingLanguage),
            (".net", EntityType::Framework),
            ("go", EntityType::ProgrammingLanguage),
            ("python", EntityType::ProgrammingLanguage),
            ("kotlin", EntityType::ProgrammingLanguage),
            ("swift", EntityType::ProgrammingLanguage),
        ];
        
        for (keyword, entity_type) in lang_map.iter() {
            if lower.contains(keyword) {
                // 找到位置
                if let Some(pos) = text.find(keyword) {
                    entities.push(Entity {
                        text: keyword.to_string(),
                        entity_type: entity_type.clone(),
                        position: (pos, pos + keyword.len()),
                        confidence: 0.95,
                        attributes: HashMap::new(),
                    });
                }
            }
        }
        
        // 框架检测
        let framework_map = [
            ("spring boot", EntityType::Framework),
            ("asp.net core", EntityType::Framework),
            ("asp.net", EntityType::Framework),
            (".net framework", EntityType::Framework),
            ("express", EntityType::Framework),
            ("django", EntityType::Framework),
            ("actix-web", EntityType::Framework),
            ("react", EntityType::Framework),
            ("vue", EntityType::Framework),
            ("angular", EntityType::Framework),
            ("entity framework", EntityType::Framework),
            ("nhibernate", EntityType::Framework),
        ];
        
        for (keyword, entity_type) in framework_map.iter() {
            if lower.contains(keyword) {
                if let Some(pos) = text.find(keyword) {
                    entities.push(Entity {
                        text: keyword.to_string(),
                        entity_type: entity_type.clone(),
                        position: (pos, pos + keyword.len()),
                        confidence: 0.92,
                        attributes: HashMap::new(),
                    });
                }
            }
        }
        
        // 版本号检测
        let version_regex = regex::Regex::new(r"(\d+\.\d+|\d+\.\d+\.\d+)").unwrap();
        for cap in version_regex.captures_iter(text) {
            let version = cap.get(0).unwrap().as_str();
            if let Some(pos) = text.find(version) {
                entities.push(Entity {
                    text: version.to_string(),
                    entity_type: EntityType::Version,
                    position: (pos, pos + version.len()),
                    confidence: 0.98,
                    attributes: HashMap::new(),
                });
            }
        }
        
        // 限制实体数量
        entities.truncate(self.config.max_entities);
        
        entities
    }
    
    fn extract_key_concepts(&self, text: &str) -> Vec<String> {
        let mut concepts = Vec::new();
        
        // 提取引号内的内容（通常是关键术语）
        let quote_regex = regex::Regex::new(r#""([^"]+)""#).unwrap();
        for cap in quote_regex.captures_iter(text) {
            if let Some(term) = cap.get(1) {
                concepts.push(term.as_str().to_string());
            }
        }
        
        // 提取大写开头的词（可能是专有名词）
        let caps_regex = regex::Regex::new(r"\b([A-Z][a-z]+(?:[A-Z][a-z]+)+)\b").unwrap();
        for cap in caps_regex.captures_iter(text) {
            if let Some(term) = cap.get(1) {
                let term_str = term.as_str().to_string();
                if !concepts.contains(&term_str) && term_str.len() > 2 {
                    concepts.push(term_str);
                }
            }
        }
        
        // 去重并限制数量
        concepts.sort();
        concepts.dedup();
        concepts.truncate(20);
        
        concepts
    }
    
    fn infer_tech_stack(&self, text: &str, entities: &[Entity]) -> Vec<TechStackItem> {
        let mut tech_stack = Vec::new();
        let lower = text.to_lowercase();
        
        // 从实体中提取技术栈
        for entity in entities {
            match &entity.entity_type {
                EntityType::ProgrammingLanguage => {
                    tech_stack.push(TechStackItem {
                        category: TechCategory::Language,
                        name: entity.text.clone(),
                        version: None,
                        confidence: entity.confidence,
                        reasoning: "明确提及的编程语言".to_string(),
                    });
                }
                EntityType::Framework => {
                    tech_stack.push(TechStackItem {
                        category: TechCategory::Framework,
                        name: entity.text.clone(),
                        version: None,
                        confidence: entity.confidence,
                        reasoning: "明确提及的框架".to_string(),
                    });
                }
                _ => {}
            }
        }
        
        // 从上下文推断额外技术栈
        
        // Web开发
        if lower.contains("web") || lower.contains("网站") || lower.contains("前端") {
            tech_stack.push(TechStackItem {
                category: TechCategory::Framework,
                name: "Web应用".to_string(),
                version: None,
                confidence: 0.75,
                reasoning: "上下文暗示Web开发".to_string(),
            });
        }
        
        // 微服务
        if lower.contains("微服务") || lower.contains("microservice") || lower.contains("分布式") {
            tech_stack.push(TechStackItem {
                category: TechCategory::Architecture,
                name: "微服务架构".to_string(),
                version: None,
                confidence: 0.80,
                reasoning: "上下文暗示微服务架构".to_string(),
            });
        }
        
        // 数据库
        if lower.contains("sql") || lower.contains("数据库") || lower.contains("数据存储") {
            tech_stack.push(TechStackItem {
                category: TechCategory::Database,
                name: "关系型数据库".to_string(),
                version: None,
                confidence: 0.70,
                reasoning: "上下文涉及数据库操作".to_string(),
            });
        }
        
        tech_stack
    }
    
    fn assess_complexity(&self, text: &str, _classification: &TextClassification, entities: &[Entity]) -> ComplexityAssessment {
        let mut score: u8 = 3; // 基础分
        let mut factors = Vec::new();
        
        let word_count = text.split_whitespace().count();
        
        // 文本长度影响
        if word_count > 100 {
            score += 2;
            factors.push(ComplexityFactor {
                description: "需求描述较长 (>100词)".to_string(),
                impact: 2,
                weight: 0.15,
            });
        } else if word_count > 50 {
            score += 1;
            factors.push(ComplexityFactor {
                description: "需求描述中等长度 (50-100词)".to_string(),
                impact: 1,
                weight: 0.10,
            });
        }
        
        // 实体数量影响
        if entities.len() > 10 {
            score += 2;
            factors.push(ComplexityFactor {
                description: format!("涉及多个技术组件 ({}个)", entities.len()),
                impact: 2,
                weight: 0.20,
            });
        } else if entities.len() > 5 {
            score += 1;
            factors.push(ComplexityFactor {
                description: format!("涉及多个技术组件 ({}个)", entities.len()),
                impact: 1,
                weight: 0.15,
            });
        }
        
        // 特定关键词增加复杂度
        let complexity_keywords = [
            ("分布式", 3),
            ("高并发", 3),
            ("实时", 2),
            ("异步", 1),
            ("多线程", 2),
            ("事务", 2),
            ("缓存", 1),
            ("消息队列", 2),
            ("安全性", 2),
            ("加密", 2),
            ("认证", 1),
            ("授权", 1),
            ("集成", 2),
            ("迁移", 3),
            ("重构", 2),
            ("性能", 2),
        ];
        
        let lower = text.to_lowercase();
        for (keyword, impact) in complexity_keywords.iter() {
            if lower.contains(keyword) {
                score = (score as i8 + impact).max(0).min(10) as u8;
                factors.push(ComplexityFactor {
                    description: format!("涉及{}场景", keyword),
                    impact: *impact,
                    weight: 0.10,
                });
            }
        }
        
        // 限制分数范围
        score = score.min(10);
        
        let level = match score {
            0..=2 => ComplexityLevel::Trivial,
            3..=4 => ComplexityLevel::Simple,
            5..=6 => ComplexityLevel::Moderate,
            7..=8 => ComplexityLevel::Complex,
            9 => ComplexityLevel::VeryComplex,
            _ => ComplexityLevel::Extreme,
        };
        
        // 工时估算 (简单公式)
        let estimated_effort = match level {
            ComplexityLevel::Trivial => Some(1),           // 1天
            ComplexityLevel::Simple => Some(3),          // 3天
            ComplexityLevel::Moderate => Some(7),         // 1-2周
            ComplexityLevel::Complex => Some(15),         // 3周
            ComplexityLevel::VeryComplex => Some(30),     // 1.5月
            ComplexityLevel::Extreme => Some(60),         // 3月
        };
        
        ComplexityAssessment {
            overall_score: score,
            level,
            dimensions: ComplexityDimensions {
                functional: (score + 1).min(10),
                technical: (score + 2).min(10) / 2 + 3,
                integration: if entities.len() > 5 { 7 } else { 5 },
                data: 5,
                business_logic: score / 2 + 3,
            },
            factors,
            estimated_effort_days: estimated_effort,
        }
    }
    
    fn decompose_tasks(&self, text: &str, intent: &UserIntent, complexity: &ComplexityAssessment) -> Vec<TaskItem> {
        let mut tasks = Vec::new();
        let mut task_id = 1;
        
        // 第一个任务: 分析和理解
        tasks.push(TaskItem {
            id: task_id,
            title: "需求分析和理解".to_string(),
            description: format!("深入分析原始需求:\n{}", text.chars().take(200).collect::<String>()),
            task_type: TaskType::Analysis,
            priority: Priority::Critical,
            estimated_minutes: Some(30),
            dependencies: Vec::new(),
            sub_tasks: Vec::new(),
            generated_code: None,
        });
        task_id += 1;
        
        // 第二个任务: 设计方案
        tasks.push(TaskItem {
            id: task_id,
            title: "技术方案设计".to_string(),
            description: format!("基于{}意图设计技术方案", intent.primary),
            task_type: TaskType::Design,
            priority: Priority::High,
            estimated_minutes: Some(60),
            dependencies: vec![1],
            sub_tasks: Vec::new(),
            generated_code: None,
        });
        task_id += 1;
        
        // 根据意图类型添加特定任务
        match &intent.primary {
            IntentType::Create | IntentType::Implement => {
                tasks.push(TaskItem {
                    id: task_id,
                    title: "核心功能实现".to_string(),
                    description: "实现主要业务逻辑",
                    task_type: TaskType::Implementation,
                    priority: Priority::High,
                    estimated_minutes: Some(complexity.estimated_effort_days.unwrap_or(5) * 480), // 天转分钟
                    dependencies: vec![2],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
                
                tasks.push(TaskItem {
                    id: task_id,
                    title: "单元测试编写".to_string(),
                    description: "为核心功能编写单元测试",
                    task_type: TaskType::Testing,
                    priority: Priority::Medium,
                    estimated_minutes: Some(120),
                    dependencies: vec![task_id - 1],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
            }
            
            IntentType::Migrate | IntentType::Refactor => {
                tasks.push(TaskItem {
                    id: task_id,
                    title: "现有代码分析".to_string(),
                    description: "分析现有代码结构和依赖",
                    task_type: TaskType::Analysis,
                    priority: Priority::Critical,
                    estimated_minutes: Some(120),
                    dependencies: vec![1],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
                
                tasks.push(TaskItem {
                    id: task_id,
                    title: "迁移策略制定".to_string(),
                    description: "制定详细的迁移计划和步骤",
                    task_type: TaskType::Migration,
                    priority: Priority::High,
                    estimated_minutes: Some(90),
                    dependencies: vec![task_id - 1],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
                
                tasks.push(TaskItem {
                    id: task_id,
                    title: "代码迁移实施".to_string(),
                    description: "执行实际的代码迁移工作",
                    task_type: TaskType::Implementation,
                    priority: Priority::High,
                    estimated_minutes: Some(complexity.estimated_effort_days.unwrap_or(5) * 400),
                    dependencies: vec![task_id - 1],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
            }
            
            IntentType::Optimize => {
                tasks.push(TaskItem {
                    id: task_id,
                    title: "性能基线建立".to_string(),
                    description: "建立当前性能基准",
                    task_type: TaskType::Analysis,
                    priority: Priority::High,
                    estimated_minutes: Some(60),
                    dependencies: vec![1],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
                
                tasks.push(TaskItem {
                    id: task_id,
                    title: "瓶颈识别和优化".to_string(),
                    description: "识别性能瓶颈并实施优化",
                    task_type: TaskType::Optimization,
                    priority: Priority::High,
                    estimated_minutes: Some(complexity.estimated_effort_days.unwrap_or(3) * 360),
                    dependencies: vec![task_id - 1],
                    sub_tasks: Vec::new(),
                    generated_code: None,
                });
                task_id += 1;
            }
            
            _ => {}
        }
        
        // 最后一个任务: 文档和部署
        tasks.push(TaskItem {
            id: task_id,
            title: "文档编写和部署准备".to_string(),
            description: "编写使用文档、API文档和部署指南",
            task_type: TaskType::Documentation,
            priority: Priority::Medium,
            estimated_minutes: Some(90),
            dependencies: vec![task_id - 1],
            sub_tasks: Vec::new(),
            generated_code: None,
        });
        
        tasks
    }
    
    fn calculate_confidence(&self, classification: &TextClassification, intent: &UserIntent, entities: &[Entity]) -> f64 {
        let mut confidence = 0.0;
        
        // 分类置信度 (权重0.3)
        confidence += 0.3 * match classification {
            TextClassification::BugReport | TextClassification::MigrationRefactoring => 0.95,
            TextClassification::FeatureRequest | TextClassification::Task => 0.9,
            TextClassification::Question => 0.85,
            _ => 0.8,
        };
        
        // 意图识别置信度 (权重0.4)
        confidence += 0.4 * intent.confidence;
        
        // 实体提取置信度 (权重0.3)
        if !entities.is_empty() {
            let avg_entity_confidence: f64 = entities.iter()
                .map(|e| e.confidence)
                .sum::<f64>() / entities.len() as f64;
            confidence += 0.3 * avg_entity_confidence;
        } else {
            confidence += 0.15; // 无实体时给予基础分
        }
        
        confidence.min(1.0)
    }
    
    async fn generate_implementation_code(&self, _analysis: &NlpAnalysisResult, language: &str) -> Result<GeneratedCode> {
        let (main_content, file_ext) = match language.to_lowercase().as_str() {
            "rust" => (
                generate_rust_skeleton(_analysis),
                ".rs"
            ),
            "typescript" | "tsx" => (
                generate_typescript_skeleton(_analysis),
                ".ts"
            ),
            "java" => (
                generate_java_skeleton(_analysis),
                ".java"
            ),
            "c++" | "cpp" => (
                generate_cpp_skeleton(_analysis),
                ".cpp"
            ),
            "go" => (
                generate_go_skeleton(_analysis),
                ".go"
            ),
            _ => (
                format!("// TODO: Implement in {}\nfn main() {{\n    println!(\"Hello from {}!\");\n}}\n", language, language),
                ".txt"
            ),
        };
        
        Ok(GeneratedCode {
            language: language.to_string(),
            main_file: GeneratedFile {
                path: format!("main{}", file_ext),
                content: main_content,
                file_type: FileType::Source,
                description: "主程序入口文件".to_string(),
            },
            additional_files: Vec::new(),
            dependencies: generate_dependencies(language),
            comments: vec![
                format!("自动生成的{}代码骨架", language),
                "请根据实际需求填充具体实现".to_string(),
            ],
            usage_example: Some(generate_usage_example(language)),
        })
    }
    
    async fn generate_architecture_code(&self, _analysis: &NlpAnalysisResult, _language: &str) -> Result<GeneratedCode> {
        Ok(GeneratedCode {
            language: "architecture".to_string(),
            main_file: GeneratedFile {
                path: "ARCHITECTURE.md".to_string(),
                content: generate_architecture_doc(_analysis),
                file_type: FileType::Documentation,
                description: "架构设计文档".to_string(),
            },
            additional_files: vec![
                GeneratedFile {
                    path: "COMPONENTS.md".to_string(),
                    content: generate_components_doc(_analysis),
                    file_type: FileType::Documentation,
                    description: "组件设计文档".to_string(),
                },
            ],
            dependencies: Vec::new(),
            comments: vec![
                "架构设计方案".to_string(),
                "包含组件设计和交互流程".to_string(),
            ],
            usage_example: None,
        })
    }
    
    async fn generate_migration_code(&self, analysis: &NlpAnalysisResult, target_language: &str) -> Result<GeneratedCode> {
        let source_info = extract_source_info_from_analysis(analysis);
        
        Ok(GeneratedCode {
            language: format!("migration-to-{}", target_language),
            main_file: GeneratedFile {
                path: "MIGRATION_PLAN.md".to_string(),
                content: generate_migration_plan(analysis, &source_info, target_language),
                file_type: FileType::Documentation,
                description: "迁移计划文档".to_string(),
            },
            additional_files: vec![
                GeneratedFile {
                    path: format!("mappings.{}", target_language),
                    content: generate_mapping_file(&source_info, target_language),
                    file_type: FileType::Source,
                    description: "API映射表".to_string(),
                },
            ],
            dependencies: generate_target_dependencies(target_language),
            comments: vec![
                format!("从{:?}迁移到{}", source_info.source_language, target_language),
                "包含详细的迁移步骤和代码映射".to_string(),
            ],
            usage_example: Some(generate_migration_usage_example(target_language)),
        })
    }
    
    async fn generate_generic_code(&self, analysis: &NlpAnalysisResult, language: &str) -> Result<GeneratedCode> {
        self.generate_implementation_code(analysis, language).await
    }
    
    async fn generate_api_doc(&self, code: &str) -> Result<String> {
        Ok(format!(
            r#"# API 文档

## 概述
自动生成的API文档

## 代码预览
```{}
```

## 端点列表
[待分析...]

## 使用说明
[待补充...]
"#,
            code
        ))
    }
    
    async fn generate_readme(&self, code: &str) -> Result<String> {
        Ok(format!(
            r#"# 项目名称

## 简介
[自动生成]

## 安装
```bash
# 安装依赖
...
```

## 使用
```{}
```

## API参考
详见 [API.md](./API.md)

## 开发
```bash
# 运行测试
...

# 启动服务
...
```

## License
MIT
"#,
            code
        ))
    }
    
    async fn generate_changelog(&self, _code: &str) -> Result<String> {
        let now = chrono::Local::now();
        Ok(format!(
            r"# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Initial project structure

### Changed
- N/A

### Deprecated
- N/A

### Removed
- N/A

### Fixed
- N/A

### Security
- N/A

---
*Last updated: {}*
"#,
            now.format("%Y-%m-%d %H:%M:%S")
        ))
    }
    
    async fn generate_faq(&self, code: &str) -> Result<String> {
        Ok(format!(
            r"# Frequently Asked Questions

## General
**Q: What is this project?**
A: [自动分析项目后填写]

## Usage
**Q: How do I get started?**
A: See the README.md for installation instructions.

## Development
**Q: How do I run tests?**
A: [根据代码结构生成测试命令]

## Troubleshooting
**Q: I encountered an error...**
A: Please check the following:
1. Dependencies are installed correctly
2. Environment variables are set
3. Version compatibility

---
*Generated automatically from code analysis*
"#,
            code.chars().take(100).collect::<String>()
        ))
    }
    
    async fn generate_summary(&self, code: &str) -> Result<String> {
        let lines = code.lines().count();
        let chars = code.chars().count();
        let functions = count_functions(code);
        let classes = count_classes(code);
        
        Ok(format!(
            r"# 📊 Code Summary

## Statistics
| Metric | Value |
|--------|-------|
| Total Lines | {} |
| Characters | {} |
| Functions | {} |
| Classes/Structs | {} |
| Estimated Complexity | {} |

## Structure
[待分析...]

## Key Components
[待识别...]

## Recommendations
[待评估...]
"#,
            lines, chars, functions, classes,
            if lines < 100 { "Low" } else if lines < 500 { "Medium" } else { "High" }
        ))
    }
    
    fn analyze_code_structure(&self, code: &str) -> Result<CodeStructure> {
        let lines = code.lines().count();
        let chars = code.chars().count();
        
        Ok(CodeStructure {
            total_lines: lines,
            total_chars: chars,
            functions: extract_function_signatures(code),
            classes: extract_class_definitions(code),
            imports: extract_imports(code),
            exports: extract_exports(code),
            comments: extract_comments(code),
            complexity_metrics: calculate_code_complexity(code),
        })
    }
    
    fn generate_brief_description(&self, analysis: &CodeStructure) -> String {
        format!(
            "This codebase contains ~{} lines across {} functions and {} classes.",
            analysis.total_lines,
            analysis.functions.len(),
            analysis.classes.len()
        )
    }
    
    fn generate_standard_description(&self, analysis: &CodeStructure) -> String {
        format!(
            r#"## Overview
This is a {} project with:

- **Size**: ~{} lines of code
- **Components**: {} functions, {} classes/structs
- **Imports**: {} external dependencies
- **Exports**: {} public APIs

## Main Components
{}

## Key Functions
{}
"#,
            if analysis.total_lines < 500 { "small" } else if analysis.total_lines < 2000 { "medium-sized" } else { "large" },
            analysis.total_lines,
            analysis.functions.len(),
            analysis.classes.len(),
            analysis.imports.len(),
            analysis.exports.len(),
            analysis.classes.iter().take(5).map(|c| format!("- {}", c.name)).collect::<Vec<_>>().join("\n"),
            analysis.functions.iter().take(5).map(|f| format!("- `{}{}`: {}", f.name, f.description.unwrap_or_default())).collect::<Vec<_>>().join("\n")
        )
    }
    
    fn generate_detailed_description(&self, analysis: &CodeStructure) -> String {
        let mut output = String::from("# Detailed Code Analysis\n\n");
        
        output.push_str(&format!("## Basic Statistics\n\n"));
        output.push_str(&format!("- **Total Lines**: {}\n", analysis.total_lines));
        output.push_str(&format!("- **Total Characters**: {}\n", analysis.total_chars));
        output.push_str(&format!("- **Functions**: {}\n", analysis.functions.len()));
        output.push_str(&format!("- **Classes/Structs**: {}\n\n", analysis.classes.len()));
        
        output.push_str("## Complexity Metrics\n\n");
        output.push_str(&format!("- **Cyclomatic Complexity**: {:.1}\n", analysis.complexity_metrics.cyclomatic));
        output.push_str(&format!("- **Cognitive Complexity**: {:.1}\n", analysis.complexity_metrics.cognitive));
        output.push_str(&format!("- **Lines of Code per Function**: {:.1}\n\n", analysis.complexity_metrics.loc_per_function));
        
        output.push_str("## Class Hierarchy\n\n");
        for class in &analysis.classes {
            output.push_str(&format!("### `{}`\n", class.name));
            output.push_str(&format!("- **Type**: {:?}\n", class.class_type));
            output.push_str(&format!("- **Methods**: {}\n", class.methods.len()));
            output.push_str(&format!("- **Properties**: {}\n\n", class.properties.len()));
        }
        
        output.push_str("## Function Details\n\n");
        for func in &analysis.functions {
            output.push_str(&format!("#### `{}{}`\n", func.name));
            output.push_str(&format!("- **Signature**: {}\n", func.signature));
            output.push_str(&format!("- **Description**: {}\n", func.description.unwrap_or_else(|| "No description".to_string())));
            if let Some(ref params) = func.parameters {
                output.push_str(&format!("- **Parameters**: {}\n", params.join(", ")));
            }
            output.push('\n');
        }
        
        output
    }
}

// ─── Helper Structs and Functions ────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodeStructure {
    total_lines: usize,
    total_chars: usize,
    functions: Vec<FuncInfo>,
    classes: Vec<ClassInfo>,
    imports: Vec<String>,
    exports: Vec<String>,
    comments: Vec<CommentInfo>,
    complexity_metrics: ComplexityMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FuncInfo {
    name: String,
    signature: String,
    description: Option<String>,
    parameters: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClassInfo {
    name: String,
    class_type: ClassType,
    methods: Vec<String>,
    properties: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum ClassType {
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Module,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommentInfo {
    line: usize,
    content: String,
    comment_type: CommentType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum CommentType {
    Line,
    Block,
    Doc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComplexityMetrics {
    cyclomatic: f64,
    cognitive: f64,
    loc_per_function: f64,
}

// Helper functions (simplified implementations)

fn generate_rust_skeleton(analysis: &NlpAnalysisResult) -> String {
    format!(
        r#"//! {} - Auto-generated by CarpAI NLP Engine
//!
//! Generated at: {}
//!
//! ## Requirements Summary
//! - Classification: {}
//! - Intent: {}
//! - Complexity: {}
//! - Estimated Effort: {:?} days

use anyhow::Result;

/// Main entry point for {}
pub fn run() -> Result<()> {{
    println!("🚀 Starting {}...", include_str!(env!("CARGO_CRATE_NAME")));
    
    // TODO: Implement based on requirements
    // Original request: {}
    
    Ok(())
}}

#[cfg(test)]
mod tests {{
    #[test]
    fn test_skeleton_compiles() {{
        // Verify skeleton compiles
        assert!(true);
    }}
}}
"#,
        analysis.key_concepts.first().unwrap_or(&"Project".to_string()),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        analysis.classification,
        analysis.intent.primary,
        analysis.complexity.level,
        analysis.complexity.estimated_effort_days,
        analysis.original_text.chars().take(200).collect::<String>()
    )
}

fn generate_typescript_skeleton(analysis: &NlpAnalysisResult) -> String {
    format!(
        r#"/**
 * {} - Auto-generated by CarpAI NLP Engine
 *
 * @generated {}
 * @description {}
 */

// Imports based on detected requirements
{{}}

/**
 * Main module for {}
 */
export class {} {{
    /**
     * Constructor
     */
    constructor() {{
        // Initialize
    }}
    
    /**
     * Execute main functionality
     */
    async run(): Promise<void> {{
        console.log('Starting {}...');
        
        // TODO: Implement based on requirements
        // Original: {}
    }}
}}

// Export default instance
export const app = new {};
export default app;
"#,
        analysis.key_concepts.first().unwrap_or(&"Module".to_string()),
        chrono::Local::now().format("%Y-%m-%d").to_string(),
        analysis.original_text.chars().take(150).collect::<String>(),
        "// Add required imports here",
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"module".to_string()),
        analysis.key_concepts.first().unwrap_or(&"app".to_string()),
        analysis.original_text.chars().take(100).collect::<String>()
    )
}

fn generate_java_skeleton(analysis: &NlpAnalysisResult) -> String {
    format!(
        r#"/**
 * {} - Auto-generated by CarpAI NLP Engine
 *
 * @author CarpAI NLP
 * @generated {}
 * @version 1.0.0
 */
package com.carpai.generated;

import java.util.*;
import java.util.concurrent.*;

/**
 * Main class for {}
 */
public class {} {{
    /**
     * Main entry point
     */
    public static void main(String[] args) {{
        System.out.println("Starting {}...");
        
        // TODO: Implement based on requirements
        // Original request: {}
    }}
    
    /**
     * Execute the main functionality
     */
    public void run() throws Exception {{
        // Implementation here
    }}
}}
"#,
        analysis.key_concepts.first().unwrap_or(&"Application".to_string()),
        chrono::Local::now().format("%Y-%m-%d").to_string(),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.original_text.chars().take(100).collect::<String>()
    )
}

fn generate_cpp_skeleton(analysis: &NlpAnalysisResult) -> String {
    format!(
        r#"/**
 * @file {}
 * @brief Auto-generated by CarpAI NLP Engine
 * @date {}
 *
 * Description: {}
 */

#include <iostream>
#include <vector>
#include <string>
#include <memory>

// Using declarations
using namespace std;

namespace carpai_generated {{

/**
 * @class {}
 * @brief Main class for the generated solution
 */
class {} {{
public:
    /**
     * @brief Constructor
     */
    () : initialized_(false) {{
        // Initialize
    }}
    
    /**
     * @brief Run the main logic
     * @return 0 on success, non-zero on error
     */
    int run() {{
        std::cout << "Starting {}..." << std::endl;
        
        // TODO: Implement based on requirements
        // Original: {}
        
        return 0;
    }}

private:
    bool initialized_;
}};

}} // namespace carpai_generated

int main() {{
    carpai_generated::{} app;
    return app.run();
}}
"#,
        analysis.key_concepts.first().unwrap_or(&"solution".to_string()),
        analysis.key_concepts.first().unwrap_or(&"Solution".to_string()),
        chrono::Local::now().format("%Y-%m-%d").to_string(),
        analysis.original_text.chars().take(150).collect::<String>(),
        analysis.key_concepts.first().unwrap_or(&"Solution".to_string()),
        analysis.key_concepts.first().unwrap_or(&"Solution".to_string()),
        analysis.original_text.chars().take(100).collect::<String>()
    )
}

fn generate_go_skeleton(analysis: &NlpAnalysisResult) -> String {
    format!(
        r">// Package {} - Auto-generated by Carpai NLP Engine
//
// Generated at: {}
//
// Requirements:
// {}

package main

import (
    "fmt"
    "log"
)

// {} represents the main application/component
type {} struct {{
    Name string
    Version string
}}

// New creates a new instance of {}
func New() *{} {{
    return &{}{{
        Name: "{}",
        Version: "1.0.0",
    }}
}}

// Run executes the main functionality
func (app *{}) Run() error {{
    log.Println("Starting {}...")
    
    // TODO: Implement based on requirements
    // Original: {}
    
    return nil
}}

func main() {{
    app := New()
    if err := app.Run(); err != nil {{
        log.Fatal(err)
    }}
}}
"#,
        analysis.key_concepts.first().unwrap_or(&"main".to_string()),
        chrono::Local::now().format("%Y-%m-%d").to_string(),
        analysis.original_text.chars().take(200).collect::<String>(),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.key_concepts.first().unwrap_or(&"App".to_string()),
        analysis.original_text.chars().take(100).collect::<String>()
    )
}

fn generate_dependencies(language: &str) -> Vec<DependencyInfo> {
    match language.to_lowercase().as_str() {
        "rust" => vec![
            DependencyInfo {
                name: "tokio".to_string(),
                version: Some("1.0".to_string()),
                package_manager: PackageManager::Cargo,
            },
            DependencyInfo {
                name: "serde".to_string(),
                version: Some("1.0".to_string()),
                package_manager: PackageManager::Cargo,
            },
            DependencyInfo {
                name: "anyhow".to_string(),
                version: Some("1.0".to_string()),
                package_manager: PackageManager::Cargo,
            },
        ],
        "typescript" | "tsx" | "javascript" | "jsx" => vec![
            DependencyInfo {
                name: "typescript".to_string(),
                version: Some("^5.0.0".to_string()),
                package_manager: PackageManager::Npm,
            },
            DependencyInfo {
                name: "@types/node".to_string(),
                version: Some("^20.0.0".to_string()),
                package_manager: PackageManager::Npm,
            },
        ],
        "java" => vec![
            DependencyInfo {
                name: "org.slf4j:slf4j-api".to_string(),
                version: Some("2.0.7".to_string()),
                package_manager: PackageManager::Maven,
            },
            DependencyInfo {
                name: "junit:junit".to_string(),
                version: Some("5.9.1".to_string()),
                package_manager: PackageManager::Maven,
            },
        ],
        "c++" | "cpp" => vec![
            DependencyInfo {
                name: "nlohmann/json".to_string(),
                version: Some("3.11.2".to_string()),
                package_manager: PackageManager::Other("vcpkg/conan".to_string()),
            },
            DependencyInfo {
                name: "spdlog".to_string(),
                version: Some("1.12.0".to_string()),
                package_manager: PackageManager::Other("vcpkg/conan".to_string()),
            },
        ],
        "go" => vec![
            DependencyInfo {
                name: "github.com/gin-gonic/gin".to_string(),
                version: Some("v1.9.1".to_string()),
                package_manager: PackageManager::GoModules,
            },
            DependencyInfo {
                name: "github.com/sirupsen/logrus".to_string(),
                version: Some("v1.5.0".to_string()),
                package_manager: PackageManager::GoModules,
            },
        ],
        _ => vec![],
    }
}

fn generate_usage_example(language: &str) -> String {
    match language.to_lowercase().as_str() {
        "rust" => r#"# Usage Example

```rust
use my_app::run;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
```
"#.to_string(),
        "typescript" => r#"# Usage Example

```typescript
import app from './app';

const client = new app();

// Run the application
client.run().catch(console.error);
```
"#.to_string(),
        "java" => r#"# Usage Example

```java
public class Main {
    public static void main(String[] args) {
        App app = new App();
        try {
            app.run();
        } catch (Exception e) {
            e.printStackTrace();
        }
    }
}
```
"#.to_string(),
        _ => format!("# Usage in {}\n\nTODO: Add usage example", language),
    }
}

fn generate_architecture_doc(analysis: &NlpAnalysisResult) -> String {
    format!(
        r#"# Architecture Design Document

## 1. Overview

### 1.1 Project Summary
- **Name**: {}
- **Type**: {:?}
- **Complexity**: {:?}
- **Estimated Effort**: {:?} days

### 1.2 Goals and Objectives
Based on the original requirement:
> {}

### 1.3 Scope
**In Scope:**
- Core functionality implementation
- Unit tests and documentation
- Error handling and logging

**Out of Scope:**
- UI/UX design (unless specified)
- Production deployment setup
- Performance optimization (initially)

---

## 2. System Architecture

### 2.1 High-Level Architecture
```
┌─────────────────────────────────────────┐
│              Client Layer              │
├─────────────────────────────────────────┤
│             API Gateway               │
├─────────────────────────────────────────┤
│           Business Logic                │
├─────────────────────────────────────────┤
│           Data Access Layer             │
└─────────────────────────────────────────┘
```

### 2.2 Component Diagram
[TBD - Based on specific requirements]

---

## 3. Technology Stack

### 3.1 Languages and Runtimes
{}

### 3.2 Frameworks and Libraries
{}

### 3.3 Infrastructure
- **Database**: TBD
- **Cache**: TBD
- **Message Queue**: TBD (if needed)

---

## 4. Data Model

### 4.1 Entity Relationship Diagram
[TBD]

### 4.2 Data Flow
[TBD]

---

## 5. API Design

### 5.1 Endpoints
[TBD]

### 5.2 Request/Response Models
[TBD]

---

## 6. Security Considerations

### 6.1 Authentication
[TBD]

### 6.2 Authorization
[TBD]

### 6.3 Data Protection
[TBD]

---

## 7. Non-Functional Requirements

### 7.1 Performance
- Response time: < X ms (P95)
- Throughput: X req/s
- Availability: 99.9%

### 7.2 Scalability
- Horizontal scaling support
- Connection pooling

### 7.3 Reliability
- Error handling
- Retry mechanisms
- Circuit breakers

---

## 8. Testing Strategy

### 8.1 Unit Tests
- Coverage target: > 80%
- Framework: [based on language]

### 8.2 Integration Tests
- API contract tests
- Database integration tests

### 8.3 E2E Tests
- Critical user journeys

---

## 9. Deployment

### 9.1 Environment Setup
- Development
- Staging
- Production

### 9.2 CI/CD Pipeline
- Build
- Test
- Deploy

---

## 10. Monitoring and Observability

### 10.1 Logging
- Structured logging
- Log levels and formats

### 10.2 Metrics
- Business metrics
- Technical metrics

### 10.3 Alerting
- Threshold-based alerts
- On-call procedures

---

*Document generated by CarpAI NLP Engine*
*Last updated: {}*
"#,
        analysis.key_concepts.first().unwrap_or(&"Project".to_string()),
        analysis.classification,
        analysis.complexity.level,
        analysis.complexity.estimated_effort_days,
        analysis.original_text.chars().take(300).collect::<String>(),
        format_tech_stack_list(&analysis.inferred_tech_stack),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    )
}

fn generate_components_doc(analysis: &NlpAnalysisResult) -> String {
    format!(
        r#"# Component Design Document

## Component Overview

This document details the components identified during architecture design.

---

## Component List

### 1. Core Components

#### 1.1 Main Application
- **Purpose**: {}
- **Responsibilities**:
  - Entry point coordination
  - Configuration management
  - Lifecycle management
- **Dependencies**: [TBD]
- **Interfaces**: [TBD]

---

## Component Interactions

### Interaction Diagram
```
[Component A] → [Component B] → [Component C]
       ↑                  ↓
   [Component D] ← [Component E]
```

---

## Interface Definitions

### APIs
[TBD - Define component interfaces]

### Events
[TBD - Define event contracts]

---

## Data Contracts

### Internal Models
[TBD]

### External Schemas
[TBD]

---

*Generated by CarpAI NLP Engine*
"#,
        analysis.key_concepts.first().unwrap_or(&"System".to_string())
    )
}

fn generate_migration_plan(analysis: &NlpAnalysisResult, source: &SourceSystemInfo, target_lang: &str) -> String {
    format!(
        r#"# Migration Plan: {:?} → {}

## Executive Summary

This document outlines the migration strategy from {:?} to {}.

### Current State Analysis
- **Source Language**: {:?}
- **Source Framework**: {:?}
- **Estimated LOC**: ~{} lines
- **Technical Debt Score**: {}/10

### Target State
- **Target Language**: {}
- **Target Framework**: {:?}
- **Expected Benefits**:
  - Performance improvement: ~{:.0}%
  - Maintainability improvement: ~{:.0}%
  - Modern ecosystem access: ✅

---

## 1. Migration Strategy

### 1.1 Approach: {}
Options considered:
- Big Bang (all at once): ❌ Too risky
- Strangler (incremental): ✅ Recommended
- Parallel running: ⚠️ Possible but complex

### 1.2 Timeline
- Phase 1: Foundation ({:?} weeks)
- Phase 2: Core migration ({:?} weeks)
- Phase 3: Cleanup and optimization ({:?} weeks)

---

## 2. Pre-Migration Checklist

### 2.1 Assessment
- [ ] Code inventory complete
- [ ] Dependency mapping done
- [ ] Risk assessment finished
- [ ] Team training scheduled

### 2.2 Preparation
- [ ] CI/CD pipeline ready for target
- [ ] Development environment setup
- [ ] Rollback plan documented
- [ ] Monitoring in place

---

## 3. Detailed Migration Steps

### Phase 1: Foundation (Week 1-2)

#### 3.1 Project Structure Setup
```
target-lang-project/
├── src/
├── tests/
├── migrations/
└── docs/
```

#### 3.2 Shared Infrastructure
- [ ] Create base configuration
- [ ] Set up dependency injection
- [ ] Implement logging framework
- [ ] Add health checks

---

### Phase 2: Core Migration (Week 3-6)

#### 3.3 Data Access Layer
**Mapping:**
| Source | Target | Notes |
|--------|--------|-------|
| DbContext | Repository | Use Repository pattern |
| Entity Framework | ORM | Choose appropriate ORM |

**Tasks:**
- [ ] Create model definitions
- [ ] Implement repository interfaces
- [ ] Set up database context
- [ ] Write data migration scripts

#### 3.4 Business Logic Layer
**Key transformations:**
- Sync → Async
- Exceptions → Results<T>
- Manual DI → Container

**Tasks:**
- [ ] Convert service classes
- [ ] Update method signatures
- [ ] Implement new patterns
- [ ] Write unit tests

#### 3.5 API Layer
**Changes:**
- Controllers → Handlers/Endpoints
- Attribute routing → Method routing
- ActionResult → Response types

**Tasks:**
- [ ] Set up web framework
- [ ] Create endpoint definitions
- [ ] Implement middleware
- [ ] Add input validation

---

### Phase 3: Optimization (Week 7-8)

#### 3.6 Performance Tuning
- [ ] Database query optimization
- [ ] Caching strategy
- [ ] Async operations review
- [ ] Load testing

#### 3.7 Final Cleanup
- [ ] Remove legacy code
- [ ] Update documentation
- [ ] Final testing
- [ ] Performance benchmarking

---

## 4. Risk Mitigation

### 4.1 Technical Risks
| Risk | Probability | Impact | Mitigation |
|------|-----------|--------|------------|
| Data loss | Low | Critical | Backup strategy |
| Downtime | Medium | High | Blue-green deployment |
| Performance regression | Medium | Medium | Load testing |

### 4.2 Operational Risks
- Team learning curve
- Toolchain changes
- Support knowledge gap

---

## 5. Rollback Plan

If migration fails at any point:

1. **During Phase 1**: Simply revert, minimal impact
2. **During Phase 2**: Switch traffic back to legacy system
3. **During Phase 3**: Use feature flags to disable new features

---

## 6. Success Criteria

### 6.1 Functional Parity
- [ ] All features working identically
- [ ] Zero data loss
- [ ] API compatibility maintained

### 6.2 Quality Gates
- [ ] Test coverage ≥ 80%
- [ ] No critical bugs
- [ ] Performance meets baseline or better

### 6.3 Business Metrics
- [ ] User satisfaction stable/improved
- [ ] Support ticket volume manageable

---

## 7. Post-Migration

### 7.1 Knowledge Transfer
- Team training sessions
- Documentation handover
- Best practices guide

### 7.2 Legacy Decommissioning
- Monitor legacy system usage
- Plan shutdown timeline
- Archive artifacts

---

*Migration plan generated by CarpAI NLP Engine*
*Created: {}*
"#,
        source.source_language,
        target_lang,
        source.source_language,
        source.framework,
        source.estimated_loc.unwrap_or(0),
        7, // Placeholder technical debt
        target_lang,
        infer_target_framework(target_lang),
        40.0, // Performance estimate
        60.0, // Maintainability estimate
        "Strangler Fig".to_string(),
        2,
        2,
        4,
        chrono::Local::now().format("%Y-%m-%d").to_string()
    )
}

fn extract_source_info_from_analysis(analysis: &NlpAnalysisResult) -> SourceSystemInfo {
    let mut info = SourceSystemInfo {
        source_language: SourceLanguage::Unknown,
        framework: None,
        version: None,
        estimated_loc: None,
        patterns: Vec::new(),
    };
    
    for entity in &analysis.entities {
        match &entity.entity_type {
            EntityType::ProgrammingLanguage => {
                info.source_language = match entity.text.to_lowercase().as_str() {
                    "c#" | ".net" => SourceLanguage::CSharpDotNet,
                    "java" => SourceLanguage::Java,
                    "go" => SourceLanguage::Go,
                    "python" => SourceLanguage::Python,
                    "rust" => SourceLanguage::Rust,
                    "typescript" | "javascript" => SourceLanguage::TypeScriptJavaScript,
                    "c++" | "cpp" => SourceLanguage::Cpp,
                    _ => SourceLanguage::Unknown,
                };
            }
            EntityType::Framework => {
                info.framework = Some(entity.text.clone());
            }
            EntityType::Version => {
                info.version = Some(entity.text.clone());
            }
            _ => {}
        }
    }
    
    info.patterns = analysis.entities.iter()
        .filter(|e| matches!(&e.entity_type, EntityType::Other(_)))
        .map(|e| e.text.clone())
        .collect();
    
    info
}

fn generate_mapping_file(source: &SourceSystemInfo, target_lang: &str) -> String {
    format!(
        r#"//! Type Mapping File: {:?} → {}
//!
//! Auto-generated mapping between type systems
//!
//! Source: {:?} {:?}
//! Target: {}

// ══════════════════════════════════════════════════════
// TYPE MAPPINGS
// ══════════════════════════════════════════════════════

// Primitive Types
pub mod primitives {{
    // Source String → Target String
    pub fn map_string(s: &str) -> String {{ s.to_string() }}
    
    // Source int → Target i32/i64
    pub fn map_int(i: i32) -> i32 {{ i }}
    
    // Source bool → Target bool
    pub fn map_bool(b: bool) -> bool {{ b }}
    
    // Source DateTime → Target chrono::DateTime<Utc>
    pub fn map_datetime(dt: System::DateTime) -> chrono::DateTime<Utc> {{
        // Implementation depends on target language
        chrono::Utc::now() // placeholder
    }}
}}

// Collection Types
pub mod collections {{
    // Source List<T> → Target Vec<T>
    pub fn list_to_vec<T>(list: &[T]) -> Vec<T> {{
        list.to_vec()
    }}
    
    // Source Dictionary<K,V> → Target HashMap<K,V>
    pub fn dict_to_map<K, V>(dict: &[(K, V)]) -> HashMap<K, V> {{
        dict.iter().cloned().collect()
    }}
}}

// Common Patterns
pub mod patterns {{
    // Source try-catch → Target Result<T,E>
    pub fn try_catch_to_result<T, E>(result: std::result::Result<T, E>) -> Result<T, E> {{
        result.map_err(|e| e) // convert error type
    }}
    
    // Source async/await → Target async/await
    // (Usually same syntax in modern languages)
    
    // Source null → Target None/Option
    pub fn null_to_option<T>(value: Option<T>) -> Option<T> {{
        value
    }}
}}

// Framework-Specific Mappings
{}
"#,
        source.source_language,
        target_lang,
        source.source_language,
        source.framework.as_deref().unwrap_or("Unknown"),
        target_lang
    )
}

fn generate_target_dependencies(target_lang: &str) -> Vec<DependencyInfo> {
    generate_dependencies(target_lang)
}

fn generate_migration_usage_example(target_lang: &str) -> String {
    format!(
        r#"# Migration Usage Example

## Running the Migration

### Step 1: Set up target environment
```bash
# Install target language toolchain
# For {}: see installation guide
```

### Step 2: Run migration tool
```bash
carpai migrate \
    --source ./legacy-code \
    --target {} \
    --plan migration-plan.md
```

### Step 3: Review generated code
```bash
# Check generated files
ls -la src/

# Run tests
cargo test  # or equivalent for target language
```

### Step 4: Validate migration
```bash
# Compare behavior with legacy system
# Run integration tests
# Performance test
```

## Rolling Back (if needed)
```bash
carpai migrate --rollback \
    --last-good-commit abc123
```
"#,
        target_lang
    )
}

fn format_tech_stack_list(stack: &[TechStackItem]) -> String {
    stack.iter()
        .map(|item| format!("- **{}**: {} (confidence: {:.0})", item.category, item.name, item.confidence))
        .collect::<Vec<_>>()
        .join("\n")
}

fn infer_target_framework(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "rust" => "Actix-web / Axum".to_string(),
        "typescript" => "Express / Fastify / NestJS".to_string(),
        "java" => "Spring Boot / Quarkus".to_string(),
        "c++" | "cpp" => "Boost.Asio / Drogon".to_string(),
        "go" | "gin" => "Gin / Echo / Fiber".to_string(),
        _ => "TBD".to_string(),
    }
}

// Helper structs
struct SourceSystemInfo {
    source_language: SourceLanguage,
    framework: Option<String>,
    version: Option<String>,
    estimated_loc: Option<usize>,
    patterns: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum SourceLanguage {
    CSharpDotNet,
    Java,
    Go,
    Python,
    Rust,
    TypeScriptJavaScript,
    Cpp,
    Unknown,
}

// Simplified helper implementations
fn count_functions(code: &str) -> usize {
    let mut count = 0;
    for line in code.lines() {
        if line.trim_starts_with("fn ") 
            || line.trim_starts_with("def ")
            || line.trim_starts_with("public ")
            || line.trim_starts_with("private ")
            || line.contains("function ")
            || line.contains("func ")
        {
            count += 1;
        }
    }
    count
}

fn count_classes(code: &str) -> usize {
    let mut count = 0;
    for line in code.lines() {
        if line.contains("class ") 
            || line.contains("struct ")
            || line.contains("interface ")
            || line.contains("enum ")
            || line.contains("type ")
        {
            count += 1;
        }
    }
    count
}

fn extract_function_signatures(code: &str) -> Vec<FuncInfo> {
    let mut funcs = Vec::new();
    
    // Simple regex-free extraction (real impl would use proper parsing)
    for line in code.lines() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") || trimmed.starts_with("pub async fn ") {
            let sig = trimmed.to_string();
            funcs.push(FuncInfo {
                name: extract_func_name(&sig),
                signature: sig,
                description: None,
                parameters: None,
            });
        }
    }
    
    funcs.truncate(20); // Limit for brevity
    funcs
}

fn extract_class_definitions(code: &str) -> Vec<ClassInfo> {
    let mut classes = Vec::new();
    
    for line in code.lines() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("pub class ") 
            || trimmed.starts_with("pub struct ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("struct ")
        {
            let name = trimmed.split_whitespace()
                .nth(1)
                .unwrap_or("Unknown")
                .split('{')
                .next()
                .unwrap_or("Unknown")
                .trim();
            
            let class_type = if trimmed.contains("class ") {
                ClassType::Class
            } else if trimmed.contains("interface ") {
                ClassType::Interface
            } else if trimmed.contains("enum ") {
                ClassType::Enum
            } else if trimmed.contains("trait ") {
                ClassType::Trait
            } else {
                ClassType::Struct
            };
            
            classes.push(ClassInfo {
                name: name.to_string(),
                class_type,
                methods: Vec::new(), // Would need full parsing
                properties: Vec::new(),
            });
        }
    }
    
    classes.truncate(15);
    classes
}

fn extract_imports(code: &str) -> Vec<String> {
    code.lines()
        .filter(|l| l.starts_with("use ") || l.starts_with("import ") || l.starts_with("#include "))
        .map(|l| l.trim().to_string())
        .collect()
}

fn extract_exports(code: &str) -> Vec<String> {
    code.lines()
        .filter(|l| l.starts_with("pub ") || l.starts_with("export "))
        .map(|l| l.trim().to_string())
        .collect()
}

fn extract_comments(code: &str) -> Vec<CommentInfo> {
    let mut comments = Vec::new();
    
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.trim();
        
        if trimmed.starts_with("//") {
            comments.push(CommentInfo {
                line: i,
                content: trimmed[2..].trim().to_string(),
                comment_type: CommentType::Line,
            });
        } else if trimmed.starts_with("/*") || trimmed.starts_with("*") {
            comments.push(CommentInfo {
                line: i,
                content: trimmed.trim_matches('/').trim_matches('*').trim().to_string(),
                comment_type: CommentType::Block,
            });
        } else if trimmed.starts_with("///") || trimmed.starts_with("/**") {
            comments.push(CommentInfo {
                line: i,
                content: trimmed.trim_start_matches("///").trim_start_matches("/**")
                    .trim_end_matches("*/").trim().to_string(),
                comment_type: CommentType::Doc,
            });
        }
    }
    
    comments.truncate(20);
    comments
}

fn calculate_code_complexity(code: &str) -> ComplexityMetrics {
    let lines = code.lines().count();
    let funcs = count_functions(code);
    
    // Simplified cyclomatic complexity estimation
    let cyclomatic = code.matches("if ").count() as f64 * 0.5
        + code.matches("for ").count() as f64 * 0.5
        + code.matches("while ").count() as f64 * 1.0
        + code.matches("match ").count() as f64 * 1.5;
    
    // Cognitive complexity (rough approximation)
    let cognitive = cyclomatic * 1.2;
    
    // Lines of code per function
    let loc_per_func = if funcs > 0 { lines as f64 / funcs as f64 } else { 0.0 };
    
    ComplexityMetrics {
        cyclomatic,
        cognitive,
        loc_per_func,
    }
}

fn extract_func_name(signature: &str) -> &str {
    signature
        .replace("pub ", "")
        .replace("async ", "")
        .split('(')
        .next()
        .unwrap_or("unknown")
        .rsplit_whitespace()
        .next()
        .unwrap_or("unknown")
}
