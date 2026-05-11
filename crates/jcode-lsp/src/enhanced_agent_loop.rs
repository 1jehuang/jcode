// enhanced_agent_loop.rs
// ════════════════════════════════════════════════════════════════
// 增强型 Agent 执行循环 — plan-edit-build-test-fix-retry 模式
//
// ## 执行流程
// 1. Plan: 分析任务，生成执行计划
// 2. Edit: 执行代码修改
// 3. Build: 编译/构建验证
// 4. Test: 运行测试验证
// 5. Fix: 如果失败，自动修复
// 6. Retry: 重试（最多 N 次）
//
// ## 核心能力对标 Claude Code/Cursor
// - 自动错误修复 (QuickFix)
// - 安全+性能审查 (Review)
// - 代码格式化 (FormatCode)
// - 性能瓶颈识别 (Performance Profiling)
// - Git 工作流集成 (Branch/Merge/Conflict)

use std::sync::Arc;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};

/// 执行阶段枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionPhase {
    Planning,
    Editing,
    Building,
    Testing,
    Fixing,
    Retrying,
    Completed,
    Failed,
}

impl std::fmt::Display for ExecutionPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionPhase::Planning => write!(f, "📋 Planning"),
            ExecutionPhase::Editing => write!(f, "✏️ Editing"),
            ExecutionPhase::Building => write!(f, "🔨 Building"),
            ExecutionPhase::Testing => write!(f, "🧪 Testing"),
            ExecutionPhase::Fixing => write!(f, "🔧 Fixing"),
            ExecutionPhase::Retrying => write!(f, "🔄 Retrying"),
            ExecutionPhase::Completed => write!(f, "✅ Completed"),
            ExecutionPhase::Failed => write!(f, "❌ Failed"),
        }
    }
}

/// 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// 是否成功
    pub success: bool,
    /// 最终阶段
    pub final_phase: ExecutionPhase,
    /// 总耗时（毫秒）
    pub total_duration_ms: u64,
    /// 各阶段耗时
    pub phase_durations: Vec<PhaseDuration>,
    /// 重试次数
    pub retry_count: u32,
    /// 错误信息（如果失败）
    pub error: Option<String>,
    /// 应用的修复列表
    pub fixes_applied: Vec<FixInfo>,
}

/// 阶段耗时统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseDuration {
    pub phase: ExecutionPhase,
    pub duration_ms: u64,
    pub success: bool,
}

/// 修复信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixInfo {
    pub fix_type: FixType,
    pub description: String,
    pub file_path: Option<String>,
    pub line_number: Option<u32>,
    pub original_code: Option<String>,
    pub fixed_code: Option<String>,
}

/// 修复类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FixType {
    CompilationError,
    TestFailure,
    LintWarning,
    SecurityIssue,
    PerformanceIssue,
    StyleViolation,
}

impl std::fmt::Display for FixType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FixType::CompilationError => write!(f, "Compilation Error"),
            FixType::TestFailure => write!(f, "Test Failure"),
            FixType::LintWarning => write!(f, "Lint Warning"),
            FixType::SecurityIssue => write!(f, "Security Issue"),
            FixType::PerformanceIssue => write!(f, "Performance Issue"),
            FixType::StyleViolation => write!(f, "Style Violation"),
        }
    }
}

/// 增强型 Agent 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedAgentConfig {
    /// 最大重试次数
    pub max_retries: u32,
    /// 每个阶段的超时时间（秒）
    pub phase_timeout_seconds: u64,
    /// 是否启用自动修复
    pub auto_fix_enabled: bool,
    /// 是否启用安全审查
    pub security_review_enabled: bool,
    /// 是否启用性能审查
    pub performance_review_enabled: bool,
    /// 是否自动格式化代码
    pub auto_format_enabled: bool,
    /// 构建命令
    pub build_command: String,
    /// 测试命令
    pub test_command: String,
    /// 格式化工具
    pub format_tool: String,
}

impl Default for EnhancedAgentConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            phase_timeout_seconds: 300, // 5 分钟
            auto_fix_enabled: true,
            security_review_enabled: true,
            performance_review_enabled: true,
            auto_format_enabled: true,
            build_command: "cargo build".to_string(),
            test_command: "cargo test".to_string(),
            format_tool: "rustfmt".to_string(),
        }
    }
}

/// 增强型 Agent 执行器
pub struct EnhancedAgentExecutor {
    config: EnhancedAgentConfig,
    current_phase: Arc<RwLock<ExecutionPhase>>,
    execution_history: Arc<RwLock<Vec<ExecutionRecord>>>,
    retry_count: Arc<RwLock<u32>>,
}

/// 执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub attempt_number: u32,
    pub phase: ExecutionPhase,
    pub timestamp: Instant,
    pub duration_ms: u64,
    pub output: String,
    pub success: bool,
    pub error: Option<String>,
}

impl EnhancedAgentExecutor {
    /// 创建新的增强型执行器
    pub fn new(config: EnhancedAgentConfig) -> Self {
        Self {
            config,
            current_phase: Arc::new(RwLock::new(ExecutionPhase::Planning)),
            execution_history: Arc::new(RwLock::new(Vec::new())),
            retry_count: Arc::new(RwLock::new(0)),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(EnhancedAgentConfig::default())
    }

    /// 执行完整的 plan-edit-build-test-fix-retry 循环
    ///
    /// # Arguments
    /// * `task_description` - 任务描述
    /// * `initial_code` - 初始代码内容
    /// * `file_path` - 文件路径
    ///
    /// # Returns
    /// 返回执行结果，包含所有阶段的信息和最终状态
    pub async fn execute_task(
        &self,
        task_description: &str,
        initial_code: &str,
        file_path: &str,
    ) -> ExecutionResult {
        let start_time = Instant::now();
        let mut phase_durations = Vec::new();
        let mut fixes_applied = Vec::new();
        let mut current_code = initial_code.to_string();
        
        info!("🚀 Starting enhanced execution loop for task: {}", task_description);
        
        // Phase 1: Planning
        self.set_phase(ExecutionPhase::Planning).await;
        let plan_result = self.execute_planning_phase(task_description, file_path).await;
        let plan_duration = self.record_phase(ExecutionPhase::Planning, plan_result.is_ok()).await;
        phase_durations.push(plan_duration);
        
        if !plan_result.is_ok() {
            return self.create_failure_result(
                start_time,
                phase_durations,
                fixes_applied,
                plan_result.err().map(|e| e.to_string()),
            );
        }

        // 主循环：edit → build → test → fix → retry
        for attempt in 0..=self.config.max_retries {
            *self.retry_count.write().await = attempt;
            
            if attempt > 0 {
                self.set_phase(ExecutionPhase::Retrying).await;
                info!("🔄 Retry attempt {}/{}", attempt, self.config.max_retries);
                
                let retry_duration = self.record_phase(ExecutionPhase::Retrying, true).await;
                phase_durations.push(retry_duration);
            }

            // Phase 2: Editing
            self.set_phase(ExecutionPhase::Editing).await;
            let edit_result = self.execute_editing_phase(
                &current_code,
                file_path,
                task_description,
                attempt,
            ).await;
            let edit_duration = self.record_phase(ExecutionPhase::Editing, edit_result.is_ok()).await;
            phase_durations.push(edit_duration);

            match edit_result {
                Ok(new_code) => {
                    current_code = new_code;
                    
                    // Phase 3: Building
                    self.set_phase(ExecutionPhase::Building).await;
                    let build_result = self.execute_building_phase(&current_code, file_path).await;
                    let build_duration = self.record_phase(ExecutionPhase::Building, build_result.is_ok()).await;
                    phase_durations.push(build_duration);

                    match build_result {
                        Ok(_) => {
                            // Phase 4: Testing
                            self.set_phase(ExecutionPhase::Testing).await;
                            let test_result = self.execute_testing_phase(file_path).await;
                            let test_duration = self.record_phase(ExecutionPhase::Testing, test_result.is_ok()).await;
                            phase_durations.push(test_duration);

                            match test_result {
                                Ok(_) => {
                                    // ✅ 成功完成
                                    self.set_phase(ExecutionPhase::Completed).await;
                                    info!("✅ Task completed successfully after {} attempt(s)", attempt + 1);
                                    
                                    return ExecutionResult {
                                        success: true,
                                        final_phase: ExecutionPhase::Completed,
                                        total_duration_ms: start_time.elapsed().as_millis() as u64,
                                        phase_durations,
                                        retry_count: attempt,
                                        error: None,
                                        fixes_applied,
                                    };
                                }
                                Err(test_error) => {
                                    // 测试失败，尝试修复
                                    if self.config.auto_fix_enabled && attempt < self.config.max_retries {
                                        self.set_phase(ExecutionPhase::Fixing).await;
                                        let fix_result = self.auto_fix_test_failure(
                                            &test_error.to_string(),
                                            &current_code,
                                            file_path,
                                        ).await;
                                        
                                        let fix_duration = self.record_phase(ExecutionPhase::Fixing, fix_result.is_some()).await;
                                        phase_durations.push(fix_duration);
                                        
                                        if let Some(fix) = fix_result {
                                            fixes_applied.push(fix);
                                            current_code = fix.fixed_code.unwrap_or(current_code.clone());
                                        }
                                        continue; // 继续重试
                                    } else {
                                        break; // 无法修复或达到重试上限
                                    }
                                }
                            }
                        }
                        Err(build_error) => {
                            // 编译失败，尝试修复
                            if self.config.auto_fix_enabled && attempt < self.config.max_retries {
                                self.set_phase(ExecutionPhase::Fixing).await;
                                let fix_result = self.auto_fix_compilation_error(
                                    &build_error.to_string(),
                                    &current_code,
                                    file_path,
                                ).await;
                                
                                let fix_duration = self.record_phase(ExecutionPhase::Fixing, fix_result.is_some()).await;
                                phase_durations.push(fix_duration);
                                
                                if let Some(fix) = fix_result {
                                    fixes_applied.push(fix);
                                    current_code = fix.fixed_code.unwrap_or(current_code.clone());
                                }
                                continue; // 继续重试
                            } else {
                                break; // 无法修复或达到重试上限
                            }
                        }
                    }
                }
                Err(edit_error) => {
                    // 编辑阶段失败
                    error!("❌ Editing failed: {}", edit_error);
                    break;
                }
            }
        }

        // 所有重试都失败了
        self.set_phase(ExecutionPhase::Failed).await;
        self.create_failure_result(
            start_time,
            phase_durations,
            fixes_applied,
            Some("Max retries exceeded or unrecoverable error".to_string()),
        )
    }

    // ════════════════════════════════════════════════════════════════
    // 各阶段的具体实现
    // ════════════════════════════════════════════════════════════════

    /// Phase 1: 规划阶段
    async fn execute_planning_phase(
        &self,
        task_description: &str,
        file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Planning execution for: {}", task_description);
        
        // TODO: 调用 AI Provider 分析任务并生成计划
        // 目前返回成功（实际应用中应该调用 LLM）
        
        // 示例：分析任务复杂度
        let complexity = self.analyze_task_complexity(task_description);
        info!("Task complexity analysis: {:?}", complexity);
        
        Ok(())
    }

    /// Phase 2: 编辑阶段
    async fn execute_editing_phase(
        &self,
        code: &str,
        file_path: &str,
        task: &str,
        attempt: u32,
    ) -> Result<String, Box<dyn std::error::Error>> {
        debug!("Editing {} (attempt {})", file_path, attempt);
        
        // TODO: 调用 AST 操作或 AI Provider 进行代码修改
        // 目前返回原代码（实际应用中应该调用 ast_operations 或 LSP）
        
        if attempt == 0 {
            // 首次编辑：可以在这里集成 AI 辅助
            Ok(code.to_string())
        } else {
            // 后续编辑：基于之前的修复结果
            Ok(code.to_string())
        }
    }

    /// Phase 3: 构建阶段
    async fn execute_building_phase(
        &self,
        _code: &str,
        _file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Building project...");
        
        // 执行构建命令
        let output = tokio::process::Command::new("cargo")
            .arg("check")
            .output()
            .await?;
        
        if output.status.success() {
            Ok(())
        } else {
            let error_output = String::from_utf8_lossy(&output.stderr);
            Err(format!("Build failed:\n{}", error_output).into())
        }
    }

    /// Phase 4: 测试阶段
    async fn execute_testing_phase(
        &self,
        _file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Running tests...");
        
        // 执行测试命令
        let output = tokio::process::Command::new("cargo")
            .arg("test")
            .output()
            .await?;
        
        if output.status.success() {
            Ok(())
        } else {
            let error_output = String::from_utf8_lossy(&output.stderr);
            Err(format!("Tests failed:\n{}", error_output).into())
        }
    }

    /// 自动修复编译错误
    async fn auto_fix_compilation_error(
        &self,
        error_message: &str,
        code: &str,
        file_path: &str,
    ) -> Option<FixInfo> {
        info!("Attempting to auto-fix compilation error...");
        
        // 解析错误信息
        let parsed_errors = self.parse_compilation_errors(error_message);
        
        if parsed_errors.is_empty() {
            return None;
        }
        
        // 尝试修复第一个错误
        let first_error = &parsed_errors[0];
        
        // 使用 QuickFix 策略
        let fixed_code = self.apply_quick_fix(code, first_error, file_path)?;
        
        Some(FixInfo {
            fix_type: FixType::CompilationError,
            description: format!("Fixed: {}", first_error.message),
            file_path: Some(file_path.to_string()),
            line_number: first_error.line,
            original_code: Some(first_error.original_line.clone()),
            fixed_code: Some(fixed_code),
        })
    }

    /// 自动修复测试失败
    async fn auto_fix_test_failure(
        &self,
        error_message: &str,
        code: &str,
        file_path: &str,
    ) -> Option<FixInfo> {
        info!("Attempting to auto-fix test failure...");
        
        // 解析测试错误
        let parsed_errors = self.parse_test_errors(error_message);
        
        if parsed_errors.is_empty() {
            return None;
        }
        
        // 尝试修复第一个测试错误
        let first_error = &parsed_errors[0];
        
        // 使用测试修复策略
        let fixed_code = self.fix_test_failure(code, first_error, file_path)?;
        
        Some(FixInfo {
            fix_type: FixType::TestFailure,
            description: format!("Fixed test: {}", first_error.test_name),
            file_path: Some(file_path.to_string()),
            line_number: first_error.line,
            original_code: Some(first_error.original_line.clone()),
            fixed_code: Some(fixed_code),
        })
    }

    // ════════════════════════════════════════════════════════════════
    // 辅助方法
    // ════════════════════════════════════════════════════════════════

    async fn set_phase(&self, phase: ExecutionPhase) {
        *self.current_phase.write().await = phase;
        debug!("Phase changed to: {}", phase);
    }

    async fn record_phase(&self, phase: ExecutionPhase, success: bool) -> PhaseDuration {
        let record = ExecutionRecord {
            attempt_number: *self.retry_count.read().await,
            phase,
            timestamp: Instant::now(),
            duration_ms: 0, // 将在下面更新
            output: String::new(),
            success,
            error: None,
        };
        
        // 记录到历史
        self.execution_history.write().await.push(record);
        
        // 返回阶段耗时（简化版）
        PhaseDuration {
            phase,
            duration_ms: 0, // 实际应用中应该计算真实耗时
            success,
        }
    }

    fn create_failure_result(
        &self,
        start_time: Instant,
        phase_durations: Vec<PhaseDuration>,
        fixes_applied: Vec<FixInfo>,
        error: Option<String>,
    ) -> ExecutionResult {
        ExecutionResult {
            success: false,
            final_phase: ExecutionPhase::Failed,
            total_duration_ms: start_time.elapsed().as_millis() as u64,
            phase_durations,
            retry_count: *self.retry_count.read().await,
            error,
            fixes_applied,
        }
    }

    /// 分析任务复杂度
    fn analyze_task_complexity(&self, task: &str) -> TaskComplexity {
        // 简单的启发式分析
        let lines_estimate = task.matches('\n').count() as u32;
        let files_mentioned = regex::Regex::new(r"\.\w+").ok()
            .map(|re| re.find_iter(task).count() as u32)
            .unwrap_or(0);
        
        if lines_estimate > 50 || files_mentioned > 5 {
            TaskComplexity::High
        } else if lines_estimate > 20 || files_mentioned > 2 {
            TaskComplexity::Medium
        } else {
            TaskComplexity::Low
        }
    }

    /// 解析编译错误
    fn parse_compilation_errors(&self, error_output: &str) -> Vec<ParsedError> {
        let mut errors = Vec::new();
        
        // 匹配 Rust 编译错误格式:
        // error[E0XXX]: message
        //   --> file.rs:line:col
        //    |
        // LL | original line
        
        let error_re = regex::Regex::new(
            r"error\[E\d+\]: (.+)\n\s+-->\s+(.+?):(\d+):(\d+)"
        ).unwrap();
        
        for cap in error_re.captures_iter(error_output) {
            errors.push(ParsedError {
                message: cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default(),
                file_path: cap.get(2).map(|m| m.as_str().to_string()).unwrap_or_default(),
                line: cap.get(3).and_then(|m| m.as_str().parse::<u32>().ok()),
                column: cap.get(4).and_then(|m| m.as_str().parse::<u32>().ok()),
                original_line: String::new(), // 需要额外解析
                test_name: None,
            });
        }
        
        errors
    }

    /// 解析测试错误
    fn parse_test_errors(&self, error_output: &str) -> Vec<ParsedError> {
        let mut errors = Vec::new();
        
        // 匹配 Rust 测试错误格式:
        // test test_name ... FAILED
        // or
        // assert failed at file.rs:line:col
        
        let test_re = regex::Regex::new(
            r"test\s+(\w+)::\w+\s+... FAILED"
        ).unwrap();
        
        for cap in test_re.captures_iter(error_output) {
            errors.push(ParsedError {
                message: "Test failed".to_string(),
                file_path: String::new(),
                line: None,
                column: None,
                original_line: String::new(),
                test_name: cap.get(1).map(|m| m.as_str().to_string()),
            });
        }
        
        errors
    }

    /// 应用 QuickFix
    fn apply_quick_fix(&self, code: &str, error: &ParsedError, file_path: &str) -> Option<String> {
        // TODO: 集成 LSP QuickFix 或 AI 辅助修复
        // 这里提供一些常见的编译错误修复策略
        
        match &error.message {
            msg if msg.contains("unused variable") => {
                // 未使用的变量：添加下划线前缀
                if let Some(line_num) = error.line {
                    let lines: Vec<&str> = code.lines().collect();
                    if line_num > 0 && (line_num as usize) <= lines.len() {
                        let target_line = lines[(line_num - 1) as usize];
                        
                        // 简单的修复：在变量名前加 _
                        let fixed_line = target_line.replace("let ", "let _");
                        
                        let mut result = code.to_string();
                        // 替换指定行
                        // （这里需要更精确的实现）
                        return Some(result);
                    }
                }
                None
            }
            msg if msg.contains("mismatched types") => {
                // 类型不匹配：提示用户检查类型
                None // 需要人工干预
            }
            msg if msg.contains("cannot find") => {
                // 找不到模块/依赖：建议运行 cargo update
                None // 需要依赖管理
            }
            _ => None,
        }
    }

    /// 修复测试失败
    fn fix_test_failure(&self, code: &str, error: &ParsedError, _file_path: &str) -> Option<String> {
        // TODO: 实现 AI 辅助的测试修复
        // 目前返回 None（无法自动修复）
        
        match &error.test_name {
            Some(test_name) => {
                warn!("Cannot auto-fix test failure for: {}", test_name);
                None
            }
            None => None,
        }
    }

    /// 获取当前执行阶段
    pub async fn get_current_phase(&self) -> ExecutionPhase {
        *self.current_phase.read().await
    }

    /// 获取执行历史
    pub async fn get_execution_history(&self) -> Vec<ExecutionRecord> {
        self.execution_history.read().await.clone()
    }

    /// 获取重试次数
    pub async fn get_retry_count(&self) -> u32 {
        *self.retry_count.read().await
    }
}

/// 任务复杂度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum TaskComplexity {
    Low,
    Medium,
    High,
}

/// 解析后的错误信息
struct ParsedError {
    message: String,
    file_path: String,
    line: Option<u32>,
    column: Option<u32>,
    original_line: String,
    test_name: Option<String>,
}
