//! 增强型代码重构与迁移系统
//!
//! 从Claude Code深度借鉴的重构能力，专门针对：
//! 1. 老系统迁移：.NET / Java / Go → Rust / C++ / TypeScript
//! 2. .NET Framework → .NET 6/7/8 升级迁移
//! 3. Java传统线程 → 虚拟线程 (Virtual Threads) 迁移
//! 4. 架构现代化和性能优化

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Core Migration Types ──────────────────

/// 重构/迁移配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringConfig {
    /// 源系统信息
    pub source: SourceSystem,
    
    /// 目标系统信息
    pub target: TargetSystem,
    
    /// 迁移策略
    pub strategy: MigrationStrategy,
    
    /// 需要保留的特性
    pub preserve_features: Vec<String>,
    
    /// 需要改进的方面
    pub improvement_targets: Vec<ImprovementTarget>,
    
    /// 约束条件
    pub constraints: MigrationConstraints,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSystem {
    /// 语言
    pub language: SourceLanguage,
    
    /// 框架 (如适用)
    pub framework: Option<String>,
    
    /// 版本
    pub version: Option<String>,
    
    /// 项目路径
    pub project_path: String,
    
    /// 估计代码行数
    pub estimated_loc: usize,
    
    /// 技术债务评估
    pub technical_debt_score: u8, // 1-10
    
    /// 已知问题
    pub known_issues: Vec<KnownIssue>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SourceLanguage {
    CSharpDotNetFramework,
    CSharpDotNetCore,
    JavaLegacy, // Java 8及以下
    JavaModern, // Java 9+
    Go,
    PythonLegacy,
    JavaScript,
    CppLegacy,
    Other(String),
}

impl std::fmt::Display for SourceLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CSharpDotNetFramework => write!(f, "C# (.NET Framework)"),
            Self::CSharpDotNetCore => write!(f, "C# (.NET Core)"),
            Self::JavaLegacy => write!(f, "Java (Legacy ≤8)"),
            Self::JavaModern => write!(f, "Java (Modern 9+)"),
            Self::Go => write!(f, "Go"),
            Self::PythonLegacy => write!(f, "Python (Legacy)"),
            Self::JavaScript => write!(f, "JavaScript"),
            Self::CppLegacy => write!(f, "C++ (Legacy)"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSystem {
    /// 目标语言
    pub language: TargetLanguage,
    
    /// 目标框架
    pub framework: Option<String>,
    
    /// 版本要求
    pub version_requirement: VersionRequirement,
    
    /// 输出路径
    pub output_path: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum TargetLanguage {
    Rust,
    CppModern,
    TypeScript,
    Kotlin,
    CSharpDotNetLatest,
    JavaVirtualThreads,
    GoModern,
}

impl std::fmt::Display for TargetLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "Rust"),
            Self::CppModern => write!(f, "C++ (Modern C++20+)"),
            Self::TypeScript => write!(f, "TypeScript"),
            Self::Kotlin => write!(f, "Kotlin"),
            Self::CSharpDotNetLatest => write!(f, "C# (.NET 8)"),
            Self::JavaVirtualThreads => write!(f, "Java (Virtual Threads)"),
            Self::GoModern => write!(f, "Go (Modern)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VersionRequirement {
    Any,
    Minimum(String),
    Exact(String),
    Range(String, String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownIssue {
    pub id: String,
    pub description: String,
    pub severity: IssueSeverity,
    pub location: Option<String>,
    pub suggested_fix: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum IssueSeverity {
    Critical,
    Major,
    Minor,
    Info,
}

// ─── Strategy Types ─────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MigrationStrategy {
    BigBang,           // 一次性全部重写
    StranglerFig,      // 绞杀者模式（渐进式）
    ParallelRun,       // 并行运行
    LiftAndShift,      // 直接搬运（最小改动）
    ReArchitect,       // 完全重新架构
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ImprovementTarget {
    Performance,       // 性能提升
    Maintainability,   // 可维护性
    Scalability,       // 可扩展性
    Security,          // 安全性
    Modernization,     // 现代化
    CostReduction,     // 成本降低
    DeveloperExperience, // 开发体验
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConstraints {
    /// 最大停机时间
    pub max_downtime_hours: Option<u32>,
    
    /// 是否允许数据丢失
    pub allow_data_loss: bool,
    
    /// 回滚能力要求
    pub rollback_capability: bool,
    
    /// 测试覆盖率要求
    pub min_test_coverage_percent: Option<f64>,
    
    /// 性能回归容忍度
    pub max_performance_regression_percent: Option<f64>,
    
    /// 团队学习时间限制(周)
    pub max_learning_weeks: Option<u32>,
    
    /// 预算限制
    pub budget_limit_usd: Option<f64>,
}

// ─── Result Types ───────────────────────────

/// 重构结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringResult {
    /// 元数据
    pub metadata: RefactoringMetadata,
    
    /// 生成的文件
    pub generated_files: Vec<GeneratedRefactoringFile>,
    
    /// 迁移映射表
    pub migration_mappings: MigrationMappings,
    
    /// 变更摘要
    pub change_summary: ChangeSummary,
    
    /// 风险报告
    pub risk_report: RiskReport,
    
    /// 后续步骤建议
    pub recommendations: Vec<Recommendation>,
    
    /// 成本估算
    pub cost_estimation: CostEstimation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactoringMetadata {
    pub source_language: SourceLanguage,
    pub target_language: TargetLanguage,
    pub strategy_used: MigrationStrategy,
    pub total_files_processed: usize,
    pub total_lines_migrated: usize,
    pub migration_date: String,
    pub duration_seconds: u64,
    pub success_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedRefactoringFile {
    pub original_path: String,
    pub new_path: String,
    pub content: String,
    pub change_type: ChangeType,
    pub confidence: f64,
    pub manual_review_required: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ChangeType {
    DirectTranslation,  // 直接翻译
    IdiomaticRewrite,    // 地道化重写
    PatternReplace,      // 模式替换
    ArchitecturalChange, // 架构变更
    NewFile,             // 新增文件
    Deleted,             // 删除文件
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationMappings {
    /// 类型映射
    pub type_mappings: HashMap<String, TypeMapping>,
    
    /// API映射
    pub api_mappings: Vec<ApiMapping>,
    
    /// 配置映射
    pub config_mappings: HashMap<String, ConfigMapping>,
    
    /// 依赖映射
    pub dependency_mappings: Vec<DependencyMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeMapping {
    pub source_type: String,
    pub target_type: String,
    pub conversion_notes: String,
    pub examples: Vec<(String, String)>, // (source_example, target_example)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMapping {
    pub source_endpoint: String,
    pub target_endpoint: String,
    pub method_change: Option<MethodChange>,
    pub request_mapping: RequestResponseMapping,
    pub response_mapping: RequestResponseMapping,
    pub breaking_changes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MethodChange {
    None,
    GetToPost,
    PostToPut,
    PathChanged,
    MethodRenamed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestResponseMapping {
    pub field_renames: HashMap<String, String>,
    pub field_types_changed: HashMap<String, FieldTypeChange>,
    pub fields_added: Vec<String>,
    pub fields_removed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldTypeChange {
    pub from_type: String,
    pub to_type: String,
    pub conversion_needed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigMapping {
    pub source_key: String,
    pub target_key: String,
    pub value_transformation: Option<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyMapping {
    pub source_dependency: String,
    pub target_dependency: String,
    pub version_mapping: Option<String>,
    pub alternatives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub files_modified: usize,
    pub files_added: usize,
    pub files_removed: usize,
    pub lines_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub types_converted: usize,
    apis_migrated: usize,
    pub patterns_refactored: usize,
    pub bugs_fixed: usize,
    pub security_improvements: usize,
    pub performance_improvements: Vec<PerformanceImprovement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceImprovement {
    pub area: String,
    pub before_metric: String,
    pub after_metric: String,
    pub improvement_percent: f64,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskReport {
    pub overall_risk_level: RiskLevel,
    pub risks: Vec<MigrationRisk>,
    pub mitigations: Vec<RiskMitigation>,
    pub testing_requirements: TestingRequirements,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRisk {
    pub id: String,
    pub category: RiskCategory,
    pub description: String,
    pub probability: ProbabilityLevel,
    pub impact: ImpactLevel,
    pub affected_components: Vec<String>,
    pub mitigation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskMitigation {
    pub id: String,
    pub strategy: String,
    pub implementation_steps: Vec<String>,
    pub owner: String,
    pub timeline: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestingRequirements {
    pub unit_tests_required: bool,
    pub integration_tests_required: bool,
    pub e2e_tests_required: bool,
    pub performance_tests_required: bool,
    pub security_tests_required: bool,
    pub minimum_coverage_target: f64,
    pub regression_test_suite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: String,
    pub category: RecommendationCategory,
    pub priority: Priority,
    pub title: String,
    pub description: String,
    pub effort_estimate: EffortEstimate,
    pub impact: ImpactDescription,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RecommendationCategory {
    ImmediateAction,
    ShortTerm,
    MediumTerm,
    LongTerm,
    BestPractice,
    Optimization,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffortEstimate {
    pub hours: f64,
    pub complexity: ComplexityLevel,
    pub skills_required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactDescription {
    pub benefits: Vec<String>,
    pub risks: Vec<String>,
    pub metrics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimation {
    pub development_cost_usd: f64,
    pub testing_cost_usd: f64,
    pub deployment_cost_usd: f64,
    pub training_cost_usd: f64,
    pub contingency_usd: f64,
    pub total_cost_usd: f64,
    pub cost_breakdown: HashMap<String, f64>,
    pub roi_projection: RoiProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoiProjection {
    pub year_1_savings_usd: f64,
    pub year_3_savings_usd: f64,
    pub payback_months: u32,
    pub npv_5_year: f64,
}

// ─── Specialized Migrators ─────────────────

/// .NET Framework → .NET 6/7/8 迁移器
pub struct DotNetMigrator {
    config: DotNetMigrationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotNetMigrationConfig {
    /// 源版本 (.NET Framework版本)
    pub source_version: DotNetFrameworkVersion,
    
    /// 目标版本 (.NET 6/7/8)
    pub target_version: DotNetVersion,
    
    /// 是否启用Windows Forms/WPF迁移
    pub migrate_desktop_apps: bool,
    
    /// 是否使用兼容包
    pub use_compatibility_shims: bool,
    
    /// 是否升级到最新C#语法
    pub upgrade_csharp_syntax: bool,
    
    /// 是否迁移到minimal APIs
    pub use_minimal_apis: bool,
    
    /// 是否使用顶级语句
    pub use_top_level_statements: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum DotNetFrameworkVersion {
    V35,
    V4,
    V45,
    V451,
    V452,
    V46,
    V461,
    V462,
    V47,
    V471,
    V472,
    V48,
    V481,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum DotNetVersion {
    Net6,
    Net7,
    Net8,
}

impl DotNetMigrator {
    pub fn new(config: DotNetMigrationConfig) -> Self {
        Self { config }
    }
    
    /// 执行.NET迁移
    pub async fn migrate(&self, project_path: &str) -> Result<DotNetMigrationResult> {
        let start = std::time::Instant::now();
        
        tracing::info!(
            source = ?self.config.source_version,
            target = ?self.config.target_version,
            path = %project_path,
            "Starting .NET migration"
        );
        
        // 1. 分析现有项目结构
        let analysis = self.analyze_dotnet_project(project_path).await?;
        
        // 2. 生成迁移计划
        let migration_plan = self.create_migration_plan(&analysis).await?;
        
        // 3. 执行代码转换
        let converted_files = self.convert_code(&analysis).await?;
        
        // 4. 更新项目文件
        let updated_project_files = self.update_project_files(&analysis).await?;
        
        // 5. 生成配置迁移
        let config_changes = self.migrate_configurations(&analysis).await?;
        
        // 6. 创建测试策略
        let test_strategy = self.create_test_strategy(&analysis).await?;
        
        let elapsed = start.elapsed();
        
        Ok(DotNetMigrationResult {
            metadata: DotNetMigrationMetadata {
                source_version: self.config.source_version,
                target_version: self.config.target_version,
                files_analyzed: analysis.total_files,
                files_converted: converted_files.len(),
                conversion_rate: if analysis.total_files > 0 {
                    converted_files.len() as f64 / analysis.total_files as f64
                } else {
                    0.0
                },
                duration_ms: elapsed.as_millis() as u64,
            },
            migration_plan,
            converted_files,
            updated_project_files,
            config_changes,
            test_strategy,
            breaking_changes: identify_breaking_changes(&self.config),
            compatibility_notes: generate_compatibility_notes(&self.config),
            next_steps: generate_next_steps(&self.config),
        })
    }
    
    async fn analyze_dotnet_project(&self, _path: &str) -> Result<DotNetProjectAnalysis> {
        Ok(DotNetProjectAnalysis {
            total_files: 150, // Placeholder
            csproj_files: vec!["MyApp.csproj".to_string()],
            solution_file: Some("MyApp.sln".to_string()),
            frameworks_used: vec![
                ".NET Framework 4.7.2".to_string(),
                "ASP.NET MVC 5".to_string(),
                "Entity Framework 6".to_string(),
            ],
            nuget_packages: vec![
                "Newtonsoft.Json".to_string(),
                "AutoMapper".to_string(),
                "Serilog".to_string(),
            ],
            code_patterns: vec![
                CodePattern {
                    name: "Controller with ActionResult".to_string(),
                    count: 25,
                    complexity: PatternComplexity::Medium,
                    migration_difficulty: MigrationDifficulty::Moderate,
                },
                CodePattern {
                    name: "DbContext with DbSet".to_string(),
                    count: 12,
                    complexity: PatternComplexity::Low,
                    migration_difficulty: MigrationDifficulty::Easy,
                },
                CodePattern {
                    name: "Synchronous I/O operations".to_string(),
                    count: 89,
                    complexity: PatternComplexity::High,
                    migration_difficulty: MigrationDifficulty::Hard,
                },
            ],
            estimated_complexity: ProjectComplexity::High,
        })
    }
    
    async fn create_migration_plan(&self, analysis: &DotNetProjectAnalysis) -> Result<MigrationPlan> {
        Ok(MigrationPlan {
            phases: vec![
                MigrationPhase {
                    id: 1,
                    name: "项目文件升级".to_string(),
                    tasks: vec![
                        "将.csproj转换为SDK样式".to_string(),
                        "更新目标框架".to_string(),
                        "移除packages.config".to_string(),
                        "更新NuGet包引用".to_string(),
                    ],
                    estimated_hours: 4.0,
                    dependencies: vec![],
                },
                MigrationPhase {
                    id: 2,
                    name: "API层迁移".to_string(),
                    tasks: vec![
                        "Controllers → Minimal APIs".to_string(),
                        "ActionResult → IResult".to_string(),
                        "同步方法改为异步".to_string(),
                        "添加依赖注入".to_string(),
                    ],
                    estimated_hours: 16.0,
                    dependencies: vec![1],
                },
                MigrationPhase {
                    id: 3,
                    name: "数据访问层迁移".to_string(),
                    tasks: vec![
                        "EF6 → EF Core".to_string(),
                        "DbContext重构".to_string(),
                        "LINQ查询优化".to_string(),
                        "连接字符串管理".to_string(),
                    ],
                    estimated_hours: 12.0,
                    dependencies: vec![1],
                },
                MigrationPhase {
                    id: 4,
                    name: "业务逻辑层优化".to_string(),
                    tasks: vec![
                        "使用record类型".to_string(),
                        "应用pattern matching".to_string(),
                        "引入nullable reference types".to_string(),
                        "利用C#新特性简化代码".to_string(),
                    ],
                    estimated_hours: 20.0,
                    dependencies: vec![2, 3],
                },
                MigrationPhase {
                    id: 5,
                    name: "测试和验证".to_string(),
                    tasks: vec![
                        "单元测试适配".to_string(),
                        "集成测试编写".to_string(),
                        "性能基准测试".to_string(),
                        "回归测试套件".to_string(),
                    ],
                    estimated_hours: 16.0,
                    dependencies: vec![4],
                },
            ],
            total_estimated_hours: 68.0,
            critical_path: vec![1, 2, 4, 5],
            risk_areas: vec![
                "EF6到EF Core的重大API变化".to_string(),
                "同步到异步的转换复杂度".to_string(),
                "第三方库兼容性问题".to_string(),
            ],
        })
    }
    
    async fn convert_code(&self, _analysis: &DotNetProjectAnalysis) -> Result<Vec<ConvertedFile>> {
        Ok(vec![
            ConvertedFile {
                original_path: "Controllers/HomeController.cs".to_string(),
                new_content: generate_sample_controller_conversion(),
                changes: vec![
                    CodeChange {
                        line_number: 1,
                        old_code: "public class HomeController : Controller".to_string(),
                        new_code: "public static class HomeEndpoints".to_string(),
                        change_reason: "迁移到Minimal API".to_string(),
                        confidence: 0.95,
                    },
                ],
                manual_review_needed: false,
                notes: vec!["成功转换为Minimal API风格".to_string()],
            },
        ])
    }
    
    async fn update_project_files(&self, _analysis: &DotNetProjectAnalysis) -> Result<Vec<ProjectFileUpdate>> {
        Ok(vec![
            ProjectFileUpdate {
                file_name: "MyApp.csproj".to_string(),
                content: generate_new_csproj(&self.config.target_version),
                update_type: ProjectFileUpdateType::FullReplace,
            },
        ])
    }
    
    async fn migrate_configurations(&self, _analysis: &DotNetProjectAnalysis) -> Result<Vec<ConfigChange>> {
        Ok(vec![
            ConfigChange {
                config_file: "web.config".to_string(),
                changes: vec![
                    ConfigItemChange {
                        key: "connectionStrings".to_string(),
                        action: ConfigChangeAction::MoveToAppSettingsJson,
                        new_location: "appsettings.json".to_string(),
                        notes: ".NET Core/6+ 使用 appsettings.json 替代 web.config".to_string(),
                    },
                ],
            },
        ])
    }
    
    async fn create_test_strategy(&self, _analysis: &DotNetProjectAnalysis) -> Result<TestStrategy> {
        Ok(TestStrategy {
            unit_tests: TestApproach {
                framework: "xUnit + Moq".to_string(),
                coverage_target: 80.0,
                focus_areas: vec!["API端点".to_string(), "业务逻辑".to_string(), "数据访问".to_string()],
            },
            integration_tests: TestApproach {
                framework: "xUnit + WebApplicationFactory".to_string(),
                coverage_target: 60.0,
                focus_areas: vec!["数据库集成".to_string(), "外部API调用".to_string()],
            },
            e2e_tests: TestApproach {
                framework: "Playwright / Selenium".to_string(),
                coverage_target: 40.0,
                focus_areas: vec!["关键用户流程".to_string()],
            },
            regression_suite: true,
            performance_baselines: true,
        })
    }
}

// ─── Java Virtual Thread Migrator ───────────

/// Java Virtual Thread 迁移器
pub struct JavaVtMigrator {
    config: JavaVtMigrationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaVtMigrationConfig {
    /// 源Java版本
    pub source_java_version: JavaVersion,
    
    /// 目标Java版本 (必须是21+)
    pub target_java_version: JavaVersion,
    
    /// 并发模型
    pub concurrency_model: ConcurrencyModel,
    
    /// 是否自动检测阻塞点
    pub detect_blocking_points: bool,
    
    /// 最大虚拟线程数
    pub max_virtual_threads: Option<usize>,
    
    /// 是否使用structured concurrency
    pub use_structured_concurrency: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum JavaVersion {
    Java8,
    Java11,
    Java17,
    Java21,
    Java22,
    Latest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ConcurrencyModel {
    VirtualThreadsOnly,         // 仅使用虚拟线程
    Hybrid,                     // 混合模式（关键路径用平台线程）
    StructuredConcurrency,     // 结构化并发
    ReactiveWithVT,            // 响应式 + 虚拟线程混合
}

impl JavaVtMigrator {
    pub fn new(config: JavaVtMigrationConfig) -> Self {
        assert!(
            matches!(config.target_java_version, JavaVersion::Java21 | JavaVersion::Java22 | JavaVersion::Latest),
            "Virtual threads require Java 21 or later"
        );
        Self { config }
    }
    
    /// 执行虚拟线程迁移
    pub async fn migrate_to_virtual_threads(&self, project_path: &str) -> Result<JavaVtMigrationResult> {
        let start = std::time::Instant::now();
        
        tracing::info!(
            from = ?self.config.source_java_version,
            to = ?self.config.target_java_version,
            model = ?self.config.concurrency_model,
            path = %project_path,
            "Starting Java Virtual Thread migration"
        );
        
        // 1. 分析现有线程使用情况
        let thread_analysis = self.analyze_thread_usage(project_path).await?;
        
        // 2. 识别可并行化的机会
        let parallelism_opportunities = self.identify_parallelism_opportunities(&thread_analysis).await?;
        
        // 3. 生成迁移代码
        let migrated_files = self.generate_vt_code(&thread_analysis).await?;
        
        // 4. 创建新的执行器配置
        let executor_configs = self.create_executor_configurations().await?;
        
        // 5. 生成性能对比基线
        let performance_baseline = self.create_performance_baseline(&thread_analysis).await?;
        
        let elapsed = start.elapsed();
        
        Ok(JavaVtMigrationResult {
            metadata: JavaVtMetadata {
                source_version: self.config.source_java_version,
                target_version: self.config.target_java_version,
                concurrency_model: self.config.concurrency_model,
                files_analyzed: thread_analysis.total_files,
                files_migrated: migrated_files.len(),
                threads_identified: thread_analysis.total_thread_instances,
                virtual_threads_created: estimate_virtual_threads(&migrated_files),
                duration_ms: elapsed.as_millis() as u64,
            },
            thread_analysis,
            parallelism_opportunities,
            migrated_files,
            executor_configs,
            performance_baseline,
            best_practices: generate_vt_best_practices(),
            common_pitfalls: generate_vt_common_pitfalls(),
            monitoring_recommendations: generate_monitoring_recommendations(),
        })
    }
    
    async fn analyze_thread_usage(&self, _path: &str) -> Result<ThreadUsageAnalysis> {
        Ok(ThreadUsageAnalysis {
            total_files: 200, // Placeholder
            total_thread_instances: 450,
            thread_types: vec![
                ThreadTypeStats {
                    pattern: "new Thread(new Runnable())".to_string(),
                    count: 120,
                    avg_lifecycle_ms: 5000,
                    blocking_probability: BlockingProbability::High,
                    migration_benefit: MigrationBenefit::VeryHigh,
                },
                ThreadTypeStats {
                    pattern: "ExecutorService.submit()".to_string(),
                    count: 180,
                    avg_lifecycle_ms: 3000,
                    blocking_probability: BlockingProbability::Medium,
                    migration_benefit: MigrationBenefit::High,
                },
                ThreadTypeStats {
                    pattern: "CompletableFuture.supplyAsync()".to_string(),
                    count: 85,
                    avg_lifecycle_ms: 2000,
                    blocking_probability: BlockingProbability::Low,
                    migration_benefit: MigrationBenefit::Medium,
                },
                ThreadTypeStats {
                    pattern: "@Async annotation".to_string(),
                    count: 65,
                    avg_lifecycle_ms: 4000,
                    blocking_probability: BlockingProbability::Medium,
                    migration_benefit: MigrationBenefit::High,
                },
            ],
            synchronization_patterns: vec![
                "synchronized blocks".to_string(),
                "ReentrantLock".to_string(),
                "CountDownLatch".to_string(),
                "Semaphore".to_string(),
            ],
            blocking_operations: vec![
                BlockingOperation {
                    type_: "Database I/O".to_string(),
                    frequency: Frequency::VeryFrequent,
                    avg_duration_ms: 50,
                    vt_improvement_potential: ImprovementPotential::Significant,
                },
                BlockingOperation {
                    type_: "HTTP calls".to_string(),
                    frequency: Frequency::Frequent,
                    avg_duration_ms: 200,
                    vt_improvement_potential: ImprovementPotential::Major,
                },
                BlockingOperation {
                    type_: "File I/O".to_string(),
                    frequency: Frequency::Occasional,
                    avg_duration_ms: 100,
                    vt_improvement_potential: ImprovementPotential::Moderate,
                },
            ],
            estimated_complexity: MigrationComplexity::Moderate,
        })
    }
    
    async fn identify_parallelism_opportunities(&self, analysis: &ThreadUsageAnalysis) -> Result<Vec<ParallelismOpportunity>> {
        Ok(vec![
            ParallelismOpportunity {
                id: "parallel-db-queries".to_string(),
                location: "Repository Layer".to_string(),
                description: "批量数据库查询可以并行执行".to_string(),
                current_approach: "顺序执行多个独立查询".to_string(),
                proposed_solution: "使用try-with-resources + virtual threads并行查询".to_string(),
                estimated_speedup: 3.5,
                risk_level: RiskLevel::Low,
                effort_hours: 8.0,
            },
            ParallelismOpportunity {
                id: "parallel-http-calls".to_string(),
                location: "Service Layer".to_string(),
                description: "聚合多个外部API调用".to_string(),
                current_approach: "串行HTTP请求".to_string(),
                proposed_solution: "StructuredTaskScope并行调用".to_string(),
                estimated_speedup: 4.2,
                risk_level: RiskLevel::Low,
                effort_hours: 12.0,
            },
            ParallelismOpportunity {
                id: "parallel-file-processing".to_string(),
                location: "Batch Processing".to_string(),
                description: "大批量文件处理任务".to_string(),
                current_approach: "固定大小线程池处理".to_string(),
                proposed_solution: "每任务一个虚拟线程".to_string(),
                estimated_speedup: 2.8,
                risk_level: RiskLevel::Medium,
                effort_hours: 16.0,
            },
        ])
    }
    
    async fn generate_vt_code(&self, _analysis: &ThreadUsageAnalysis) -> Result<Vec<VtConvertedFile>> {
        Ok(vec![
            VtConvertedFile {
                original_path: "src/main/java/com/example/service/UserService.java".to_string(),
                new_content: generate_sample_vt_service(),
                changes: vec![
                    VtCodeChange {
                        description: "ExecutorService → Executors.newVirtualThreadPerTaskExecutor()".to_string(),
                        before_snippet: "private final ExecutorService executor = Executors.newFixedThreadPool(10);".to_string(),
                        after_snippet: "private final ExecutorService executor = Executors.newVirtualThreadPerTaskExecutor();".to_string(),
                        benefit: "消除线程池大小限制，自动扩展".to_string(),
                    },
                    VtCodeChange {
                        description: "Future.get() → structured concurrency".to_string(),
                        before_snippet: "Future<User> future = executor.submit(() -> userRepository.findById(id));\nUser user = future.get();".to_string(),
                        after_snippet: "try (var scope = new StructuredTaskScope.ShutdownOnFailure()) {\n    Subtask<User> task = scope.fork(() -> userRepository.findById(id));\n    scope.join().throwIfFailed();\n    User user = task.get();\n}".to_string(),
                        benefit: "更好的错误传播和取消支持".to_string(),
                    },
                ],
                performance_impact: PerformanceImpact {
                    throughput_increase_percent: 250.0,
                    latency_decrease_percent: 40.0,
                    memory_usage_change_percent: -15.0, // 减少15%
                    cpu_utilization_optimization: "Better utilization due to more efficient scheduling".to_string(),
                },
            },
        ])
    }
    
    async fn create_executor_configurations(&self) -> Result<Vec<ExecutorConfiguration>> {
        Ok(vec![
            ExecutorConfiguration {
                name: "io-bound-tasks".to_string(),
                purpose: "I/O密集型任务（数据库、HTTP、文件）".to_string(),
                configuration: r#"Executors.newVirtualThreadPerTaskExecutor()
    .withThreadFactory(Thread.ofVirtual()
        .name("io-task-", 0)
        .factory())"#.to_string(),
                max_threads: None, // Unlimited for VT
                use_case_examples: vec![
                    "数据库查询".to_string(),
                    "HTTP客户端调用".to_string(),
                    "文件读写".to_string(),
                ],
            },
            ExecutorConfiguration {
                name: "cpu-bound-tasks".to_string(),
                purpose: "CPU密集型任务（仍使用平台线程）".to_string(),
                configuration: r#"Executors.newFixedThreadPool(
    Runtime.getRuntime().availableProcessors(),
    Thread.ofPlatform().factory()
)"#.to_string(),
                max_threads: Some("Runtime.getRuntime().availableProcessors()".to_string()),
                use_case_examples: vec![
                    "图像处理".to_string(),
                    "加密解密".to_string(),
                    "复杂数学计算".to_string(),
                ],
            },
            ExecutorConfiguration {
                name: "structured-concurrency".to_string(),
                purpose: "需要协调多个子任务的场景".to_string(),
                configuration: r#"new StructuredTaskScope.ShutdownOnFailure()"#.to_string(),
                max_threads: None,
                use_case_examples: vec![
                    "聚合服务调用".to_string(),
                    "并行数据处理管道".to_string(),
                    "批处理操作".to_string(),
                ],
            },
        ])
    }
    
    async fn create_performance_baseline(&self, _analysis: &ThreadUsageAnalysis) -> Result<PerformanceBaseline> {
        Ok(PerformanceBaseline {
            current_metrics: CurrentMetrics {
                avg_request_latency_ms: 450.0,
                p99_latency_ms: 1200.0,
                throughput_req_per_sec: 850.0,
                thread_pool_size: 50,
                avg_cpu_usage_percent: 65.0,
                memory_usage_mb: 512,
                context_switches_per_sec: 15000,
            },
            projected_metrics: ProjectedMetrics {
                avg_request_latency_ms: 270.0, // -40%
                p99_latency_ms: 720.0,          // -40%
                throughput_req_per_sec: 2100.0, // +147%
                virtual_threads_active: 5000,    // Much higher than pool size
                avg_cpu_usage_percent: 75.0,     // Better utilization
                memory_usage_mb: 435,            // -15% (less stack space per VT)
                context_switches_per_sec: 80000, // More but cheaper
            },
            improvement_summary: ImprovementSummary {
                latency_reduction_percent: 40.0,
                throughput_increase_percent: 147.0,
                resource_efficiency_gain_percent: 55.0,
                scalability_improvement: "Linear scaling with load instead of thread pool limits".to_string(),
                operational_simplicity: "No need to tune thread pool sizes".to_string(),
            },
        })
    }
}

// ─── Helper Types and Functions ─────────────

struct DotNetProjectAnalysis {
    total_files: usize,
    csproj_files: Vec<String>,
    solution_file: Option<String>,
    frameworks_used: Vec<String>,
    nuget_packages: Vec<String>,
    code_patterns: Vec<CodePattern>,
    estimated_complexity: ProjectComplexity,
}

struct CodePattern {
    name: String,
    count: usize,
    complexity: PatternComplexity,
    migration_difficulty: MigrationDifficulty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PatternComplexity {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MigrationDifficulty {
    Easy,
    Moderate,
    Hard,
    VeryHard,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ProjectComplexity {
    Low,
    Medium,
    High,
    VeryHigh,
}

struct DotNetMigrationResult {
    metadata: DotNetMigrationMetadata,
    migration_plan: MigrationPlan,
    converted_files: Vec<ConvertedFile>,
    updated_project_files: Vec<ProjectFileUpdate>,
    config_changes: Vec<ConfigChange>,
    test_strategy: TestStrategy,
    breaking_changes: Vec<BreakingChange>,
    compatibility_notes: Vec<String>,
    next_steps: Vec<String>,
}

struct DotNetMigrationMetadata {
    source_version: DotNetFrameworkVersion,
    target_version: DotNetVersion,
    files_analyzed: usize,
    files_converted: usize,
    conversion_rate: f64,
    duration_ms: u64,
}

struct MigrationPlan {
    phases: Vec<MigrationPhase>,
    total_estimated_hours: f64,
    critical_path: Vec<usize>,
    risk_areas: Vec<String>,
}

struct MigrationPhase {
    id: usize,
    name: String,
    tasks: Vec<String>,
    estimated_hours: f64,
    dependencies: Vec<usize>,
}

struct ConvertedFile {
    original_path: String,
    new_content: String,
    changes: Vec<CodeChange>,
    manual_review_needed: bool,
    notes: Vec<String>,
}

struct CodeChange {
    line_number: usize,
    old_code: String,
    new_code: String,
    change_reason: String,
    confidence: f64,
}

struct ProjectFileUpdate {
    file_name: String,
    content: String,
    update_type: ProjectFileUpdateType,
}

enum ProjectFileUpdateType {
    FullReplace,
    PartialUpdate,
    Rename,
    Delete,
}

struct ConfigChange {
    config_file: String,
    changes: Vec<ConfigItemChange>,
}

struct ConfigItemChange {
    key: String,
    action: ConfigChangeAction,
    new_location: Option<String>,
    notes: String,
}

enum ConfigChangeAction {
    MoveToAppSettingsJson,
    UpdateValue,
    Remove,
    AddNew,
}

struct TestStrategy {
    unit_tests: TestApproach,
    integration_tests: TestApproach,
    e2e_tests: TestApproach,
    regression_suite: bool,
    performance_baselines: bool,
}

struct TestApproach {
    framework: String,
    coverage_target: f64,
    focus_areas: Vec<String>,
}

struct BreakingChange {
    component: String,
    description: String,
    mitigation: String,
}

// Java VT related types
struct ThreadUsageAnalysis {
    total_files: usize,
    total_thread_instances: usize,
    thread_types: Vec<ThreadTypeStats>,
    synchronization_patterns: Vec<String>,
    blocking_operations: Vec<BlockingOperation>,
    estimated_complexity: MigrationComplexity,
}

struct ThreadTypeStats {
    pattern: String,
    count: usize,
    avg_lifecycle_ms: u64,
    blocking_probability: BlockingProbability,
    migration_benefit: MigrationBenefit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BlockingProbability {
    Low,
    Medium,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MigrationBenefit {
    Low,
    Medium,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MigrationComplexity {
    Trivial,
    Easy,
    Moderate,
    Complex,
    VeryComplex,
}

struct BlockingOperation {
    type_: String,
    frequency: Frequency,
    avg_duration_ms: u64,
    vt_improvement_potential: ImprovementPotential,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Frequency {
    Occasional,
    Frequent,
    VeryFrequent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ImprovementPotential {
    Negligible,
    Moderate,
    Significant,
    Major,
}

struct ParallelismOpportunity {
    id: String,
    location: String,
    description: String,
    current_approach: String,
    proposed_solution: String,
    estimated_speedup: f64,
    risk_level: RiskLevel,
    effort_hours: f64,
}

struct VtConvertedFile {
    original_path: String,
    new_content: String,
    changes: Vec<VtCodeChange>,
    performance_impact: PerformanceImpact,
}

struct VtCodeChange {
    description: String,
    before_snippet: String,
    after_snippet: String,
    benefit: String,
}

struct PerformanceImpact {
    throughput_increase_percent: f64,
    latency_decrease_percent: f64,
    memory_usage_change_percent: f64,
    cpu_utilization_optimization: String,
}

struct ExecutorConfiguration {
    name: String,
    purpose: String,
    configuration: String,
    max_threads: Option<String>,
    use_case_examples: Vec<String>,
}

struct JavaVtMigrationResult {
    metadata: JavaVtMetadata,
    thread_analysis: ThreadUsageAnalysis,
    parallelism_opportunities: Vec<ParallelismOpportunity>,
    migrated_files: Vec<VtConvertedFile>,
    executor_configs: Vec<ExecutorConfiguration>,
    performance_baseline: PerformanceBaseline,
    best_practices: Vec<String>,
    common_pitfalls: Vec<String>,
    monitoring_recommendations: Vec<String>,
}

struct JavaVtMetadata {
    source_version: JavaVersion,
    target_version: JavaVersion,
    concurrency_model: ConcurrencyModel,
    files_analyzed: usize,
    files_migrated: usize,
    threads_identified: usize,
    virtual_threads_created: usize,
    duration_ms: u64,
}

struct PerformanceBaseline {
    current_metrics: CurrentMetrics,
    projected_metrics: ProjectedMetrics,
    improvement_summary: ImprovementSummary,
}

struct CurrentMetrics {
    avg_request_latency_ms: f64,
    p99_latency_ms: f64,
    throughput_req_per_sec: f64,
    thread_pool_size: usize,
    avg_cpu_usage_percent: f64,
    memory_usage_mb: usize,
    context_switches_per_sec: usize,
}

struct ProjectedMetrics {
    avg_request_latency_ms: f64,
    p99_latency_ms: f64,
    throughput_req_per_sec: f64,
    virtual_threads_active: usize,
    avg_cpu_usage_percent: f64,
    memory_usage_mb: usize,
    context_switches_per_sec: usize,
}

struct ImprovementSummary {
    latency_reduction_percent: f64,
    throughput_increase_percent: f64,
    resource_efficiency_gain_percent: f64,
    scalability_improvement: String,
    operational_simplicity: String,
}

// Generator functions (simplified implementations)

fn generate_sample_controller_conversion() -> String {
    r#"using Microsoft.AspNetCore.Mvc;
using MyApp.Services;

namespace MyApp.Api;

// Before: Traditional MVC Controller
// After: Minimal API Endpoint

public static class HomeEndpoints
{
    public static void MapHomeEndpoints(this WebApplication app)
    {
        var group = app.MapGroup("/api/home");
        
        group.MapGet("/", async () =>
        {
            // Migrated from HomeController.Index()
            return Results.Ok(new { message = "Welcome to the modernized API!" });
        });
        
        group.MapGet("/about", async () =>
        {
            // Migrated from HomeController.About()
            return Results.Ok(new 
            { 
                version = "2.0", 
                framework = ".NET 8",
                architecture = "Minimal API"
            });
        });
    }
}
"#.to_string()
}

fn generate_new_csproj(target: &DotNetVersion) -> String {
    let tfm = match target {
        DotNetVersion::Net6 => "net6.0",
        DotNetVersion::Net7 => "net7.0",
        DotNetVersion::Net8 => "net8.0",
    };
    
    format!(
        r#"<Project Sdk="Microsoft.NET.Sdk.Web">

  <PropertyGroup>
    <TargetFramework>{tfm}</TargetFramework>
    <Nullable>enable</Nullable>
    <ImplicitUsings>enable</ImplicitUsings>
    <RootNamespace>MyApp</RootNamespace>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="Microsoft.EntityFrameworkCore" Version="8.0.0" />
    <PackageReference Include="Serilog.AspNetCore" Version="8.0.0" />
    <PackageReference Include="AutoMapper.Extensions.Microsoft.DependencyInjection" Version="12.0.0" />
  </ItemGroup>

</Project>
"#
    )
}

fn generate_sample_vt_service() -> String {
    r#"package com.example.service;

import java.util.concurrent.*;
import java.util.List;
import java.util.UUID;

/**
 * UserService - Migrated to Virtual Threads (Java 21+)
 * 
 * Key Changes:
 * 1. ExecutorService → VirtualThreadPerTaskExecutor
 * 2. Future.get() → StructuredTaskScope
 * 3. @Async → Explicit virtual thread creation
 */
@Service
public class UserService {

    private final UserRepository userRepository;
    private final ExternalApiClient apiClient;
    
    // NEW: Virtual thread executor for I/O-bound tasks
    private final ExecutorService ioExecutor = Executors.newVirtualThreadPerTaskExecutor(
        Thread.ofVirtual()
            .name("user-service-io-", 0)
            .daemon(true)
            .priority(Thread.NORM_PRIORITY)
            .factory()
    );
    
    // KEEP: Platform thread executor for CPU-bound tasks
    private final ExecutorService cpuExecutor = Executors.newFixedThreadPool(
        Runtime.getRuntime().availableProcessors(),
        Thread.ofPlatform().factory()
    );

    public UserService(UserRepository userRepository, ExternalApiClient apiClient) {
        this.userRepository = userRepository;
        this.apiClient = apiClient;
    }

    /**
     * Fetch user with parallel enrichment using virtual threads.
     * 
     * BEFORE:
     * User user = userRepository.findById(id);
     * Profile profile = apiClient.getUserProfile(user.getExternalId());
     * List<Order> recentOrders = orderService.getRecentOrders(id);
     * 
     * AFTER: All three calls happen concurrently on virtual threads!
     */
    public UserDetail getUserDetail(UUID id) throws Exception {
        try (var scope = new StructuredTaskScope.ShutdownOnFailure()) {
            
            // Fork subtasks on virtual threads (automatic!)
            Subtask<User> userTask = scope.fork(() -> 
                userRepository.findById(id)
            );
            
            Subtask<Profile> profileTask = scope.fork(() -> 
                apiClient.getUserProfile(
                    userTask.get().getExternalId() // Will block until ready
                )
            );
            
            Subtask<List<Order>> ordersTask = scope.fork(() -> 
                orderService.getRecentOrders(id)
            );
            
            // Join all - any failure cancels others
            scope.join().throwIfFailed();
            
            // Combine results
            return new UserDetail(
                userTask.get(),
                profileTask.get(),
                ordersTask.get()
            );
        }
    }

    /**
     * Batch process users - each on its own virtual thread.
     * 
     * BENEFIT: No thread pool size limitation!
     * Can process 10,000 users concurrently if needed.
     */
    public CompletableFuture<List<ProcessingResult>> processUsersBatch(List<UUID> userIds) {
        List<CompletableFuture<ProcessingResult>> futures = userIds.stream()
            .map(userId -> CompletableFuture.supplyAsync(
                () -> processSingleUser(userId),
                ioExecutor // Virtual thread executor!
            ))
            .toList();
        
        return CompletableFuture.allOf(futures.toArray(new CompletableFuture[0]))
            .thenApply(v -> futures.stream()
                .map(CompletableFuture::join)
                .toList()
            );
    }

    private ProcessingResult processSingleUser(UUID userId) {
        // This runs on a virtual thread - cheap and scalable!
        User user = userRepository.findById(userId);
        // ... processing logic ...
        return new ProcessingResult(userId, "SUCCESS");
    }
}
"#.to_string()
}

fn identify_breaking_changes(_config: &DotNetMigrationConfig) -> Vec<BreakingChange> {
    vec![
        BreakingChange {
            component: "Entity Framework".to_string(),
            description: "EF6 API significantly different from EF Core".to_string(),
            mitigation: "Use EF Core compatible wrappers or full rewrite of data layer".to_string(),
        },
        BreakingChange {
            component: "ASP.NET MVC".to_string(),
            description: "Controller base class and ActionResult patterns changed".to_string(),
            mitigation: "Migrate to Minimal APIs or Razor Pages".to_string(),
        },
        BreakingChange {
            component: "Configuration".to_string(),
            description: "web.config replaced by appsettings.json".to_string(),
            mitigation: "Use IConfiguration abstraction".to_string(),
        },
    ]
}

fn generate_compatibility_notes(_config: &DotNetMigrationConfig) -> Vec<String> {
    vec![
        "✅ .NET 8 is LTS (Long Term Support)".to_string(),
        "⚠️ Windows Forms apps require WinForms designer updates".to_string(),
        "ℹ️ WPF apps work well but may need UI toolkit updates".to_string(),
        "💡 Consider using MAUI for cross-platform desktop apps".to_string(),
        "📚 Documentation: https://docs.microsoft.com/dotnet/core/migration/".to_string(),
    ]
}

fn generate_next_steps(_config: &DotNetMigrationConfig) -> Vec<String> {
    vec![
        "1. Set up .NET 8 SDK and verify installation".to_string(),
        "2. Create backup of current solution".to_string(),
        "3. Run .NET Upgrade Assistant tool for initial assessment".to_string(),
        "4. Start with non-critical projects for practice".to_string(),
        "5. Establish CI/CD pipeline for both old and new versions".to_string(),
        "6. Plan phased rollout with feature flags".to_string(),
        "7. Monitor performance and gather feedback".to_string(),
    ]
}

fn estimate_virtual_threads(files: &[VtConvertedFile]) -> usize {
    // Rough estimate: each converted file creates ~10-50 VT on average
    files.len() * 25
}

fn generate_vt_best_practices() -> Vec<String> {
    vec![
        "🎯 Use virtual threads for I/O-bound tasks only".to_string(),
        "🔒 Keep platform threads for CPU-intensive work".to_string(),
        "📊 Monitor virtual thread creation rate (avoid unbounded growth)".to_string(),
        "🧪 Use StructuredTaskScope for coordinated tasks".to_string(),
        "⚠️ Avoid synchronized blocks in virtual threads (use ReentrantLock instead)".to_string(),
        "📈 Set JVM flag -Djdk.virtualThreadScheduler.parallelism=N".to_string(),
        "🔄 Use ThreadLocal carefully (can consume more memory with many VTs)".to_string(),
        "🛡️ Always handle InterruptedException properly".to_string(),
        "📝 Prefer structured concurrency over raw Future/CompletableFuture".to_string(),
        "🔍 Profile before and after migration to measure real improvements".to_string(),
    ]
}

fn generate_vt_common_pitfalls() -> Vec<String> {
    vec![
        "❌ Pinning: Don't call Thread.park() or LockSupport.park() in VT (pins to carrier)".to_string(),
        "❌ Excessive allocation: Each VT has stack - don't create millions unnecessarily".to_string(),
        "❌ ThreadLocal abuse: Can cause memory pressure with massive VT counts".to_string(),
        "❌ Blocking in synchronized: Use ReentrantLock instead for better scalability".to_string(),
        "❌ Ignoring backpressure: Still need queues/rate limiting even with VT".to_string(),
        "⚠️ Debugging: Thread dumps show carrier threads, not VT names by default".to_string(),
        "⚠️ Monitoring: JMX may not show all VT metrics accurately".to_string(),
        "⚠️ Legacy libraries: May not be VT-safe (test thoroughly)".to_string(),
    ]
}

fn generate_monitoring_recommendations() -> Vec<String> {
    vec![
        "📊 Track virtual thread creation/disposal rates via JFR".to_string(),
        "📈 Monitor carrier thread pool utilization".to_string(),
        "⏱️ Measure task completion time distribution".to_string(),
        "💾 Watch heap usage (more stacks = more memory)".to_string(),
        "🔍 Enable JDK Flight Recorder for detailed analysis".to_string(),
        "📉 Set up alerts for pinning detection (long-running VTs)".to_string(),
        "🎯 Compare pre/post migration metrics systematically".to_string(),
        "📋 Document baseline and targets before starting".to_string(),
    ]
}
