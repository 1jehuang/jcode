//! 重构模板库 - 常见重构模式的自动化实现
//!
//! 这个模块包含了各种常见的重构模式和自动化模板，
//! 用于加速代码重构工作。

use std::collections::{HashMap, HashSet};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

// --- 重构模板类型 ---------------------------------

/// 重构模板类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefactorTemplateType {
    /// 提取函数/方法
    ExtractFunction,
    /// 提取类/结构体
    ExtractStruct,
    /// 重命名符号
    RenameSymbol,
    /// 内联变量/函数
    Inline,
    /// 拆分大文件
    SplitFile,
    /// 合并小文件
    MergeFiles,
    /// 添加错误处理
    AddErrorHandling,
    /// 简化条件表达式
    SimplifyCondition,
    /// 替换魔法数字
    ReplaceMagicNumber,
    /// 改进命名
    ImproveNaming,
    /// 添加文档注释
    AddDocumentation,
    /// 性能优化
    PerformanceOptimization,
    /// 安全加固
    SecurityHardening,
    /// 代码格式化
    CodeFormatting,
    /// 移除死代码
    RemoveDeadCode,
    /// 解耦重构
    Decouple,
    /// 架构重构
    Architecture,
    /// 其他自定义重构
    Custom,
}

impl std::fmt::Display for RefactorTemplateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExtractFunction => write!(f, "Extract Function/Method"),
            Self::ExtractStruct => write!(f, "Extract Struct/Class"),
            Self::RenameSymbol => write!(f, "Rename Symbol"),
            Self::Inline => write!(f, "Inline Variable/Function"),
            Self::SplitFile => write!(f, "Split Large File"),
            Self::MergeFiles => write!(f, "Merge Small Files"),
            Self::AddErrorHandling => write!(f, "Add Error Handling"),
            Self::SimplifyCondition => write!(f, "Simplify Condition"),
            Self::ReplaceMagicNumber => write!(f, "Replace Magic Number"),
            Self::ImproveNaming => write!(f, "Improve Naming"),
            Self::AddDocumentation => write!(f, "Add Documentation"),
            Self::PerformanceOptimization => write!(f, "Performance Optimization"),
            Self::SecurityHardening => write!(f, "Security Hardening"),
            Self::CodeFormatting => write!(f, "Code Formatting"),
            Self::RemoveDeadCode => write!(f, "Remove Dead Code"),
            Self::Decouple => write!(f, "Decouple Components"),
            Self::Architecture => write!(f, "Architecture Refactoring"),
            Self::Custom => write!(f, "Custom Refactor"),
        }
    }
}

// --- 重构模板定义 ---------------------------------

/// 重构模板
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorTemplate {
    /// 模板ID
    pub id: String,
    /// 模板名称
    pub name: String,
    /// 模板类型
    pub template_type: RefactorTemplateType,
    /// 描述
    pub description: String,
    /// 适用语言
    pub languages: Vec<String>,
    /// 难度等级
    pub difficulty: RefactorDifficulty,
    /// 风险等级
    pub risk_level: RefactorRiskLevel,
    /// 前置条件
    pub preconditions: Vec<Precondition>,
    /// 步骤
    pub steps: Vec<RefactorStep>,
    /// 验证检查
    pub validation_checks: Vec<ValidationCheck>,
    /// 成功案例
    pub success_examples: Vec<RefactorExample>,
    /// 失败案例
    pub failure_examples: Vec<RefactorExample>,
    /// 是否默认启用
    pub enabled: bool,
}

/// 重构难度等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefactorDifficulty {
    /// 简单 - 可以自动完成
    Easy,
    /// 中等 - 需要一些人工确认
    Medium,
    /// 困难 - 需要大量人工确认
    Hard,
    /// 专家 - 需要架构设计
    Expert,
}

/// 重构风险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefactorRiskLevel {
    /// 无风险
    Low,
    /// 低风险
    Medium,
    /// 中风险
    High,
    /// 高风险
    Critical,
}

/// 前置条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Precondition {
    pub id: String,
    pub description: String,
    pub check_type: CheckType,
}

/// 检查类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckType {
    CodePattern,
    FileStructure,
    Dependency,
    TestCoverage,
    Compilation,
    Custom,
}

/// 重构步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorStep {
    pub step_id: String,
    pub name: String,
    pub description: String,
    pub actions: Vec<StepAction>,
    pub requires_approval: bool,
}

/// 步骤动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepAction {
    pub action_type: ActionType,
    pub description: String,
    pub code_pattern: Option<String>,
    pub target_pattern: Option<String>,
}

/// 动作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    Extract,
    Rename,
    Inline,
    Move,
    Add,
    Remove,
    Modify,
    Format,
    Validate,
}

/// 验证检查
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationCheck {
    pub check_id: String,
    pub name: String,
    pub description: String,
    pub check_type: ValidationCheckType,
    pub required: bool,
}

/// 验证检查类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationCheckType {
    Compile,
    Test,
    Lint,
    Behavior,
    Performance,
    Security,
    Custom,
}

/// 重构示例
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorExample {
    pub name: String,
    pub before_code: String,
    pub after_code: String,
    pub description: String,
}

// --- 重构结果 --------------------------------------

/// 重构执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorExecutionResult {
    pub template_id: String,
    pub template_name: String,
    pub success: bool,
    pub steps_completed: Vec<String>,
    pub steps_failed: Vec<String>,
    pub changes: Vec<FileChange>,
    pub validation_passed: Vec<String>,
    pub validation_failed: Vec<String>,
    pub error_message: Option<String>,
    pub duration_ms: u64,
}

/// 文件变更
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub file_path: String,
    pub change_type: ChangeType,
    pub lines_changed: usize,
    pub diff_preview: Option<String>,
}

/// 变更类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Removed,
    Renamed,
    Moved,
}

// --- 模板库管理器 ----------------------------------

/// 重构模板库管理器
pub struct RefactorTemplateLibrary {
    templates: HashMap<String, RefactorTemplate>,
    language_filters: HashMap<String, HashSet<String>>,
    type_filters: HashMap<RefactorTemplateType, HashSet<String>>,
}

impl RefactorTemplateLibrary {
    /// 创建新的模板库
    pub fn new() -> Self {
        let mut library = Self {
            templates: HashMap::new(),
            language_filters: HashMap::new(),
            type_filters: HashMap::new(),
        };
        
        library.init_builtin_templates();
        library
    }

    /// 初始化内置模板
    fn init_builtin_templates(&mut self) {
        // 1. 提取函数模板
        self.add_template(Self::create_extract_function_template());
        
        // 2. 重命名符号模板
        self.add_template(Self::create_rename_symbol_template());
        
        // 3. 替换魔法数字模板
        self.add_template(Self::create_replace_magic_number_template());
        
        // 4. 简化条件表达式模板
        self.add_template(Self::create_simplify_condition_template());
        
        // 5. 添加错误处理模板
        self.add_template(Self::create_add_error_handling_template());
        
        // 6. 改善命名模板
        self.add_template(Self::create_improve_naming_template());
        
        // 7. 拆分大文件模板
        self.add_template(Self::create_split_file_template());
        
        // 8. 移除死代码模板
        self.add_template(Self::create_remove_dead_code_template());
        
        // 9. 添加文档模板
        self.add_template(Self::create_add_documentation_template());
        
        // 10. 安全加固模板
        self.add_template(Self::create_security_hardening_template());
        
        info!("Initialized refactor template library with {} built-in templates", self.templates.len());
    }

    /// 创建提取函数模板
    fn create_extract_function_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "extract_function".to_string(),
            name: "Extract Function/Method".to_string(),
            template_type: RefactorTemplateType::ExtractFunction,
            description: "Extract reusable code blocks into functions/methods".to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![
                Precondition {
                    id: "long_function".to_string(),
                    description: "Function has more than 50 lines".to_string(),
                    check_type: CheckType::CodePattern,
                },
                Precondition {
                    id: "duplicate_code".to_string(),
                    description: "Identifies duplicated code blocks".to_string(),
                    check_type: CheckType::CodePattern,
                },
            ],
            steps: vec![
                RefactorStep {
                    step_id: "identify".to_string(),
                    name: "Identify Extraction Candidates".to_string(),
                    description: "Find code blocks suitable for extraction".to_string(),
                    actions: vec![
                        StepAction {
                            action_type: ActionType::Extract,
                            description: "Extract code block".to_string(),
                            code_pattern: Some("// Identify candidate blocks".to_string()),
                            target_pattern: Some("fn extracted_fn() { ... }".to_string()),
                        },
                    ],
                    requires_approval: true,
                },
                RefactorStep {
                    step_id: "extract".to_string(),
                    name: "Perform Extraction".to_string(),
                    description: "Extract the code into a new function".to_string(),
                    actions: vec![
                        StepAction {
                            action_type: ActionType::Add,
                            description: "Add new function".to_string(),
                            code_pattern: None,
                            target_pattern: None,
                        },
                    ],
                    requires_approval: false,
                },
                RefactorStep {
                    step_id: "replace".to_string(),
                    name: "Replace Callsites".to_string(),
                    description: "Replace old code with calls to new function".to_string(),
                    actions: vec![
                        StepAction {
                            action_type: ActionType::Modify,
                            description: "Replace callsites".to_string(),
                            code_pattern: None,
                            target_pattern: None,
                        },
                    ],
                    requires_approval: false,
                },
            ],
            validation_checks: vec![
                ValidationCheck {
                    check_id: "compile".to_string(),
                    name: "Compile Check".to_string(),
                    description: "Verify code compiles after refactor".to_string(),
                    check_type: ValidationCheckType::Compile,
                    required: true,
                },
                ValidationCheck {
                    check_id: "test".to_string(),
                    name: "Test Check".to_string(),
                    description: "Run tests to verify functionality preserved".to_string(),
                    check_type: ValidationCheckType::Test,
                    required: true,
                },
            ],
            success_examples: vec![
                RefactorExample {
                    name: "Simple Extraction".to_string(),
                    before_code: "fn main() {\n    let x = 5;\n    let y = 10;\n    let z = x + y;\n    println!(\"Sum: {}\", z);\n}".to_string(),
                    after_code: "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    let x = 5;\n    let y = 10;\n    let z = add(x, y);\n    println!(\"Sum: {}\", z);\n}".to_string(),
                    description: "Extract addition logic into a reusable function".to_string(),
                },
            ],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建重命名符号模板
    fn create_rename_symbol_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "rename_symbol".to_string(),
            name: "Rename Symbol".to_string(),
            template_type: RefactorTemplateType::RenameSymbol,
            description: "Rename variables, functions, types with better names".to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![
                Precondition {
                    id: "bad_name".to_string(),
                    description: "Symbol has non-descriptive name".to_string(),
                    check_type: CheckType::CodePattern,
                },
            ],
            steps: vec![
                RefactorStep {
                    step_id: "find".to_string(),
                    name: "Find All Usages".to_string(),
                    description: "Identify all usages of the symbol".to_string(),
                    actions: vec![],
                    requires_approval: false,
                },
                RefactorStep {
                    step_id: "propose".to_string(),
                    name: "Propose New Name".to_string(),
                    description: "Suggest improved names based on context".to_string(),
                    actions: vec![],
                    requires_approval: true,
                },
                RefactorStep {
                    step_id: "apply".to_string(),
                    name: "Apply Rename".to_string(),
                    description: "Rename all usages across the codebase".to_string(),
                    actions: vec![],
                    requires_approval: false,
                },
            ],
            validation_checks: vec![
                ValidationCheck {
                    check_id: "compile".to_string(),
                    name: "Compile Check".to_string(),
                    description: "Verify code compiles after rename".to_string(),
                    check_type: ValidationCheckType::Compile,
                    required: true,
                },
            ],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建替换魔法数字模板
    fn create_replace_magic_number_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "replace_magic_number".to_string(),
            name: "Replace Magic Numbers".to_string(),
            template_type: RefactorTemplateType::ReplaceMagicNumber,
            description: "Replace magic numbers with named constants".to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![
                Precondition {
                    id: "magic_number".to_string(),
                    description: "Find magic numbers in code".to_string(),
                    check_type: CheckType::CodePattern,
                },
            ],
            steps: vec![
                RefactorStep {
                    step_id: "identify".to_string(),
                    name: "Identify Magic Numbers".to_string(),
                    description: "Find all magic numbers in code".to_string(),
                    actions: vec![],
                    requires_approval: true,
                },
                RefactorStep {
                    step_id: "name".to_string(),
                    name: "Propose Constant Names".to_string(),
                    description: "Suggest descriptive names for constants".to_string(),
                    actions: vec![],
                    requires_approval: true,
                },
                RefactorStep {
                    step_id: "replace".to_string(),
                    name: "Replace Magic Numbers".to_string(),
                    description: "Replace numbers with named constants".to_string(),
                    actions: vec![],
                    requires_approval: false,
                },
            ],
            validation_checks: vec![
                ValidationCheck {
                    check_id: "compile".to_string(),
                    name: "Compile Check".to_string(),
                    description: "Verify code compiles".to_string(),
                    check_type: ValidationCheckType::Compile,
                    required: true,
                },
            ],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建简化条件表达式模板
    fn create_simplify_condition_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "simplify_condition".to_string(),
            name: "Simplify Condition Expressions".to_string(),
            template_type: RefactorTemplateType::SimplifyCondition,
            description: "Simplify complex conditional expressions".to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建添加错误处理模板
    fn create_add_error_handling_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "add_error_handling".to_string(),
            name: "Add Error Handling".to_string(),
            template_type: RefactorTemplateType::AddErrorHandling,
            description: "Add proper error handling to code".to_string(),
            languages: vec!["rust".to_string(), "python".to_string()],
            difficulty: RefactorDifficulty::Medium,
            risk_level: RefactorRiskLevel::Medium,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建改善命名模板
    fn create_improve_naming_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "improve_naming".to_string(),
            name: "Improve Naming".to_string(),
            template_type: RefactorTemplateType::ImproveNaming,
            description: "Improve variable and function names".to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建拆分大文件模板
    fn create_split_file_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "split_file".to_string(),
            name: "Split Large File".to_string(),
            template_type: RefactorTemplateType::SplitFile,
            description: "Split large files into smaller, focused modules".to_string(),
            languages: vec!["rust".to_string(), "python".to_string()],
            difficulty: RefactorDifficulty::Medium,
            risk_level: RefactorRiskLevel::Medium,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建移除死代码模板
    fn create_remove_dead_code_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "remove_dead_code".to_string(),
            name: "Remove Dead Code".to_string(),
            template_type: RefactorTemplateType::RemoveDeadCode,
            description: "Remove unused functions, variables, and imports".to_string(),
            languages: vec!["rust".to_string(), "python".to_string(), "javascript".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建添加文档模板
    fn create_add_documentation_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "add_documentation".to_string(),
            name: "Add Documentation".to_string(),
            template_type: RefactorTemplateType::AddDocumentation,
            description: "Add documentation comments to public APIs".to_string(),
            languages: vec!["rust".to_string(), "python".to_string()],
            difficulty: RefactorDifficulty::Easy,
            risk_level: RefactorRiskLevel::Low,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 创建安全加固模板
    fn create_security_hardening_template() -> RefactorTemplate {
        RefactorTemplate {
            id: "security_hardening".to_string(),
            name: "Security Hardening".to_string(),
            template_type: RefactorTemplateType::SecurityHardening,
            description: "Apply security best practices to code".to_string(),
            languages: vec!["rust".to_string(), "python".to_string()],
            difficulty: RefactorDifficulty::Medium,
            risk_level: RefactorRiskLevel::Medium,
            preconditions: vec![],
            steps: vec![],
            validation_checks: vec![],
            success_examples: vec![],
            failure_examples: vec![],
            enabled: true,
        }
    }

    /// 添加模板到库中
    fn add_template(&mut self, template: RefactorTemplate) {
        let template_id = template.id.clone();
        
        // 添加到语言过滤
        for lang in &template.languages {
            self.language_filters
                .entry(lang.clone())
                .or_insert_with(HashSet::new)
                .insert(template_id.clone());
        }
        
        // 添加到类型过滤
        self.type_filters
            .entry(template.template_type)
            .or_insert_with(HashSet::new)
            .insert(template_id.clone());
        
        self.templates.insert(template_id, template);
    }

    /// 获取所有模板
    pub fn get_all_templates(&self) -> Vec<&RefactorTemplate> {
        self.templates.values().collect()
    }

    /// 根据语言过滤模板
    pub fn get_templates_by_language(&self, language: &str) -> Vec<&RefactorTemplate> {
        self.language_filters
            .get(language)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.templates.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 根据类型过滤模板
    pub fn get_templates_by_type(&self, template_type: RefactorTemplateType) -> Vec<&RefactorTemplate> {
        self.type_filters
            .get(&template_type)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.templates.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 根据ID获取模板
    pub fn get_template(&self, template_id: &str) -> Option<&RefactorTemplate> {
        self.templates.get(template_id)
    }

    /// 获取启用的模板
    pub fn get_enabled_templates(&self) -> Vec<&RefactorTemplate> {
        self.templates
            .values()
            .filter(|t| t.enabled)
            .collect()
    }
}

impl Default for RefactorTemplateLibrary {
    fn default() -> Self {
        Self::new()
    }
}

// --- 重构预览系统 ----------------------------------

/// 重构预览系统
#[derive(Debug, Clone)]
pub struct RefactorPreviewSystem {
    pending_refactors: Vec<PendingRefactor>,
    template_library: RefactorTemplateLibrary,
}

/// 待处理的重构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRefactor {
    pub refactor_id: String,
    pub template_id: String,
    pub template_name: String,
    pub target_files: Vec<String>,
    pub description: String,
    pub diff_preview: Option<String>,
    pub created_at: String,
    pub status: RefactorStatus,
    pub approvals: Vec<Approval>,
}

/// 重构状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefactorStatus {
    Draft,
    PendingReview,
    Approved,
    Rejected,
    Applied,
    Failed,
    Cancelled,
}

/// 审批
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub approver: String,
    pub approved: bool,
    pub comment: Option<String>,
    pub timestamp: String,
}

impl RefactorPreviewSystem {
    /// 创建新的预览系统
    pub fn new() -> Self {
        Self {
            pending_refactors: Vec::new(),
            template_library: RefactorTemplateLibrary::new(),
        }
    }

    /// 获取模板库
    pub fn template_library(&self) -> &RefactorTemplateLibrary {
        &self.template_library
    }

    /// 创建新的待处理重构
    pub fn create_pending_refactor(
        &mut self,
        template_id: String,
        target_files: Vec<String>,
        description: String,
    ) -> String {
        let template = self.template_library.get_template(&template_id);
        let template_name = template.map(|t| t.name.clone()).unwrap_or_else(|| "Unknown".to_string());
        
        let refactor_id = format!("refactor_{}", uuid::Uuid::new_v4().simple());
        let pending = PendingRefactor {
            refactor_id: refactor_id.clone(),
            template_id,
            template_name,
            target_files,
            description,
            diff_preview: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            status: RefactorStatus::Draft,
            approvals: Vec::new(),
        };
        
        self.pending_refactors.push(pending);
        refactor_id
    }

    /// 获取待处理重构
    pub fn get_pending_refactors(&self) -> &[PendingRefactor] {
        &self.pending_refactors
    }

    /// 获取待处理重构
    pub fn get_pending_refactor(&self, refactor_id: &str) -> Option<&PendingRefactor> {
        self.pending_refactors.iter().find(|r| r.refactor_id == refactor_id)
    }

    /// 更新重构状态
    pub fn update_status(&mut self, refactor_id: &str, status: RefactorStatus) -> bool {
        if let Some(refactor) = self.pending_refactors.iter_mut().find(|r| r.refactor_id == refactor_id) {
            refactor.status = status;
            true
        } else {
            false
        }
    }

    /// 添加审批
    pub fn add_approval(&mut self, refactor_id: &str, approval: Approval) -> bool {
        if let Some(refactor) = self.pending_refactors.iter_mut().find(|r| r.refactor_id == refactor_id) {
            refactor.approvals.push(approval);
            true
        } else {
            false
        }
    }

    /// 设置差异预览
    pub fn set_diff_preview(&mut self, refactor_id: &str, diff: String) -> bool {
        if let Some(refactor) = self.pending_refactors.iter_mut().find(|r| r.refactor_id == refactor_id) {
            refactor.diff_preview = Some(diff);
            true
        } else {
            false
        }
    }

    /// 移除待处理重构
    pub fn remove_pending_refactor(&mut self, refactor_id: &str) -> bool {
        let initial_len = self.pending_refactors.len();
        self.pending_refactors.retain(|r| r.refactor_id != refactor_id);
        self.pending_refactors.len() < initial_len
    }
}

impl Default for RefactorPreviewSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// 重构UI显示格式
pub mod ui {
    use super::*;
    
    /// 为TUI格式化模板列表
    pub fn format_templates_for_tui(templates: &[&RefactorTemplate]) -> String {
        let mut output = String::new();
        output.push_str("Available Refactor Templates:\n");
        output.push_str(&"=".repeat(60));
        output.push('\n');
        
        for (idx, template) in templates.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", idx + 1, template.name));
            output.push_str(&format!("   Type: {}\n", template.template_type));
            output.push_str(&format!("   Risk: {:?}, Difficulty: {:?}\n", template.risk_level, template.difficulty));
            output.push_str(&format!("   Languages: {}\n", template.languages.join(", ")));
            output.push_str(&format!("   Description: {}\n", template.description));
            output.push('\n');
        }
        
        output
    }
    
    /// 为Web IDE格式化模板列表
    pub fn format_templates_for_web(templates: &[&RefactorTemplate]) -> serde_json::Value {
        let template_list: Vec<_> = templates.iter().map(|t| {
            serde_json::json!({
                "id": t.id,
                "name": t.name,
                "type": format!("{:?}", t.template_type),
                "risk_level": format!("{:?}", t.risk_level),
                "difficulty": format!("{:?}", t.difficulty),
                "languages": t.languages,
                "description": t.description,
                "enabled": t.enabled,
            })
        }).collect();
        
        serde_json::json!({
            "templates": template_list,
            "total_count": templates.len(),
        })
    }
}
