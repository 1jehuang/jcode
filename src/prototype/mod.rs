//! 快速原型开发引擎 (Rapid Prototype Engine)
//!
//! 从Claude Code深度借鉴的快速原型能力：
//! 1. 从自然语言描述快速生成完整项目骨架
//! 2. 自动选择最优技术栈
//! 3. 生成MVP (Minimum Viable Product) 代码
//! 4. 支持多种项目类型 (Web/API/Desktop/Mobile/CLI)
//! 5. 自动生成测试、文档和配置文件

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use crate::nlp::types::Priority;

/// File type for generated project files
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FileType {
    Source,
    Test,
    Configuration,
    Documentation,
    Script,
    Asset,
    Other,
}

// --- Types ---------------------------------

/// Helper macro to create a ProjectFile with String content
#[allow(unused_macros)]
macro_rules! project_file {
    (source: $path:expr, $content:literal, $purpose:expr) => {
        ProjectFile {
            path: $path.to_string(),
            content: $content.to_string(),
            file_type: FileType::Source,
            purpose: $purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    };
    (config: $path:expr, $content:literal, $purpose:expr) => {
        ProjectFile {
            path: $path.to_string(),
            content: $content.to_string(),
            file_type: FileType::Configuration,
            purpose: $purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    };
    (doc: $path:expr, $content:literal, $purpose:expr) => {
        ProjectFile {
            path: $path.to_string(),
            content: $content.to_string(),
            file_type: FileType::Documentation,
            purpose: $purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    };
    (test: $path:expr, $content:literal, $purpose:expr) => {
        ProjectFile {
            path: $path.to_string(),
            content: $content.to_string(),
            file_type: FileType::Test,
            purpose: $purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    };
}

impl From<&'static str> for FileType {
    fn from(s: &'static str) -> Self {
        match s {
            "Source" => FileType::Source,
            "Test" => FileType::Test,
            "Configuration" => FileType::Configuration,
            "Documentation" => FileType::Documentation,
            "Script" => FileType::Script,
            "Asset" => FileType::Asset,
            _ => FileType::Other,
        }
    }
}

/// 原型配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrototypeConfig {
    /// 项目名称
    pub project_name: String,
    
    /// 项目描述
    pub description: String,
    
    /// 项目类型
    pub project_type: ProjectType,
    
    /// 目标语言 (可选，自动推断)
    pub target_language: Option<String>,
    
    /// 目标框架 (可选，自动选择)
    pub target_framework: Option<String>,
    
    /// 特性需求
    pub features: Vec<FeatureRequirement>,
    
    /// 约束条件
    pub constraints: PrototypeConstraints,
    
    /// 输出选项
    pub output_options: OutputOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProjectType {
    WebApp,
    ApiRest,
    ApiGraphQL,
    Desktop,
    Mobile,
    CliTool,
    Library,
    Microservice,
    Monolith,
}

impl Default for ProjectType {
    fn default() -> Self {
        ProjectType::WebApp
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureRequirement {
    /// 功能ID
    pub id: String,
    
    /// 功能名称
    pub name: String,
    
    /// 描述
    pub description: String,
    
    /// 优先级
    pub priority: Priority,
    
    /// 是否必需
    pub is_required: bool,
    
    /// 复杂度估计
    pub estimated_complexity: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrototypeConstraints {
    /// 最大响应时间 (ms)
    pub max_response_time_ms: Option<u32>,
    
    /// 并发用户数
    pub concurrent_users: Option<u32>,
    
    /// 数据存储要求
    pub storage_requirements: StorageType,
    
    /// 安全级别
    pub security_level: SecurityLevel,
    
    /// 预算约束 (影响技术栈选择)
    pub budget_tier: BudgetTier,
    
    /// 团队规模 (影响架构选择)
    pub team_size: TeamSize,
    
    /// 时间限制 (周)
    pub time_constraint_weeks: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StorageType {
    Relational,
    NoSQLDocument,
    NoSQLKeyVal,
    Graph,
    TimeSeries,
    FileBased,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SecurityLevel {
    Basic,
    Standard,
    High,
    Enterprise,
    Government,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BudgetTier {
    Bootstrap,
    Startup,
    MidMarket,
    Enterprise,
    EnterprisePlus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TeamSize {
    Solo,
    Small(usize), // 2-5
    Medium(usize), // 6-15
    Large(usize), // 16-50
    Enterprise(usize), // 50+
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputOptions {
    /// 是否生成完整项目结构
    pub generate_full_project: bool,
    
    /// 是否包含测试代码
    pub include_tests: bool,
    
    /// 是否生成文档
    pub generate_docs: bool,
    
    /// 是否包含CI/CD配置
    pub include_cicd: bool,
    
    /// 是否包含Docker配置
    pub include_docker: bool,
    
    /// 代码风格偏好
    pub code_style: CodeStyle,
    
    /// 注释语言
    pub comment_language: CommentLanguage,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            generate_full_project: true,
            include_tests: true,
            generate_docs: true,
            include_cicd: false,
            include_docker: false,
            code_style: CodeStyle::Idiomatic,
            comment_language: CommentLanguage::English,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum CodeStyle {
    #[default]
    Idiomatic,
    Functional,
    OOP,
    Minimal,
    Verbose,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum CommentLanguage {
    #[default]
    English,
    Chinese,
    Bilingual,
}

// --- Generated Project ----------------------

/// 生成的项目结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedProject {
    /// 项目元数据
    pub metadata: ProjectMetadata,
    
    /// 文件树
    pub file_tree: Vec<ProjectFile>,
    
    /// 技术栈决策
    pub tech_stack: TechStackDecision,
    
    /// 架构说明
    pub architecture: ArchitectureOverview,
    
    /// 实施指南
    pub implementation_guide: ImplementationGuide,
    
    /// 风险评估
    pub risk_assessment: RiskAssessment,
    
    /// 成本估算
    pub cost_estimate: CostEstimate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub name: String,
    pub version: String,
    pub description: String,
    pub generated_at: String,
    pub generator_version: String,
    pub total_files: usize,
    pub total_lines_of_code: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub path: String,
    #[serde(serialize_with = "serialize_content", deserialize_with = "deserialize_content")]
    pub content: Cow<'static, str>,
    pub file_type: FileType,
    pub purpose: String,
    pub is_entry_point: bool,
    pub dependencies: Vec<String>,
}

fn serialize_content<S>(content: &Cow<'static, str>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&content)
}

fn deserialize_content<'de, D>(deserializer: D) -> Result<Cow<'static, str>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(Cow::Owned(s))
}

impl ProjectFile {
    /// Create a new ProjectFile with source type
    pub fn source(path: &str, content: impl Into<Cow<'static, str>>, purpose: &str) -> Self {
        Self {
            path: path.to_string(),
            content: content.into(),
            file_type: FileType::Source,
            purpose: purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    }

    /// Create a new ProjectFile with config type
    pub fn config(path: &str, content: impl Into<Cow<'static, str>>, purpose: &str) -> Self {
        Self {
            path: path.to_string(),
            content: content.into(),
            file_type: FileType::Configuration,
            purpose: purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    }

    /// Create a new ProjectFile with documentation type
    pub fn doc(path: &str, content: impl Into<Cow<'static, str>>, purpose: &str) -> Self {
        Self {
            path: path.to_string(),
            content: content.into(),
            file_type: FileType::Documentation,
            purpose: purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    }

    /// Create a new ProjectFile with test type
    pub fn test(path: &str, content: impl Into<Cow<'static, str>>, purpose: &str) -> Self {
        Self {
            path: path.to_string(),
            content: content.into(),
            file_type: FileType::Test,
            purpose: purpose.to_string(),
            is_entry_point: false,
            dependencies: vec![],
        }
    }

    /// Create a new ProjectFile with entry point
    pub fn entry_point(path: &str, content: impl Into<Cow<'static, str>>, purpose: &str, deps: Vec<String>) -> Self {
        Self {
            path: path.to_string(),
            content: content.into(),
            file_type: FileType::Source,
            purpose: purpose.to_string(),
            is_entry_point: true,
            dependencies: deps,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechStackDecision {
    /// 语言选择及理由
    pub language: LanguageChoice,
    
    /// 框架选择及理由
    pub framework: FrameworkChoice,
    
    /// 数据库选择
    pub database: DatabaseChoice,
    
    /// 基础设施组件
    pub infrastructure: InfrastructureChoices,
    
    /// 开发工具链
    pub toolchain: ToolchainChoices,
    
    /// 决策依据
    pub rationale: DecisionRationale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageChoice {
    pub name: String,
    pub version: String,
    pub reasoning: String,
    pub alternatives_considered: Vec<String>,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameworkChoice {
    pub name: String,
    pub version: String,
    pub reasoning: String,
    pub key_features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseChoice {
    pub primary: DatabaseOption,
    pub secondary: Option<DatabaseOption>,
    pub caching: Option<CachingOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseOption {
    pub name: String,
    pub type_: StorageType,
    pub reasoning: String,
    pub use_case: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachingOption {
    pub name: String,
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfrastructureChoices {
    pub web_server: Option<String>,
    pub message_queue: Option<String>,
    pub search_engine: Option<String>,
    pub monitoring: Vec<String>,
    pub cdn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainChoices {
    pub package_manager: String,
    pub build_tool: String,
    pub test_framework: String,
    pub linter: String,
    pub formatter: String,
    pub ci_cd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRationale {
    pub performance_weight: f64,
    pub developer_experience_weight: f64,
    pub ecosystem_weight: f64,
    pub cost_weight: f64,
    pub community_support_weight: f64,
    pub learning_curve_weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureOverview {
    pub pattern: ArchitecturePattern,
    pub layers: Vec<LayerDescription>,
    pub data_flow: DataFlowDiagram,
    pub api_design: ApiDesign,
    pub security_model: SecurityModel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ArchitecturePattern {
    Layered,
    Hexagonal,
    EventDriven,
    Microservices,
    MonolithicModular,
    CleanArchitecture,
    CQRS,
    Serverless,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDescription {
    pub name: String,
    pub responsibility: String,
    pub technologies: Vec<String>,
    pub interfaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFlowDiagram {
    pub description: String,
    pub components: Vec<DataFlowComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFlowComponent {
    pub name: String,
    pub type_: ComponentType,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComponentType {
    Client,
    Gateway,
    Service,
    Database,
    Cache,
    Queue,
    ExternalService,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiDesign {
    pub style: ApiStyle,
    pub base_path: String,
    pub endpoints: Vec<ApiEndpoint>,
    pub authentication: AuthMethod,
    pub rate_limiting: RateLimitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ApiStyle {
    Rest,
    GraphQL,
    Grpc,
    WebSocket,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEndpoint {
    pub method: HttpMethod,
    pub path: String,
    pub description: String,
    pub request_body: Option<String>,
    pub response_body: String,
    pub auth_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Head,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthMethod {
    None,
    Jwt,
    OAuth2,
    ApiKey,
    Session,
    BasicAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub burst_limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityModel {
    pub authentication: AuthStrategy,
    pub authorization: AuthorizationModel,
    pub data_protection: DataProtectionMeasures,
    pub compliance_requirements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStrategy {
    pub method: AuthMethod,
    pub implementation_notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationModel {
    pub model: String, // RBAC, ABAC, etc.
    pub roles: Vec<RoleDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefinition {
    pub name: String,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataProtectionMeasures {
    pub encryption_at_rest: bool,
    pub encryption_in_transit: bool,
    pub hashing_algorithm: Option<String>,
    pub pii_handling: PiiHandlingPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PiiHandlingPolicy {
    None,
    Masked,
    Encrypted,
    Anonymized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationGuide {
    pub phases: Vec<ImplementationPhase>,
    pub prerequisites: Vec<String>,
    pub common_pitfalls: Vec<Pitfall>,
    pub best_practices: Vec<BestPractice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationPhase {
    pub id: usize,
    pub name: String,
    pub duration_days: u32,
    pub tasks: Vec<TaskDescription>,
    pub deliverables: Vec<String>,
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDescription {
    pub title: String,
    pub description: String,
    pub estimated_hours: f64,
    pub dependencies: Vec<usize>,
    pub resources_needed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pitfall {
    pub scenario: String,
    pub impact: String,
    pub prevention: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestPractice {
    pub category: String,
    pub practice: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub risks: Vec<RiskItem>,
    pub mitigation_strategies: HashMap<String, MitigationStrategy>,
    pub overall_risk_level: RiskLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskItem {
    pub id: String,
    pub category: RiskCategory,
    pub description: String,
    pub probability: ProbabilityLevel,
    pub impact: ImpactLevel,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RiskCategory {
    Technical,
    Operational,
    Financial,
    Security,
    Compliance,
    Market,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProbabilityLevel {
    Low,
    Medium,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ImpactLevel {
    Negligible,
    Minor,
    Moderate,
    Major,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MitigationStrategy {
    pub strategy: String,
    pub owner: String,
    pub timeline: String,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub development_cost: Money,
    pub infrastructure_monthly: Money,
    pub total_first_year: Money,
    pub breakdown: CostBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Money {
    pub amount: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostBreakdown {
    pub development: f64,
    pub testing: f64,
    pub deployment: f64,
    pub monitoring: f64,
    pub maintenance_annual: f64,
}

// --- Core Engine ------------------------------

/// 快速原型引擎
pub struct PrototypeEngine {
    config: PrototypeEngineConfig,
}

#[derive(Debug, Clone)]
pub struct PrototypeEngineConfig {
    pub max_files_per_project: usize,
    pub max_lines_per_file: usize,
    pub include_examples: bool,
    pub include_comments: bool,
}

impl Default for PrototypeEngineConfig {
    fn default() -> Self {
        Self {
            max_files_per_project: 50,
            max_lines_per_file: 500,
            include_examples: true,
            include_comments: true,
        }
    }
}

impl PrototypeEngine {
    /// 创建新的原型引擎
    pub fn new(config: Option<PrototypeEngineConfig>) -> Self {
        Self {
            config: config.unwrap_or_default(),
        }
    }
    
    /// 从描述生成完整原型项目
    pub async fn generate_prototype(&self, prototype_config: &PrototypeConfig) -> Result<GeneratedProject> {
        let start = std::time::Instant::now();
        
        tracing::info!(
            project_name = %prototype_config.project_name,
            project_type = ?prototype_config.project_type,
            "Starting prototype generation"
        );
        
        // 1. 分析需求和约束
        let analysis = self.analyze_requirements(prototype_config).await;
        
        // 2. 选择技术栈
        let tech_stack = self.select_tech_stack(&analysis, prototype_config).await;
        
        // 3. 设计架构
        let architecture = self.design_architecture(&tech_stack, prototype_config).await;
        
        // 4. 生成文件树
        let file_tree = self.generate_file_tree(&architecture, &tech_stack, prototype_config).await;
        
        // 5. 创建实施指南
        let implementation_guide = self.create_implementation_guide(&architecture, prototype_config).await;
        
        // 6. 评估风险
        let risk_assessment = self.assess_risks(&tech_stack, prototype_config).await;
        
        // 7. 估算成本
        let cost_estimate = self.estimate_costs(&tech_stack, prototype_config).await;
        
        // 8. 汇总项目元数据
        let total_lines: usize = file_tree.iter().map(|f| f.content.lines().count()).sum();
        
        let metadata = ProjectMetadata {
            name: prototype_config.project_name.clone(),
            version: "0.1.0".to_string(),
            description: prototype_config.description.clone(),
            generated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            generator_version: env!("JCODE_VERSION").to_string(),
            total_files: file_tree.len(),
            total_lines_of_code: total_lines,
        };
        
        let elapsed = start.elapsed();
        
        tracing::info!(
            elapsed_ms = elapsed.as_millis(),
            file_count = file_tree.len(),
            lines_of_code = total_lines,
            "Prototype generation completed"
        );
        
        Ok(GeneratedProject {
            metadata,
            file_tree,
            tech_stack,
            architecture,
            implementation_guide,
            risk_assessment,
            cost_estimate,
        })
    }
    
    /// 从简短描述快速生成MVP
    pub async fn generate_mvp(&self, description: &str, project_type: ProjectType) -> Result<GeneratedProject> {
        let config = PrototypeConfig {
            project_name: extract_project_name(description),
            description: description.to_string(),
            project_type,
            target_language: None,
            target_framework: None,
            features: infer_features_from_description(description),
            constraints: PrototypeConstraints {
                max_response_time_ms: Some(500),
                concurrent_users: Some(100),
                storage_requirements: StorageType::Relational,
                security_level: SecurityLevel::Standard,
                budget_tier: BudgetTier::Startup,
                team_size: TeamSize::Small(3),
                time_constraint_weeks: Some(4),
            },
            output_options: OutputOptions {
                generate_full_project: true,
                include_tests: true,
                generate_docs: true,
                include_cicd: true,
                include_docker: true,
                code_style: CodeStyle::Idiomatic,
                comment_language: CommentLanguage::Bilingual,
            },
        };
        
        self.generate_prototype(&config).await
    }
    
    /// 仅生成项目骨架 (无详细实现)
    pub async fn generate_skeleton(&self, description: &str, language: &str) -> Result<Vec<ProjectFile>> {
        let config = PrototypeConfig {
            project_name: extract_project_name(description),
            description: description.to_string(),
            project_type: ProjectType::Library,
            target_language: Some(language.to_string()),
            target_framework: None,
            features: vec![],
            constraints: PrototypeConstraints::default(),
            output_options: OutputOptions {
                generate_full_project: false,
                include_tests: false,
                generate_docs: false,
                include_cicd: false,
                include_docker: false,
                code_style: CodeStyle::Minimal,
                comment_language: CommentLanguage::English,
            },
        };
        
        let project = self.generate_prototype(&config).await?;
        Ok(project.file_tree)
    }
}

// --- Implementation Methods -----------------

impl PrototypeEngine {
    async fn analyze_requirements(&self, config: &PrototypeConfig) -> RequirementAnalysis {
        RequirementAnalysis {
            complexity_score: calculate_complexity_score(&config.features),
            scalability_requirement: match config.constraints.concurrent_users {
                Some(n) if n > 10000 => ScalabilityRequirement::High,
                Some(n) if n > 1000 => ScalabilityRequirement::Medium,
                _ => ScalabilityRequirement::Low,
            },
            real_time_requirement: config.constraints.max_response_time_ms.map_or(false, |ms| ms < 100),
            security_requirement: matches!(config.constraints.security_level, 
                SecurityLevel::High | SecurityLevel::Enterprise | SecurityLevel::Government),
            team_experience_level: match config.constraints.team_size {
                TeamSize::Solo => ExperienceLevel::Intermediate,
                TeamSize::Small(_) => ExperienceLevel::Mixed,
                _ => ExperienceLevel::Senior,
            },
        }
    }
    
    async fn select_tech_stack(
        &self, 
        analysis: &RequirementAnalysis, 
        config: &PrototypeConfig
    ) -> TechStackDecision {
        let language_choice = select_optimal_language(config.target_language.as_deref(), analysis);
        let framework_choice = select_optimal_framework(
            config.target_framework.as_deref(), 
            &language_choice.name,
            &config.project_type,
            analysis
        );
        let database_choice = select_database(&config.constraints.storage_requirements, &config.project_type);
        let infrastructure = select_infrastructure(&config.project_type, analysis);
        let toolchain = select_toolchain(&language_choice.name);
        
        TechStackDecision {
            language: language_choice,
            framework: framework_choice,
            database: database_choice,
            infrastructure,
            toolchain,
            rationale: calculate_decision_rationale(config),
        }
    }
    
    async fn design_architecture(
        &self, 
        tech_stack: &TechStackDecision, 
        config: &PrototypeConfig
    ) -> ArchitectureOverview {
        let pattern = select_architecture_pattern(&config.project_type, &config.constraints.team_size);
        let layers = design_layers(&pattern, tech_stack);
        let data_flow = design_data_flow(&pattern, &config.project_type);
        let api_design = design_api(&config.project_type, &pattern);
        let security_model = design_security_model(&config.constraints.security_level);
        
        ArchitectureOverview {
            pattern,
            layers,
            data_flow,
            api_design,
            security_model,
        }
    }
    
    async fn generate_file_tree(
        &self,
        architecture: &ArchitectureOverview,
        tech_stack: &TechStackDecision,
        config: &PrototypeConfig,
    ) -> Vec<ProjectFile> {
        let mut files = Vec::new();
        
        // 根据项目类型生成不同的文件结构
        match &config.project_type {
            ProjectType::WebApp | ProjectType::ApiRest | ProjectType::ApiGraphQL => {
                files.extend(generate_web_api_files(architecture, tech_stack, config));
            }
            ProjectType::CliTool => {
                files.extend(generate_cli_files(architecture, tech_stack, config));
            }
            ProjectType::Library => {
                files.extend(generate_library_files(architecture, tech_stack, config));
            }
            ProjectType::Microservice => {
                files.extend(generate_microservice_files(architecture, tech_stack, config));
            }
            _ => {
                files.extend(generic_project_files(architecture, tech_stack, config));
            }
        }
        
        // 添加通用文件
        if config.output_options.include_tests {
            files.extend(generate_test_files(tech_stack, config));
        }
        
        if config.output_options.generate_docs {
            files.extend(generate_documentation_files(config));
        }
        
        if config.output_options.include_cicd {
            files.extend(generate_ci_cd_files(tech_stack));
        }
        
        if config.output_options.include_docker {
            files.extend(generate_docker_files());
        }
        
        // 添加配置文件
        files.extend(generate_config_files(tech_stack, config));
        
        // 限制文件数量
        files.truncate(self.config.max_files_per_project);
        
        files
    }
    
    async fn create_implementation_guide(
        &self,
        architecture: &ArchitectureOverview,
        config: &PrototypeConfig,
    ) -> ImplementationGuide {
        ImplementationGuide {
            phases: create_phases(architecture, config),
            prerequisites: get_prerequisites(architecture),
            common_pitfalls: get_common_pitfalls(&config.project_type),
            best_practices: get_best_practices(&architecture.pattern),
        }
    }
    
    async fn assess_risks(&self, _tech_stack: &TechStackDecision, config: &PrototypeConfig) -> RiskAssessment {
        let risks = identify_risks(config);
        let mitigations = create_mitigation_strategies(&risks);
        let overall_level = calculate_overall_risk_level(&risks);
        
        RiskAssessment {
            risks,
            mitigation_strategies: mitigations,
            overall_risk_level: overall_level,
        }
    }
    
    async fn estimate_costs(&self, _tech_stack: &TechStackDecision, config: &PrototypeConfig) -> CostEstimate {
        estimate_project_costs(config)
    }
}

// --- Helper Types and Functions -------------

struct RequirementAnalysis {
    complexity_score: u8,
    scalability_requirement: ScalabilityRequirement,
    real_time_requirement: bool,
    security_requirement: bool,
    team_experience_level: ExperienceLevel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ScalabilityRequirement {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum ExperienceLevel {
    Junior,
    Intermediate,
    Senior,
    Mixed,
}

fn extract_project_name(description: &str) -> String {
    // 简单提取：取前几个词或第一个大写词
    let words: Vec<&str> = description.split_whitespace().collect();
    if words.is_empty() {
        return "my-project".to_string();
    }
    
    // 尝试找首字母大写的词（可能是专有名词）
    for word in words.iter().take(5) {
        if word.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) && word.len() > 2 {
            return word.to_lowercase().replace(['.', ',', '!', '?'], "");
        }
    }
    
    // 默认使用第一个词
    words[0].to_lowercase()
}

fn infer_features_from_description(_description: &str) -> Vec<FeatureRequirement> {
    // 简化实现：实际应使用NLP分析
    vec![
        FeatureRequirement {
            id: "core".to_string(),
            name: "核心功能".to_string(),
            description: "主要业务逻辑".to_string(),
            priority: Priority::Critical,
            is_required: true,
            estimated_complexity: 7,
        },
        FeatureRequirement {
            id: "auth".to_string(),
            name: "认证授权".to_string(),
            description: "用户登录和权限管理".to_string(),
            priority: Priority::High,
            is_required: false,
            estimated_complexity: 5,
        },
        FeatureRequirement {
            id: "api".to_string(),
            name: "API接口".to_string(),
            description: "RESTful API端点".to_string(),
            priority: Priority::High,
            is_required: true,
            estimated_complexity: 4,
        },
    ]
}

impl Default for PrototypeConstraints {
    fn default() -> Self {
        Self {
            max_response_time_ms: None,
            concurrent_users: None,
            storage_requirements: StorageType::Relational,
            security_level: SecurityLevel::Basic,
            budget_tier: BudgetTier::Bootstrap,
            team_size: TeamSize::Solo,
            time_constraint_weeks: None,
        }
    }
}

// ... (更多辅助函数实现将在实际使用时补充)

fn calculate_complexity_score(features: &[FeatureRequirement]) -> u8 {
    if features.is_empty() { return 3; }
    
    let sum: u32 = features.iter().map(|f| f.estimated_complexity as u32).sum();
    let avg = sum / features.len() as u32;
    
    (avg as f64 * (features.len() as f64 / 10.0).min(1.0)).min(10.0) as u8
}

fn select_optimal_language(preferred: Option<&str>, _analysis: &RequirementAnalysis) -> LanguageChoice {
    match preferred {
        Some(lang) => LanguageChoice {
            name: lang.to_string(),
            version: get_latest_version(lang),
            reasoning: format!("User specified language: {}", lang),
            alternatives_considered: vec![],
            pros: vec!["符合团队技能".to_string()],
            cons: vec![],
        },
        None => {
            // 默认推荐Rust或TypeScript
            LanguageChoice {
                name: "Rust".to_string(),
                version: "1.75+".to_string(),
                reasoning: "性能优秀、内存安全、现代语法".to_string(),
                alternatives_considered: vec!["TypeScript".to_string(), "Go".to_string()],
                pros: vec![
                    "零成本抽象".to_string(),
                    "内存安全保证".to_string(),
                    "优秀的并发支持".to_string(),
                    "强大的类型系统".to_string(),
                ],
                cons: vec![
                    "学习曲线较陡".to_string(),
                    "编译时间较长".to_string(),
                ],
            }
        }
    }
}

fn get_latest_version(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "rust" => "1.75+".to_string(),
        "typescript" => "5.0+".to_string(),
        "java" => "21 LTS".to_string(),
        "go" => "1.21+".to_string(),
        "c#" => "12 (.NET 8)".to_string(),
        "c++" => "C++23".to_string(),
        "python" => "3.12+".to_string(),
        _ => "latest".to_string(),
    }
}

fn select_optimal_framework(
    preferred: Option<&str>, 
    language: &str, 
    project_type: &ProjectType,
    _analysis: &RequirementAnalysis
) -> FrameworkChoice {
    if let Some(fw) = preferred {
        return FrameworkChoice {
            name: fw.to_string(),
            version: "latest".to_string(),
            reasoning: format!("User specified framework: {}", fw),
            key_features: vec![],
        };
    }
    
    let (name, version, features) = match (language, project_type) {
        ("rust", ProjectType::WebApp | ProjectType::ApiRest) => (
            "Actix-web / Axum",
            "4.x / 0.6+",
            vec!["高性能异步框架".to_string(), "类型安全路由".to_string()]
        ),
        ("typescript", ProjectType::WebApp | ProjectType::ApiRest) => (
            "Express / Fastify / NestJS",
            "^4.18 / ^4.25 / ^10.0",
            vec!["生态丰富".to_string(), "中间件支持".to_string()]
        ),
        ("java", ProjectType::WebApp | ProjectType::ApiRest) => (
            "Spring Boot",
            "3.2+",
            vec!["企业级特性".to_string(), "自动配置".to_string()]
        ),
        ("go", ProjectType::WebApp | ProjectType::ApiRest) => (
            "Gin / Echo / Fiber",
            "v1.9 / v4.11 / v2.51",
            vec!["高性能".to_string(), "简洁API".to_string()]
        ),
        (_, _) => (
            "Custom Stack",
            "1.0",
            vec!["自定义实现".to_string()]
        ),
    };
    
    FrameworkChoice {
        name: name.to_string(),
        version: version.to_string(),
        reasoning: format!("Best fit for {} + {:?}", language, project_type),
        key_features: features,
    }
}

fn select_database(storage: &StorageType, _project_type: &ProjectType) -> DatabaseChoice {
    let primary = match storage {
        StorageType::Relational => DatabaseOption {
            name: "PostgreSQL".to_string(),
            type_: StorageType::Relational,
            reasoning: "功能强大、开源、ACID合规".to_string(),
            use_case: "事务性数据、复杂查询".to_string(),
        },
        StorageType::NoSQLDocument => DatabaseOption {
            name: "MongoDB".to_string(),
            type_: StorageType::NoSQLDocument,
            reasoning: "灵活Schema、水平扩展".to_string(),
            use_case: "文档数据、快速迭代".to_string(),
        },
        _ => DatabaseOption {
            name: "PostgreSQL".to_string(),
            type_: StorageType::Relational,
            reasoning: "默认可靠选择".to_string(),
            use_case: "通用场景".to_string(),
        },
    };
    
    DatabaseChoice {
        primary,
        secondary: Some(DatabaseOption {
            name: "Redis".to_string(),
            type_: StorageType::NoSQLKeyVal,
            reasoning: "缓存层、会话存储".to_string(),
            use_case: "高速缓存、实时数据".to_string(),
        }),
        caching: Some(CachingOption {
            name: "Redis".to_string(),
            reasoning: "高性能缓存".to_string(),
        }),
    }
}

fn select_infrastructure(_project_type: &ProjectType, _analysis: &RequirementAnalysis) -> InfrastructureChoices {
    InfrastructureChoices {
        web_server: Some("Nginx / Caddy".to_string()),
        message_queue: Some("RabbitMQ / Kafka".to_string()),
        search_engine: Some("Elasticsearch".to_string()),
        monitoring: vec!["Prometheus".to_string(), "Grafana".to_string()],
        cdn: Some("Cloudflare".to_string()),
    }
}

fn select_toolchain(language: &str) -> ToolchainChoices {
    match language.to_lowercase().as_str() {
        "rust" => ToolchainChoices {
            package_manager: "Cargo".to_string(),
            build_tool: "cargo build".to_string(),
            test_framework: "cargo test / tokio-test".to_string(),
            linter: "clippy".to_string(),
            formatter: "rustfmt".to_string(),
            ci_cd: Some("GitHub Actions".to_string()),
        },
        "typescript" => ToolchainChoices {
            package_manager: "npm / pnpm / yarn".to_string(),
            build_tool: "tsc / webpack / vite".to_string(),
            test_framework: "Jest / Vitest".to_string(),
            linter: "ESLint".to_string(),
            formatter: "Prettier".to_string(),
            ci_cd: Some("GitHub Actions".to_string()),
        },
        "java" => ToolchainChoices {
            package_manager: "Maven / Gradle".to_string(),
            build_tool: "mvn package / gradle build".to_string(),
            test_framework: "JUnit 5 / TestNG".to_string(),
            linter: "Checkstyle / SpotBugs".to_string(),
            formatter: "Google Java Format".to_string(),
            ci_cd: Some("GitHub Actions / Jenkins".to_string()),
        },
        "go" => ToolchainChoices {
            package_manager: "Go Modules".to_string(),
            build_tool: "go build".to_string(),
            test_framework: "Go testing".to_string(),
            linter: "golangci-lint".to_string(),
            formatter: "gofmt / goimports".to_string(),
            ci_cd: Some("GitHub Actions".to_string()),
        },
        _ => ToolchainChoices {
            package_manager: "N/A".to_string(),
            build_tool: "make".to_string(),
            test_framework: "Native test framework".to_string(),
            linter: "N/A".to_string(),
            formatter: "N/A".to_string(),
            ci_cd: None,
        },
    }
}

fn calculate_decision_rationale(config: &PrototypeConfig) -> DecisionRationale {
    DecisionRationale {
        performance_weight: if config.constraints.max_response_time_ms.map_or(false, |ms| ms < 200) { 0.35 } else { 0.20 },
        developer_experience_weight: match config.constraints.team_size {
            TeamSize::Solo | TeamSize::Small(_) => 0.30,
            _ => 0.20,
        },
        ecosystem_weight: 0.20,
        cost_weight: match config.constraints.budget_tier {
            BudgetTier::Bootstrap | BudgetTier::Startup => 0.15,
            _ => 0.05,
        },
        community_support_weight: 0.10,
        learning_curve_weight: match config.constraints.team_size {
            TeamSize::Solo | TeamSize::Small(_) => 0.05,
            _ => 0.10,
        },
    }
}

fn select_architecture_pattern(project_type: &ProjectType, team_size: &TeamSize) -> ArchitecturePattern {
    match (project_type, team_size) {
        (ProjectType::Microservice, _) => ArchitecturePattern::EventDriven,
        (ProjectType::ApiRest | ProjectType::ApiGraphQL, TeamSize::Large(_) | TeamSize::Enterprise(_)) => ArchitecturePattern::CleanArchitecture,
        (ProjectType::WebApp, _) => ArchitecturePattern::Layered,
        (ProjectType::Desktop | ProjectType::Mobile, _) => ArchitecturePattern::Hexagonal,
        (_, _) => ArchitecturePattern::MonolithicModular,
    }
}

fn design_layers(pattern: &ArchitecturePattern, _tech_stack: &TechStackDecision) -> Vec<LayerDescription> {
    match pattern {
        ArchitecturePattern::Layered => vec![
            LayerDescription {
                name: "Presentation Layer".to_string(),
                responsibility: "处理HTTP请求和响应".to_string(),
                technologies: vec!["Web Framework".to_string(), "JSON Serialization".to_string()],
                interfaces: vec!["REST API Endpoints".to_string(), "Request Validation".to_string()],
            },
            LayerDescription {
                name: "Business Logic Layer".to_string(),
                responsibility: "实现业务规则和工作流".to_string(),
                technologies: vec!["Domain Models".to_string(), "Services".to_string()],
                interfaces: vec!["Use Cases".to_string(), "Domain Services".to_string()],
            },
            LayerDescription {
                name: "Data Access Layer".to_string(),
                responsibility: "数据库操作和数据持久化".to_string(),
                technologies: vec!["ORM / Query Builder".to_string(), "Database Driver".to_string()],
                interfaces: vec!["Repositories".to_string(), "Data Mappers".to_string()],
            },
        ],
        ArchitecturePattern::CleanArchitecture => vec![
            LayerDescription {
                name: "Presentation (Controllers/Handlers)".to_string(),
                responsibility: "接收请求、返回响应".to_string(),
                technologies: vec!["HTTP Framework".to_string()],
                interfaces: vec!["REST Endpoints".to_string()],
            },
            LayerDescription {
                name: "Application (Use Cases)".to_string(),
                responsibility: "协调业务逻辑".to_string(),
                technologies: vec!["Service Layer".to_string()],
                interfaces: vec!["Use Case Interfaces".to_string()],
            },
            LayerDescription {
                name: "Domain (Entities & Business Rules)".to_string(),
                responsibility: "核心业务实体和规则".to_string(),
                technologies: vec!["Pure Business Logic".to_string()],
                interfaces: vec!["Entity Interfaces".to_string(), "Repository Interfaces".to_string()],
            },
            LayerDescription {
                name: "Infrastructure (DB, External APIs)".to_string(),
                responsibility: "具体技术实现".to_string(),
                technologies: vec!["Database Drivers".to_string(), "HTTP Clients".to_string()],
                interfaces: vec!["Concrete Implementations".to_string()],
            },
        ],
        _ => vec![
            LayerDescription {
                name: "Core Module".to_string(),
                responsibility: "主要功能实现".to_string(),
                technologies: vec!["Core Logic".to_string()],
                interfaces: vec!["Public API".to_string()],
            },
        ],
    }
}

fn design_data_flow(_pattern: &ArchitecturePattern, _project_type: &ProjectType) -> DataFlowDiagram {
    DataFlowDiagram {
        description: "标准请求-响应流程".to_string(),
        components: vec![
            DataFlowComponent {
                name: "Client".to_string(),
                type_: ComponentType::Client,
                inputs: vec![],
                outputs: vec!["HTTP Request".to_string()],
            },
            DataFlowComponent {
                name: "API Gateway".to_string(),
                type_: ComponentType::Gateway,
                inputs: vec!["HTTP Request".to_string()],
                outputs: vec!["Validated Request".to_string()],
            },
            DataFlowComponent {
                name: "Business Service".to_string(),
                type_: ComponentType::Service,
                inputs: vec!["Validated Request".to_string()],
                outputs: vec!["Business Response".to_string()],
            },
            DataFlowComponent {
                name: "Database".to_string(),
                type_: ComponentType::Database,
                inputs: vec!["Query/Command".to_string()],
                outputs: vec!["Data Results".to_string()],
            },
        ],
    }
}

fn design_api(project_type: &ProjectType, _pattern: &ArchitecturePattern) -> ApiDesign {
    let base_path = match project_type {
        ProjectType::ApiRest | ProjectType::ApiGraphQL => "/api/v1".to_string(),
        _ => "/".to_string(),
    };
    
    ApiDesign {
        style: match project_type {
            ProjectType::ApiGraphQL => ApiStyle::GraphQL,
            ProjectType::ApiRest => ApiStyle::Rest,
            _ => ApiStyle::Hybrid,
        },
        base_path,
        endpoints: vec![
            ApiEndpoint {
                method: HttpMethod::Get,
                path: "/health".to_string(),
                description: "健康检查端点".to_string(),
                request_body: None,
                response_body: r#"{"status": "ok"}"#.to_string(),
                auth_required: false,
            },
            ApiEndpoint {
                method: HttpMethod::Post,
                path: "/items".to_string(),
                description: "创建新资源".to_string(),
                request_body: Some(r#"{"name": "string"}"#.to_string()),
                response_body: r#"{"id": "uuid", "name": "string"}"#.to_string(),
                auth_required: true,
            },
            ApiEndpoint {
                method: HttpMethod::Get,
                path: "/items/:id".to_string(),
                description: "获取单个资源".to_string(),
                request_body: None,
                response_body: r#"{"id": "uuid", "name": "string"}"#.to_string(),
                auth_required: true,
            },
        ],
        authentication: AuthMethod::Jwt,
        rate_limiting: RateLimitConfig {
            requests_per_minute: 60,
            burst_limit: 10,
        },
    }
}

fn design_security_model(security_level: &SecurityLevel) -> SecurityModel {
    SecurityModel {
        authentication: AuthStrategy {
            method: match security_level {
                SecurityLevel::Basic | SecurityLevel::Standard => AuthMethod::Jwt,
                SecurityLevel::High | SecurityLevel::Enterprise | SecurityLevel::Government => AuthMethod::OAuth2,
            },
            implementation_notes: "使用JWT/OAuth2进行身份验证".to_string(),
        },
        authorization: AuthorizationModel {
            model: "RBAC (Role-Based Access Control)".to_string(),
            roles: vec![
                RoleDefinition {
                    name: "admin".to_string(),
                    permissions: vec!["read".to_string(), "write".to_string(), "delete".to_string(), "admin".to_string()],
                },
                RoleDefinition {
                    name: "user".to_string(),
                    permissions: vec!["read".to_string(), "write".to_string()],
                },
            ],
        },
        data_protection: DataProtectionMeasures {
            encryption_at_rest: matches!(security_level, SecurityLevel::Standard | SecurityLevel::High | SecurityLevel::Enterprise | SecurityLevel::Government),
            encryption_in_transit: true,
            hashing_algorithm: Some("Argon2 / bcrypt".to_string()),
            pii_handling: match security_level {
                SecurityLevel::Government | SecurityLevel::Enterprise => PiiHandlingPolicy::Encrypted,
                SecurityLevel::High => PiiHandlingPolicy::Masked,
                _ => PiiHandlingPolicy::None,
            },
        },
        compliance_requirements: match security_level {
            SecurityLevel::Government => vec!["GDPR".to_string(), "HIPAA".to_string(), "SOC2".to_string()],
            SecurityLevel::Enterprise => vec!["GDPR".to_string(), "SOC2".to_string()],
            SecurityLevel::High => vec!["GDPR".to_string()],
            _ => vec![],
        },
    }
}

fn generate_web_api_files(architecture: &ArchitectureOverview, tech_stack: &TechStackDecision, config: &PrototypeConfig) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    
    if tech_stack.language.name == "Rust" {
        files.extend(generate_rust_web_api_files(architecture, tech_stack, config));
    } else if tech_stack.language.name.to_lowercase() == "typescript" {
        files.extend(generate_typescript_web_api_files(architecture, tech_stack, config));
    }
    
    files
}

fn generate_rust_web_api_files(_architecture: &ArchitectureOverview, _tech_stack: &TechStackDecision, config: &PrototypeConfig) -> Vec<ProjectFile> {
    let project_name = &config.project_name;
    let snake_name = project_name.replace("-", "_").to_lowercase();
    
    vec![
        ProjectFile {
            path: "src/main.rs".to_string(),
            content: format!(r#"use {snake_name}::app::create_app;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let app = create_app().await;
    
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    println!("Server listening on {{}}", addr);
    
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    
    Ok(())
}}
"#).into(),
            file_type: FileType::Source,
            purpose: "Main entry point".to_string(),
            is_entry_point: true,
            dependencies: vec!["axum".to_string(), "tokio".to_string()],
        },
        ProjectFile {
            path: "src/app.rs".to_string(),
            content: r#"use axum::{routing::get, Router};

pub async fn create_app() -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/v1/items", get(list_items).post(create_item))
        .route("/api/v1/items/:id", get(get_item))
}

async fn health_check() -> &'static str {
    "OK"
}

async fn list_items() -> String {
    serde_json::json!([]).to_string()
}

async fn create_item() -> String {
    serde_json::json!({
        "id": "1",
        "name": "New Item"
    }).to_string()
}

async fn get_item() -> String {
    serde_json::json!({
        "id": "1",
        "name": "Item"
    }).to_string()
}
"#.into(),
            file_type: FileType::Source,
            purpose: "Application router setup".to_string(),
            is_entry_point: false,
            dependencies: vec!["axum".to_string(), "serde_json".to_string()],
        },
        ProjectFile {
            path: "src/lib.rs".to_string(),
            content: format!(r#"pub mod app;

pub use app::create_app;

/// {project_name} API Library
/// 
/// This crate provides the core functionality for the {project_name} service.
pub const VERSION: &str = "0.1.0";
"#).into(),
            file_type: FileType::Source,
            purpose: "Library entry point".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
        ProjectFile {
            path: "Cargo.toml".to_string(),
            content: format!(r#"[package]
name = "{snake_name}"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = {{ version = "1.0", features = ["full"] }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
tracing = "0.1"
tracing-subscriber = "0.3"
"#).into(),
            file_type: FileType::Configuration,
            purpose: "Cargo package configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
    ]
}

fn generate_typescript_web_api_files(_architecture: &ArchitectureOverview, _tech_stack: &TechStackDecision, config: &PrototypeConfig) -> Vec<ProjectFile> {
    let project_name = &config.project_name;
    
    vec![
        ProjectFile {
            path: "src/index.ts".to_string(),
            content: format!(r#"import express from 'express';
import {{ router }} from './routes';

const app = express();
const PORT = process.env.PORT || 3000;

app.use(express.json());
app.use('/api/v1', router);

app.get('/health', (req, res) => {{
    res.json({{ status: 'ok' }});
}});

app.listen(PORT, () => {{
    console.log(`{project_name} API running on port ${{PORT}}`);
}});
"#, project_name = project_name).into(),
            file_type: FileType::Source,
            purpose: "Main entry point".to_string(),
            is_entry_point: true,
            dependencies: vec!["express".to_string(), "@types/express".to_string()],
        },
        ProjectFile {
            path: "src/routes/index.ts".to_string(),
            content: r#"import { Router } from 'express';
import { listItems, createItem, getItem } from '../controllers/itemController';

export const router = Router();

router.get('/items', listItems);
router.post('/items', createItem);
router.get('/items/:id', getItem);
"#.into(),
            file_type: FileType::Source,
            purpose: "API routes definition".to_string(),
            is_entry_point: false,
            dependencies: vec!["express".to_string()],
        },
        ProjectFile {
            path: "src/controllers/itemController.ts".to_string(),
            content: r#"import { Request, Response } from 'express';

export const listItems = (req: Request, res: Response) => {
    res.json([]);
};

export const createItem = (req: Request, res: Response) => {
    const item = req.body;
    res.status(201).json({
        id: '1',
        ...item,
    });
};

export const getItem = (req: Request, res: Response) => {
    const { id } = req.params;
    res.json({
        id,
        name: 'Item',
    });
};
"#.into(),
            file_type: FileType::Source,
            purpose: "Item controller with CRUD operations".to_string(),
            is_entry_point: false,
            dependencies: vec!["express".to_string()],
        },
        ProjectFile {
            path: "package.json".to_string(),
            content: r#"{
  "name": "api-service",
  "version": "1.0.0",
  "main": "dist/index.js",
  "scripts": {
    "start": "node dist/index.js",
    "build": "tsc",
    "dev": "ts-node src/index.ts",
    "test": "jest"
  },
  "dependencies": {
    "express": "^4.18.0"
  },
  "devDependencies": {
    "@types/express": "^4.17.0",
    "@types/node": "^20.0.0",
    "typescript": "^5.0.0",
    "ts-node": "^10.9.0",
    "jest": "^29.0.0"
  }
}
"#.into(),
            file_type: FileType::Configuration,
            purpose: "npm package configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
        ProjectFile {
            path: "tsconfig.json".to_string(),
            content: r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "outDir": "./dist",
    "rootDir": "./src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules"]
}
"#.into(),
            file_type: FileType::Configuration,
            purpose: "TypeScript configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
    ]
}

fn generate_cli_files(_architecture: &ArchitectureOverview, tech_stack: &TechStackDecision, _config: &PrototypeConfig) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    
    if tech_stack.language.name == "Rust" {
        files.push(ProjectFile {
            path: "src/main.rs".to_string(),
            content: r#"use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    name: Option<String>,
    
    #[arg(short, long, default_value_t = 10)]
    count: u32,
}

fn main() {
    let cli = Cli::parse();
    
    if let Some(name) = cli.name {
        println!("Hello, {}!", name);
    } else {
        println!("Hello, World!");
    }
    
    println!("Count: {}", cli.count);
}
"#.into(),
            file_type: FileType::Source,
            purpose: "CLI main entry point".to_string(),
            is_entry_point: true,
            dependencies: vec!["clap".to_string()],
        });
        files.push(ProjectFile {
            path: "Cargo.toml".to_string(),
            content: r#"[package]
name = "cli-tool"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4.0", features = ["derive"] }
"#.into(),
            file_type: FileType::Configuration,
            purpose: "Cargo package configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        });
    }
    
    files
}

fn generate_library_files(_architecture: &ArchitectureOverview, tech_stack: &TechStackDecision, config: &PrototypeConfig) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    let project_name = &config.project_name;
    let snake_name = project_name.replace("-", "_").to_lowercase();
    
    if tech_stack.language.name == "Rust" {
        files.push(ProjectFile {
            path: "src/lib.rs".to_string(),
            content: format!(r#"//! {project_name}
//! 
//! A Rust library providing core functionality for {project_name}.
//! 
//! # Features
//! 
//! - Feature 1 description
//! - Feature 2 description

#![warn(missing_docs)]

/// Main module
pub mod core;

/// Utilities module
pub mod utils;

/// Version of the library
pub const VERSION: &str = "0.1.0";

#[cfg(test)]
mod tests {{
    #[test]
    fn it_works() {{
        assert_eq!(2 + 2, 4);
    }}
}}
"#).into(),
            file_type: FileType::Source,
            purpose: "Library entry point".to_string(),
            is_entry_point: true,
            dependencies: vec![],
        });
        files.push(ProjectFile {
            path: "src/core.rs".to_string(),
            content: r#"//! Core functionality module

/// Core struct
#[derive(Debug, Clone)]
pub struct Core {
    value: String,
}

impl Core {
    /// Create a new Core instance
    pub fn new(value: &str) -> Self {
        Self {
            value: value.to_string(),
        }
    }
    
    /// Get the value
    pub fn value(&self) -> &str {
        &self.value
    }
    
    /// Process the value
    pub fn process(&self) -> String {
        format!("processed: {}", self.value)
    }
}
"#.into(),
            file_type: FileType::Source,
            purpose: "Core module implementation".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        });
        files.push(ProjectFile {
            path: "src/utils.rs".to_string(),
            content: r#"//! Utility functions

/// Format a string
pub fn format_string(s: &str) -> String {
    format!("[{}]", s)
}

/// Validate input
pub fn validate_input(s: &str) -> Result<(), String> {
    if s.is_empty() {
        Err("Input cannot be empty".to_string())
    } else {
        Ok(())
    }
}
"#.into(),
            file_type: FileType::Source,
            purpose: "Utility functions".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        });
        files.push(ProjectFile {
            path: "Cargo.toml".to_string(),
            content: format!(r#"[package]
name = "{snake_name}"
version = "0.1.0"
edition = "2021"
description = "{project_name} library"
license = "MIT"

[dependencies]

[dev-dependencies]
"#).into(),
            file_type: FileType::Configuration,
            purpose: "Cargo package configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        });
    }
    
    files
}

fn generate_microservice_files(_architecture: &ArchitectureOverview, _tech_stack: &TechStackDecision, _config: &PrototypeConfig) -> Vec<ProjectFile> {
    vec![]
}

fn generic_project_files(_architecture: &ArchitectureOverview, tech_stack: &TechStackDecision, _config: &PrototypeConfig) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    
    if tech_stack.language.name == "Rust" {
        files.push(ProjectFile {
            path: "src/main.rs".to_string(),
            content: r#"fn main() {
    println!("Hello, World!");
}
"#.into(),
            file_type: FileType::Source,
            purpose: "Main entry point".to_string(),
            is_entry_point: true,
            dependencies: vec![],
        });
        files.push(ProjectFile {
            path: "Cargo.toml".to_string(),
            content: r#"[package]
name = "generic-project"
version = "0.1.0"
edition = "2021"

[dependencies]
"#.into(),
            file_type: FileType::Configuration,
            purpose: "Cargo package configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        });
    }
    
    files
}

fn generate_test_files(_tech_stack: &TechStackDecision, _config: &PrototypeConfig) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    
    if _tech_stack.language.name == "Rust" {
        files.push(ProjectFile {
            path: "src/tests.rs".to_string(),
            content: r#"#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_basic_functionality() {
        assert!(true);
    }
    
    #[test]
    fn test_edge_cases() {
        assert_eq!(2 + 2, 4);
    }
}
"#.into(),
            file_type: FileType::Source,
            purpose: "Test suite".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        });
    } else if _tech_stack.language.name.to_lowercase() == "typescript" {
        files.push(ProjectFile {
            path: "tests/itemController.test.ts".to_string(),
            content: r#"import { listItems, createItem, getItem } from '../src/controllers/itemController';
import { Request, Response } from 'express';

describe('Item Controller', () => {
    it('should list items', () => {
        const req = {} as Request;
        const res = {
            json: jest.fn()
        } as unknown as Response;
        
        listItems(req, res);
        
        expect(res.json).toHaveBeenCalledWith([]);
    });
    
    it('should create item', () => {
        const req = {
            body: { name: 'Test' }
        } as unknown as Request;
        const res = {
            status: jest.fn().mockReturnThis(),
            json: jest.fn()
        } as unknown as Response;
        
        createItem(req, res);
        
        expect(res.status).toHaveBeenCalledWith(201);
    });
});
"#.into(),
            file_type: FileType::Source,
            purpose: "Item controller tests".to_string(),
            is_entry_point: false,
            dependencies: vec!["jest".to_string(), "@types/jest".to_string()],
        });
    }
    
    files
}

fn generate_documentation_files(config: &PrototypeConfig) -> Vec<ProjectFile> {
    vec![
        ProjectFile {
            path: "README.md".to_string(),
            content: format!(r#"# {}

{}

## Features

- Feature 1
- Feature 2
- Feature 3

## Getting Started

### Prerequisites

- Rust 1.75+ / Node.js 20+

### Installation

```bash
# Install dependencies
npm install
# or
cargo build
```

### Running

```bash
npm start
# or
cargo run
```

## Usage

```typescript
// Example usage
const result = await api.call();
```

## Testing

```bash
npm test
# or
cargo test
```

## License

MIT
"#, config.project_name, config.description).into(),
            file_type: FileType::Documentation,
            purpose: "Project README".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
        ProjectFile {
            path: "docs/api.md".to_string(),
            content: r#"# API Documentation

## Endpoints

### GET /health

**Description**: Health check endpoint

**Response**:
```json
{
    "status": "ok"
}
```

### GET /api/v1/items

**Description**: List all items

**Response**:
```json
[]
```

### POST /api/v1/items

**Description**: Create new item

**Request**:
```json
{
    "name": "string"
}
```

**Response**:
```json
{
    "id": "uuid",
    "name": "string"
}
```
"#.into(),
            file_type: FileType::Documentation,
            purpose: "API documentation".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
    ]
}

fn generate_ci_cd_files(_tech_stack: &TechStackDecision) -> Vec<ProjectFile> {
    vec![
        ProjectFile {
            path: ".github/workflows/ci.yml".to_string(),
            content: r#"name: CI/CD

on:
  push:
    branches: [ main ]
  pull_request:
    branches: [ main ]

jobs:
  build:
    runs-on: ubuntu-latest
    
    steps:
    - uses: actions/checkout@v4
    
    - name: Set up Rust
      uses: dtolnay/rust-toolchain@stable
      
    - name: Build
      run: cargo build --release
      
    - name: Test
      run: cargo test
      
    - name: Lint
      run: cargo clippy
"#.into(),
            file_type: FileType::Configuration,
            purpose: "GitHub Actions CI/CD workflow".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
    ]
}

fn generate_docker_files() -> Vec<ProjectFile> {
    vec![
        ProjectFile {
            path: "Dockerfile".to_string(),
            content: r#"FROM rust:1.75-slim as builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

WORKDIR /app

COPY --from=builder /app/target/release/app .

EXPOSE 8080

CMD ["./app"]
"#.into(),
            file_type: FileType::Configuration,
            purpose: "Docker container configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
        ProjectFile {
            path: "docker-compose.yml".to_string(),
            content: r#"version: '3.8'

services:
  app:
    build: .
    ports:
      - "8080:8080"
    environment:
      - NODE_ENV=production
    volumes:
      - .:/app
    depends_on:
      - db

  db:
    image: postgres:15
    environment:
      POSTGRES_USER: user
      POSTGRES_PASSWORD: password
      POSTGRES_DB: appdb
    volumes:
      - postgres_data:/var/lib/postgresql/data

volumes:
  postgres_data:
"#.into(),
            file_type: FileType::Configuration,
            purpose: "Docker Compose configuration".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
    ]
}

fn generate_config_files(_tech_stack: &TechStackDecision, _config: &PrototypeConfig) -> Vec<ProjectFile> {
    vec![
        ProjectFile {
            path: ".gitignore".to_string(),
            content: r#"# Dependencies
node_modules/
target/
dist/

# IDE
.vscode/
.idea/
*.swp
*.swo

# OS
.DS_Store
Thumbs.db

# Environment
.env
.env.local
.env.*.local

# Logs
*.log

# Coverage
coverage/
.nyc_output/
"#.into(),
            file_type: FileType::Configuration,
            purpose: "Git ignore rules".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
        ProjectFile {
            path: ".env.example".to_string(),
            content: r#"# Environment Variables
PORT=8080
DATABASE_URL=postgres://user:password@localhost:5432/db
API_KEY=your-api-key
"#.into(),
            file_type: FileType::Configuration,
            purpose: "Environment variables template".to_string(),
            is_entry_point: false,
            dependencies: vec![],
        },
    ]
}

fn create_phases(_architecture: &ArchitectureOverview, config: &PrototypeConfig) -> Vec<ImplementationPhase> {
    let duration = config.constraints.time_constraint_weeks.unwrap_or(8);
    
    vec![
        ImplementationPhase {
            id: 1,
            name: "需求分析与设计".to_string(),
            duration_days: (duration * 7 / 4) as u32,
            tasks: vec![
                TaskDescription {
                    title: "需求收集和分析".to_string(),
                    description: "收集用户需求，进行需求分析".to_string(),
                    estimated_hours: 24.0,
                    dependencies: vec![],
                    resources_needed: vec!["业务分析师".to_string()],
                },
                TaskDescription {
                    title: "架构设计".to_string(),
                    description: "设计系统架构和技术方案".to_string(),
                    estimated_hours: 32.0,
                    dependencies: vec![1],
                    resources_needed: vec!["架构师".to_string()],
                },
                TaskDescription {
                    title: "数据库设计".to_string(),
                    description: "设计数据库 schema".to_string(),
                    estimated_hours: 16.0,
                    dependencies: vec![2],
                    resources_needed: vec!["开发人员".to_string()],
                },
            ],
            deliverables: vec![
                "需求文档".to_string(),
                "架构设计文档".to_string(),
                "数据库设计文档".to_string(),
            ],
            acceptance_criteria: vec![
                "需求文档评审通过".to_string(),
                "架构方案确认".to_string(),
            ],
        },
        ImplementationPhase {
            id: 2,
            name: "核心功能开发".to_string(),
            duration_days: (duration * 7 / 2) as u32,
            tasks: vec![
                TaskDescription {
                    title: "基础框架搭建".to_string(),
                    description: "搭建项目基础框架和配置".to_string(),
                    estimated_hours: 16.0,
                    dependencies: vec![],
                    resources_needed: vec!["开发人员".to_string()],
                },
                TaskDescription {
                    title: "核心业务逻辑实现".to_string(),
                    description: "实现核心业务功能".to_string(),
                    estimated_hours: 80.0,
                    dependencies: vec![1],
                    resources_needed: vec!["开发人员".to_string()],
                },
                TaskDescription {
                    title: "API 接口开发".to_string(),
                    description: "开发 RESTful API 接口".to_string(),
                    estimated_hours: 40.0,
                    dependencies: vec![2],
                    resources_needed: vec!["开发人员".to_string()],
                },
            ],
            deliverables: vec![
                "基础框架代码".to_string(),
                "核心功能代码".to_string(),
                "API 接口文档".to_string(),
            ],
            acceptance_criteria: vec![
                "框架搭建完成".to_string(),
                "核心功能开发完成".to_string(),
            ],
        },
        ImplementationPhase {
            id: 3,
            name: "测试与上线".to_string(),
            duration_days: (duration * 7 / 4) as u32,
            tasks: vec![
                TaskDescription {
                    title: "单元测试".to_string(),
                    description: "编写单元测试用例".to_string(),
                    estimated_hours: 24.0,
                    dependencies: vec![],
                    resources_needed: vec!["测试人员".to_string()],
                },
                TaskDescription {
                    title: "集成测试".to_string(),
                    description: "进行集成测试".to_string(),
                    estimated_hours: 32.0,
                    dependencies: vec![1],
                    resources_needed: vec!["测试人员".to_string()],
                },
                TaskDescription {
                    title: "部署上线".to_string(),
                    description: "部署到生产环境".to_string(),
                    estimated_hours: 16.0,
                    dependencies: vec![2],
                    resources_needed: vec!["运维人员".to_string()],
                },
            ],
            deliverables: vec![
                "测试报告".to_string(),
                "部署脚本".to_string(),
                "上线文档".to_string(),
            ],
            acceptance_criteria: vec![
                "测试覆盖率达标".to_string(),
                "系统稳定运行".to_string(),
            ],
        },
    ]
}

fn get_prerequisites(_architecture: &ArchitectureOverview) -> Vec<String> {
    vec![
        "开发环境已配置".to_string(),
        "代码仓库已创建".to_string(),
        "依赖包已安装".to_string(),
        "数据库服务可用".to_string(),
        "CI/CD 流水线已配置".to_string(),
    ]
}

fn get_common_pitfalls(_project_type: &ProjectType) -> Vec<Pitfall> {
    vec![
        Pitfall {
            scenario: "数据库连接泄漏".to_string(),
            impact: "系统性能下降，连接池耗尽".to_string(),
            prevention: "使用连接池，确保及时释放连接".to_string(),
        },
        Pitfall {
            scenario: "未处理异常".to_string(),
            impact: "系统崩溃，用户体验差".to_string(),
            prevention: "完善异常处理机制，添加日志记录".to_string(),
        },
        Pitfall {
            scenario: "并发数据竞争".to_string(),
            impact: "数据不一致，业务逻辑错误".to_string(),
            prevention: "使用锁机制或乐观锁，设计幂等接口".to_string(),
        },
    ]
}

fn get_best_practices(_pattern: &ArchitecturePattern) -> Vec<BestPractice> {
    vec![
        BestPractice {
            category: "代码质量".to_string(),
            practice: "遵循 SOLID 原则".to_string(),
            rationale: "提高代码可维护性和可扩展性".to_string(),
        },
        BestPractice {
            category: "测试".to_string(),
            practice: "测试驱动开发".to_string(),
            rationale: "保证代码质量，便于重构".to_string(),
        },
        BestPractice {
            category: "安全".to_string(),
            practice: "输入验证和参数校验".to_string(),
            rationale: "防止注入攻击和数据泄露".to_string(),
        },
        BestPractice {
            category: "性能".to_string(),
            practice: "合理使用缓存".to_string(),
            rationale: "减少数据库压力，提高响应速度".to_string(),
        },
    ]
}

fn identify_risks(config: &PrototypeConfig) -> Vec<RiskItem> {
    let mut risks = Vec::new();
    
    if matches!(config.constraints.security_level, 
        SecurityLevel::High | SecurityLevel::Enterprise | SecurityLevel::Government) {
        risks.push(RiskItem {
            id: "SEC-001".to_string(),
            category: RiskCategory::Security,
            description: "安全漏洞风险".to_string(),
            probability: ProbabilityLevel::Medium,
            impact: ImpactLevel::Critical,
            score: 0.75,
        });
    }
    
    if config.constraints.concurrent_users.map_or(false, |n| n > 10000) {
        risks.push(RiskItem {
            id: "PERF-001".to_string(),
            category: RiskCategory::Technical,
            description: "高并发性能风险".to_string(),
            probability: ProbabilityLevel::High,
            impact: ImpactLevel::Major,
            score: 0.8,
        });
    }
    
    if config.constraints.time_constraint_weeks.map_or(false, |w| w < 4) {
        risks.push(RiskItem {
            id: "SCHED-001".to_string(),
            category: RiskCategory::Operational,
            description: "项目进度风险".to_string(),
            probability: ProbabilityLevel::High,
            impact: ImpactLevel::Moderate,
            score: 0.6,
        });
    }
    
    risks.push(RiskItem {
        id: "TECH-001".to_string(),
        category: RiskCategory::Technical,
        description: "技术选型风险".to_string(),
        probability: ProbabilityLevel::Medium,
        impact: ImpactLevel::Moderate,
        score: 0.45,
    });
    
    risks
}

fn create_mitigation_strategies(risks: &[RiskItem]) -> HashMap<String, MitigationStrategy> {
    let mut strategies = HashMap::new();
    
    for risk in risks {
        let strategy = match risk.category {
            RiskCategory::Security => MitigationStrategy {
                strategy: "进行安全审计，使用安全编码规范，定期漏洞扫描".to_string(),
                owner: "安全工程师".to_string(),
                timeline: "持续进行".to_string(),
                cost: Some(10000.0),
            },
            RiskCategory::Technical => MitigationStrategy {
                strategy: "进行性能测试，优化关键路径，考虑水平扩展".to_string(),
                owner: "架构师".to_string(),
                timeline: "开发阶段".to_string(),
                cost: Some(5000.0),
            },
            RiskCategory::Operational => MitigationStrategy {
                strategy: "制定详细计划，设置里程碑，定期进度检查".to_string(),
                owner: "项目经理".to_string(),
                timeline: "整个项目周期".to_string(),
                cost: None,
            },
            _ => MitigationStrategy {
                strategy: "持续监控，及时响应".to_string(),
                owner: "团队负责人".to_string(),
                timeline: "持续进行".to_string(),
                cost: None,
            },
        };
        
        strategies.insert(risk.id.clone(), strategy);
    }
    
    strategies
}

fn calculate_overall_risk_level(risks: &[RiskItem]) -> RiskLevel {
    if risks.is_empty() {
        return RiskLevel::Low;
    }
    
    let avg_score: f64 = risks.iter().map(|r| r.score).sum::<f64>() / risks.len() as f64;
    
    match avg_score {
        s if s >= 0.7 => RiskLevel::Critical,
        s if s >= 0.5 => RiskLevel::High,
        s if s >= 0.3 => RiskLevel::Medium,
        _ => RiskLevel::Low,
    }
}

fn estimate_project_costs(config: &PrototypeConfig) -> CostEstimate {
    let team_size = match config.constraints.team_size {
        TeamSize::Solo => 1,
        TeamSize::Small(n) => n,
        TeamSize::Medium(n) => n,
        TeamSize::Large(n) => n,
        TeamSize::Enterprise(n) => n,
    };
    
    let duration_weeks = config.constraints.time_constraint_weeks.unwrap_or(8);
    let hourly_rate = match config.constraints.budget_tier {
        BudgetTier::Bootstrap => 50.0,
        BudgetTier::Startup => 100.0,
        BudgetTier::MidMarket => 125.0,
        BudgetTier::Enterprise => 150.0,
        BudgetTier::EnterprisePlus => 200.0,
    };
    
    let development_hours = team_size as f64 * duration_weeks as f64 * 40.0;
    let development_cost = development_hours * hourly_rate;
    
    let infrastructure_monthly = match config.constraints.budget_tier {
        BudgetTier::Bootstrap => 100.0,
        BudgetTier::Startup => 500.0,
        BudgetTier::MidMarket => 1000.0,
        BudgetTier::Enterprise => 2000.0,
        BudgetTier::EnterprisePlus => 5000.0,
    };
    
    let testing_cost = development_cost * 0.2;
    let deployment_cost = 2000.0;
    let monitoring_cost = infrastructure_monthly * 12.0;
    let maintenance_annual = development_cost * 0.15;
    
    CostEstimate {
        development_cost: Money {
            amount: development_cost + testing_cost + deployment_cost,
            currency: "USD".to_string(),
        },
        infrastructure_monthly: Money {
            amount: infrastructure_monthly,
            currency: "USD".to_string(),
        },
        total_first_year: Money {
            amount: development_cost + testing_cost + deployment_cost + monitoring_cost + maintenance_annual,
            currency: "USD".to_string(),
        },
        breakdown: CostBreakdown {
            development: development_cost,
            testing: testing_cost,
            deployment: deployment_cost,
            monitoring: monitoring_cost,
            maintenance_annual,
        },
    }
}
